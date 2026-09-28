# Vortex Sentinel

**Incidents, not transactions.** Sentinel watches a Solana program on mainnet in real time. It tells you when something breaks, which error is breaking it, which instruction, how many wallets are affected, and the exact transactions. Then it pages you.

It runs on top of [Vortex](https://github.com/Don-Vicks/solana-tx-stack). Vortex handles the Solami Yellowstone gRPC stream and decodes each transaction. Sentinel turns that stream into metrics, incidents, investigations and alerts.

```
Solana mainnet
   │  Solami Yellowstone gRPC (server-side program filters, failed txs included)
   ▼
VORTEX   geyser::client → geyser::stream (live-updatable filters, keepalive, reconnect)
         geyser::decode → VortexTransaction (errors, CU, call tree from logs, SOL/SPL transfers)
         VortexHub      → broadcast bus + program filter registry
   │  trait VortexSource
   ▼
SENTINEL engine: 1s rolling windows → detectors + alert rules → incidents (linked txs, failure fingerprints)
         tracer: value flow, balance and state changes, account owners via Solami RPC
         pricing: USD values for every mint seen, via Solami Blur
         SQLite (incidents, rules, deliveries) · HTTP + SSE API · webhooks (Discord/Slack/any)
   ▼
Dashboard (live over SSE)
```

## What it does

- **Live program health.** TPS, failure rate against its baseline, compute units, unique signers and fees, updated every second. The transaction feed streams in as it lands.
- **Failure fingerprints.** Every failed transaction is attributed to the deepest failing frame in its call tree, as *program::instruction → error* (Anchor error names come from logs). For example: "83% of failures are `TooLittleSolReceived` in `Pump.fun::Sell`".
- **Explainable detection.** No ML. Each detector compares a short window against a trailing baseline and states the rule it applied with real numbers:
  - failure-rate spike
  - activity spike (mean + zσ)
  - activity stopped
  - compute spike
  - large transfer, by amount per asset or by USD value (Blur-priced, liquid tokens only)

  Seconds inside an open incident are left out of later baselines, so one incident doesn't mask the next.
- **Incidents** link to the actual transactions (stored, so they survive the in-memory window). They record affected wallets, onset, peak, detection latency, and auto-resolve.
- **Investigation.** For any transaction, Sentinel shows the items below. Recent and incident-linked transactions come from memory or SQLite; any other signature is fetched through Solami RPC and run through the same Vortex decoder.
  - a plain-language narrative
  - value flow between labelled parties (PDAs are resolved to their owner program through RPC)
  - net balance changes
  - the program call tree with per-program compute
  - account state diffs, instructions and logs
- **Alert rules.** Supported conditions: failure rate, failed count, TPS, avg or max compute over a window, single transfers above a size or a USD value, or any incident above a severity. Each rule can open an incident, POST a webhook (with Discord and Slack formatting), or both. Every delivery is logged with its status and latency.
- **Stream health.** The dashboard shows ingest tx/s, current slot, tip lag in slots, time since the last transaction, and events dropped.

## How Solami is used

| Solami product | Role |
|---|---|
| **Yellowstone gRPC** | The only data path. One subscription carries slots plus a named transaction filter over every monitored program (`account_include`, failed transactions included). Adding a program in the UI re-sends the filter over the open stream, with no reconnect. The stream answers pings and reconnects with backoff. |
| **Blur** | `POST /data/token/price` prices every mint Sentinel sees move, in batches of up to 1000. A new mint is priced within about a second; known ones refresh every 30s. USD appears on the live feed, value-flow edges, balance changes and narratives. It also powers the USD large-transfer detector and "transfer worth ≥ $X" rules. Tokens under $10K liquidity are displayed but never trigger alerts, since one trade can move their price arbitrarily. |
| **RPC** | `getTransaction` for investigating any signature (rebuilt into a Yellowstone frame, decoded by Vortex). `getMultipleAccounts` resolves the owners of accounts in a trace, so a vault shows up as "Pump.fun account" rather than a raw address. The canary example sends controlled demo transactions through it. |

## Run it

Prerequisites: Rust 1.75+, Node 18+, `protoc` (Vortex compiles Jito protos), and a Solami API key. [Sign up](https://solami.dev/signup); the Pro trial includes gRPC.

The Vortex crate is expected next to this repo, on the `feat/decoded-program-stream` branch:

```bash
git clone -b feat/decoded-program-stream https://github.com/Don-Vicks/solana-tx-stack ../solana-tx-stack
```

```bash
cp .env.example .env        # add your Solami key
cd web && npm install && npm run build && cd ..
cargo run --release
```

Open http://localhost:8080. Pump.fun is monitored out of the box; add any program ID from the Overview page. Detectors arm after a two-minute baseline.

### Environment

| Variable | Default | Purpose |
|---|---|---|
| `YELLOWSTONE_ENDPOINT` | — | Solami gRPC endpoint, e.g. `https://grpc.solami.dev` |
| `YELLOWSTONE_TOKEN` | — | Solami API key (sent as `x-token`) |
| `SOLANA_RPC_URL` | — | Solami RPC URL, used for owner labels in traces and by the canary |
| `BLUR_API_KEY` | `YELLOWSTONE_TOKEN` | Solami key with the `DataApi` permission, for USD prices. Leave unset to fall back to the gRPC key |
| `BLUR_API_URL` | `https://api.solami.dev` | Blur REST base URL |
| `SENTINEL_PROGRAMS` | — | Comma-separated program IDs to monitor at startup |
| `SENTINEL_PORT` | `8080` | HTTP port (API + dashboard) |
| `SENTINEL_DB` | `sentinel.db` | SQLite file |
| `SENTINEL_PUBLIC_URL` | `http://localhost:8080` | Base URL for incident links in webhooks |
| `SENTINEL_SIMULATE` | — | **Dev only.** Program ID to feed with synthetic traffic instead of gRPC |

### Frontend development

```bash
cargo run            # API on :8080
cd web && npm run dev   # Vite on :5173, proxies /api
```

## Demo: a controlled incident on mainnet

1. Create a throwaway wallet with a little SOL:

   ```bash
   solana-keygen new -o canary-keypair.json
   ```

2. In Sentinel, monitor the canary wallet's address. Any account works as a filter, not just programs.
3. Add an alert rule for that wallet: **Failed transactions exceed 3 over 60s**, pointing at your webhook (a Discord webhook works well on video).
4. Send failing transactions (invalid UTF-8 memos that land and fail on chain):

   ```bash
   cargo run --example canary -- --count 8 --fail
   ```

The transactions travel Solami gRPC → Vortex decoder → Sentinel rule. An incident opens with the transactions linked, the webhook fires, and the delivery appears on the Alerts page. Each transaction costs about 5,000 lamports plus a small priority fee.

## API

| Method | Path | |
|---|---|---|
| GET | `/api/status` | Stream health + every program snapshot |
| GET/POST | `/api/programs` | List / start monitoring `{program_id, label?}` |
| GET/PATCH/DELETE | `/api/programs/{id}` | Detail (snapshot, 10-min series, recent tx, incidents) / update label or detection config / stop |
| GET | `/api/programs/{id}/transactions?failed=true` | Recent transactions |
| GET | `/api/incidents?program=` | Incidents, newest first |
| GET/PATCH | `/api/incidents/{id}` | Incident + linked transactions / set `status` |
| GET | `/api/transactions/{signature}` | Decoded transaction + trace |
| GET/POST, PATCH/DELETE | `/api/rules`, `/api/rules/{id}` | Alert rules |
| POST | `/api/rules/{id}/test` | Send a test webhook |
| GET | `/api/alerts` | Webhook deliveries |
| GET | `/api/stream?program=` | SSE: `transactions`, `metrics`, `incident`, `alert`, `stream` |

Webhook payloads are JSON with `event` (`sentinel.alert`, `sentinel.incident`, `sentinel.test`), `rule`, `severity`, `program`, `message`, `incident` and `links.incident`. Each request carries an `X-Sentinel-Delivery` id for idempotency.

## Tests

```bash
cargo test
```

`tests/real_transactions.rs` runs fingerprinting and tracing on real mainnet Pump.fun transactions: a failed Buy reached through a bot router, a successful trade, and a version 1 transaction. It asserts, for example, that the failure is attributed to `Pump.fun::Buy → TooMuchSolRequired (6002)`. The decoder itself has matching fixture tests in Vortex (`crates/vortex/tests/real_transactions.rs`).

`tests/pricing.rs` runs a mock Blur server with the documented response shape (decimals as strings, key in `x-api-key`). It also checks that USD large-transfer detection ignores thin-liquidity tokens and that a USD alert rule fires on a liquid one.

`tests/pipeline.rs` drives the real engine through a full cycle:
- healthy baseline
- failure spike → incident with linked transactions and fingerprints
- auto-resolve
- a second spike judged against a clean baseline
- a webhook delivered to a local receiver

## Limits

- Timestamps are Sentinel's receive time at `Processed` commitment; a transaction on a dropped fork can appear briefly.
- Version 1 transactions carry compute-unit limit and price in a transaction config, not ComputeBudget instructions. Sentinel reports compute used for them, but not the limit or priority fee.
- USD values use Blur's last-trade price at the time Sentinel sees the transfer. A mint seen for the first time is valued about a second later, so its very first transfer can't trigger a USD alert.
- Program instructions are named from logs (Anchor `Instruction: X`), not IDLs. Truncated logs lose names beyond the cut.
- Metrics and recent transactions live in memory (15 min). Incidents and their transactions are persisted. Detector state resets on restart, and incidents left open are closed out.

See [docs/VORTEX_AUDIT.md](docs/VORTEX_AUDIT.md) for how Sentinel was fitted onto the existing Vortex code: what was reused, extended and built new.
