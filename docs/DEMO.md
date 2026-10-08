# Demo script (about 2:50, live on mainnet)

Everything on screen comes from the real Solami stream: no simulator, no pre-recorded data. The alert you trigger on camera fires from live traffic, so nothing needs to be staged.

## Which programs to watch

Use the **showcase set**: 15 programs across swaps, launchpads, lending and perps, staking, oracles, NFTs and infrastructure. It is built into the app (the **Watch showcase set** button) and chosen so one stream carries it comfortably. Measured on a 370 ms-latency home connection over more than four minutes: **60 to 170 tx/s, 0.0 s behind the chain** (one 4-second blip that recovered on its own).

Leave **Meteora DLMM, Pump.fun and PumpSwap** out of a recording. Their messages are large and bursty (Meteora DLMM alone is about 2 MB/s, PumpSwap is a firehose), and on a home connection they push the stream 25 to 50 seconds behind the chain, which turns the status bar amber. They are in the catalog if you want to show them, but not while you are recording.

The showcase set mixes busy programs (Jupiter v6, Orca Whirlpool, Raydium) with quiet ones (Squads, Kamino, MarginFi, Wormhole, Streamflow). Quiet programs are where the interesting incidents come from: a single $2M transfer through Squads, or $107K of USDC through Kamino, is a real incident here.

## Before recording (do this 20 minutes ahead)

**1. Start everything with one command**

```bash
cd ~/Developer/rust-projects/vortex-sentinel
./scripts/demo.sh
```

That builds the dashboard and Sentinel if they changed, starts the **API and the dashboard together on http://localhost:8080** (one process, one port), loads the 15-program showcase set, waits until it is healthy, and prints a checklist like this:

```
  [ OK ] Solami gRPC streaming   80 tx/s
  [ OK ] Behind the chain       0.0s
  [ OK ] Blur prices            197 tokens
  [ OK ] Programs monitored     15 of 15
```

Any line that says FAIL is the thing to fix before recording. It starts from a **fresh database** every time, so you get a clean Alerts page. Use `./scripts/demo.sh --keep` to keep the previous incidents, rules and watchlists. **Ctrl+C stops it.** The first run compiles Rust and takes a few minutes; later runs start in seconds. Detectors need about five minutes of history to arm, and a longer history makes the charts and timelines look much better, so start early.

It needs a `.env` file with `YELLOWSTONE_ENDPOINT`, `YELLOWSTONE_TOKEN` and `SOLANA_RPC_URL`, plus `BLUR_API_KEY` for dollar values. It also lifts the default limit of 10 programs per wallet to 40, so the **Watch showcase set** button works.

**2. Check the screen is healthy.** All of these must be true before you press record:

- [ ] Top bar says **Live**, **behind** is 1.0 s or less, and **blur** shows a number of priced tokens (not "error").
- [ ] On the Overview, the **Powered by Solami** panel shows green dots for gRPC, RPC + Comet, Blur and Beam, and the program table lists 15 programs with real names. (Mirage shows "Off" unless the key has Mirage permissions. That is fine; see the honest claims below.)
- [ ] No amber "behind" figure. If it is amber, wait a minute. If it stays amber, restart with `./scripts/demo.sh`.

**3. Get a webhook destination.** The nicest on camera is a Discord channel in a second window: Server Settings, then Integrations, then Webhooks, then New Webhook, then Copy URL. Sentinel formats Discord messages natively. Fallback: a free URL from webhook.site.

