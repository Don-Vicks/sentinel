# Vortex → Sentinel: Phase 0 Audit

This is the architecture map of the existing Vortex codebase, produced before any Sentinel code was written. It records what Sentinel reuses, what had to change, and why.

## 1. What Vortex is today

Vortex is a Rust **transaction execution** stack. It sends transactions (Jito gRPC + RPC dual-send), picks tips with an LLM, and tracks the lifecycle of *its own* submissions.

```
crates/vortex (lib: solana-vortex)          crates/core (bin)
├── geyser/client.rs   Yellowstone connect  └── main.rs
├── geyser/stream.rs   slot + wallet-tx sub      ├── spawns RPC slot poller + Geyser stream
├── geyser/rpc_fallback.rs  400ms getSlot        ├── mpsc<GeyserEvent> consumer (slot atomic, lifecycle update)
├── jito/*             bundles, tips, leaders    ├── Axum :3000  POST /api/relay
├── agent/*            multi-LLM tip/failure     └── RUN_DAEMON loop (leader window → tip → bundle → retry)
├── failures/*         error-string classifier
└── lifecycle/*        LifecycleEvent + JSON file logger
frontend/ (Vite + React 19 + Tailwind 4)
└── Dashboard.tsx      polls /logs/lifecycle.json every 2s, POSTs /api/relay
```

## 2. Findings

| Question | Finding |
|---|---|
| Solana connection | Yellowstone gRPC via `yellowstone-grpc-client 1.15` (`geyser::client::connect`, `YELLOWSTONE_ENDPOINT` + `YELLOWSTONE_TOKEN`), with an HTTP `getSlot` poller as fallback. Standard RPC via `solana-client 1.18`. |
| How transactions are received | `geyser::stream::subscribe_slots` makes one `SubscribeRequest`: slots, plus transactions filtered by a single `SENDER_PUBKEY` with `failed: false`, at `Processed` commitment. |
| How transactions are parsed | **They are not.** The stream keeps only the signature and slot (`TxConfirmation`). Meta, logs, CU, balances and instructions are all dropped. |
| Internal event model | `GeyserEvent { Slot(SlotInfo), Tx(TxConfirmation) }` sent over a bounded `mpsc` (capacity 100) with **one consumer** in `core/main.rs`. |
| Persistence | `logs/lifecycle.json`, a JSON array that is read and fully rewritten on every event. It only holds Vortex's own submissions. There is no database. |
| Queues / event bus | None beyond that single mpsc channel. |
| APIs | `POST /api/relay` only (Axum 0.8). It needs a keypair file at startup. |
| Real-time to the UI | None. The frontend polls a static file every 2s, and nothing serves that path. |
| Retry / error handling | Geyser reconnects with exponential backoff, but the attempt counter never resets, so after 10 lifetime drops the stream stops for good. The failure classifier matches substrings of *send-side* errors. |
| Monitoring | Tracing logs plus lifecycle latency deltas. |
| Frontend | Vite (not Next.js), React 19, Tailwind 4, lucide-react. One page, no router, no charting. |

## 3. Classification

| Capability | Class | Decision |
|---|---|---|
| Yellowstone connection (`geyser::client`) | **REUSE** | Sentinel adds no gRPC client of its own. Pointed at Solami (`grpc.solami.dev`). |
| Geyser subscription (`geyser::stream`) | **EXTEND** | The same stream gets named program filters (`account_include`, failed txs included). Filters change live through the subscription sink, with no reconnect. The existing wallet filter behaves exactly as before. Adds ping/pong keepalive, and the reconnect counter now resets after a healthy session. |
| Transaction parsing | **BUILD NEW (in Vortex)** | Vortex had no parser. Parsing is infrastructure, so it lives in `vortex::events` and not in Sentinel. It decodes meta, error, CU, logs → invocation tree, balances, and System/SPL transfers. |
| Event model `GeyserEvent` | **EXTEND** | New variant `Transaction(Arc<VortexTransaction>)`. Existing variants are unchanged. |
| Event fan-out | **ADAPT** | New `vortex::hub::VortexHub`: a broadcast bus plus a program-filter registry. The host process forwards decoded transactions into it, and any number of consumers subscribe. |
| RPC client | **REUSE** | Sentinel's tracer resolves account owners through the same RPC (Solami) to label PDAs and vaults. |
| Axum server | **Pattern reused** | Sentinel runs as its own service (this repo) with the same Axum 0.8 stack. `/api/relay` in Vortex is untouched. |
| Failure classifier | **REUSE (partial)** | Tags the class of failed program transactions. Sentinel adds Anchor error-code parsing from logs. |
| Lifecycle logger (JSON file) | **Not used by Sentinel** | Rewriting the whole file per event can't keep up with program-level volume. Left as-is for Vortex. |
| Metrics / rolling windows | **BUILD NEW** | Sentinel, in memory, with 1s buckets. |
| Detection, incidents, alert rules, webhooks | **BUILD NEW** | Sentinel. SQLite for rules, incidents, incident transaction snapshots and alert executions. |
| Real-time push | **BUILD NEW** | SSE from the existing Axum server. |
| Frontend | **Stack reused** | Same Vite + React 19 + Tailwind 4 stack, in this repo's `web/`. |

## 4. Integration boundary

```
Solana ─▶ Solami Yellowstone gRPC
            │
   ┌────────▼──────────────── VORTEX ────────────────────────┐
   │ geyser::client (REUSE) → geyser::stream (EXTEND)         │
   │      ├─ "client_txs" → GeyserEvent::Tx → lifecycle       │
   │      └─ "vortex_programs" → events::decode → VortexTxn   │
   │ VortexHub (broadcast + program filter registry)          │
   └────────┬─────────────────────────────────────────────────┘
            │  trait VortexSource { subscribe(), set_programs() }
   ┌────────▼─────── SENTINEL (vortex-sentinel repo) ─────────┐
   │ adapter → engine → metrics → detectors → incidents       │
   │                          └→ alert rules → webhooks       │
   │ tracer (value flow, state diff) · SQLite store · SSE API │
   └──────────────────────────────────────────────────────────┘
```

Sentinel depends only on `vortex::events::VortexTransaction`, a documented public model, and on the `VortexSource` trait. It never touches the proto types or `core` internals. The Vortex changes are on `master` of [Don-Vicks/vortex](https://github.com/Don-Vicks/vortex).
