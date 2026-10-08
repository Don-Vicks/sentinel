import { getCluster } from '../lib/format';
import { useEffect, useState } from 'react';
import { Link, NavLink, Outlet, useNavigate } from 'react-router';
import { Bell, LayoutGrid, Moon, Search, Siren, Star, Sun } from 'lucide-react';
import { usePrograms } from '../lib/programs';
import { useLiveStatus } from '../lib/live';
import { HealthDot } from './ui';
import { AccountChip } from './SignIn';
import { useAuth } from '../lib/auth';
import { compact, num, pct } from '../lib/format';
import { parseQuery } from '../lib/query';

/** Sentinel mark: a radar sweep over a program's signal. */
export function Mark({ className = 'size-7' }: { className?: string }) {
  return (
    <svg viewBox="0 0 28 28" className={className} aria-hidden>
      <defs>
        <linearGradient id="mark-bg" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="var(--color-brand-2)" />
          <stop offset="1" stopColor="#6c4fe0" />
        </linearGradient>
      </defs>
      <rect width="28" height="28" rx="7" fill="url(#mark-bg)" />
      <circle cx="14" cy="14" r="8.5" fill="none" stroke="#fff" strokeOpacity="0.35" strokeWidth="1.5" />
      <circle cx="14" cy="14" r="4" fill="none" stroke="#fff" strokeOpacity="0.35" strokeWidth="1.5" />
      <path d="M14 14 L20.5 8.5" stroke="#fff" strokeWidth="2" strokeLinecap="round" />
      <circle cx="18.6" cy="17.4" r="2" fill="#fff" />
    </svg>
  );
}

const navCls = ({ isActive }: { isActive: boolean }) =>
  `flex items-center gap-2 rounded-md px-2.5 h-9 text-sm ${
    isActive ? 'bg-accent-soft text-ink font-medium shadow-[inset_2px_0_0_var(--color-brand)]' : 'text-ink-2 hover:text-ink hover:bg-sunken'
  }`;

function useClock() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(t);
  }, []);
  return now;
}

