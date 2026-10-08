//! Sentinel persistence. Only Sentinel's own records live here (programs,
//! incidents, rules, alert executions) plus snapshots of the transactions an
//! incident points at, so investigations survive the in-memory window.
//! Rows keep a few indexed columns and the full record as JSON.

use crate::model::{AlertExecution, AlertRule, ApiToken, Incident, MonitoredProgram, SummarySchedule, TxSummary};
use crate::rollup::Rollup;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use std::io::{Read, Write};
use std::sync::Mutex;
use vortex::events::VortexTransaction;

/// Hourly rollups are kept this long.
const ROLLUP_RETENTION_SECS: i64 = 35 * 24 * 3600;

pub struct Store {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS programs (
    program_id TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS incidents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    program_id TEXT NOT NULL,
    status TEXT NOT NULL,
    detected_at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS incidents_program ON incidents(program_id, id DESC);
CREATE TABLE IF NOT EXISTS incident_transactions (
    incident_id INTEGER NOT NULL,
    signature TEXT NOT NULL,
    summary TEXT NOT NULL,
    tx TEXT NOT NULL,
    PRIMARY KEY (incident_id, signature)
);
CREATE INDEX IF NOT EXISTS incident_tx_signature ON incident_transactions(signature);
CREATE TABLE IF NOT EXISTS rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT PRIMARY KEY,
    account TEXT NOT NULL,
    expires_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS watchlist (
    account TEXT NOT NULL,
    program_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (account, program_id)
);
CREATE TABLE IF NOT EXISTS alert_executions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    rule_id INTEGER NOT NULL,
    data TEXT NOT NULL
);
-- The message a channel created for an incident, so updates and the resolution
-- can reply to it (Telegram message id, ...).
-- What a program did in each hour, for daily and weekly summaries.
CREATE TABLE IF NOT EXISTS rollups (
    program_id TEXT NOT NULL,
    hour INTEGER NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (program_id, hour)
);
CREATE TABLE IF NOT EXISTS api_tokens (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token_hash TEXT NOT NULL UNIQUE,
    account TEXT NOT NULL,
    name TEXT NOT NULL,
    scope TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_used_at TEXT
);
CREATE TABLE IF NOT EXISTS summary_schedules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS delivery_refs (
    rule_id INTEGER NOT NULL,
    incident_id INTEGER NOT NULL,
    channel INTEGER NOT NULL,
    msg_ref TEXT NOT NULL,
    PRIMARY KEY (rule_id, incident_id, channel)
);
"#;

fn parse<T: DeserializeOwned>(s: String) -> rusqlite::Result<T> {
    serde_json::from_str(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

/// Full transactions are stored gzip-compressed (a BLOB in the `tx` column).
/// Rows written by older versions hold plain JSON text; both are readable.
fn pack(tx: &VortexTransaction) -> Result<Vec<u8>> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(&serde_json::to_vec(tx)?)?;
    Ok(enc.finish()?)
}

fn unpack(v: rusqlite::types::ValueRef<'_>) -> rusqlite::Result<VortexTransaction> {
    use rusqlite::types::ValueRef;
    let conv = |e: Box<dyn std::error::Error + Send + Sync>| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Blob, e)
    };
    match v {
        ValueRef::Blob(b) => {
            let mut json = Vec::new();
            flate2::read::GzDecoder::new(b)
                .read_to_end(&mut json)
                .map_err(|e| conv(Box::new(e)))?;
            serde_json::from_slice(&json).map_err(|e| conv(Box::new(e)))
        }
        ValueRef::Text(t) => serde_json::from_slice(t).map_err(|e| conv(Box::new(e))),
        _ => Err(rusqlite::Error::InvalidColumnType(
            0,
            "tx".into(),
            v.data_type(),
        )),
    }
}

