# HTTP API

| Method | Path | |
|---|---|---|
| POST | `/api/auth/challenge` | `{pubkey}` → message for the wallet to sign |
| POST | `/api/auth/verify` | `{pubkey, message, signature}` → session cookie |
| POST / GET | `/api/auth/logout`, `/api/auth/me` | End the session / current account and watchlist |
| GET | `/api/status` | Stream health + every program snapshot |
| POST | `/api/resolve` | Paste a program, upgrade-authority address, signature or explorer link; returns the programs it points at `{query}` (public data, no sign-in) |
| GET/POST | `/api/programs` | List / add to your watchlist `{program_id, label?}` 🔒 |
| GET/PATCH/DELETE | `/api/programs/{id}` | Detail (snapshot, 10-min series, recent tx, incidents) / update settings 🔒 / remove from your watchlist 🔒 |
| GET | `/api/programs/{id}/transactions?failed=true` | Recent transactions |
| GET | `/api/incidents?program=` | Incidents, newest first |
| GET/PATCH | `/api/incidents/{id}` | Incident + linked transactions / set `status` |
| GET | `/api/incidents/{id}/timeline` | 10s metric series around the incident |
| GET | `/api/incidents/{id}/report` | Markdown post-mortem |
| GET | `/api/incidents/{id}/diagnosis` | Likely cause, confidence, evidence, similar incidents, next steps |
| GET | `/api/programs/{id}/health` | 0-100 health score with each check |
| GET | `/api/programs/{id}/summary?period=24h` | What the program did over `1h` to `30d` |
| GET | `/api/programs/{id}/posture` | Upgrade authority and what it implies (needs `SOLANA_RPC_URL`) |
| PUT / DELETE | `/api/programs/{id}/idl` | Use an IDL you supply (the raw IDL JSON, up to 2 MB) in place of the chain's, or go back to the chain's |
| GET | `/api/programs/{id}/idl` | Instructions and events, with the accounts, arguments and fields a rule can filter on |
| GET / POST | `/api/programs/{id}/suggestions` | Rules suggested from the program's IDL; POST creates the chosen ones (`ids`, `values`, `channels` or `channels_from_rule`) |
| GET | `/api/programs/{id}/events` | The latest decoded program events, newest first (`?name=`, `?limit=`), with the names seen and counts |
| GET | `/api/programs/{id}/dependencies` | Programs it calls, how often, and which are watched for upgrades |
| PUT | `/api/programs/{id}/mute` | `{minutes, reason?}` hold notifications for a maintenance window (0 lifts it) 🔒 |
| POST | `/api/programs/{id}/protect` | `{channels}` or `{channels_from_rule}`: create the usual rules once 🔒 |
| GET | `/api/public/status/{id}` | What the public status page shows (no sign-in) |
| GET | `/badge/{id}.svg` | Health badge |
| GET | `/metrics` | Prometheus text format |
| GET/PUT | `/api/programs/{id}/vaults` | Vaults watched for drains, with balances and candidates / replace the list 🔒 |
| GET/POST, PATCH/DELETE | `/api/summary-schedules`, `/api/summary-schedules/{id}` | Your scheduled summaries 🔒 |
| POST | `/api/summary-schedules/{id}/send` | Send a summary now 🔒 |
| GET/POST, DELETE | `/api/tokens`, `/api/tokens/{id}` | API tokens for agents; the secret is shown once 🔒 |
| POST | `/mcp` | MCP server (JSON-RPC), `Authorization: Bearer snt_...`. See [docs/MCP.md](MCP.md) |
| GET | `/api/transactions/{signature}` | Decoded transaction + trace |
| GET/POST, PATCH/DELETE | `/api/rules`, `/api/rules/{id}` | Your alert rules 🔒 |
| POST | `/api/rules/{id}/test` | Send a test delivery through every channel of the rule 🔒 |
| GET | `/api/alerts` | Your deliveries (alerts and summaries) 🔒 |
| GET | `/api/stream?program=` | SSE: `transactions`, `metrics`, `incident`, `alert`, `stream` |

🔒 requires a session (wallet sign-in). Changing an incident's status requires watching its program. Changing a program's detection settings is operator-only (`SENTINEL_ADMINS`), since everyone watching it shares them.

Setup instructions for each service (creating the Slack app or the Telegram bot, finding the chat id, getting a PagerDuty key) are in [docs/CHANNELS.md](CHANNELS.md). A rule's `channels` is a list of objects, each with a `type` and an optional `min_severity`:

```json
{ "type": "slack_bot", "bot_token": "xoxb-...", "channel": "#alerts" }
{ "type": "slack",     "url": "https://hooks.slack.com/services/..." }
{ "type": "discord",   "url": "https://discord.com/api/webhooks/..." }
{ "type": "telegram",  "bot_token": "123456:ABC...", "chat_id": "-1001234567890" }
{ "type": "pagerduty", "routing_key": "<32-character Events API v2 key>" }
{ "type": "webhook",   "url": "https://example.com/hook" }
```

Rules created with the older single `webhook_url` keep working; Slack and Discord URLs are recognised. Secrets (bot tokens, routing keys, the path of a webhook URL) are masked in every response; send the masked value back when editing and the stored secret is kept.

Webhook payloads are JSON with `event`, `lifecycle`, `rule`, `severity`, `program`, `message`, `incident` and `links.incident`. `event` is `sentinel.alert`, `sentinel.incident`, `sentinel.incident.updated`, `sentinel.incident.resolved`, `sentinel.summary` or `sentinel.test`; `lifecycle` is `opened`, `updated`, `resolved`, `test` or `summary`. Each request carries an `X-Sentinel-Delivery` id for idempotency.

How each channel handles an incident over its life:

| Channel | Opened | Escalated | Resolved |
|---|---|---|---|
| Telegram | New message, with a button to the incident when `SENTINEL_PUBLIC_URL` is https | Reply to the first message | Reply to the first message |
| PagerDuty | `trigger` with `dedup_key` `sentinel-incident-{id}` | `trigger` again, which updates the same alert | `resolve` with the same key |
| Slack app (`slack_bot`) | New message in the channel | Reply in that message's thread | Reply in that thread |
| Slack webhook, Discord, webhook | New message | New message | New message |

**Slack:** prefer the **Slack app** channel. Create a Slack app with the `chat:write` scope (add `chat:write.public` to post in channels the bot hasn't joined), install it, paste the Bot User OAuth Token (`xoxb-…`) and a channel (`#alerts` or its id), and invite the bot with `/invite @YourApp`. An incident's escalation and resolution then stay in one thread instead of flooding the channel, and Slack's refusals (bot not in the channel, revoked token, missing scope) come back as readable errors. Incoming webhooks still work but can't thread. Set `SENTINEL_SLACK_API` only to point at a test server.

**Saved destinations.** On the Alerts page, add a channel once under a name, send a test message to it, and pick it in any rule instead of pasting the token again (`GET/POST /api/destinations`, `DELETE /api/destinations/{id}`, `POST /api/destinations/{id}/test`; `POST /api/channels/test` tests a channel from the form before saving, and a rule accepts `destination_ids`). Saving a rule copies the destination's channel, so editing or deleting a destination later doesn't change rules that already use it.

Deliveries to one incident and channel go out in order. Summaries are sent to every channel type except PagerDuty. A failed delivery is retried three times with backoff (not on a client error other than a rate limit) and is logged with the error.
