# Built on Vortex

Vortex is not a third-party dependency. It is [my own open-source Rust project](https://github.com/Don-Vicks/vortex), the same author and the same GitHub account. I started it in June as a transaction execution stack: Jito bundles, a tip engine and failure recovery. Its Yellowstone client was already there, and Sentinel's needs went well past it, so I extended Vortex and kept it as the ingest library. Keeping the two apart means Sentinel has no stream-ingest code of its own, which is the point of the audit in [[`VORTEX_AUDIT.md`](VORTEX_AUDIT.md)](VORTEX_AUDIT.md). Its Blur, RPC and Beam calls are in this repo.

**What existed before this bounty.** The Yellowstone gRPC client with reconnect, and the transaction-sending stack. Sentinel doesn't use the sending side.

**What I wrote for this bounty (Sep 28 – Oct 1).** 1,842 added lines in Vortex (tests and examples included), all in public commits:

| Piece | Commit |
|---|---|
| Decoder: errors, compute, call tree from logs, SOL/SPL transfers, address lookup tables, version 1 transactions | [`4571b95`](https://github.com/Don-Vicks/vortex/commit/4571b95) |
| `VortexHub`: fan-out of decoded transactions and live program filters | [`4571b95`](https://github.com/Don-Vicks/vortex/commit/4571b95) |
| Rebuilding Yellowstone frames from JSON-RPC, plus tests on real mainnet transactions | [`6973c2d`](https://github.com/Don-Vicks/vortex/commit/6973c2d) |
| Base64 instruction data (6x faster decode) | [`2d252c3`](https://github.com/Don-Vicks/vortex/commit/2d252c3) |
| Solami Mirage WebSocket transport | [`4cbc733`](https://github.com/Don-Vicks/vortex/commit/4cbc733), [`c11de1c`](https://github.com/Don-Vicks/vortex/commit/c11de1c) |

**What lives in this repo.** Everything that makes it a product: about 12,400 lines of Rust (including the unit tests inside each file) for the detectors, rolling metrics, incident engine, investigation and tracing, Anchor IDL decoding, alert channels, upgrade and vault watching, rollups, health and summaries, the MCP server, wallet sign-in, the finder, Blur pricing and Beam lookups, plus about 3,400 lines of integration tests and examples and a 5,500-line React dashboard. Sentinel reaches Vortex through one small trait, [`VortexSource`](../src/source.rs).

**Third-party pieces,** all standard: the Solana SDK, the Yellowstone protocol definitions (`yellowstone-grpc-proto`), Tokio, Axum, SQLite and React. The data comes from Solami.

See [VORTEX_AUDIT.md](VORTEX_AUDIT.md) for how Sentinel was fitted onto the existing Vortex code: what was reused, extended and built new.
