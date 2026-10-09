# Evidence: what Sentinel caught on live mainnet

Two real events Sentinel recorded while running against the Solami stream, with the on-chain facts you can check yourself. Nothing here was staged or simulated.

How to read this: Sentinel's record says what it saw and when. The "Check it" lines say how to confirm the same facts independently on any Solana explorer or RPC.

## 1. A dependency was redeployed under seven programs (October 8, 2026)

At 17:56 UTC, while Sentinel was watching fifteen programs, a program that **seven of them call** was redeployed. Sentinel saw the upgrade on the Yellowstone stream and opened one incident per dependent program, about a second after the block.

| | |
|---|---|
| Upgraded program | `BiSoNHVpsVZW2F7rx2eQ59yQwKxzU5NvBcmKshCSUypi` |
| Transaction | `grdGaPoVYStyEVTyzkiYPJQkbAejh6G7G6oGj1JC3kJbBUpenZgPY1grfjsocL3KhZpKL4RpYTbjiNuyCmD6dhS` |
| Slot, block time | 454,617,760, 17:56:09 UTC |
| Instruction | BPF upgradeable loader `Upgrade` |
| Signed by | `6dDjBZdpafRKe7WuVHYTF1HphW1BWiKqioMD5eBGYipS`, an ordinary wallet that is still the program's upgrade authority (no multisig) |
| Sentinel's incidents | #1019 to #1025, all detected at 17:56:10 UTC, within 12 ms of each other |
| Programs flagged | Jupiter v6, Jupiter Perps, Kamino Lend, MarginFi v2, Orca Whirlpool, Meteora DBC, Raydium CPMM |
| A second upstream upgrade | `EVoLVE9obLsZ3AHRNEddTXMtRVvtptLHRmVi1V3ujtWt` at slot 454,617,797 (incident #1026) |

**Check it.** Open the transaction in a Solana explorer: it shows the slot, the UTC timestamp, and the fee payer above.

**What it was not.** Nothing indicates an attack, and nothing ties any failure to this upgrade. Sentinel's diagnosis called it a heads-up. Its value is that a team that calls the upgraded program learns, in about a second and without watching that program themselves, that code it depends on changed, and by whom.

## 2. A critical failure spike on Meteora DBC (October 9, 2026)

At 09:31 UTC the share of failing transactions on Meteora DBC jumped from its norm to about half, for ten minutes. Sentinel opened a critical incident while it was starting and recorded why.

| | |
|---|---|
| Incident | #1169, severity critical, `failure_spike` |
| Onset, detected | 09:31:00 UTC, 09:31:05 UTC |
| Detection latency | 434 ms after the triggering transaction reached Sentinel |
| Failure rate | peaked at **52.3%**, against a normal of about 4.6% |
| Affected | 3,281 transactions from 535 wallets |
| Resolved | 09:41:13 UTC, ten minutes later, automatically |

**What failed, by Sentinel's failure fingerprints** (the program, instruction and error that raised each failure):

| Share | Where | Error |
|---|---|---|
| 52% | Meteora DBC `Swap2` | `ExceededSlippage` (the price moved past the swap's tolerance) |
| 45% | Token-2022 `TransferChecked` | `Custom(1)` (inside a swap's token transfer) |
| 1% | Meteora DBC `Swap2` | `InsufficientLiquidity` |

**Check it.** Two of the failing transactions Sentinel linked to the incident, both of which failed on chain exactly as recorded:

- `232s8uQAmA37ixGb9Qy6Gja39rqX5RZ18FGmPLpy6Ewt15ubDQ3pGLj2aHQbJu3CwnfG12nCYSsuwewoa24guyga`: slot 454,826,663, error `InstructionError [3, Custom(6002)]` (Meteora's `ExceededSlippage`), block time 09:31:11 UTC.
- `217E3oL2hc3yKXxyaHbB3hMxC97MReqcGZ4Eyzct89ceP7t4728xbbH4L5ksGEU6fGAnHpVgUS9Ped3gKK2DBR6U`: slot 454,826,643, error `InstructionError [3, Custom(1)]`, block time 09:31:05 UTC.

**What it was and was not.** This is the normal turbulence of a busy launchpad when prices move fast, not an exploit. The point is what a team gets from it: the moment it started, how bad it was against the program's own baseline, which instruction and error caused it, and the wallets involved, without reading a single transaction by hand.

## Other incidents from the same day

Across the runs on October 8 and 9, the same database recorded, on the same stream:

| Kind | Count |
|---|---|
| Alert-rule incidents | 51 |
| Error spikes | 46 |
| Transaction failure spikes | 44 |
| Activity spikes | 35 |
| Large transfers | 32 |
| Compute spikes | 15 |
| Dependency changes | 9 |
| One wallet dominating a program's traffic | 6 |

Most are the ordinary churn of busy programs. The point is that each one carries its baseline, its cause and its evidence.

## How to capture your own

Real failure spikes are not rare on a busy program, so the simplest way is to watch one and wait.

1. Run Sentinel on the showcase set (`./scripts/demo.sh`), which watches fifteen busy programs. Failure spikes on Raydium, Orca and Meteora appear within the first hour. Open **Incidents**, filter by `failure_spike`, and pick a resolved one.
2. For a specific program, watch it and add a rule such as "failure rate above 50% over 60 s", which fires as soon as the condition holds.
3. For a **controlled** failure on your own program, `examples/canary.rs` sends real failing transactions through your Solami RPC (about 5,000 lamports each, needs a funded `canary-keypair.json`):

   ```bash
   cargo run --example canary -- --count 8 --fail
   ```

   Watch the canary wallet in Sentinel and add a rule "Failed transactions exceed 3 over 60 s".
4. To check an incident later: its page shows the example transactions; paste any signature into an explorer and compare the error code with the fingerprint.