**3b. Optional: show the newer features.** Seed a database that already has a day of history, an upgrade followed by the failures it caused, and a vault outflow: `cargo run --example seed_demo -- demo.db`, then start with `SENTINEL_DB=demo.db`. Open the Pump.fun **Summary** page (daily report, health check), incident #1002 (the failure spike with its "started after a program upgrade" banner and the Post-mortem button) and #1001 (the upgrade itself). For Telegram, create a bot with @BotFather, add it to a chat, and add a Telegram channel to a rule: the opening, escalation and resolution arrive as one thread. For MCP, create a token on the Alerts page and ask an agent to "triage incident 1002" ([docs/MCP.md](MCP.md)). On the program page, **Protect this program** creates the usual rules on a channel in one step, **Mute for a deploy** holds notifications, and **Status page** opens the public view with its badge. In Telegram, tap **Acknowledge** on an incident message, or send `/status`.

**4. Sign in once and rehearse the alert** in a throwaway Discord channel, so you know it works. Then **delete the rule** from the Alerts page so you create it fresh on camera. Any Solana wallet works (Phantom, Backpack, Solflare). Signing in costs nothing and sends no transaction. Because the demo starts from a fresh database, you will sign in again after each restart.

**5. Pre-warm the finder.** Paste Pump.fun's upgrade authority into the box once, so the lookup is instant on camera (the first lookup takes about 7 seconds):

```
7gZufwwAo17y5kg8FMyJy2phgpvv9RSdzWtdXiWHjFr8
```

**6. Pick your two transactions.** This prints a failed transaction and a USD-priced swap from live traffic:

```bash
python3 scripts/demo_picks.py
```

Keep both links in a notes file.

**7. Screen.** 1440×900 or similar, one theme (light or dark) the whole way through, other tabs closed, notifications off. **Never show `.env`, the terminal environment or any API key.**

## Shot list

Times are targets. The total should land between 2:40 and 3:00.

| Time | On screen | What to do | What to say |
|---|---|---|---|
| 0:00 | Overview, programs live, Solami panel visible | Let the numbers tick for a second | "A Solana program fails in ways an explorer won't show you. Sentinel watches every transaction live, tells you when something breaks, shows you what and why, and alerts you. This is real mainnet traffic, right now." |
| 0:15 | **Powered by Solami** panel and the top status bar | Point at each tile, then the bar | "Everything here runs on Solami. Yellowstone gRPC streams about four hundred transactions a second, and 'behind' is measured against the real chain tip over RPC: zero seconds. RPC with Comet loads each program's on-chain IDL and does the program scans. Blur prices tokens in dollars. Beam tells us how a transaction landed. Vortex, my own Rust library, decodes the stream." |
| 0:40 | Finder (signed out, Overview hero) | Paste the authority address, press **Find programs** | "I don't make you type program IDs. Paste an upgrade authority. It's public, nothing is signed, and the list of every program it can upgrade comes back from a Comet-accelerated scan." |
| 0:55 | Click **Sign in with wallet** | Choose the wallet and sign | "Sign in with Solana: a free message, no transaction. It gives me my own watchlist and alerts." |
| 1:05 | Overview: **Popular programs**, then Jupiter v6's page | Click **Watch showcase set (15)**, open Jupiter v6, scroll the charts and the instructions table | "Fifteen programs across eight categories in one click. Jupiter, live: success against failure each second, compute, and each instruction's failure rate. Failures that Jupiter itself raised are separated from other programs' errors inside the same transactions, so noise doesn't look like an incident." |
| 1:30 | **Alerts** page | Click the **Failure rate above 50%** quick start, paste the Discord webhook, click **Create rule** | "Now an alert. One click fills a rule: page me when the failure rate passes fifty percent. I point it at Discord and create it." |
| 1:45 | Deliveries table, then the Discord window | Wait a few seconds for the row to appear | "It fired on live traffic. The delivery log shows HTTP 200 and the latency, and here it is in Discord." Show the message. |
| 2:00 | Click the **#incident** link in the delivery row | Scroll: Why this fired, stats, timeline, fingerprints | "Every alert explains itself: the observed value, the threshold, the exact rule, and a timeline. Below it, the failure fingerprints, grouped by where and why transactions fail." |
| 2:20 | Open the **failed transaction** from `demo_picks.py` | Show narrative, then the decoded instruction | "Click any transaction. Plain-language narrative, the error with the program's own message from its on-chain IDL, and the call tree showing which program failed and what it cost." |
| 2:35 | Open the **swap** from `demo_picks.py` | Show Value flow | "And for a swap, where the value moved and what it was worth in dollars, priced by Blur." |
| 2:45 | Back to the Overview | Final shot | "Vortex Sentinel: incidents, not transactions. Open source, built on Solami gRPC, RPC, Blur and Beam. One command to run it on your own program." |

