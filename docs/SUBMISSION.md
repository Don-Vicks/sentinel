# Submission draft: Superteam Earn, "Build something live on Solana data" (Solami)

Fill in the two links marked TODO after recording. Paste each section into the matching form field.

**Project name:** Vortex Sentinel

**One-liner:** Real-time incident detection for Solana programs. It tells you which error, in which instruction, since when and for whom, within a second, and pages you.

**Links**
- Repo: https://github.com/Don-Vicks/sentinel
- Vortex (the streaming and decoding layer it runs on): https://github.com/Don-Vicks/vortex
- Demo video: TODO
- Live instance: TODO (optional)

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

## How Solami is used

- **Yellowstone gRPC is the only data path.** One subscription streams every monitored program, including failed transactions, through server-side `account_include` filters. Adding a program in the UI updates the filter over the open stream without reconnecting. Vortex decodes each frame: errors, compute, the call tree from logs, and SOL/SPL transfers.
- **Blur** (`POST /data/token/price`) prices every mint Sentinel sees move. That powers USD value flow and the "transfer worth ≥ $X" alerts, with a liquidity guard so thin meme tokens can't trigger false alerts.
- **RPC** fetches on-chain Anchor IDLs, resolves account owners so vaults are labelled by the program that controls them, and lets you investigate any signature through the same decoder.

## Tech

- Rust (Tokio, Axum, SQLite) with a React dashboard, live over SSE. Decoder at ~13K tx/s, engine at ~45K tx/s on one core.
- Tests run against real mainnet Pump.fun transactions and Pump.fun's real on-chain IDL, including a version 1 transaction.
- `docker compose up` to run it.
