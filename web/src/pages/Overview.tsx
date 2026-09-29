import { useState } from 'react';
import { Link, useNavigate } from 'react-router';
import { Plus, Radar, X } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import { usePrograms } from '../lib/programs';
import { useLive } from '../lib/live';
import type { Incident, MonitoredProgram, ProgramSnapshot, SeriesPoint } from '../lib/types';
import { compact, num, pct, short } from '../lib/format';
import { Empty, ErrorState, HealthDot, PageHeader, Panel, Skeleton, Spinner, Stat } from '../components/ui';
import { Sparkline } from '../components/charts';
import { IncidentList, mergeIncident } from '../components/IncidentList';

const SUGGESTED = [
  { id: '6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P', label: 'Pump.fun' },
  { id: 'pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA', label: 'PumpSwap AMM' },
  { id: 'JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4', label: 'Jupiter v6' },
  { id: 'whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc', label: 'Orca Whirlpool' },
];

function AddProgram({ onDone }: { onDone?: () => void }) {
  const { reload, programs } = usePrograms();
  const navigate = useNavigate();
  const [id, setId] = useState('');
  const [label, setLabel] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const add = async (programId: string, programLabel?: string) => {
    setBusy(true);
    setError(null);
    try {
      const p = await send<MonitoredProgram>('POST', '/api/programs', {
        program_id: programId.trim(),
        label: programLabel?.trim() || undefined,
      });
      await reload();
      onDone?.();
      navigate(`/programs/${p.program_id}`);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const watched = new Set(programs.map((p) => p.program_id));
  const suggestions = SUGGESTED.filter((s) => !watched.has(s.id));
  return (
    <div className="p-4 space-y-3">
      <form
        className="grid gap-3 md:grid-cols-[2fr_1fr_auto] md:items-end"
        onSubmit={(e) => {
          e.preventDefault();
          if (id.trim()) add(id, label);
        }}
      >
        <div>
          <label className="label" htmlFor="program-id">Program or account address</label>
          <input
            id="program-id"
            className="input font-mono"
            value={id}
            onChange={(e) => setId(e.target.value)}
            placeholder="6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"
            autoComplete="off"
            spellCheck={false}
            autoFocus
            aria-invalid={!!error}
            aria-describedby={error ? 'program-error' : undefined}
          />
        </div>
        <div>
          <label className="label" htmlFor="program-label">Name (optional)</label>
          <input id="program-label" className="input" value={label} onChange={(e) => setLabel(e.target.value)} autoComplete="off" />
        </div>
        <button className="btn-primary" disabled={busy || !id.trim()}>
          {busy ? <Spinner /> : <Plus className="size-4" aria-hidden />}
          Start monitoring
        </button>
      </form>
      {error && (
        <p id="program-error" className="text-sm text-crit" role="alert">
          {error}
        </p>
      )}
      {suggestions.length > 0 && (
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <span className="text-ink-3">Busy mainnet programs:</span>
          {suggestions.map((s) => (
            <button key={s.id} type="button" className="btn h-8 text-xs" disabled={busy} onClick={() => add(s.id, s.label)}>
              {s.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function failTone(p: ProgramSnapshot) {
  if (p.tx_60s < 20) return '';
  if (p.failure_rate_60s >= Math.max(p.baseline_failure_rate * 2.5, p.baseline_failure_rate + 5)) return 'text-crit';
  return '';
}

function ProgramRow({ p, series }: { p: ProgramSnapshot; series: SeriesPoint[] }) {
  // 5-second buckets: per-second failure rates are too spiky at low TPS.
  const buckets: { tx: number; failed: number }[] = [];
  for (let i = 0; i < series.length; i += 5) {
    const c = series.slice(i, i + 5);
    buckets.push({ tx: c.reduce((n, p) => n + p.tx, 0), failed: c.reduce((n, p) => n + p.failed, 0) });
  }
  const tps = buckets.map((b) => b.tx / 5);
  const fail = buckets.map((b) => (b.tx ? (b.failed * 100) / b.tx : null));
  return (
    <Link
      to={`/programs/${p.program_id}`}
      className="grid grid-cols-[minmax(0,1.4fr)_1fr_1fr_auto] md:grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)_minmax(0,1fr)_90px_110px] items-center gap-4 px-4 py-3 hover:bg-sunken"
    >
      <div className="min-w-0">
        <span className="flex items-center gap-2">
          <HealthDot health={p.health} />
          <span className="font-medium text-ink truncate">{p.label}</span>
        </span>
        <span className="mt-0.5 block font-mono text-xs text-ink-3 truncate">{short(p.program_id, 6)}</span>
      </div>
      <div className="min-w-0">
        <span className="flex items-baseline justify-between gap-2">
          <span className="num font-semibold">{compact(p.tps_10s)}</span>
          <span className="text-[11px] text-ink-3">tx/s</span>
        </span>
        <Sparkline values={tps} height={22} className="mt-1" />
      </div>
      <div className="min-w-0">
        <span className="flex items-baseline justify-between gap-2">
          <span className={`num font-semibold ${failTone(p)}`}>{p.tx_60s ? pct(p.failure_rate_60s) : '—'}</span>
          <span className="text-[11px] text-ink-3 truncate">
            {p.health === 'warming_up' ? 'learning' : `normal ${pct(p.baseline_failure_rate)}`}
          </span>
        </span>
        <Sparkline values={fail} tone="fail" height={22} className="mt-1" />
      </div>
      <div className="hidden md:block text-right">
        <span className="block num font-semibold">{compact(p.avg_cu_60s)}</span>
        <span className="text-[11px] text-ink-3">avg CU</span>
      </div>
      <div className="text-right">
        {p.open_incidents > 0 ? (
          <span className="inline-flex rounded bg-crit-soft px-2 py-1 text-xs font-medium text-crit">
            {p.open_incidents} open
          </span>
        ) : (
          <span className="text-xs text-ink-3">{p.health === 'warming_up' ? 'Learning baseline' : p.health === 'idle' ? 'No traffic yet' : 'Healthy'}</span>
        )}
      </div>
    </Link>
  );
}

export function Overview() {
  const { programs, series, loading, error, reload } = usePrograms();
  const [adding, setAdding] = useState(false);
  const incidents = useFetch<Incident[]>('/api/incidents?limit=15');
  useLive((e) => {
    if (e.type === 'incident') incidents.setData((prev) => mergeIncident(prev ?? [], e.incident).slice(0, 15));
  });

  const open = programs.reduce((n, p) => n + p.open_incidents, 0);
  const tx60 = programs.reduce((n, p) => n + p.tx_60s, 0);
  const failed60 = programs.reduce((n, p) => n + p.failed_60s, 0);
  const observed = programs.reduce((n, p) => n + p.total_tx, 0);
  const healthy = programs.filter((p) => p.health === 'healthy').length;
  const learning = programs.filter((p) => p.health === 'warming_up').length;
  const programsSub = [
    healthy && `${healthy} healthy`,
    learning && `${learning} learning`,
    programs.length - healthy - learning > 0 && `${programs.length - healthy - learning} degraded`,
  ]
    .filter(Boolean)
    .join(' · ');
  const showForm = adding || (!loading && !error && programs.length === 0);

  return (
    <div className="space-y-6">
      <PageHeader
        title="Overview"
        meta="Program health on Solana mainnet, streamed through Vortex."
        actions={
          programs.length > 0 && (
            <button className={adding ? 'btn' : 'btn-primary'} onClick={() => setAdding((a) => !a)} aria-expanded={adding}>
              {adding ? <X className="size-4" aria-hidden /> : <Plus className="size-4" aria-hidden />}
              {adding ? 'Cancel' : 'Monitor a program'}
            </button>
          )
        }
      />

      {showForm && (
        <Panel title="Monitor a program">
          <AddProgram onDone={() => setAdding(false)} />
        </Panel>
      )}

      {programs.length > 0 && (
        <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
          <Stat label="Programs" value={programs.length} sub={programsSub || '—'} />
          <Stat
            label="Open incidents"
            value={open}
            tone={open ? 'crit' : undefined}
            sub={open ? 'needs attention' : 'all quiet'}
          />
          <Stat label="Failure rate (60s)" value={tx60 ? pct((failed60 * 100) / tx60) : '—'} sub={`${num(failed60)} of ${num(tx60)} tx`} />
          <Stat label="Transactions observed" value={compact(observed)} sub="since Sentinel started" />
        </div>
      )}

      {error ? (
        <ErrorState message={`Couldn't reach Sentinel: ${error}`} onRetry={reload} />
      ) : loading ? (
        <Skeleton className="h-40" />
      ) : programs.length === 0 ? (
        <div className="panel">
          <Empty title="No programs monitored" icon={<Radar className="size-6" aria-hidden />}>
            Add a program above. Sentinel starts streaming its transactions immediately and learns a baseline over the
            first two minutes.
          </Empty>
        </div>
      ) : (
        <Panel title="Programs" action={<span className="text-xs text-ink-3">last 5 min</span>}>
          <div className="hidden md:grid grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)_minmax(0,1fr)_90px_110px] gap-4 px-4 py-2 border-b border-line text-xs text-ink-3">
            <span>Program</span>
            <span>Throughput</span>
            <span>Failure rate (60s)</span>
            <span className="text-right">Compute</span>
            <span className="text-right">Status</span>
          </div>
          <div className="divide-y divide-line">
            {programs.map((p) => (
              <ProgramRow key={p.program_id} p={p} series={series[p.program_id] ?? []} />
            ))}
          </div>
        </Panel>
      )}

      <Panel title="Recent incidents" action={<Link to="/incidents" className="link text-xs">View all</Link>}>
        {incidents.loading && !incidents.data ? (
          <div className="p-4 space-y-2">{[0, 1].map((i) => <Skeleton key={i} className="h-12" />)}</div>
        ) : incidents.error ? (
          <div className="p-4 text-crit text-sm">{incidents.error}</div>
        ) : incidents.data?.length ? (
          <IncidentList incidents={incidents.data} />
        ) : (
          <Empty title="No incidents">Incidents appear here the moment a detector or alert rule fires.</Empty>
        )}
      </Panel>
    </div>
  );
}
