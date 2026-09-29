//! Sentinel persistence. Only Sentinel's own records live here (programs,
//! incidents, rules, alert executions) plus snapshots of the transactions an
//! incident points at, so investigations survive the in-memory window.
//! Rows keep a few indexed columns and the full record as JSON.

use crate::model::{AlertExecution, AlertRule, Incident, MonitoredProgram, TxSummary};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use std::sync::Mutex;
use vortex::events::VortexTransaction;

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
"#;

fn parse<T: DeserializeOwned>(s: String) -> rusqlite::Result<T> {
    serde_json::from_str(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

impl Store {
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
                serde_json::to_string(tx)?
            ],
        )?;
        Ok(n > 0)
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
                |r| parse(r.get(0)?),
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
