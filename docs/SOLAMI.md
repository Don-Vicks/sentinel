# How Solami is used

| Solami product | Role |
|---|---|
| **Yellowstone gRPC** | The primary data path. One subscription carries slots plus a named transaction filter over every monitored program (`account_include`, failed transactions included). Adding a program in the UI re-sends the filter over the open stream, with no reconnect. The stream answers pings and reconnects with backoff. If it can't connect, Mirage takes over (below). |
| **Blur** | `POST /data/token/price` prices every mint Sentinel sees move, in batches of up to 1000. A new mint is priced within about a second; known ones refresh every 30s. USD appears on the live feed, value-flow edges, balance changes and narratives. It also powers the USD large-transfer detector and "transfer worth ≥ $X" rules. Tokens under $10K liquidity are displayed but never trigger alerts, since one trade can move their price arbitrarily. |
| **RPC** | Anchor IDLs are fetched from chain with `getAccountInfo`. `getTransaction` for investigating any signature (rebuilt into a Yellowstone frame, decoded by Vortex). `getMultipleAccounts` resolves the owners of accounts in a trace, so a vault shows up as "Pump.fun account" rather than a raw address. The canary example sends controlled demo transactions through it. The finder's "which programs does this authority upgrade?" reads the upgradeable loader with `getProgramAccountsV2` (Solami requires pagination on a program this large). Listing an authority's programs takes about a second on Solami; finding the owner of each one is a scan of the whole loader, so those run in parallel in the background, the page fills in as they land, and every answer is cached for good. Measured on one real authority with 11 programs: all resolved in about 7 seconds on Solami's RPC, against about 26 seconds on the public RPC; a repeat takes milliseconds. |
| **Mirage** | Failover for the stream. Mirage sends the same Yellowstone `SubscribeUpdate` frames over a WebSocket, so Vortex decodes them with the same code as gRPC. Sentinel sets it up itself: given a key with the `MirageView`, `MirageManage` and `MirageStream` permissions, it finds or creates one subscription named "sentinel", keeps its filter equal to the programs being watched (failed transactions included), and builds the stream URL. After repeated gRPC connection failures it switches over, shows "Solami Mirage (gRPC failover)" in the status bar, and retries gRPC every two minutes. Without those permissions Mirage stays off and nothing else changes. Set `SENTINEL_TRANSPORT=mirage` to use it as the primary, or `SENTINEL_MIRAGE=off` to disable it. |
| **Beam** | Read side only. For any transaction, `GET /swqos/tx/{signature}` shows whether Beam carried it and how it landed: Jito or direct to a leader, region, tip, and how long Beam took to forward it. Transfers to Beam's tip accounts (`GET /onchain/tip-addresses`) are called out in the narrative. Both endpoints are public and need no key. Sentinel does not send through Beam; that needs a swQoS key and a funded wallet. |

# Measured performance

Measured with `cargo run --release --example bench` on real mainnet Pump.fun transactions, one core, Apple M-series laptop:

| Path | Throughput |
|---|---|
| Vortex decoder (Yellowstone frame → `VortexTransaction`) | ~13,000 tx/s (≈75 µs per Pump.fun transaction) |
| Sentinel engine (metrics, detectors, rules, incident linking into SQLite) | ~45,000 tx/s, with a failure storm in progress |

Pump.fun, one of the busiest programs on Solana, runs well below both. Detection runs on a 1-second tick. Each incident records its own detection latency, measured from the triggering transaction reaching Sentinel.

**Live on Solami gRPC (Oct 1, 2026),** Pump.fun and Jupiter v6 together, over a 370 ms round-trip link:

| | |
|---|---|
| Throughput | up to about 570 tx/s combined, and 163,000 transactions in 8 minutes with none dropped |
| Freshness | 0 slots behind the chain tip, measured against the tip read over RPC every 5 seconds (shown as "behind" in the status bar) |
| Detection latency recorded on incidents | 25 to 680 ms |
| Token prices | about 4,000 mints priced through Blur within 8 minutes |

**What the stream taught us.** On a long-latency link the default HTTP/2 window (64 KB) caps one stream at window ÷ round-trip time, about 170 KB/s here. With default settings the same subscription delivered 293 tx/s and ran about 25 seconds behind the chain; Solami's buffer filled and the server dropped the connection every minute or two. Vortex's client now uses 16 MB / 32 MB windows, adaptive windowing and gzip, which delivered 540 to 640 tx/s and 0 to 11 seconds of lag in the same test. The remaining drops are the server's buffer policy (`grpc_buffer_size`), which is an account setting. Run Sentinel close to the endpoint if you can; a hosted instance in the same region removes most of this.

Both programs fail 55 to 80% of the time in steady state, mostly arbitrage bots losing races, so a failure-rate alert has to judge them against their own baseline, not a fixed number. Tuning on this traffic is what produced the error-spike rules above: the first untuned run opened 23 incidents in ten minutes, the tuned one 7 in seventeen.
