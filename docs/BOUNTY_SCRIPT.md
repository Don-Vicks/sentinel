# Demo script for the Solami bounty, version 4: a true story (planning notes)

> **Source of truth: `docs/NARRATION.md`.** That file has the exact words to record, with every claim verified and the timing budgeted. This file is the planning background (story, evidence, claims, checklist). Where the two disagree, for example the agent prompts, the Protect program, or the Kamino references below, **NARRATION.md wins.**

This version is built around something that **actually happened while Sentinel was running**, so the video has a story and not only features. It is verified on chain (below). It also leads with what a program team pays for: alerts where they work, and an agent that triages. The listing's rules still hold: **2 to 3 minutes, live on mainnet, Solami as the data path, "a submission that does not run live is not judged."**

## The true story

On **October 8, 2026 at 17:56 UTC**, while Sentinel was watching fifteen programs, **a program that seven of them call was redeployed.** Nobody announced it. Sentinel saw the upgrade on the Solami stream about a second after it landed, and opened an incident on **each of the seven programs that depend on it**: Jupiter v6, Jupiter Perps, Kamino Lend, MarginFi, Orca Whirlpool, Meteora DBC and Raydium CPMM. Ten seconds later a second upstream program, one Meteora DBC calls, was upgraded too.

Why it matters, in one sentence you can say: **"Whoever holds a program's upgrade authority can replace its code at any time, so if you call that program, your failures can start in code you never touched."**

**Be honest about what it was.** It was an upgrade, not an exploit, and nothing says it broke anything. Sentinel's own diagnosis says so ("this is a heads-up"). That is the point: a team would want to *know*, and to rule it out in a minute instead of an afternoon.

### Verified (you can show these)

| Fact | Evidence |
|---|---|
| The upgrade happened | The BPF upgradeable loader's `Upgrade` instruction on `BiSoNHVpsVZW2F7rx2eQ59yQwKxzU5NvBcmKshCSUypi`, signature `grdGaPoVYStyEVTyzkiYPJQkbAejh6G7G6oGj1JC3kJbBUpenZgPY1grfjsocL3KhZpKL4RpYTbjiNuyCmD6dhS`, slot 454,617,760, block time 17:56:09 UTC. Checked independently over Solami RPC. |
| Sentinel saw it | Incident detected 17:56:10 UTC, about a second later. Seven incidents, #1019 to #1025. |
| Second upgrade | Program `EVoLVE9obLsZ3AHRNEddTXMtRVvtptLHRmVi1V3ujtWt`, slot 454,617,797 (an extend at 454,617,325 first), incident #1026. Also checked over RPC. |
| Scale | The upgraded program had 38,621 calls seen from Jupiter v6 alone, 16% of its calls. |

**What you must not say:** that it caused failures, that it was malicious, or what the program is called. I do not know its name and Sentinel shows only the address. Say "a program that seven of my watched programs call."

### Keep the evidence

The incidents live in the demo database, which `./scripts/demo.sh` **wipes on every restart**. I saved a copy:

