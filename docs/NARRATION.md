# Narration script (about 2:50)

Read each segment as its own short recording. The video will be timed to your audio, so **your pace decides how long each screen stays up**. Speak naturally; do not rush to hit a time.

Anything in **[square brackets]** is a number to read off the screen at that moment. If you are recording the voice before the video exists, say a round, honest figure ("well over a hundred") or leave that phrase out. Nothing here should claim more than the app shows.

## How to record the audio

1. Use your phone's voice memo app, or any recorder. A quiet room matters more than the microphone.
2. Record **one file per segment**, in order, and name them `01`, `02` ... `11` (any format: m4a, wav, mp3).
3. Leave about half a second of silence at the start and end of each file. I trim it.
4. Put them all in one folder, for example `~/Developer/rust-projects/vortex-sentinel/demo/audio/` (git ignores it), and tell me.
5. A re-take of one segment is cheap: replace that file and I rebuild the video.

## Segments

| # | On screen | Say |
|---|---|---|
| 01 | Overview, programs live | "A Solana program can fail in ways an explorer will never show you. Sentinel watches every transaction live, tells you the moment something breaks, shows you what went wrong, and sends you an alert. This is real mainnet traffic, right now." |
| 02 | Powered by Solami panel, then the status bar | "Everything here runs on Solami. Yellowstone gRPC streams the transactions live, and the status bar shows we are [zero] seconds behind the chain tip, measured against Solami RPC. RPC also loads each program's on-chain IDL. Blur prices tokens in dollars, and Beam shows how a transaction landed. A Rust library I wrote, Vortex, decodes the stream." |
| 03 | Finder: paste an upgrade authority | "You do not need to type program IDs. Paste an upgrade authority, it is public and nothing is signed, and Sentinel lists every program that address can upgrade." |
| 04 | Sign in with wallet | "Signing in uses a free message from my wallet. No transaction, no cost. It gives me my own watchlist and my own alerts." |
| 05 | Watch showcase set, then Jupiter's page | "One click watches a set of programs across swaps, lending, perps, oracles and more. Here is Jupiter, live: successes and failures every second, and each instruction's failure rate. Errors Jupiter raised itself are kept apart from other programs' errors, so noise does not look like an incident." |
| 06 | Alerts page: quick start, webhook, Create rule | "Now an alert. One click fills in a rule: page me when the failure rate goes above fifty percent. I point it at my webhook and create it." |
| 07 | Deliveries table, then the webhook page | "It fired on live traffic within seconds. The delivery log shows the HTTP status and the latency, and here is the message arriving." |
| 08 | Incident page | "Every alert explains itself: the value it saw, the threshold, the exact rule, and a timeline. Below that, the failure fingerprints, grouped by where and why transactions fail." |
| 09 | A failed transaction | "Open any transaction. A plain-language summary, the error with the program's own message from its on-chain IDL, and a call tree showing which program failed and what it cost." |
| 10 | A USD swap | "And for a swap, where the value moved and what it was worth in dollars, priced by Blur." |
| 11 | Overview, final shot | "Vortex Sentinel: incidents, not transactions. Open source, built on Solami gRPC, RPC, Blur and Beam. One command runs it on your own program." |

## Honest claims

- Say **Mirage** only if you actually run it as failover and the panel shows "Standby". It is not mentioned above.
- Beam is read-side: Sentinel shows how a transaction landed. It does not send through Beam.
- Vortex is your own library, started before the bounty. The decoder, hub and Mirage transport were written for it. The README's "Built on Vortex" section has the commit links.