/** Always-visible live state of the Vortex stream. */
function StatusBar({ theme, toggle }: { theme: 'dark' | 'light'; toggle: () => void }) {
  const { stream, programs } = usePrograms();
  const sse = useLiveStatus();
  const now = useClock();
  const stalled = !!stream?.stalled;
  const live = !!stream?.connected && sse && !stalled;
  const openIncidents = programs.reduce((n, p) => n + p.open_incidents, 0);
  const age = stream?.last_transaction_age_ms;
  const blur = stream?.pricing;

  const item = 'hidden md:inline-flex items-baseline gap-1.5 whitespace-nowrap';
  return (
    <div className="sticky top-0 z-20 flex h-11 items-center gap-4 overflow-hidden border-b border-line bg-canvas/90 px-4 text-xs backdrop-blur lg:px-8">
      <span className={`inline-flex items-center gap-2 whitespace-nowrap font-medium ${live ? 'text-good' : 'text-warn'}`}>
        <span className={`size-2 rounded-full ${live ? 'bg-good' : 'bg-warn'} pulse-dot`} aria-hidden />
        {live ? 'Live' : stalled ? 'Feed stalled · detectors paused' : sse ? 'Waiting for data' : 'Reconnecting'}
      </span>
      {getCluster() !== 'mainnet' && (
        <span className="whitespace-nowrap rounded bg-warn-soft px-1.5 py-px font-medium text-warn" title="This instance watches a test network; explorer links follow it">
          {getCluster()}
        </span>
      )}
      <span
        className={`hidden xl:inline whitespace-nowrap ${stream?.transport === 'mirage' ? 'text-warn' : 'text-ink-3'}`}
        title={stream?.transport === 'mirage' ? 'gRPC was unavailable, so the stream is running over Solami Mirage' : undefined}
      >
        {stream?.transport === 'mirage' ? 'Solami Mirage (gRPC failover)' : 'Solami Yellowstone gRPC'}
      </span>
      <span className={item}>
        <span className="text-ink-3">ingest</span>
        <span className="num text-ink">{stream ? `${compact(stream.ingest_tps)} tx/s` : '—'}</span>
      </span>
      <span className={item}>
        <span className="text-ink-3">slot</span>
        <span className="num text-ink">{stream?.last_slot ? num(stream.last_slot) : '—'}</span>
      </span>
      <span className={item} title="How far the stream is behind the real chain tip, measured over RPC (a slot is about 0.4 s)">
        <span className="text-ink-3">behind</span>
        <span className={`num ${(stream?.behind_chain_slots ?? 0) > 25 ? 'text-warn' : 'text-ink'}`}>
          {stream?.behind_chain_slots != null ? `${(stream.behind_chain_slots * 0.4).toFixed(1)}s` : '—'}
        </span>
      </span>
      <span className={item} title="Slots between the chain tip on the stream and the newest transaction">
        <span className="text-ink-3">lag</span>
        <span className="num text-ink">{stream ? stream.slot_lag : '—'}</span>
      </span>
      <span className={`${item} max-xl:hidden`}>
        <span className="text-ink-3">last tx</span>
        <span className="num text-ink">{age == null ? '—' : `${(age / 1000).toFixed(1)}s`}</span>
      </span>
      <span className={`${item} max-xl:hidden`} title={blur?.last_error ?? 'USD prices from Solami Blur'}>
        <span className="text-ink-3">blur</span>
        <span className={`num ${blur?.last_error ? 'text-warn' : 'text-ink'}`}>
          {!blur ? '—' : !blur.enabled ? 'off' : blur.last_error ? 'error' : `${num(blur.priced_mints)} priced`}
        </span>
      </span>
      {!!stream?.dropped && (
        <span className="inline-flex items-baseline gap-1.5 text-warn">
          <span>dropped</span>
          <span className="num">{num(stream.dropped)}</span>
        </span>
      )}
      <span className="ml-auto flex items-center gap-4">
        <Link
          to="/incidents"
          className={`inline-flex items-center gap-1.5 whitespace-nowrap rounded px-2 py-1 ${
            openIncidents ? 'bg-crit-soft text-crit font-medium' : 'text-ink-3 hover:text-ink'
          }`}
        >
          <Siren className="size-3.5" aria-hidden />
          {openIncidents ? `${openIncidents} open` : 'None open'}
        </Link>
        <button className="text-ink-3 hover:text-ink" onClick={toggle} aria-label={theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'} title="Theme">
          {theme === 'dark' ? <Sun className="size-4" aria-hidden /> : <Moon className="size-4" aria-hidden />}
        </button>
        <span className="hidden sm:inline whitespace-nowrap num text-ink-3" title="UTC">
          {now.toISOString().slice(11, 19)} UTC
        </span>
      </span>
    </div>
  );
}

function TxSearch() {
  const navigate = useNavigate();
  const [sig, setSig] = useState('');
  const [bad, setBad] = useState(false);
  return (
    <form
      role="search"
      className="relative px-3"
      onSubmit={(e) => {
        e.preventDefault();
        if (!sig.trim()) return;
        const q = parseQuery(sig);
        if (!q) return setBad(true);
        // A signature is investigated; an address (program or authority) goes to the finder.
        navigate(q.kind === 'signature' ? `/tx/${q.value}` : `/?find=${q.value}`);
        setSig('');
      }}
    >
      <label htmlFor="tx-search" className="sr-only">Investigate a transaction signature</label>
      <Search className="pointer-events-none absolute left-5.5 top-2.5 size-4 text-ink-3" aria-hidden />
      <input
        id="tx-search"
        className="input pl-8 font-mono text-xs"
        placeholder="Signature, program or address"
        aria-invalid={bad}
        value={sig}
        onChange={(e) => {
          setSig(e.target.value);
          setBad(false);
        }}
        autoComplete="off"
        spellCheck={false}
      />
      {bad && (
        <p className="mt-1 text-xs text-crit" role="alert">
          Not a Solana signature or address
        </p>
      )}
    </form>
  );
}

function useTheme() {
  const [theme, setTheme] = useState<'dark' | 'light'>(() => (document.documentElement.dataset.theme === 'light' ? 'light' : 'dark'));
  const toggle = () => {
    const next = theme === 'dark' ? 'light' : 'dark';
    document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem('sentinel-theme', next);
    } catch {
      /* the choice just won't persist */
    }
    setTheme(next);
  };
  return { theme, toggle };
}

