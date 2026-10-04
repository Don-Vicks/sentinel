# Submission draft: Superteam Earn, "Build something live on Solana data" (Solami)

Fill in the link marked TODO (the demo video) once it is uploaded. Paste each section into the matching form field.

**Project name:** Vortex Sentinel

**One-liner:** Real-time incident detection for Solana programs. It tells you which error, in which instruction, since when and for whom, within a second, and pages you.

**Links**
- Repo: https://github.com/Don-Vicks/sentinel
- Vortex (the streaming and decoding layer it runs on): https://github.com/Don-Vicks/vortex
- Demo video: TODO
- Live instance: https://sentinel-production-b1d2.up.railway.app (watching a set of programs on mainnet; sign in with any Solana wallet to keep your own watchlist and alerts)

## Description

When a Solana program starts failing on mainnet, teams find out from users, then dig through explorers one transaction at a time. Sentinel watches a program live and turns its transaction stream into incidents: what broke, which error, in which instruction, since when, how many wallets were hit, and the exact transactions. Then it pages you through Discord, Slack or any webhook.

- **Failure fingerprints.** Every failed transaction is attributed to the deepest failing frame of its call tree. Error names and messages come from the program's own on-chain Anchor IDL, e.g. "83% of failures are `TooMuchSolRequired` in `Pump.fun::Buy`".
- **Explainable detectors, no ML:**
  - failure-rate spikes
  - one error type surging, or a brand-new error appearing, even when the overall rate looks normal
  - activity spikes and stops
  - compute spikes
  - large transfers by amount or USD value

  Each incident states its rule in plain numbers, links to its transactions, and keeps a timeline with onset and detection markers.
- **Investigation of any transaction:**
  - a plain-language narrative
  - value flow between labelled parties, priced in USD
  - the program call tree with per-program compute
  - IDL-decoded arguments and named accounts
  - account state changes
- **Alert rules and webhooks,** with a delivery log showing status and latency.

## Built on my own Vortex library

Sentinel uses [Vortex](https://github.com/Don-Vicks/vortex) for ingest. Vortex is my own project (same author, same GitHub account), started in June as a transaction execution stack. The decoder, the fan-out hub, the RPC frame rebuild and the Mirage transport were written for this submission: 1,842 lines (tests and examples included) in public commits from Sep 28 to Oct 1. Everything that makes Sentinel a product, about 6,500 lines of Rust plus a React dashboard, is in this repo. The README's "Built on Vortex" section has the commit links.

## How Solami is used

- **Yellowstone gRPC is the primary data path.** One subscription streams every monitored program, including failed transactions, through server-side `account_include` filters. Adding a program in the UI updates the filter over the open stream without reconnecting. Vortex decodes each frame: errors, compute, the call tree from logs, and SOL/SPL transfers.
- **Mirage** is the failover: the same Yellowstone frames over a WebSocket, through the same decoder. If gRPC can't connect, Sentinel switches, says so in the status bar, and retries gRPC.
- **Beam** (read side) shows how any transaction landed, including route, region, tip and forwarding latency, and labels Beam tips in the narrative. Public endpoints, no key. Sentinel doesn't send through Beam.
- **Blur** (`POST /data/token/price`) prices every mint Sentinel sees move. That powers USD value flow and the "transfer worth ≥ $X" alerts, with a liquidity guard so thin meme tokens can't trigger false alerts.
- **RPC** (with Comet's fast program-account scans) fetches on-chain Anchor IDLs, finds every program an upgrade authority controls, resolves account owners so vaults are labelled by the program that controls them, and lets you investigate any signature through the same decoder.

## Tech

- Rust (Tokio, Axum, SQLite) with a React dashboard, live over SSE. Decoder at ~13K tx/s, engine at ~45K tx/s on one core.
- Tests run against real mainnet Pump.fun transactions and Pump.fun's real on-chain IDL, including a version 1 transaction.
- `docker compose up` to run it.
