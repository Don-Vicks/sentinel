# Demo script (2:45, live on mainnet)

The rule is "runs live on mainnet, and clearly shows the product works", so every shot uses the real stream: no simulator and no pre-recorded data.

## Before recording

- [ ] `.env` has the Solami key. `docker compose up --build` (or `cargo run --release`) is running.
- [ ] Start **at least 15 minutes early**, so Pump.fun has a real baseline and a transaction history.
- [ ] Sidebar stream panel shows **Live**, ingest tx/s > 0, and Blur prices with no error.
- [ ] Signed in with your wallet (sidebar, bottom), and Pump.fun is starred (**Watch** on its page). Rules only fire for programs you watch.
- [ ] A Discord channel is open in a second window. Its webhook is in an alert rule: **"An incident opens with severity at least: medium"**, scope all programs. Press **Test** once to confirm it arrives.
- [ ] Canary fallback: fund `canary-keypair.json` with ~0.01 SOL. Monitor its address in Sentinel and add a rule: **Failed transactions exceed 3 over 60s**, with the Discord webhook.
- [ ] Find a real Pump.fun trade with a USD value flow and note its signature. Also find one failed transaction with an IDL error message.
- [ ] Browser at 1440×900, dark mode, other tabs closed, notifications off.

## Shot list

| Time | Screen | Say |
|---|---|---|
| 0:00 | Overview, Pump.fun card live | "Your Solana program starts failing on mainnet. Explorers show you one transaction at a time. Sentinel tells you which error, in which instruction, since when, and who it hits, within a second, then pages you." |
| 0:15 | Sidebar stream panel | "All of it comes live off Solami's Yellowstone gRPC stream, decoded by Vortex, my Rust transaction stack. That's the ingest rate, the slot, and lag behind the chain tip. USD prices come from Solami Blur." |
| 0:25 | Program page: stats, TPS chart | "Pump.fun, live. Success versus failure every second, compute, unique signers, each compared with its own baseline." |
| 0:40 | Instructions table, "Why transactions fail" | "Per instruction: Buy versus Sell failure rates. And the reason failures happen, grouped by where they fail and why. The names come from Pump.fun's own on-chain IDL." |
| 0:55 | Live transactions feed | "This is streaming in real time. Failed transactions show their decoded error." |
| 1:05 | **Trigger**: run the canary in a terminal (`cargo run --example canary -- --count 8 --fail`), or wait for a natural spike if one is open | "Let's break something on purpose: real failing transactions, landing on mainnet right now." |
| 1:20 | Discord pings; Incidents page updates live | "Sentinel caught it, opened an incident, and paged me on Discord." |
| 1:30 | Incident page: "Why this fired", stats, timeline | "Every alert explains itself: the observed rate, the normal baseline, and the exact threshold rule. No black-box ML. The timeline shows onset, detection, and the error types driving it." |
| 1:50 | Affected transactions → open one | "Every incident links to the actual transactions behind it." |
| 2:00 | Transaction page: narrative, value flow (USD), call tree | "Sentinel explains the transaction in plain language: who called what, where value moved and what it was worth, through Blur prices, and which program in the call tree failed, with its compute cost." |
| 2:20 | Instructions expanded: decoded args, named accounts, IDL error message | "Arguments and account names are decoded from the program's IDL: max SOL cost, bonding curve, user. The error comes with the program's own message." |
| 2:30 | README: diagram and numbers | "Open source: Solami gRPC, RPC and Blur, feeding a Rust decoder at thirteen thousand transactions per second. One `docker compose up` to run it on your own program." |
| 2:45 | End on the Overview | "Vortex Sentinel. Incidents, not transactions." |

## If something goes wrong live

- **No natural spike during recording:** use the canary. It's deterministic.
- **The stream shows "Waiting for data":** check the key's gRPC access. The Pro trial covers it.
- **Blur shows "error":** the key lacks `DataApi`. Set `BLUR_API_KEY` or skip the USD line. Everything else still works.