export function Layout() {
  const { theme, toggle } = useTheme();
  const { programs } = usePrograms();
  const { watching } = useAuth();
  const mine = new Set(watching);
  const openIncidents = programs.reduce((n, p) => n + p.open_incidents, 0);
  const nav = [
    { to: '/', label: 'Overview', Icon: LayoutGrid, end: true, badge: 0 },
    { to: '/incidents', label: 'Incidents', Icon: Siren, end: false, badge: openIncidents },
    { to: '/alerts', label: 'Alerts', Icon: Bell, end: false, badge: 0 },
  ];
  return (
    <div className="min-h-screen lg:grid lg:grid-cols-[232px_1fr]">
      <aside className="border-b lg:border-b-0 lg:border-r border-line bg-canvas lg:sticky lg:top-0 lg:h-screen flex flex-col gap-3 pb-3 lg:pb-0">
        <Link to="/" className="flex items-center gap-2.5 px-4 h-14 shrink-0">
          <Mark />
          <span className="leading-tight">
            <span className="block font-semibold tracking-tight text-ink">Sentinel</span>
            <span className="block text-[11px] text-ink-3">built on Vortex</span>
          </span>
        </Link>
        <nav aria-label="Main" className="px-2 flex lg:flex-col gap-0.5 overflow-x-auto">
          {nav.map(({ to, label, Icon, end, badge }) => (
            <NavLink key={to} to={to} end={end} className={navCls}>
              <Icon className="size-4" aria-hidden />
              {label}
              {badge > 0 && (
                <span className="ml-auto rounded bg-crit-soft px-1.5 num text-[11px] font-medium text-crit">{badge}</span>
              )}
            </NavLink>
          ))}
        </nav>
        <TxSearch />
        <div className="hidden lg:flex flex-col min-h-0 flex-1 mt-2">
          <div className="px-4 mb-1 flex items-baseline justify-between">
            <span className="text-[11px] font-semibold uppercase tracking-wide text-ink-3">Programs</span>
            <span className="text-[11px] text-ink-3">fail % · 60s</span>
          </div>
          <div className="px-2 overflow-y-auto flex-1 space-y-0.5 pb-4">
            {programs.length === 0 && <div className="px-2.5 text-xs text-ink-3">None yet</div>}
            {programs.map((p) => (
              <NavLink key={p.program_id} to={`/programs/${p.program_id}`} className={navCls}>
                <HealthDot health={p.health} />
                <span className="truncate">{p.label}</span>
                {mine.has(p.program_id) && (
                  <Star className="size-3 shrink-0 fill-current text-accent" aria-label="On your watchlist" />
                )}
                <span
                  className={`ml-auto num text-xs ${
                    p.open_incidents > 0 ? 'text-crit font-medium' : 'text-ink-3'
                  }`}
                >
                  {p.tx_60s ? pct(p.failure_rate_60s, 1) : '—'}
                </span>
              </NavLink>
            ))}
          </div>
        </div>
        <div className="px-3 lg:pb-3">
          <AccountChip />
        </div>
      </aside>
      <div className="min-w-0">
        <StatusBar theme={theme} toggle={toggle} />
        <main className="min-w-0 px-4 py-6 lg:px-8 max-w-[1440px]">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
