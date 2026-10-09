# Tests

```bash
cargo test
```

`tests/real_transactions.rs` runs fingerprinting and tracing on real mainnet Pump.fun transactions: a failed Buy reached through a bot router, a successful trade, and a version 1 transaction. It asserts, for example, that the failure is attributed to `Pump.fun::Buy → TooMuchSolRequired (6002)`. The decoder itself has matching fixture tests in [Vortex](https://github.com/Don-Vicks/vortex/blob/master/crates/vortex/tests/real_transactions.rs).

`tests/idl.rs` decodes a real mainnet Pump.fun Buy with Pump.fun's real on-chain IDL. It checks the named accounts and decoded arguments against the transfers Vortex decoded independently, plus IDL error naming.

`tests/pricing.rs` runs a mock Blur server with the documented response shape (decimals as strings, key in `x-api-key`). It also checks that USD large-transfer detection ignores thin-liquidity tokens and that a USD alert rule fires on a liquid one.

`tests/auth.rs` drives the real HTTP router through wallet sign-in:
- a valid signature opens a session
- a wrong wallet, a reused nonce or a tampered message are rejected
- public reads work; writes need a session
- one account can't see or delete another's rules, or target programs it doesn't watch
- rules fire only on their owner's programs
- webhooks to private networks are refused

`tests/limits.rs` covers the public-instance guards: watchlist and rule caps (the operator is exempt), operator-only detection settings, watcher-only incident updates, per-IP rate limits, and capped sign-in challenges.

`tests/channels.rs` runs an incident through mock Telegram, PagerDuty and Slack servers and checks the whole lifecycle: Telegram replies to the opening message for the escalation and the resolution, PagerDuty resolves what it triggered under one `dedup_key`, Slack gets Block Kit, the delivery log names channel and event, and no secret reaches it. `tests/channels_api.rs` checks that secrets are masked, kept on edit and validated, and that scheduled summaries are validated and private to their owner. `tests/slack_bot.rs` checks that an incident stays in one Slack thread, that Slack's refusals come back readable, and that saved destinations are tested, reused by rules and private to their account.

`tests/upgrades.rs` feeds synthetic upgradeable-loader instructions (including a failed one, and a SetAuthority that touches only the ProgramData account) and checks the incidents they open, and that a failure spike 30 seconds after an upgrade carries it as evidence. `tests/vaults.rs` checks that churn which nets out is not a drain and that a real net outflow is, with its severity and the vault status the dashboard reads.

`tests/summaries.rs` replays three hours of traffic, checks the exact totals in the summary, and checks the scheduler sends each period once, skips a slot missed by more than six hours, and waits for the next slot after a schedule is created. `tests/mcp.rs` drives the MCP server over HTTP: tokens and scopes, every kind of tool, resources, prompts, ownership between accounts, and revocation.

`tests/instruction_rules.rs` matches calls by name, decoded argument and named account against Pump.fun's real on-chain IDL and a real mainnet Buy, checks that a failed call or a missing IDL changes the outcome as documented, and that a first-time signer is flagged once and remembered in the database. `tests/real_chain.rs` parses real mainnet Squads accounts and transactions kept in `tests/fixtures` (a v4 multisig and proposal, a v5 settings account and proposal, a v5 policy account that must be refused, executions that name their vault). `tests/suggestions.rs` reads suggestions from Pump.fun's real IDL (authority, funds and config instructions, and events), applies one once, refuses a size threshold left empty, and keeps a large threshold exact. `tests/events.rs` decodes the `TradeEvent` in a real Pump.fun Buy (which the program writes to its log and to a self-CPI, counted once), fires event rules on its name and fields, and checks that a failed transaction or a missing IDL changes the outcome. `tests/backfill.rs` serves real transactions from a mock RPC and checks that history arms the detectors at once, raises nothing, seeds the summaries and is not counted twice. `tests/system_alerts.rs` freezes the chain tip and checks one announcement and one recovery, for only the rules that asked.

`tests/squads_rules.rs` reads a mock proposal account (two approvals, then three) and checks the incident says so once; it also fires Squads rules on v4 and v5 approvals and executions (transactions that never touch the watched program), on a specific vault, and checks a failed action stays quiet. `tests/squads.rs` runs an upgrade executed through a Squads `vault_transaction_execute` CPI and checks the multisig, the signing member, and the threshold read back from a mock account. `tests/dependencies.rs` checks that called programs are tracked (and the token program is not), that their code accounts are added to the stream, that an upgrade of one opens an incident without being counted as traffic, and that a later failure spike blames it. `tests/concentration.rs` checks a new dominant wallet is flagged and one that was always dominant is not.

`tests/mute.rs` checks a maintenance window holds notifications and logs them as held, still records the incident, holds the resolution of an incident nobody heard begin, and announces the next one after unmuting. `tests/telegram_bot.rs` drives the bot against a mock Telegram API: strangers get no answer, each command answers only for the owner's programs, and the Acknowledge button works from the right chat only. `tests/health_rules.rs` covers health-score and wallet-balance rules and the health history. `tests/metrics.rs` checks the Prometheus output, the optional scrape token, the public status page and the badge.

`tests/pipeline.rs` drives the real engine through a full cycle:
- healthy baseline
- failure spike → incident with linked transactions and fingerprints
- auto-resolve
- a second spike judged against a clean baseline
- a webhook delivered to a local receiver
