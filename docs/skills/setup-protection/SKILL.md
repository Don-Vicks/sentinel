---
name: setup-protection
description: Set up monitoring and alerts for a Solana program with Vortex Sentinel, step by step. Use when the user wants to watch a program, be paged about it, or asks what alerts they should have.
---

# Set up protection for a program

This skill needs the Sentinel MCP server with a **write** token (see `docs/MCP.md`). Ask before creating anything.

## Steps

1. **Watch it.** Call `list_programs`. If the program is not there, call `watch_program` with its address.
2. **Say who can change it.** Call `get_posture`. Report the upgrade authority and, if it is a single key, recommend moving it to a multisig.
3. **Name the money.** Call `list_vaults`. If none are watched and `candidates` exist, ask the user which are the program's treasury or vaults, then `set_vaults`. Explain that a drain opens an incident when a vault loses 20% of its balance or $100,000 in ten minutes.
4. **Choose where alerts go.** Call `list_rules`. If any rule already has channels, offer to reuse them with `channels_from_rule` so no secret is handled. Otherwise ask which channel they use. Never ask them to paste a bot token, routing key or webhook into this chat: tell them to add the channel on the Alerts page, then reuse it.
5. **Create the rules.** Call `protect_program`. It creates: any incident of high severity or above, failure rate above 20%, an admin instruction from a wallet that never called one before, health below 60, and Sentinel's own feed problems. Rules that already exist are skipped.
6. **Prove it works.** Call `test_rule` on one of the new rules, then `list_deliveries` and confirm it arrived.
7. **Offer a daily summary.** Explain it can be scheduled to the same channel from the Summary tab of the program page, and `send_summary_now` sends an existing schedule immediately.

## Afterwards

Summarise what was created, what already existed, and the `next_steps` the tool returned. For a deploy, suggest `mute_program` for the duration so the team is not paged by its own release.
