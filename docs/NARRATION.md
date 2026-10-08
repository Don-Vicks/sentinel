# What to say: the full script (423 words, about 2:49 at 150 words a minute)
The listing's limit is **3 minutes**, so the pace matters. At 150 words a minute this is 2:49 of speech. The screen has slow moments of its own (the rule firing, the agent typing and answering), but **I time those to run underneath your voice**, so they add only a few seconds, not their full length. The finished video should land at about 2:55 at 150 words a minute.

**You need to speak at 150 words a minute or faster. That is a normal, relaxed pace: you'll feel it as a steady read with short pauses, not a rush.** If your takes add up to more than about 2:50 of speech, I will tell you, and you cut these sentences first, in this order (they are marked ✂ below): 1) Beam, 2) the Slack/Telegram/PagerDuty lines in Part 4, 3) "so a transfer has a dollar value" in Part 2. Re-recording one part is cheap.

Everything below has been checked. The upgrade, its slot, its time and its signer were verified on chain. All seven affected programs are confirmed in Sentinel's record (incidents #1019 to #1025). The agent's answer was tested twice on the real incident. Numbers in **[brackets]** are read off the screen at that moment.

**How it will be made:** you record the seven parts below (one file each, `01` to `07`, half a second of silence at each end). I record the screen and the terminal and time every scene to your voice, so your pace decides how long each screen stays up. The terminal scene replays a real captured Claude Code session against Sentinel's MCP server.

## 1. The story · 86 words, about 34 s
*Screen: incident #1019 (hold on “What changed”), then the explorer tab with the same signature.*

> "On October 8th, at 17:56 UTC, a Solana program that seven of the programs I was watching depend on was redeployed. There's no advance notice on chain when that happens. My monitor caught it about a second later. This is its record of the upgrade, and this is the same transaction on the explorer.
>
> I've no reason to think it was an attack. But if your program starts failing after something like that, the cause isn't in your code, and you'd never know where to look."

## 2. The product and Solami · 89 words, about 36 s
*Screen: Overview with live numbers, then the Powered by Solami row, pointing at each tile in turn.*

> "That's why I built Vortex Sentinel. It watches Solana programs, live, on mainnet. Right now it's reading [three hundred] transactions a second, [zero] seconds behind the chain.
>
> It runs on Solami. Yellowstone gRPC streams the transactions. RPC with Comet loads each program's IDL, so errors come back in the program's own words. Blur prices the tokens, ✂ so a transfer has a dollar value. ✂ Beam shows how a transaction landed. And Mirage is my failover, if the stream ever drops. A Rust library I wrote, Vortex, decodes all of it."

Say the Mirage sentence only if its tile reads **Standby** (it does now: configured, not active). If it reads Off, drop that sentence. Beam is read-side: it shows how a transaction landed; Sentinel does not send through it. The Beam tile reads “0 checked” until a transaction is opened; the recording opens one so the count moves.

## 3. Protect · 55 words, about 22 s
*Screen: the Protect this program card on a calm program, pick the saved destination, click; then Send test and the message arriving.*

> "The point is being told. One click on Protect this program, and I get the alerts most teams want: a failure rate above twenty percent, an admin call from a wallet that has never made one, a health score drop. I pick the channel I already use, and I send a test. There it is."

The recording uses a quiet program here, not Jupiter or Kamino: those fail 40 to 70% of transactions as a matter of course, so the failure-rate rule would fire constantly. Protect does **not** include dependency changes (those are medium severity), so never say it would have paged you for the October 8th upgrade.

## 4. A live alert · 53 words, about 21 s
*Screen: Alerts, the Transfer worth $10K+ card, Create rule; then Deliveries; then the webhook message arriving.*

> "Now a live one. I'll make a rule: any transfer worth over ten thousand dollars. It fires on real traffic, and the log shows the status and the latency. ✂ In Slack, an incident stays in one thread. In Telegram, there's an Acknowledge button. PagerDuty gets one page, and resolves it when it clears."

The Slack, Telegram and PagerDuty lines describe what the product does; they are verified against mock servers, not against your own workspace or bot. The recording shows the webhook message, not Slack or Telegram. Say so in the submission text.

## 5. The incident page · 13 words, about 5 s
*Screen: an incident page: Why this fired, the chart, the Post-mortem button.*

> "Every alert says why it fired, what it saw, and exports a post-mortem."

## 6. The agent · 93 words, about 37 s
*Screen: Alerts, Agent access, create a token (kept off camera); then the terminal replay.*

> "And this is the part I'm most excited about. I give my agent access to Sentinel, and I ask it to triage that upgrade.
>
> It reads the incident and gives me a straight answer. The upgrade was signed by a single key, not a multisig. It can't tie any failures to the upgrade. And it tells me what to check: whether that key is the program's known deployer.
>
> Then I ask it to set up protection for Squads, reusing my Discord channel. It does, and my bot token never passes through the agent."

**The two prompts on screen, typed exactly:**
1. `Triage incident 1019. Keep it short: what happened, and should I be worried?`
2. `Set up protection for Squads v4, reusing my Discord channel.`

Verified: I ran prompt 1 against the real incident twice, with real Claude Code. Both answers said the upgrade was signed by a single wallet, not a multisig; that the failure rate did not jump at the upgrade and no failures could be tied to it; that worry was mild; and to confirm the key is the known deployer. The single-key claim is also checked on chain: the signer `6dDj…YipS` is an ordinary wallet account, signed the transaction directly, and is still the program's upgrade authority. I also ran prompt 2 for real: the agent watched Squads v4, created four rules (any high-severity incident, failure rate above 20%, an admin call from a new wallet, health below 60) on the Discord channel of your existing rule, and the channel's secret never appeared anywhere in the session. In the recording the showcase set is already watched, so it skips the “watch it first” step. The scene replays a real captured run; if the final capture words anything differently, I will tell you and this part changes with it.

## 7. Close · 34 words, about 14 s
*Screen: Jupiter's Status page and its badge; then the README Run it section.*

> "Each program gets a public status page and a badge for its README. It's open source, runs with one command and your own Solami keys. Vortex Sentinel: it knew a second after it happened."

## Do not say

- That the upgrade caused failures, that it was malicious, or that it was routine. Nothing in the data says which.
- What the upgraded program is called. Only its address is known.
- That you received an alert about it. No rule was watching at the time.
- That Protect would have alerted on it. The dependency incident is medium severity and Protect covers high and above.
- "Almost no other tool does this." That is unverified.
- That Sentinel can tell you a wallet is being attacked. It watches programs and the wallets that touch them.

## Say in the submission text (not the video)

- It is a scripted recording of the live app, narration recorded separately, with a stand-in account for sign-in.
- The terminal scene replays a real captured Claude Code session against Sentinel's MCP server; the timing is edited, the words are not.
- Slack, Telegram and PagerDuty delivery is tested against mock servers; delivery to a real workspace, bot or PagerDuty service has not been run.
- The October 8th upgrade is shown from Sentinel's own record. It will not repeat on demand.
