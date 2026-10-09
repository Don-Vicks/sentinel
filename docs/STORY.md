# The upgrade nobody announced

This is the story that made me want to build Sentinel, and the day it happened to me while I was testing it. The facts are checked against the chain; the exact figures are in [EVIDENCE.md](EVIDENCE.md).

## The problem

When a Solana program breaks, its team usually finds out from users. Explorers show one failed transaction at a time. They don't say how often, since when, or why. And the cause is often not in the team's own code.

On Solana, programs call each other. A swap router calls pools, a lending market calls an oracle, a vault calls a token program. Most of those programs can be upgraded by whoever holds their upgrade key, and there is no advance notice when that happens. If a program you depend on is replaced and your failures start an hour later, you will look in your own code for a long time before you look anywhere else.

I wanted a monitor that watches both: what a program is doing, and what is being done to it.

## October 8, 2026, 17:56 UTC

I was running Sentinel against fifteen programs on mainnet, streamed through Solami's Yellowstone gRPC, mostly to check that the detectors behaved. Nothing was wrong. Then, in one second, seven programs each got an incident.

A program that seven of them call, `BiSoNHVpsVZW2F7rx2eQ59yQwKxzU5NvBcmKshCSUypi`, had been redeployed. Someone sent an `Upgrade` instruction to the upgradeable loader, and the new code went live in slot 454,617,760. Sentinel saw the transaction on the stream and did what it is built to do:

- It recognised the loader instruction, read who signed it, and saw that the signer was an ordinary wallet that is still the program's upgrade authority, with no multisig.
- It looked up which of the programs I was watching call that program, from the call trees it had already seen: Jupiter v6, Jupiter Perps, Kamino Lend, MarginFi, Orca Whirlpool, Meteora DBC and Raydium CPMM.
- It opened one incident for each, about a second after the block, within twelve milliseconds of each other.
- Ten seconds later it did the same for a second program that Meteora DBC calls.

It checks out against the chain, over the same Solami RPC and on a public explorer: same transaction, same slot, 17:56:09 UTC, same signer.

## What it was, and what it was not

It was not an attack. As far as I can tell it was an ordinary release. Nothing I could see tied any failure to it, and Sentinel's own diagnosis called it a heads-up, which is what it was.

What mattered is what a team would have known, and when. A team calling that program would have learned, within a second and without watching it themselves, that code they depend on had changed, which key changed it, and that the key belongs to a single wallet rather than a multisig. If failures had started afterwards, they would have known where to look first. A single key being able to replace the code that seven large programs call is also worth knowing, whatever happened on the day.

I asked an agent to triage it. Over MCP, it pulled the incident and Sentinel's diagnosis, and its answer was the honest one: probably not alarming, no failures it could tie to the upgrade, the authority is a single key, so check that it is the program's known deployer. It said what it could not know.

## The next day, a different kind of incident

The next morning, a critical failure spike on Meteora DBC: 52.3% of transactions failing against a normal 4.6%, across 3,281 transactions from 535 wallets, caught 434 ms after it began and resolved on its own ten minutes later. Sentinel named the cause in the incident itself: half the failures were `ExceededSlippage` in `Swap2`, and a large share were the token transfer inside swaps failing. That is the ordinary turbulence of a launchpad when prices move fast, not an exploit, and it is exactly the kind of thing a team wants explained in seconds without opening a single transaction by hand.

## What came out of it

Those two days shaped what Sentinel is:

- **It watches dependencies, not only the program.** An upgrade of any program yours calls opens an incident on yours, and a later failure spike says if it began soon after one.
- **Incidents explain themselves.** Every incident states the rule that fired, with the numbers, the baseline, the timeline, the failure fingerprints and the transactions behind them.
- **Alerts go where people already are.** Slack (as a thread), Telegram, PagerDuty, Discord or a webhook, following the incident from open to resolved. [Setup for each](CHANNELS.md).
- **An agent can run it.** Triage, post-mortems and one-step protection over [MCP](MCP.md), without channel secrets ever passing through the agent.
- **It is honest about what it does not know.** A check with nothing to judge says so; a diagnosis states its confidence; a heads-up is called a heads-up.

## Try it

Run Sentinel against a few busy programs (`./scripts/demo.sh` watches fifteen) and leave it for an hour. Failure spikes on Raydium, Orca and Meteora appear on their own. Upgrades of the programs they call are rarer, but they happen, and when one does you will see it.

How: [README](../README.md#quick-start) · What it caught: [EVIDENCE.md](EVIDENCE.md) · Everything it does: [FEATURES.md](FEATURES.md)
