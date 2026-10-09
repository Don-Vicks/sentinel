# Alert channels

Sentinel can send an alert to **Slack, Telegram, PagerDuty, Discord or any webhook**. A rule can use up to five channels at once, each with its own minimum severity, and every delivery is logged with its status and latency.

- [How alerts behave](#how-alerts-behave)
- [Slack](#slack)
- [Telegram](#telegram)
- [PagerDuty](#pagerduty)
- [Discord](#discord)
- [Webhook](#webhook)
- [Testing a channel](#testing-a-channel)
- [Saved destinations](#saved-destinations)
- [Troubleshooting](#troubleshooting)
- [What has and hasn't been verified](#what-has-and-hasnt-been-verified)

## How alerts behave

An alert follows an incident from open to resolved, so a bad hour does not become fifty separate messages.

| Event | When | Slack app | Telegram | PagerDuty | Slack webhook, Discord, webhook |
|---|---|---|---|---|---|
| **Opened** | The incident starts | New message | New message with buttons | `trigger` | New message |
| **Escalated** | Its severity rises | Reply in the thread | Reply to the first message | `trigger` again, same alert | New message |
| **Resolved** | It clears | Reply in the thread | Reply to the first message | `resolve` | New message |

- **Minimum severity.** Each channel can say "only if severity is at least high". Typical use: PagerDuty at `critical`, chat at `medium`. Severities, lowest to highest: `info`, `low`, `medium`, `high`, `critical`. Test messages and scheduled summaries ignore the filter.
- **Order.** Events for one incident and channel are delivered in order, so an escalation never overtakes the message it replies to.
- **Retries.** A failed delivery is retried up to three times, waiting 0.5 s then 1.5 s. A client error (a bad token, a missing channel) is not retried, except rate limits.
- **Cooldown.** A rule has a cooldown (default 300 s) so one noisy condition cannot repeat every second.
- **Secrets.** Bot tokens, integration keys and the secret part of a webhook URL are stored on the server, shown masked (`••••abcd`) everywhere, and removed from error messages. Sending a masked value back when you edit a rule keeps the stored secret.
- **Where to set them up.** In the dashboard, **Alerts**, then **Destinations** (to save a channel) or **Rules** (to attach one to a rule). Over the API, a rule's `channels` list. Over MCP, `create_rule` and `protect_program`.

A rule with no channel still opens incidents on the dashboard; it just notifies nobody.

## Slack

There are two ways. **Use the Slack app.** It keeps an incident's updates in one thread, which a plain webhook cannot do.

### Slack app (recommended)

1. Go to <https://api.slack.com/apps> and click **Create New App**, then **From scratch**. Name it (for example "Vortex Sentinel") and pick your workspace.
2. Open **OAuth & Permissions**. Under **Bot Token Scopes**, add `chat:write`. To post in public channels the bot has not joined, also add `chat:write.public`.
3. Click **Install to Workspace** and approve it.
4. Copy the **Bot User OAuth Token**. It starts with `xoxb-`.
5. In Slack, invite the bot to the channel: type `/invite @Vortex Sentinel` in it. (Not needed for public channels if you added `chat:write.public`.)
6. In Sentinel, **Alerts**, **Destinations**, **Slack app**. Paste the token, enter the channel (`#alerts`, or its channel id like `C0123456789`), click **Send test**, and if it says "Delivered", click **Save to reuse**.

API form:

```json
{ "type": "slack_bot", "bot_token": "xoxb-...", "channel": "#alerts", "min_severity": "medium" }
```

What you get: the opening message is a rich Block Kit card (what broke, the top failing instruction, wallets affected, next steps, a button to the incident). The escalation and the resolution are replies in that message's thread.

Slack answers HTTP 200 even when it refuses a message, so Sentinel reads Slack's own error and tells you the fix:

| Slack says | Sentinel shows |
|---|---|
| `not_in_channel` | the bot isn't in that channel; run `/invite @YourApp` there |
| `channel_not_found` | channel not found; use `#name` for a public channel or the channel id |
| `invalid_auth`, `not_authed`, `token_revoked` | the bot token is invalid or revoked |
| `missing_scope` | the app needs the `chat:write` scope (add `chat:write.public` to post without an invite) |
| `is_archived` | that channel is archived |

### Slack webhook (simpler, no threads)

1. In your Slack app, open **Incoming Webhooks**, switch it on, and click **Add New Webhook to Workspace**. Pick a channel.
2. Copy the URL (`https://hooks.slack.com/services/T…/B…/…`).
3. In Sentinel, **Slack webhook**, paste it, **Send test**.

```json
{ "type": "slack", "url": "https://hooks.slack.com/services/T000/B000/XXXX" }
```

Every update is a separate message.

## Telegram

Telegram works in both directions: Sentinel sends alerts, and the chat can ask questions back.

1. In Telegram, open **@BotFather** and send `/newbot`. Choose a name and a username. BotFather replies with a token like `123456789:AAEhBP0av28…`.
2. Add the bot to the chat or group where alerts should go (for a private chat with yourself, just open the bot and press **Start**). For a channel, add the bot as an administrator.
3. Find the **chat id**:
   - Send any message in the chat, then open `https://api.telegram.org/bot<TOKEN>/getUpdates` in a browser and look for `"chat":{"id":…}`.
   - Groups have a negative id (for example `-1001234567890`). For a public channel you can use `@channelname` instead.
4. In Sentinel, **Alerts**, **Destinations**, **Telegram**. Enter the bot token and chat id, **Send test**.

```json
{ "type": "telegram", "bot_token": "123456789:AAE...", "chat_id": "-1001234567890" }
```

**What arrives.** An HTML-formatted message: the rule and program, what broke, and the facts. The opening message has an **Acknowledge** button; tapping it marks the incident as being investigated. When `SENTINEL_PUBLIC_URL` is an `https://` address it also has a button that opens the incident (Telegram only accepts public https links on buttons). Escalations and the resolution reply to the first message.

**Commands.** Sentinel answers these in a chat that one of your rules already sends to, and only about the programs that rule's owner watches:

| Command | What it does |
|---|---|
| `/status` | Every program, its health and open incidents |
| `/incidents` | What is open right now |
| `/health [program]` | The score and what is pulling it down |
| `/summary [program] [24h\|7d]` | What a program did |
| `/mute [program] 30m` | Hold notifications for a deploy (`30m`, `2h`, `1d`) |
| `/ack <id>` | Mark an incident as being investigated |
| `/resolve <id>` | Mark it resolved |

Here it is working on a real bot: [35-second clip](https://youtu.be/SWyeZFxL5_g).

A chat that no rule sends to is ignored, so a stranger who finds the bot cannot read your programs. If the bot is in a group, it answers commands there.

## PagerDuty

Use PagerDuty for things that should wake someone.

1. In PagerDuty, open the **service** that should be paged (or create one).
2. Go to **Integrations**, **Add an integration**, and choose **Events API v2**.
3. Copy the **Integration Key**: 32 characters.
4. In Sentinel, **PagerDuty**, paste the key, **Send test**. Set the minimum severity to `high` or `critical`; the editor defaults to `high`.

```json
{ "type": "pagerduty", "routing_key": "<32-character key>", "min_severity": "critical" }
```

**How it behaves.** Each incident is one PagerDuty alert, keyed `sentinel-incident-<id>`. Opening triggers it, an escalation updates the same alert, and resolving the incident resolves the alert, so nobody is paged twice and nothing stays open. Severity maps as `critical` to critical, `high` to error, `medium` to warning, `low` and `info` to info. The payload includes the program, the facts, the rule and a link back to the incident.

A **test** sends a trigger and then immediately a resolve, so it never leaves a page open (the summary starts with `[TEST]`).

PagerDuty is for pages, so it is not offered for scheduled summaries.

## Discord

1. In Discord, open the channel's settings, **Integrations**, **Webhooks**, **New Webhook**.
2. Name it, choose the channel, and click **Copy Webhook URL** (`https://discord.com/api/webhooks/…`).
3. In Sentinel, **Discord**, paste it, **Send test**.

```json
{ "type": "discord", "url": "https://discord.com/api/webhooks/123/abc" }
```

Alerts arrive as an embed: a colour by severity, the title, what happened, the facts as fields, next steps, and a link to the incident. Every update is a new message.

## Webhook

Any HTTPS endpoint, for your own tooling.

```json
{ "type": "webhook", "url": "https://example.com/hooks/sentinel" }
```

Sentinel sends a `POST` with a JSON body and these headers:

| Header | Meaning |
|---|---|
| `X-Sentinel-Event` | `alert` |
| `X-Sentinel-Delivery` | A unique id per delivery, for idempotency |

The body has `event` (`sentinel.alert`, `sentinel.incident`, `sentinel.incident.updated`, `sentinel.incident.resolved`, `sentinel.summary` or `sentinel.test`), `lifecycle` (`opened`, `updated`, `resolved`, `test` or `summary`), `rule`, `severity`, `program`, `message`, `incident` (with its evidence) and `links.incident`. Return any `2xx` to acknowledge.

By default Sentinel refuses webhooks that point at private or loopback addresses. For local development only, set `SENTINEL_ALLOW_PRIVATE_WEBHOOKS=1`.

## Testing a channel

Always test before you rely on a channel.

- **In the dashboard.** Every channel row has **Send test**. It sends a real message and shows the result next to the button (for example "Delivered. Check the channel." or Slack's own error).
- **From the command line**, with your own credentials. This uses Sentinel's real delivery code and exits non-zero if any channel fails:

  ```bash
  SLACK_BOT_TOKEN=xoxb-... SLACK_CHANNEL=#alerts \
  SLACK_WEBHOOK_URL=https://hooks.slack.com/services/... \
  TELEGRAM_BOT_TOKEN=123456789:AAE... TELEGRAM_CHAT_ID=-1001234567890 \
  PAGERDUTY_ROUTING_KEY=... \
  DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/... \
  WEBHOOK_URL=https://example.com/hook \
  cargo run --example verify_channels
  ```

  Set only the ones you use. PagerDuty gets a trigger and then a resolve.
- **Over the API.** `POST /api/channels/test` with a channel object tests it before you save it. `POST /api/destinations/{id}/test` tests a saved one. `POST /api/rules/{id}/test` sends a test through every channel on a rule.

## Saved destinations

Instead of pasting a token into every rule, save a channel once under a name and reuse it.

- Dashboard: **Alerts**, **Destinations**. Add a channel, test it, click **Save to reuse**. In a rule's "Who should hear about it?" step, tap the saved destination.
- API: `GET/POST /api/destinations`, `DELETE /api/destinations/{id}`. A rule accepts `destination_ids: [1, 2]`.
- A rule copies the destination's channel when it is saved, so editing or deleting a destination later does not change rules that already use it.
- Destinations belong to the wallet that created them and are never shown to anyone else. You can have 20.

## Troubleshooting

Open **Alerts**, **Deliveries**. Each row shows the channel, the event, the HTTP status, the number of tries, the latency and the error.

| Symptom | Likely cause and fix |
|---|---|
| Slack: "the bot isn't in that channel" | Run `/invite @YourApp` in the channel, or add the `chat:write.public` scope |
| Slack: "the bot token is invalid or revoked" | Copy the **Bot User OAuth Token** again from **OAuth & Permissions**; reinstall the app if you changed scopes |
| Telegram `400 chat not found` | The bot has not been added to the chat, or nobody has sent it a message yet. Re-check the chat id (negative for groups) |
| Telegram `401` | The bot token is wrong or was regenerated in BotFather |
| Telegram `403` | The bot was blocked, or removed from the group |
| PagerDuty `400` | The key is not an **Events API v2** integration key |
| Discord `404` | The webhook was deleted. Create a new one |
| "host is on a private network" | Webhooks must be public. For local development only, set `SENTINEL_ALLOW_PRIVATE_WEBHOOKS=1` |
| "secret is masked; enter it again" | You edited a channel and changed its type; the stored secret can only be kept for the same type |
| Nothing arrives, no delivery row | The rule is disabled, no watched program matches it, or it is in its cooldown or a maintenance window (notifications are held, and listed in the delivery log) |
| An alert is missing a severity level | The channel's "only if severity at least" is higher than the alert |

## What has and hasn't been verified

Be clear about the evidence:

- **Covered by automated tests against mock servers:** the Slack app (including threading and every error above), Slack webhook, Telegram (including replies, the Acknowledge button and the chat commands), PagerDuty (trigger and resolve under one key), Discord and the generic webhook; masking and secret scrubbing; saved destinations.
- **Run against the real services with fake credentials:** each of Slack, Telegram, PagerDuty and Discord answered with its own real rejection (a `401`, `404` or `400`), and Sentinel reported each one correctly.
- **Verified on a real service:** Telegram, end to end, on a real bot on October 9, 2026: an incident alert arrived with its Acknowledge button within seconds, tapping the button moved the incident to "investigating" and the bot confirmed it, and `/status` and `/incidents` answered with live data. `verify_channels` also delivered a test message (HTTP 200).
- **Not yet shown against your own account:** Slack, PagerDuty and Discord. Each answered a fake credential with its own real rejection, but a successful delivery needs your workspace, service or webhook. Run `verify_channels` or use **Send test**; if a channel fails, the error will say why.
