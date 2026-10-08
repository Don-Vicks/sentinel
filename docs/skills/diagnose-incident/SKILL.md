---
name: diagnose-incident
description: Work out what happened in a Vortex Sentinel incident on a Solana program, and what to do about it. Use when the user mentions a Sentinel incident, a failure spike, a program upgrade that broke something, a vault outflow, or asks "what's wrong with <program>".
---

# Diagnose a Sentinel incident

This skill needs the Sentinel MCP server (see `docs/MCP.md`). It reads recorded data and says how sure it is; it does not guess past the data.

## Steps

1. **Find the incident.** If the user gave a number, use it. Otherwise call `list_incidents` (status `open`, or `all` for a past one) and, if there is more than one, ask which.
2. **Get the diagnosis.** Call `diagnose_incident` and `get_incident` for it. Note the `confidence`, `likely_cause`, `evidence` and `suggested_steps`.
3. **Look at real transactions.** Call `explain_transaction` on two or three of `example_transactions`, including one that failed. Compare the failing frame with the diagnosis: is the error raised by the monitored program, or by something it calls?
4. **Check what changed.** If the diagnosis mentions an upgrade or a dependency, call `get_posture` for who can upgrade the program and `list_dependencies` for what it calls. If the incident is about a vault, call `list_vaults`.
5. **Check health.** Call `get_health` so you can say whether the program is still affected now.

## Answer

Write, in this order, and keep it short:

- **What happened**, with the numbers from the tools (transactions, wallets, when it began, how long).
- **Likely cause**, with the diagnosis's confidence and the evidence for it. Say plainly where you are inferring beyond the recorded data.
- **Who is affected**, from the wallets and transactions.
- **Do now**, then **do afterwards**.

If the incident is an upgrade, authority change or vault outflow that the user does not recognise, treat it as a possible security incident first: say so, and put verifying the signer and the transaction before anything else.

## Do not

- Do not call write tools (`update_incident_status`, `mute_program`, ...) unless the user asks.
- Do not ask the user to paste bot tokens, routing keys or webhook URLs into the conversation.
