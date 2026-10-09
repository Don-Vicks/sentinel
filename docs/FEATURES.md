# Features

Everything Sentinel does, in detail. For the short version, see the [README](../README.md).

## What it does

- **Live program health.** TPS, failure rate against its baseline, compute units, unique signers and fees, updated every second. The transaction feed streams in as it lands.
- **Failure fingerprints.** Every failed transaction is attributed to the deepest failing frame in its call tree, as *program::instruction → error* (Anchor error names come from logs). For example: "83% of failures are `TooLittleSolReceived` in `Pump.fun::Sell`".
- **Explainable detection.** No ML. Each detector compares a short window against a trailing baseline and states the rule it applied with real numbers:
  - failure-rate spike
  - error-type spike: one error the monitored program itself raised surging against its own baseline, or a never-seen one appearing, even when the overall failure rate looks normal. The threshold uses how much that error actually swings from minute to minute, and it must account for at least 1% of traffic. Errors from other programs in the same transactions (a router's downstream pools, a bot's own program) count toward the failure rate but don't open incidents of their own
  - activity spike (mean + zσ)
  - activity stopped
  - compute spike
  - large transfer, by amount per asset or by USD value (Blur-priced, liquid tokens only)

  Seconds inside an open incident are left out of later baselines, so one incident doesn't mask the next.
- **It doesn't blame programs for its own blindness.** Slots arrive several times a second, so a chain tip that stops advancing means the feed is down, not that every program went quiet. Sentinel pauses all detectors, keeps those seconds out of every baseline, shows "Feed stalled" in the status bar, and holds off for 90 seconds after recovery. Vortex also reconnects any gRPC stream that goes 30 seconds without a message. A program that goes quiet while the chain keeps moving still raises "Activity stopped".
- **Per-instruction health.** Volume, failure rate and compute per instruction (Buy, Sell, …) over the last 5 minutes.
- **Incident timelines.** The metric around each incident, with baseline, threshold, onset, detection and resolution markers, plus each error type's count over time. The timeline is saved with the incident when it resolves.
- **Anchor IDL decoding.** Sentinel fetches each program's IDL from chain (current and legacy formats). It names instruction accounts (`bonding_curve`, `user`, …), decodes arguments (`amount`, `max_sol_cost`), and translates bare custom error codes into names and messages.
- **Incidents** link to the actual transactions (stored, so they survive the in-memory window). They record affected wallets, onset, peak, detection latency, and auto-resolve.
- **Investigation.** For any transaction, Sentinel shows the items below. Recent and incident-linked transactions come from memory or SQLite; any other signature is fetched through Solami RPC and run through the same Vortex decoder.
  - a plain-language narrative
  - value flow between labelled parties (PDAs are resolved to their owner program through RPC)
  - net balance changes
  - the program call tree with per-program compute
  - account state diffs, instructions and logs
