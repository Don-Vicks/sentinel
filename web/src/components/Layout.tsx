import { useState } from 'react';
import { NavLink, Outlet, useNavigate } from 'react-router';
import { Activity, Bell, LayoutGrid, Search, Siren } from 'lucide-react';
import { usePrograms } from '../lib/programs';
import { useLiveStatus } from '../lib/live';
import { HealthDot } from './ui';
import { compact, num } from '../lib/format';

const nav = [
  { to: '/', label: 'Overview', Icon: LayoutGrid, end: true },
  { to: '/incidents', label: 'Incidents', Icon: Siren, end: false },
  { to: '/alerts', label: 'Alerts', Icon: Bell, end: false },
];

const navCls = ({ isActive }: { isActive: boolean }) =>
  `flex items-center gap-2 rounded-md px-2.5 h-9 text-sm ${
    isActive ? 'bg-sunken text-ink font-medium' : 'text-ink-2 hover:text-ink hover:bg-sunken'
  }`;

function StreamPanel() {
  const { stream } = usePrograms();
  const sse = useLiveStatus();
  const connected = !!stream?.connected && sse;
  const age = stream?.last_transaction_age_ms;
  return (
    <div className="rounded-md border border-line bg-surface p-3 text-xs space-y-2">
      <div className="flex items-center justify-between">
        <span className="font-medium text-ink">Vortex stream</span>
        <span className={`inline-flex items-center gap-1.5 ${connected ? 'text-good' : 'text-warn'}`}>
          <span className={`size-2 rounded-full ${connected ? 'bg-good' : 'bg-warn'} pulse-dot`} aria-hidden />
          {connected ? 'Live' : sse ? 'Waiting for data' : 'Reconnecting'}
        </span>
      </div>
      <div className="text-ink-3">Solami Yellowstone gRPC</div>
      <dl className="grid grid-cols-2 gap-x-2 gap-y-1">
        <dt className="text-ink-3">Ingest</dt>
        <dd className="num text-right text-ink">{stream ? `${compact(stream.ingest_tps)} tx/s` : '—'}</dd>
        <dt className="text-ink-3">Slot</dt>
        <dd className="num text-right text-ink">{stream?.last_slot ? num(stream.last_slot) : '—'}</dd>
        <dt className="text-ink-3">Tip lag</dt>
        <dd className="num text-right text-ink">{stream ? `${stream.slot_lag} slots` : '—'}</dd>
        <dt className="text-ink-3">Last tx</dt>
        <dd className="num text-right text-ink">{age == null ? '—' : `${(age / 1000).toFixed(1)}s ago`}</dd>
        <dt className="text-ink-3">Blur prices</dt>
        <dd
          className={`num text-right ${stream?.pricing.last_error ? 'text-warn' : 'text-ink'}`}
          title={stream?.pricing.last_error ?? undefined}
        >
          {!stream ? '—' : !stream.pricing.enabled ? 'off' : stream.pricing.last_error ? 'error' : `${num(stream.pricing.priced_mints)} mints`}
        </dd>
        <dt className="text-ink-3">Dropped</dt>
        <dd className={`num text-right ${stream?.dropped ? 'text-warn' : 'text-ink'}`}>{stream ? num(stream.dropped) : '—'}</dd>
      </dl>
    </div>
  );
}

function TxSearch() {
  const navigate = useNavigate();
  const [sig, setSig] = useState('');
  return (
    <form
      role="search"
      className="relative px-2 mt-3"
      onSubmit={(e) => {
        e.preventDefault();
        const v = sig.trim();
        if (v) {
          navigate(`/tx/${v}`);
          setSig('');
        }
      }}
    >
      <label htmlFor="tx-search" className="sr-only">Investigate a transaction signature</label>
      <Search className="absolute left-4.5 top-2.5 size-4 text-ink-3" aria-hidden />
      <input
        id="tx-search"
        className="input pl-8 font-mono text-xs"
        placeholder="Investigate a signature"
        value={sig}
        onChange={(e) => setSig(e.target.value)}
        autoComplete="off"
        spellCheck={false}
      />
    </form>
  );
}

export function Layout() {
  const { programs } = usePrograms();
  return (
    <div className="min-h-screen lg:grid lg:grid-cols-[240px_1fr]">
      <aside className="border-b lg:border-b-0 lg:border-r border-line bg-canvas lg:sticky lg:top-0 lg:h-screen flex flex-col">
        <div className="flex items-center gap-2 px-4 h-14 shrink-0">
          <span className="inline-flex size-7 items-center justify-center rounded-md bg-ink text-canvas">
            <Activity className="size-4" aria-hidden />
          </span>
          <div className="leading-tight">
            <div className="font-semibold text-ink">Sentinel</div>
            <div className="text-[11px] text-ink-3">on Vortex</div>
          </div>
        </div>
        <nav aria-label="Main" className="px-2 flex lg:flex-col gap-0.5 overflow-x-auto">
          {nav.map(({ to, label, Icon, end }) => (
            <NavLink key={to} to={to} end={end} className={navCls}>
              <Icon className="size-4" aria-hidden />
              {label}
            </NavLink>
          ))}
        </nav>
        <TxSearch />
        <div className="hidden lg:flex flex-col min-h-0 flex-1 mt-5">
          <div className="px-4 mb-1 text-[11px] font-semibold uppercase tracking-wide text-ink-3">Programs</div>
          <div className="px-2 overflow-y-auto flex-1 space-y-0.5">
            {programs.length === 0 && <div className="px-2.5 text-xs text-ink-3">None yet</div>}
            {programs.map((p) => (
              <NavLink key={p.program_id} to={`/programs/${p.program_id}`} className={navCls}>
                <HealthDot health={p.health} />
                <span className="truncate">{p.label}</span>
                {p.open_incidents > 0 && (
                  <span className="ml-auto num text-xs text-crit">{p.open_incidents}</span>
                )}
              </NavLink>
            ))}
          </div>
          <div className="p-3">
            <StreamPanel />
          </div>
        </div>
      </aside>
      <main className="min-w-0 px-4 py-5 lg:px-8 lg:py-6 max-w-[1400px]">
        <Outlet />
      </main>
    </div>
  );
}