If you are running over three minutes, cut the **finder** segment (0:40 to 0:55). The alert is the part that matters most.

## Numbers you can say, because they were measured

- **15 programs** live at once: 60 to 170 tx/s, **0.0 s behind the chain**, over a 370 ms link, for more than four minutes.
- Detection latency recorded on incidents: **25 to 680 ms** after the triggering transaction reached Sentinel.
- Authority lookup on Solami RPC: all 11 programs resolved in **about 7 s**, against about 26 s on the public RPC.
- Blur prices thousands of tokens within minutes.

## Honest claims (so nothing you say can be caught out)

- **The submitted video is a scripted recording of the live app**, with the narration recorded separately. A scripted browser clicks through the real running Sentinel against the real Solami stream; nothing is mocked. The sign-in step uses a stand-in wallet (a throwaway in-page key, shown as "Test Wallet") because a script cannot drive a browser extension. Say so in your submission.

- **Mirage** is the gRPC failover. Say it is "configured as failover" only if the tile shows **Standby**. If it says **Off**, leave it out of the voice-over. To turn it on, give the Solami key the MirageView, MirageManage and MirageStream permissions; Sentinel then creates and maintains the subscription itself.
- **Beam** is read-side: Sentinel shows how a transaction landed. It does not send through Beam. The tile's "checked" count rises as you open transactions.
- Vortex predates the bounty, but the decoder, hub and Mirage transport were written for it. The README's "Built on Vortex" section has the commit links.

## If something goes wrong live

- **"behind" turns amber:** the stream is catching up. Cut, wait a minute, resume. Do not watch Pump.fun.
- **Status bar says "Feed stalled":** Sentinel pauses its detectors on purpose and shows it. Wait for it to recover, or restart.
- **The alert does not appear in Discord:** open the Deliveries table. It shows the HTTP status. A 4xx means the webhook URL is wrong.
- **Nothing fires:** the rule only fires for programs you watch. Make sure Jupiter has the star next to it in the sidebar.
- **The wallet does not connect:** unlock it, refresh, try once more. You can record the sign-in separately and edit it in.
- **No failed transaction yet from `demo_picks.py`:** wait a minute. Jupiter fails 60 to 80% of the time, so it comes quickly.

## Optional: a deliberately broken program on camera

For a controlled failure instead of a natural one, `examples/canary.rs` sends real failing Memo transactions through your Solami RPC (about 5,000 lamports each):

```bash
cargo run --example canary -- --count 8 --fail
```

Watch the canary wallet in Sentinel and add a rule such as "Failed transactions exceed 3 over 60s". It needs a funded `canary-keypair.json`. You do not need it for the demo above.

## Command cheat sheet

| What | Command |
|---|---|
| Start everything (API + dashboard) | `./scripts/demo.sh` |
| Start, keeping the old database | `./scripts/demo.sh --keep` |
| Stop | `Ctrl+C` in that terminal, or `pkill -f target/release/sentinel` from another |
| Is it healthy? | `curl -s localhost:8080/api/solami \| python3 -m json.tool` |
| Stream and program status | `curl -s localhost:8080/api/status` |
| Incidents so far | `curl -s localhost:8080/api/incidents` |
| A failed transaction and a USD-priced swap to open on camera | `python3 scripts/demo_picks.py` |
| Rebuild only the dashboard | `cd web && npm run build` |
| Run the tests | `cargo test` |

