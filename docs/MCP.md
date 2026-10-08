# Sentinel over MCP

Sentinel runs a [Model Context Protocol](https://modelcontextprotocol.io) server, so an agent can see what your programs are doing, work out why something broke and, with permission, set up the alerts. It is a thin layer over the same code the dashboard uses: every tool goes through the same limits and ownership checks as the REST API.

## Connect

1. Sign in on the **Alerts** page and create a token under *Connect an agent*. **Read only** can look at everything your wallet can. **Read and change** can also watch programs, create rules and mark incidents. The secret is shown once.
2. Point your client at it.

**Claude Code** (HTTP):

```bash
claude mcp add --transport http sentinel https://YOUR-SENTINEL/mcp \
  --header "Authorization: Bearer snt_..."
```

**Claude Desktop, Cursor and other clients that start a local command:** build the bridge (`cargo build --release --bin sentinel-mcp`) and add

```json
{
  "mcpServers": {
    "sentinel": {
      "command": "sentinel-mcp",
      "env": { "SENTINEL_URL": "https://YOUR-SENTINEL", "SENTINEL_TOKEN": "snt_..." }
    }
  }
}
```

Revoke a token on the same page; access ends immediately. Calls are limited per token (`SENTINEL_MCP_PER_MIN`, default 120, and `SENTINEL_MCP_WRITES_PER_MIN`, default 20).

## Tools

Read (any token):

| Tool | What it returns |
|---|---|
| `list_programs` | Every monitored program: health, throughput, failure rate, open incidents |
| `get_health` | The 0-100 score and each check with the rule and numbers behind it |
| `get_summary` | What a program did over 1h to 30d: activity, value, top instructions and errors, incidents, program changes, next actions |
| `list_incidents` | Recent incidents, filterable by program, status and kind |
| `get_incident` | One incident with its evidence and example transactions |
| `diagnose_incident` | Likely cause, confidence, evidence, similar incidents, next steps |
| `get_incident_report` | A markdown post-mortem |
| `explain_transaction` | Narrative, value flow, call tree and decoded instructions for any signature |
| `get_posture` | Upgrade authority, single key or program-controlled, last deploy, risks |
| `list_vaults` | Watched vaults with balances and net flow, plus candidates |
| `list_dependencies` | Programs it calls, how often, and which are watched for upgrades |
| `list_rules`, `list_deliveries` | Your rules (secrets masked) and what was delivered |

Change (write token):

| Tool | What it does |
|---|---|
| `watch_program` | Start monitoring a program |
| `create_rule` | Create an alert rule. Pass `channels_from_rule` to reuse an existing rule's channels, so a bot token or routing key never passes through the agent |
| `test_rule` | Send a test delivery through a rule |
| `update_incident_status` | Mark an incident investigating or resolved |
| `set_vaults` | Replace the vaults watched for drains |
| `protect_program` | Create the usual rules (high-severity incidents, failure rate, admin call from a new wallet, health, feed problems) on the channels given or copied with `channels_from_rule`; rules you already have are left alone |
| `mute_program` | Hold notifications for a program for a while (a deploy); incidents are still recorded. 0 minutes lifts it |
| `send_summary_now` | Deliver a scheduled summary immediately |

Programs can be named by address or by label (`"Pump.fun"`).

## Resources and prompts

Resources: `sentinel://program/{program}/health`, `.../summary`, `sentinel://incident/{id}/report` and `.../diagnosis`.

Prompts: `triage-incident` (diagnose, inspect example transactions, write up cause, blast radius and actions), `daily-standup` (a short status update for a channel) and `setup-protection` (watch, set vaults, reuse channels, create rules, test them).

## Try it

> "Which of my programs is unhealthy, and why?"
> "Triage incident 1002."
> "Post-mortem for the last failure spike on Pump.fun, as markdown."
> "Set up protection for this program, reusing my Telegram channel."

## Agent skills

Two ready-made skills use these tools: [`diagnose-incident`](skills/diagnose-incident/SKILL.md) and [`setup-protection`](skills/setup-protection/SKILL.md). Copy the folders into your agent's skills directory (for Claude Code, `.claude/skills/`).

## What it does not do

Diagnosis is built from recorded data and says how confident it is; it does not guess beyond it. There is no tool to delete rules, tokens or programs, and an agent cannot read a rule's secrets back.