- `demo/story/sentinel-story.db` (the database with incidents #1019 to #1026)
- `demo/story/incidents.json` (the same incidents as JSON)
- `demo/story/story-*.png` (1440 by 900 screenshots of incident #1019, the incident list and Jupiter's Security tab)

To record with the real incident on screen, start from that copy:

```bash
SENTINEL_DB=demo/story/sentinel-story.db ./scripts/demo.sh --keep
```

An upgrade like this will not happen again on cue, so **never promise a live replay.** Present it as what happened, shown from Sentinel's own record, then prove the live part with an alert firing on current traffic.

## The pitch in one line

> "Your Solana program breaks, and you hear about it from users. Vortex Sentinel tells you in seconds, says why, and your AI agent can triage it. On October 8th it caught something upstream that seven programs depend on, a second after it happened."

Say "I built" and "my". You are the builder with a real story.

| Criterion | Where it is earned |
|---|---|
| Meaningful Solami integration | The stream saw the upgrade; RPC loads the IDLs; five products, each with a job |
| Functional demo on mainnet | The real incident, plus an alert firing on live traffic |
| Code quality and accessibility | One command, your own keys, the README and MCP guide |
| Practical value for builders | A dependency you did not know changed, found in a second |
| Creative combination | Watching what your program *depends on*, not only its own traffic, and an agent that can operate it |

## Before recording (20 minutes ahead)

1. **Start from the saved database** (command above). Check all four health lines say OK. Detectors need about five minutes for the live alert.
2. **Destinations saved and tested** on the **Alerts** page: a Discord channel in a second window, and your Slack app if you have one. **Delete any rehearsal rules** so you create them fresh.
3. **Sign in** with your wallet. **Watch the showcase set** so the programs match the story.
4. **Open on-chain proof in a tab:** the upgrade signature on a Solana explorer (paste the signature above). That tab is the credibility shot.
5. **Create a "Read and change" agent token** (Alerts, Agent access). **Keep it off camera.** Add Sentinel to Claude Code (`claude mcp add --transport http sentinel http://localhost:8080/mcp --header "Authorization: Bearer …"`), large font, and dry-run the prompts below.
6. **Screen:** 1440 by 900, dark theme, notifications off. **Never show `.env`, a token or a key.**
7. Keep **Pump.fun, PumpSwap and Meteora DLMM** out of the watch list.

## Shot list

| Time | On screen | Do | Say |
|---|---|---|---|
| 0:00 | Incident **#1019** page | Hold on the title and **What changed** | "On October 8th, at 17:56 UTC, a program that seven of the programs I was watching depends on was redeployed. Nobody announced it. This is my monitor's record of it: the upgrade, the signature, the slot, a second after it landed." |
| 0:15 | The explorer tab with the same signature | Show the `Upgrade` instruction and slot | "And this is the chain agreeing: the same signature, the same slot. It is real." |
| 0:25 | Incident list, then Jupiter's **Security** tab, **Programs it calls** | Scroll to the dependency table | "Whoever holds an upgrade authority can replace that code at any time. So if you call it, your failures can start in code you never touched. I built Vortex Sentinel to tell you, seven programs at once. It was a heads-up, not an attack, and it tells you which." |
| 0:45 | Overview, live numbers, the **Powered by Solami** row | Point at tx/s, "0.0s behind", each tile | "It runs live on mainnet right now, [three hundred] transactions a second, [zero] seconds behind. Solami is the data path: Yellowstone gRPC streamed that upgrade, RPC with Comet loads the IDLs, Blur prices the tokens, Beam shows how a transaction landed, and Mirage is my failover." |
| 1:10 | **Protect this program** card on Jupiter | Pick the saved Discord (or Slack) destination, click | "Telling someone is the point. One click creates the alerts most teams want, on the channel they already use." |
| 1:25 | **Send test**, then the Discord or Slack window | Click Send test | "I test it first. There it is." |
| 1:35 | **Alerts**, **Any incident** or **Transfer worth $10K+**, **Create rule**, then **Deliveries** and the message | Create it, wait for the row | "Here is one built in three steps. It fires on live traffic, and every delivery is logged. In Slack an incident stays in one thread, in Telegram there is an Acknowledge button, and PagerDuty gets one page and resolves it." |
| 2:00 | **Alerts, Agent access**, then the Claude Code terminal | Create the token (hidden), then type: *Triage the dependency incident #1019 for Jupiter. Should I be worried?* | "Now I hand it to my agent. I give it a token and ask it to triage that upgrade." |
| 2:15 | The agent's answer | Let it finish | "It reads Sentinel, tells me how sure it is, and says what to check: whether my failures are raised inside that program, and that if my own program is healthy this is a heads-up." |
| 2:30 | Same terminal | Type: *Set up protection for Kamino Lend, reusing my Discord channel.* | "And it can set up protection, reusing my existing channel, so my bot token never passes through the agent." |
| 2:42 | **Status page**, then the README **Run it** | Click Status page, then scroll | "Each program gets a public status page and a badge. It is open source and runs with one command and your own Solami keys. Vortex Sentinel: it knew a second after it happened." |

## Claims to stay inside

- **Say only what is measured and verified:** the figures above, and the tx/s you read off the screen. Earlier measurements: 15 programs at 60 to 170 tx/s, 0.0 s behind over four minutes on a home link.
- **"A second after":** block times have one-second resolution, so say "about a second", not "within a millisecond".
- **The MCP server is verified live** (23 tools; diagnosing #1019 returned a medium-confidence answer from real data). Slack threading and Telegram buttons are verified against mock servers, not yet your own workspace. Show only what you rehearsed working.
- **No alert was sent for #1019:** no rule was watching it at the time. Do not say "I got a Discord message about it." Say what Sentinel recorded, and show alerts live with a rule created on camera.
- **Beam is read-side.** **Mirage:** say "failover" only if the tile reads Standby.
- **Do not promise detection of an attack on an arbitrary wallet.** Sentinel watches programs and the wallets that interact with them.
- **Disclose the method in the submission text, not the video:** a scripted recording of the live app with narration recorded separately, and a stand-in wallet for sign-in.

## If you are over three minutes

Cut, in order: the status page, the hand-built rule (1:35), the second agent prompt (2:30). Never cut the story (0:00 to 0:45), the alert arriving, or the agent triaging.

## Submission checklist from the listing

- Public repo with setup, environment variables and personal-key guidance. `Don-Vicks/sentinel` is still private, and the leaked `rpc_…` key should be rotated first. Link `docs/MCP.md` from the README.
- Demo video or Loom link, 2 to 3 minutes, on live mainnet.
- Solami as the data path: gRPC, RPC and Mirage are all on it.
- Deadline: **October 28, 2026**.
