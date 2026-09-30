import { useState } from 'react';
import { Link } from 'react-router';
import { Plus, Radar, Star, X } from 'lucide-react';
import { useFetch } from '../lib/api';
import { usePrograms } from '../lib/programs';
import { useAuth } from '../lib/auth';
import { useLive } from '../lib/live';
import type { Incident, ProgramSnapshot, SeriesPoint } from '../lib/types';
import { compact, num, pct, short } from '../lib/format';
import { Empty, ErrorState, HealthDot, PageHeader, Panel, Skeleton, Stat } from '../components/ui';
import { Sparkline } from '../components/charts';
import { Finder } from '../components/Finder';
import { IncidentList, mergeIncident } from '../components/IncidentList';

function failTone(p: ProgramSnapshot) {
  if (p.tx_60s < 20) return '';
  if (p.failure_rate_60s >= Math.max(p.baseline_failure_rate * 2.5, p.baseline_failure_rate + 5)) return 'text-crit';
  return '';
}

function ProgramRow({ p, series, mine }: { p: ProgramSnapshot; series: SeriesPoint[]; mine: boolean }) {
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
          {mine && <Star className="size-3 shrink-0 fill-current text-accent" aria-label="On your watchlist" />}
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
  const { account, watching } = useAuth();
  const [adding, setAdding] = useState(false);
  const [onlyMine, setOnlyMine] = useState(false);
  const mine = new Set(watching);
  const listed = onlyMine ? programs.filter((p) => mine.has(p.program_id)) : programs;
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
          (programs.length > 0 || !account) && (
            <button
              className={adding ? 'btn' : 'btn-primary'}
              onClick={() => setAdding((a) => !a)}
              aria-expanded={adding}
            >
              {adding ? <X className="size-4" aria-hidden /> : <Plus className="size-4" aria-hidden />}
              {adding ? 'Cancel' : 'Find programs'}
            </button>
          )
        }
      />

      {showForm && (
        <Panel title="Find programs to watch">
          <Finder onDone={() => setAdding(false)} />
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
            Paste a program, an upgrade-authority address or a transaction above. Sentinel starts streaming immediately and learns a baseline over the first two minutes.
          </Empty>
        </div>
      ) : (
        <Panel
          title="Programs"
          action={
            account ? (
              <div className="inline-flex rounded-md border border-line-strong p-0.5 text-xs" role="group" aria-label="Show">
                {[
                  { v: false, label: 'All' },
                  { v: true, label: `Watching (${watching.length})` },
                ].map((o) => (
                  <button
                    key={o.label}
                    className={`h-7 rounded px-2.5 ${onlyMine === o.v ? 'bg-sunken font-medium text-ink' : 'text-ink-2'}`}
                    aria-pressed={onlyMine === o.v}
                    onClick={() => setOnlyMine(o.v)}
                  >
                    {o.label}
                  </button>
                ))}
              </div>
            ) : (
              <span className="text-xs text-ink-3">last 5 min</span>
            )
          }
        >
          <div className="hidden md:grid grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)_minmax(0,1fr)_90px_110px] gap-4 px-4 py-2 border-b border-line text-xs text-ink-3">
            <span>Program</span>
            <span>Throughput</span>
            <span>Failure rate (60s)</span>
            <span className="text-right">Compute</span>
            <span className="text-right">Status</span>
          </div>
          <div className="divide-y divide-line">
            {listed.map((p) => (
              <ProgramRow key={p.program_id} p={p} series={series[p.program_id] ?? []} mine={mine.has(p.program_id)} />
            ))}
            {listed.length === 0 && (
              <Empty title="Your watchlist is empty">Add a program, or star one from its page.</Empty>
            )}
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
