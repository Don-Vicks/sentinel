# Running and configuring Sentinel

You need a Solami API key. [Sign up](https://solami.dev/signup); the Pro trial includes gRPC.

### Docker (one command)

```bash
git clone https://github.com/Don-Vicks/sentinel && cd sentinel
cp .env.example .env        # add your Solami key
docker compose up --build
```

### Railway

The repo deploys as-is (`Dockerfile` + `railway.toml`). Create a service from the repo, then:

1. **Variables:** `YELLOWSTONE_ENDPOINT`, `YELLOWSTONE_TOKEN`, `SOLANA_RPC_URL`, and `BLUR_API_KEY` for dollar values. Optionally `SENTINEL_PROGRAMS` (comma-separated program IDs to watch at startup).
2. **Volume:** add one mounted at `/data`, so incident history survives restarts. (A Dockerfile `VOLUME` instruction is not used because Railway rejects it.)
3. **Domain:** generate one, then set `SENTINEL_PUBLIC_URL` to it (`https://...`). Wallet sign-in is bound to that address.
4. **Behind the proxy:** set `SENTINEL_TRUST_PROXY=1` so per-IP rate limits see real client addresses.

The port comes from `SENTINEL_PORT`, else Railway's `PORT`, else 8080. An always-on instance pays for every byte it streams from Solami, so watch a small set of programs. If several instances share one Solami account, give each its own `MIRAGE_LABEL`.

### Demo mode (from source, one command)

```bash
cp .env.example .env        # add your Solami keys
./scripts/demo.sh           # builds, starts the API and dashboard on :8080, prints a health checklist
```

It watches a 15-program showcase set (`scripts/showcase.txt`), starts from a fresh database (`--keep` to keep it) and stops on Ctrl+C.

### From source

Prerequisites: Rust 1.75+, Node 18+, and `protoc` (Vortex compiles Jito protos). Cargo pulls Vortex from [Don-Vicks/vortex](https://github.com/Don-Vicks/vortex) automatically.

```bash
cp .env.example .env
cd web && npm install && npm run build && cd ..
cargo run --release
```

Open http://localhost:8080. Pump.fun is monitored out of the box; add any program ID from the Overview page. Detectors arm after a five-minute baseline, so they judge a program against its own real behaviour.

### Environment

| Variable | Default | Purpose |
|---|---|---|
| `YELLOWSTONE_ENDPOINT` | — | Solami gRPC endpoint, e.g. `https://grpc.solami.dev` |
| `YELLOWSTONE_TOKEN` | — | Solami API key (sent as `x-token`) |
| `SOLANA_RPC_URL` | — | Solami RPC URL, used for owner labels in traces and by the canary |
| `BLUR_API_KEY` | `YELLOWSTONE_TOKEN` | Solami key with the `DataApi` permission, for USD prices. Leave unset to fall back to the gRPC key |
| `BLUR_API_URL` | `https://api.solami.dev` | Blur REST base URL |
| `BEAM_API_URL` | `https://api.solami.dev` | Base URL for Beam landing and tip lookups |
| `MIRAGE_API_KEY` | `BLUR_API_KEY`, then `YELLOWSTONE_TOKEN` | Key for Mirage, which needs `MirageView`, `MirageManage` and `MirageStream`. Sentinel creates and maintains the subscription; no URL needed |
| `MIRAGE_STREAM_URL` | — | Manual override: a ready-made stream URL (`wss://ws.solami.dev/mirage/stream/{id}?api_key=…`). Not needed normally |
| `SENTINEL_MIRAGE` | `auto` | Set to `off` to disable Mirage failover |
| `MIRAGE_LABEL` | `sentinel` | Name of this instance's Mirage subscription; give each instance on one account its own |
| `PORT` | — | Used when `SENTINEL_PORT` is unset (hosting platforms set it) |
| `SENTINEL_TRANSPORT` | `grpc` | Set to `mirage` to use Mirage as the primary transport |
| `SENTINEL_PROGRAMS` | — | Comma-separated program IDs to monitor at startup |
| `SENTINEL_PORT` | `8080` | HTTP port (API + dashboard) |
| `SENTINEL_DB` | `sentinel.db` | SQLite file |
| `SENTINEL_PUBLIC_URL` | `http://localhost:8080` | Base URL for incident links in webhooks |
| `SENTINEL_ADMINS` | — | Comma-separated operator wallets. Only they can change shared detection settings, and they skip the caps below |
| `SENTINEL_MAX_WATCHED` | `10` | Programs one wallet can watch |
| `SENTINEL_MAX_RULES` | `25` | Alert rules one wallet can own |
| `SENTINEL_AUTH_PER_MIN` / `SENTINEL_WRITES_PER_MIN` | `20` / `60` | Per-IP limits on sign-in requests and on everything that writes |
| `SENTINEL_TRUST_PROXY` | — | `1` to take the client IP from `X-Forwarded-For` (behind a reverse proxy) |
| `SENTINEL_MCP_PER_MIN` / `SENTINEL_MCP_WRITES_PER_MIN` | `120` / `20` | Per-token limits on MCP calls, and on the ones that change things |
| `SENTINEL_METRICS_TOKEN` | — | If set, `/metrics` requires `Authorization: Bearer <token>` |
| `SENTINEL_CLUSTER` | `mainnet` | `devnet` or `testnet`: explorer links follow it and the dashboard shows a badge. What is streamed is decided by the endpoints you configure |
| `SENTINEL_TELEGRAM_API` / `SENTINEL_SLACK_API` / `SENTINEL_PAGERDUTY_API` | official endpoints | Override where Telegram, Slack (app) and PagerDuty deliveries go (tests and mocks) |
| `SENTINEL_ALLOW_PRIVATE_WEBHOOKS` | — | `1` allows webhooks to private and loopback addresses (local dev only; blocked by default) |
| `SENTINEL_SIMULATE` | — | **Dev only.** Program ID to feed with synthetic traffic instead of gRPC |

### Check it against the real world

Two read-only examples run the code against real services. Neither needs a running server.

```bash
# Real mainnet traffic through Sentinel: instructions, decoded events, the rules the IDL suggests.
# Uses the public RPC (rate limited, so keep --count small); pass your own with --rpc.
cargo run --release --example verify_mainnet -- 6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P --count 30

# One real test message per channel you configure, through Sentinel's own delivery code.
# PagerDuty gets a trigger and then a resolve. Exits non-zero if any channel fails.
SLACK_WEBHOOK_URL=… TELEGRAM_BOT_TOKEN=… TELEGRAM_CHAT_ID=… PAGERDUTY_ROUTING_KEY=… \
  cargo run --example verify_channels
```

`cargo run --example fetch_idl -- <program>` prints the Anchor IDL a program keeps on chain.

### Seed a demo database

```bash
cargo run --example seed_demo -- demo.db      # a day of rollups, an upgrade and the failures it caused, a vault outflow
SENTINEL_DB=demo.db SENTINEL_PROGRAMS=6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P cargo run
```

### Frontend development

```bash
cargo run            # API on :8080
cd web && npm run dev   # Vite on :5173, proxies /api
```

## A controlled incident on mainnet

1. Create a throwaway wallet with a little SOL:

   ```bash
   solana-keygen new -o canary-keypair.json
   ```

2. In Sentinel, monitor the canary wallet's address. Any account works as a filter, not just programs.
3. Add an alert rule for that wallet: **Failed transactions exceed 3 over 60s**, with a channel such as Telegram, Slack or Discord (a Discord webhook works well on video).
4. Send failing transactions (invalid UTF-8 memos that land and fail on chain):

   ```bash
   cargo run --example canary -- --count 8 --fail
   ```

The transactions travel Solami gRPC → Vortex decoder → Sentinel rule. An incident opens with the transactions linked, the channel is notified, and the delivery appears on the Alerts page. When the incident resolves, the channel hears about that too. Each transaction costs about 5,000 lamports plus a small priority fee.
