import { useEffect, useState, type ReactNode } from 'react';
import { api } from '../lib/api';
import { usePrograms } from '../lib/programs';
import type { SolamiStatus } from '../lib/types';
import { compact, num } from '../lib/format';

type Tone = 'good' | 'warn' | 'off';

function Tile({ name, role, tone, value, caption }: { name: string; role: string; tone: Tone; value: ReactNode; caption: ReactNode }) {
  const dot = tone === 'good' ? 'bg-good' : tone === 'warn' ? 'bg-warn' : 'bg-ink-3';
  return (
    <div className="min-w-0 bg-surface px-4 py-3">
      <div className="flex items-center gap-2">
        <span className={`size-2 shrink-0 rounded-full ${dot} ${tone === 'good' ? 'pulse-dot' : ''}`} aria-hidden />
        <span className="truncate text-sm font-medium text-ink">{name}</span>
      </div>
      <p className="mt-0.5 truncate text-xs text-ink-3">{role}</p>
      <p className="num mt-2 text-xl font-semibold leading-7 text-ink">{value}</p>
      <p className="mt-0.5 text-xs text-ink-2 line-clamp-2">{caption}</p>
    </div>
  );
}

/** Every Solami product Sentinel runs on, with what it is doing right now. */
export function SolamiPanel() {
  const { stream } = usePrograms();
  const [s, setS] = useState<SolamiStatus | null>(null);

  useEffect(() => {
    let stop = false;
    const load = () => api<SolamiStatus>('/api/solami').then((r) => !stop && setS(r)).catch(() => {});
    load();
    const t = setInterval(load, 4000);
    return () => {
      stop = true;
      clearInterval(t);
    };
  }, []);

  if (!s) return null;
  const failover = s.mirage.active;
  const behind = stream?.behind_chain_slots ?? s.grpc.behind_chain_slots;
  const tps = stream?.ingest_tps ?? s.grpc.tx_per_sec;
  const live = (stream?.connected ?? s.grpc.connected) && !stream?.stalled;

  return (
    <section aria-label="Powered by Solami" className="panel overflow-hidden">
      <div className="flex items-center justify-between gap-3 border-b border-line px-4 py-2.5">
        <h2 className="panel-title">Powered by Solami</h2>
        <span className="text-xs text-ink-3">live data path, every number below is measured</span>
      </div>
      <div className="grid gap-px bg-line sm:grid-cols-2 lg:grid-cols-5">
        <Tile
          name={failover ? 'Mirage (gRPC failover)' : 'Yellowstone gRPC'}
          role="Live transaction stream"
          tone={live ? (failover ? 'warn' : 'good') : 'warn'}
          value={`${compact(tps)} tx/s`}
          caption={
            stream?.stalled
              ? 'Feed stalled, detectors paused'
              : behind == null
                ? `${num(s.grpc.transactions)} transactions so far`
                : `${(behind * 0.4).toFixed(1)}s behind the chain tip`
          }
        />
        <Tile
          name="RPC + Comet"
          role="Lookups, IDLs, program scans"
          tone={s.rpc.enabled ? 'good' : 'off'}
          value={s.rpc.enabled ? `${s.rpc.idls_loaded} IDLs` : 'Not set'}
          caption={s.rpc.enabled ? 'Anchor IDLs decoded from chain; any signature investigated' : 'Set SOLANA_RPC_URL'}
        />
        <Tile
          name="Blur"
          role="USD prices"
          tone={s.blur.enabled ? (s.blur.error ? 'warn' : 'good') : 'off'}
          value={s.blur.enabled ? `${num(s.blur.priced_mints)} priced` : 'Off'}
          caption={s.blur.error ? s.blur.error : 'tokens priced in USD for value flow and alerts'}
        />
        <Tile
          name="Mirage"
          role="WebSocket stream, gRPC failover"
          tone={s.mirage.active ? 'warn' : s.mirage.configured ? 'good' : 'off'}
          value={s.mirage.active ? 'In use' : s.mirage.configured ? 'Standby' : 'Off'}
          caption={
            s.mirage.active
              ? 'gRPC could not connect, so the stream is running on Mirage'
              : s.mirage.configured
                ? 'Takes over if gRPC cannot connect, and follows the watchlist'
                : 'Optional failover for the gRPC stream'
          }
        />
        <Tile
          name="Beam"
          role="Landing status"
          tone="good"
          value={`${num(s.beam.lookups)} checked`}
          caption={`${num(s.beam.carried)} sent through Beam; route, region and tip shown on each transaction`}
        />
      </div>
    </section>
  );
}