impl Store {
    /// Keeps the database inside a budget: drops incidents (and their stored
    /// transactions) older than `retain_hours`, trims old alert deliveries, and
    /// if the live data still exceeds `max_bytes`, drops the oldest stored
    /// transactions until it fits. Returns the number of rows removed.
    pub fn prune(&self, retain_hours: i64, max_bytes: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let cutoff = (chrono::Utc::now() - chrono::Duration::hours(retain_hours)).to_rfc3339();
        let mut removed = conn.execute(
            "DELETE FROM incident_transactions WHERE incident_id IN
               (SELECT id FROM incidents WHERE detected_at < ?1 AND status = '\"resolved\"')",
            [&cutoff],
        )?;
        removed += conn.execute(
            "DELETE FROM incidents WHERE detected_at < ?1 AND status = '\"resolved\"'",
            [&cutoff],
        )?;
        removed += conn.execute(
            "DELETE FROM alert_executions WHERE id <= (SELECT MAX(id) FROM alert_executions) - 5000",
            [],
        )?;
        removed += conn.execute(
            "DELETE FROM rollups WHERE hour < ?1",
            [chrono::Utc::now().timestamp() - ROLLUP_RETENTION_SECS],
        )?;
        removed += conn.execute(
            "DELETE FROM delivery_refs WHERE incident_id NOT IN (SELECT id FROM incidents)",
            [],
        )?;
        let used = |c: &Connection| -> rusqlite::Result<i64> {
            let pages: i64 = c.query_row("PRAGMA page_count", [], |r| r.get(0))?;
            let free: i64 = c.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            let size: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0))?;
            Ok((pages - free) * size)
        };
        while used(&conn)? > max_bytes {
            let n = conn.execute(
                "DELETE FROM incident_transactions WHERE rowid IN
                   (SELECT rowid FROM incident_transactions ORDER BY rowid LIMIT 2000)",
                [],
            )?;
            if n == 0 {
                break;
            }
            removed += n;
        }
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        Ok(removed)
    }

    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        // Incident numbers start at 1001 so they read like ticket IDs.
        conn.execute(
            "INSERT INTO sqlite_sequence(name, seq) SELECT 'incidents', 1000
             WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'incidents')",
            [],
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // --- programs ---

    pub fn upsert_program(&self, p: &MonitoredProgram) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO programs(program_id, data) VALUES (?1, ?2)
             ON CONFLICT(program_id) DO UPDATE SET data = excluded.data",
            params![p.program_id, serde_json::to_string(p)?],
        )?;
        Ok(())
    }

    pub fn delete_program(&self, program_id: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM programs WHERE program_id = ?1", [program_id])?;
        Ok(())
    }

    pub fn programs(&self) -> Result<Vec<MonitoredProgram>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT data FROM programs ORDER BY rowid")?;
        let rows = stmt.query_map([], |r| parse(r.get(0)?))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // --- sessions ---

    pub fn create_session(&self, token_hash: &str, account: &str, expires: chrono::DateTime<chrono::Utc>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM sessions WHERE expires_at < ?1",
            [chrono::Utc::now().to_rfc3339()],
        )?;
        conn.execute(
            "INSERT INTO sessions(token_hash, account, expires_at) VALUES (?1, ?2, ?3)",
            params![token_hash, account, expires.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn session_account(&self, token_hash: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT account FROM sessions WHERE token_hash = ?1 AND expires_at > ?2",
                params![token_hash, chrono::Utc::now().to_rfc3339()],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn delete_session(&self, token_hash: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash])?;
        Ok(())
    }

    // --- API tokens ---

    pub fn create_api_token(&self, token_hash: &str, account: &str, name: &str, scope: &str) -> Result<ApiToken> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now();
        conn.execute(
            "INSERT INTO api_tokens(token_hash, account, name, scope, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![token_hash, account, name, scope, now.to_rfc3339()],
        )?;
        Ok(ApiToken {
            id: conn.last_insert_rowid(),
            account: account.into(),
            name: name.into(),
            scope: scope.into(),
            created_at: now,
            last_used_at: None,
        })
    }

    pub fn api_tokens(&self, account: &str) -> Result<Vec<ApiToken>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, account, name, scope, created_at, last_used_at FROM api_tokens WHERE account = ?1 ORDER BY id",
        )?;
        let parse_time = |s: String| chrono::DateTime::parse_from_rfc3339(&s).map(|t| t.with_timezone(&chrono::Utc)).unwrap_or_else(|_| chrono::Utc::now());
        let rows = stmt.query_map([account], |r| {
            Ok(ApiToken {
                id: r.get(0)?,
                account: r.get(1)?,
                name: r.get(2)?,
                scope: r.get(3)?,
                created_at: parse_time(r.get(4)?),
                last_used_at: r.get::<_, Option<String>>(5)?.map(parse_time),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn delete_api_token(&self, account: &str, id: i64) -> Result<bool> {
        let n = self
            .conn
            .lock()
            .unwrap()
            .execute("DELETE FROM api_tokens WHERE id = ?1 AND account = ?2", params![id, account])?;
        Ok(n > 0)
    }

    /// The token's id, account and scope; records that it was used.
    pub fn api_token_lookup(&self, token_hash: &str) -> Result<Option<(i64, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let found: Option<(i64, String, String)> = conn
            .query_row(
                "SELECT id, account, scope FROM api_tokens WHERE token_hash = ?1",
                [token_hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((id, _, _)) = &found {
            conn.execute(
                "UPDATE api_tokens SET last_used_at = ?1 WHERE id = ?2",
                params![chrono::Utc::now().to_rfc3339(), id],
            )?;
        }
        Ok(found)
    }

    // --- watchlist ---

    pub fn watch(&self, account: &str, program_id: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO watchlist(account, program_id, created_at) VALUES (?1, ?2, ?3)",
            params![account, program_id, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn unwatch(&self, account: &str, program_id: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "DELETE FROM watchlist WHERE account = ?1 AND program_id = ?2",
            params![account, program_id],
        )?;
        Ok(())
    }

    /// (account, program_id) pairs.
    pub fn watchlist(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT account, program_id FROM watchlist")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // --- incidents ---

    /// Inserts a new incident and returns it with its assigned id.
    pub fn create_incident(&self, mut incident: Incident) -> Result<Incident> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO incidents(program_id, status, detected_at, data) VALUES (?1, ?2, ?3, '{}')",
            params![
                incident.program_id,
                serde_json::to_string(&incident.status)?,
                incident.detected_at.to_rfc3339()
            ],
        )?;
        incident.id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE incidents SET data = ?1 WHERE id = ?2",
            params![serde_json::to_string(&incident)?, incident.id],
        )?;
        Ok(incident)
    }

    pub fn update_incident(&self, incident: &Incident) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE incidents SET status = ?1, data = ?2 WHERE id = ?3",
            params![
                serde_json::to_string(&incident.status)?,
                serde_json::to_string(incident)?,
                incident.id
            ],
        )?;
        Ok(())
    }

    pub fn incident(&self, id: i64) -> Result<Option<Incident>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row("SELECT data FROM incidents WHERE id = ?1", [id], |r| {
                parse(r.get(0)?)
            })
            .optional()?)
    }

    pub fn incidents(&self, program_id: Option<&str>, limit: i64) -> Result<Vec<Incident>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT data FROM incidents WHERE (?1 IS NULL OR program_id = ?1)
             ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![program_id, limit], |r| parse(r.get(0)?))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Incidents still open when the process last stopped.
    pub fn unresolved_incidents(&self) -> Result<Vec<Incident>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT data FROM incidents WHERE status != '\"resolved\"' ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| parse(r.get(0)?))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Links a transaction to an incident. Returns false if it was already linked.
    pub fn link_transaction(
        &self,
        incident_id: i64,
        summary: &TxSummary,
        tx: &VortexTransaction,
    ) -> Result<bool> {
        let n = self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO incident_transactions(incident_id, signature, summary, tx)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                incident_id,
                tx.signature,
                serde_json::to_string(summary)?,
                pack(tx)?
            ],
        )?;
        Ok(n > 0)
    }

    /// Stores many links in one SQLite transaction; duplicates are ignored.
    pub fn link_transactions(&self, links: &[(i64, TxSummary, std::sync::Arc<VortexTransaction>)]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let t = conn.transaction()?;
        {
            let mut stmt = t.prepare(
                "INSERT OR IGNORE INTO incident_transactions(incident_id, signature, summary, tx)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (id, summary, tx) in links {
                stmt.execute(params![
                    id,
                    tx.signature,
                    serde_json::to_string(summary)?,
                    pack(tx.as_ref())?
                ])?;
            }
        }
        t.commit()?;
        Ok(())
    }

    pub fn incident_transactions(&self, incident_id: i64, limit: i64) -> Result<Vec<TxSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT summary FROM incident_transactions WHERE incident_id = ?1
             ORDER BY rowid DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![incident_id, limit], |r| parse(r.get(0)?))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn stored_transaction(&self, signature: &str) -> Result<Option<VortexTransaction>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT tx FROM incident_transactions WHERE signature = ?1 LIMIT 1",
                [signature],
                |r| unpack(r.get_ref(0)?),
            )
            .optional()?)
    }

    pub fn incidents_for_transaction(&self, signature: &str) -> Result<Vec<i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT incident_id FROM incident_transactions WHERE signature = ?1")?;
        let rows = stmt.query_map([signature], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // --- rules ---

    pub fn create_rule(&self, mut rule: AlertRule) -> Result<AlertRule> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO rules(data) VALUES ('{}')", [])?;
        rule.id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE rules SET data = ?1 WHERE id = ?2",
            params![serde_json::to_string(&rule)?, rule.id],
        )?;
        Ok(rule)
    }

    pub fn update_rule(&self, rule: &AlertRule) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE rules SET data = ?1 WHERE id = ?2",
            params![serde_json::to_string(rule)?, rule.id],
        )?;
        Ok(())
    }

    pub fn delete_rule(&self, id: i64) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM rules WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn rules(&self) -> Result<Vec<AlertRule>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT data FROM rules ORDER BY id")?;
        let rows = stmt.query_map([], |r| parse(r.get(0)?))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // --- alert executions ---

    pub fn record_execution(&self, mut exec: AlertExecution) -> Result<AlertExecution> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO alert_executions(rule_id, data) VALUES (?1, '{}')",
            [exec.rule_id],
        )?;
        exec.id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE alert_executions SET data = ?1 WHERE id = ?2",
            params![serde_json::to_string(&exec)?, exec.id],
        )?;
        Ok(exec)
    }

    // --- summary schedules ---

    pub fn create_schedule(&self, mut s: SummarySchedule) -> Result<SummarySchedule> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO summary_schedules(data) VALUES ('{}')", [])?;
        s.id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE summary_schedules SET data = ?1 WHERE id = ?2",
            params![serde_json::to_string(&s)?, s.id],
        )?;
        Ok(s)
    }

    pub fn update_schedule(&self, s: &SummarySchedule) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE summary_schedules SET data = ?1 WHERE id = ?2",
            params![serde_json::to_string(s)?, s.id],
        )?;
        Ok(())
    }

    pub fn delete_schedule(&self, id: i64) -> Result<()> {
        self.conn.lock().unwrap().execute("DELETE FROM summary_schedules WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn schedules(&self) -> Result<Vec<SummarySchedule>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT data FROM summary_schedules ORDER BY id")?;
        let rows = stmt.query_map([], |r| parse(r.get(0)?))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // --- rollups ---

    pub fn put_rollup(&self, program_id: &str, hour: i64, rollup: &Rollup) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO rollups(program_id, hour, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(program_id, hour) DO UPDATE SET data = excluded.data",
            params![program_id, hour, serde_json::to_string(rollup)?],
        )?;
        Ok(())
    }

    pub fn rollup(&self, program_id: &str, hour: i64) -> Result<Option<Rollup>> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT data FROM rollups WHERE program_id = ?1 AND hour = ?2",
                params![program_id, hour],
                |r| parse(r.get(0)?),
            )
            .optional()?)
    }

    /// Hourly rollups with `from <= hour < to` (unix seconds), oldest first.
    pub fn rollups_between(&self, program_id: &str, from: i64, to: i64) -> Result<Vec<(i64, Rollup)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT hour, data FROM rollups WHERE program_id = ?1 AND hour >= ?2 AND hour < ?3 ORDER BY hour",
        )?;
        let rows = stmt.query_map(params![program_id, from, to], |r| Ok((r.get(0)?, parse(r.get(1)?)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn delivery_ref(&self, rule_id: i64, incident_id: i64, channel: usize) -> Result<Option<String>> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT msg_ref FROM delivery_refs WHERE rule_id = ?1 AND incident_id = ?2 AND channel = ?3",
                params![rule_id, incident_id, channel as i64],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_delivery_ref(&self, rule_id: i64, incident_id: i64, channel: usize, msg_ref: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO delivery_refs(rule_id, incident_id, channel, msg_ref) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(rule_id, incident_id, channel) DO UPDATE SET msg_ref = excluded.msg_ref",
            params![rule_id, incident_id, channel as i64, msg_ref],
        )?;
        Ok(())
    }

    pub fn executions_for(&self, owner: &str, limit: i64) -> Result<Vec<AlertExecution>> {
        Ok(self
            .executions(1000)?
            .into_iter()
            .filter(|e| e.owner.as_deref() == Some(owner))
            .take(limit as usize)
            .collect())
    }

    pub fn executions(&self, limit: i64) -> Result<Vec<AlertExecution>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT data FROM alert_executions ORDER BY id DESC LIMIT ?1")?;
        let rows = stmt.query_map([limit], |r| parse(r.get(0)?))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