- **Alert rules.** Supported conditions: failure rate, failed count, TPS, avg or max compute over a window, single transfers above a size or a USD value, or any incident above a severity. Each rule can open an incident, notify one or more channels (Slack, Telegram, PagerDuty, Discord or a webhook), or both. Every delivery is logged with its channel, event (opened, updated, resolved, test or summary), status and latency. [More below.](#watch-the-program-not-only-its-traffic)
- **Accounts: Sign-In With Solana.** Your wallet signs a one-time message (domain, nonce, 5-minute expiry). There are no passwords and nothing goes on chain. Each wallet has its own watchlist, alert rules, webhooks and delivery log; a rule only fires for programs its owner watches. Streaming and detection are shared per program, so everyone watching Pump.fun sees the same incidents. Browsing is public and read-only; watching programs and managing alerts need a signed-in wallet.
- **Stream health.** The dashboard shows ingest tx/s, current slot, tip lag in slots, time since the last transaction, and events dropped.

## Watch the program, not only its traffic

Failure rates and volume say how a program is behaving. These say what is being done *to* it, and tell the right people:

- **Alerts follow the incident, in Slack, Telegram, PagerDuty, Discord or a webhook.** A rule can notify several channels, each with its own minimum severity (page on critical, chat on medium). The first message says what broke: the top failing `program::instruction → error`, wallets affected, how fast it was detected. An escalation and the resolution reply to that message in Telegram, and PagerDuty triggers and resolves one alert per incident, so nobody is paged twice and nothing is left open. Bot tokens and routing keys are stored server-side, masked in the API and scrubbed from error text. **Step-by-step setup for each channel, with troubleshooting: [docs/CHANNELS.md](CHANNELS.md).**
- **Program upgrades and authority changes.** Sentinel reads the upgradeable loader's instructions, including ones a multisig executes through CPI, and opens an incident when a watched program is upgraded, its upgrade authority moves, it is made immutable or it is closed. The program page shows who can upgrade it now: a single wallet (flagged), a program-controlled authority such as a multisig, or nobody.
- **Deploy correlation.** An incident that starts within 30 minutes of an upgrade carries it as evidence: "this began 74s after the program was upgraded", in the explanation, the alert and the post-mortem.
- **Vault drains.** Name a program's treasury or vault accounts (Sentinel suggests likely ones from traffic) and it opens an incident when one loses a set share of its balance, or a set dollar amount, within ten minutes. Deposits offset withdrawals, so ordinary churn is not an incident.
- **Alerts on what an instruction was asked to do.** Match a call by name (`withdraw`, `set_*|update_*`), by decoded arguments (`args.amount > 1000000000`), by named accounts (`accounts.authority = …`) or by signer, using the program's on-chain IDL. "First time this wallet has called it" turns the same rule into an admin-call-from-a-stranger alarm.
- **Alerts on what the program said it did.** Anchor events (`emit!` and `emit_cpi!`) are decoded with the program's IDL and matched by name and field (`fields.sol_amount > 60000000`, `fields.is_buy = true`), and the Activity tab lists the latest ones.
- **Dependencies.** Sentinel learns which programs yours calls (oracles, AMMs, routers) from its call trees, watches the busiest for upgrades, and when trouble starts soon after one it says so: "began 74s after Jupiter, a program yours calls, was upgraded". Squads multisig upgrades are recognised and named, with the member who signed and how many signatures the multisig requires.
- **One wallet taking over.** A fee payer that suddenly sends most of a program's traffic, well above that program's own norm, opens an incident. A program that is always mostly bots is left alone.
- **Health check.** A 0-100 score from separate checks (failure rate against its baseline, open incidents, transactions arriving, compute, funds, decoding coverage, upgrade authority). Each states the rule it applied and the numbers it saw, and a check with nothing to judge says so instead of passing.
- **Daily and weekly summaries.** What the program did: transactions and trend, success rate, unique wallets, value moved, largest transfers, top instructions and errors, incidents with time to detect and resolve, program changes, and what is worth doing next. Built from stored hourly rollups, so it survives restarts. Read it on the dashboard or schedule it to any chat channel.
- **Post-mortems and diagnosis.** Any incident exports a markdown post-mortem, and a deterministic diagnosis names the likely cause, how confident that is and why, similar earlier incidents, and next steps. It is built only from recorded data.
- **Telegram, both ways.** Incident messages carry an Acknowledge button, and the chat can ask `/status`, `/incidents`, `/health`, `/summary 7d`, `/mute 30m`, `/ack 1001`. Sentinel only answers chats a rule already sends to, and only about programs that rule's owner watches.
- **Maintenance windows.** Mute a program for a deploy: notifications are held and the delivery log says "Held", incidents are still recorded, and a resolution is not sent for an incident nobody heard begin.
- **Rules about Sentinel itself and about keepers.** Be told when the feed stalls or RPC keeps failing (and when it recovers), when the health score drops below a number, or when a wallet's SOL balance runs low.
- **Warm start.** Add a program and Sentinel loads its last 15 minutes over RPC, so detectors have a baseline at once instead of after five minutes.
- **Rules suggested from the program's own IDL.** Sentinel reads the IDL for instructions and events that change who is in charge, pause the program, move funds out or change its settings, says why each is proposed, and creates the ones you pick on your channel. A withdrawal can come with a size threshold you choose. Severities now run from `info` (worth knowing, never lowers the health score) to `critical`.
- **One step to protect a program.** The usual rules (high-severity incidents, failure rate, admin calls from new wallets, health, feed problems) on the channel you choose, without duplicating any you have.
- **A public status page and badge.** `/status/<program>` shows health, 7-day uptime and recent incidents to anyone; `/badge/<program>.svg` is a README badge. `/metrics` exposes everything to Prometheus.
- **An MCP server** so an agent can use all of this. See [docs/MCP.md](MCP.md).
