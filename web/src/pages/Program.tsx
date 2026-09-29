import { useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { ArrowRight, Pause, Play, Trash2 } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import { useLive } from '../lib/live';
import { usePrograms } from '../lib/programs';
import type { Incident, MonitoredProgram, ProgramSnapshot, SeriesPoint, Severity, TxSummary } from '../lib/types';
import { compact, duration, KIND_LABEL, num, pct } from '../lib/format';
import { ActivityChart, LineChart, ShareBar, Sparkline } from '../components/charts';
import { Address, Empty, ErrorState, HealthDot, PageHeader, PageSkeleton, Panel, SeverityBadge, Stat } from '../components/ui';
import { IncidentList, mergeIncident } from '../components/IncidentList';
import { TxTable } from '../components/TxTable';

const SEVERITY_RANK: Record<Severity, number> = { low: 0, medium: 1, high: 2, critical: 3 };

function bucket(points: SeriesPoint[], size: number) {
  const out: { tx: number; failed: number; cu: number; cu_n: number }[] = [];
  for (let i = 0; i < points.length; i += size) {
    const chunk = points.slice(i, i + size);
    out.push({
      tx: chunk.reduce((n, p) => n + p.tx, 0),
      failed: chunk.reduce((n, p) => n + p.failed, 0),
      cu: chunk.reduce((n, p) => n + p.avg_cu * p.tx, 0),
      cu_n: chunk.reduce((n, p) => n + (p.avg_cu ? p.tx : 0), 0),
    });
  }
  return out;
}

/** "3.1× normal" when a metric is well away from its baseline. */
function ratioBadge(value: number, baseline: number, learning: boolean, minBaseline = 0) {
  if (learning || baseline <= minBaseline || value <= 0) return null;
  const r = value / baseline;
  if (r >= 2) return { text: `${r.toFixed(1)}× normal`, tone: r >= 3 ? ('crit' as const) : ('warn' as const) };
  if (r <= 0.5) return { text: `${r.toFixed(1)}× normal`, tone: 'muted' as const };
  return null;
}

/** Loud, first thing on the page while anything is open. */
function IncidentBanner({ incidents }: { incidents: Incident[] }) {
  const [top, ...rest] = incidents;
  const since = Math.max(0, Date.now() - new Date(top.onset_at ?? top.detected_at).getTime());
  return (
    <div className="rounded-lg border border-crit/40 bg-crit-soft">
      <Link
        to={`/incidents/${top.id}`}
        className="grid gap-2 px-4 py-3 hover:bg-crit/5 rounded-lg sm:grid-cols-[auto_minmax(0,1fr)_auto_auto] sm:items-center sm:gap-x-4"
      >
        <span>
          <SeverityBadge severity={top.severity} />
        </span>
        <span className="min-w-0">
          <span className="font-medium text-ink">
            {KIND_LABEL[top.kind] ?? top.kind} <span className="num text-ink-3">#{top.id}</span>
          </span>
          <span className="block text-sm text-ink-2 truncate">{top.summary}</span>
        </span>
        <span className="num text-xs text-ink-2 whitespace-nowrap">
          {num(top.affected_count)} tx · {num(top.affected_wallets)} wallets · {duration(since)}
        </span>
        <span className="inline-flex items-center gap-1 text-sm font-medium text-crit">
          Investigate <ArrowRight className="size-4" aria-hidden />
        </span>
      </Link>
      {rest.length > 0 && (
        <div className="border-t border-crit/20 px-4 py-2 text-xs text-ink-2">
          Also open:{' '}
          {rest.map((i, n) => (
            <span key={i.id}>
              {n > 0 && ', '}
              <Link to={`/incidents/${i.id}`} className="link">
                #{i.id} {KIND_LABEL[i.kind] ?? i.kind}
              </Link>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

interface Detail {
  program: MonitoredProgram;
  snapshot: ProgramSnapshot;
  series: SeriesPoint[];
  recent: TxSummary[];
  incidents: Incident[];
}

export function Program() {
  const { id = '' } = useParams();
  const navigate = useNavigate();
  const { reload: reloadPrograms } = usePrograms();
  const { data, setData, error, loading, reload } = useFetch<Detail>(`/api/programs/${id}`);
  const [paused, setPaused] = useState(false);
  const [failedOnly, setFailedOnly] = useState(false);
  const [fresh, setFresh] = useState<Set<string>>(new Set());
  const pausedRef = useRef(paused);
  pausedRef.current = paused;

  useEffect(() => setFresh(new Set()), [id]);

  useLive((e) => {
    if (e.type === 'metrics' && e.program_id === id) {
      setData((d) => {
        if (!d) return d;
        let series = d.series;
        const p = e.snapshot.point;
        if (p && (!series.length || series[series.length - 1].t < p.t)) {
          series = [...series, p].slice(-600);
        }
        return { ...d, snapshot: e.snapshot, series };
      });
    }
    if (e.type === 'transactions' && e.program_id === id && !pausedRef.current) {
      setData((d) => (d ? { ...d, recent: [...e.items, ...d.recent].slice(0, 200) } : d));
      setFresh(new Set(e.items.map((t) => t.signature)));
    }
    if (e.type === 'incident' && e.incident.program_id === id) {
      setData((d) => (d ? { ...d, incidents: mergeIncident(d.incidents, e.incident, id) } : d));
    }
  });

  if (error) return <ErrorState message={error} onRetry={reload} />;
  if (loading && !data) return <PageSkeleton />;
  if (!data) return null;

  const s = data.snapshot;
  const failTone =
    s.failure_rate_60s >= Math.max(s.baseline_failure_rate * 2.5, s.baseline_failure_rate + 5) && s.tx_60s >= 20
      ? 'crit'
      : undefined;
  const learning = s.health === 'warming_up' || s.health === 'idle';
  const base = (v: string) => (learning ? 'normal: learning' : `normal ${v}`);
  const open = data.incidents
    .filter((i) => i.status !== 'resolved')
    .sort((a, b) => SEVERITY_RANK[b.severity] - SEVERITY_RANK[a.severity]);
  const rows = failedOnly ? data.recent.filter((t) => !t.success) : data.recent;

  // Sparklines: last 2 minutes, in 5s buckets so single seconds don't dominate.
  const recent = bucket(data.series.slice(-120), 5);
  const sparkTps = recent.map((b) => b.tx / 5);
  const sparkFail = recent.map((b) => (b.tx ? (b.failed * 100) / b.tx : null));
  const sparkCu = recent.map((b) => (b.cu_n ? b.cu / b.cu_n : null));

  const remove = async () => {
    if (!confirm(`Stop monitoring ${data.program.label}? Its incidents stay in history.`)) return;
    await send('DELETE', `/api/programs/${id}`);
    await reloadPrograms();
    navigate('/');
  };

  return (
    <div className="space-y-6">
      <PageHeader
        crumbs={[{ label: 'Overview', to: '/' }]}
        title={
          <span className="flex items-center gap-3">
            {data.program.label}
            <HealthDot health={s.health} withLabel />
          </span>
        }
        meta={
          <span className="flex flex-wrap items-center gap-x-2 text-ink-3">
            <Address value={id} n={8} />
            <span aria-hidden>·</span>
            <span className="text-xs num">{num(s.total_tx)} transactions observed</span>
            {s.health === 'warming_up' && (
              <>
                <span aria-hidden>·</span>
                <span className="text-xs text-accent">
                  learning the normal baseline, detectors arm in {duration(s.warmup_remaining_secs * 1000)}
                </span>
              </>
            )}
          </span>
        }
        actions={
          <button className="btn" onClick={remove}>
            <Trash2 className="size-4" aria-hidden />
            Stop monitoring
          </button>
        }
      />

      {open.length > 0 && <IncidentBanner incidents={open} />}

      <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
        <Stat
          label="Throughput (10s)"
          value={`${compact(s.tps_10s)} tx/s`}
          sub={base(`${compact(s.baseline_tps)} tx/s`)}
          badge={ratioBadge(s.tps_10s, s.baseline_tps, learning)}
          spark={<Sparkline values={sparkTps} height={26} />}
        />
        <Stat
          label="Failure rate (60s)"
          value={pct(s.failure_rate_60s)}
          sub={`${base(pct(s.baseline_failure_rate))} · ${num(s.failed_60s)}/${num(s.tx_60s)} tx`}
          tone={failTone}
          badge={ratioBadge(s.failure_rate_60s, s.baseline_failure_rate, learning, 1)}
          spark={<Sparkline values={sparkFail} tone="fail" height={26} />}
        />
        <Stat
          label="Avg compute (60s)"
          value={`${compact(s.avg_cu_60s)} CU`}
          sub={`max ${compact(s.max_cu_60s)} · ${base(compact(s.baseline_avg_cu))}`}
          badge={ratioBadge(s.avg_cu_60s, s.baseline_avg_cu, learning)}
          spark={<Sparkline values={sparkCu} tone="muted" height={26} />}
        />
        <Stat
          label="Unique signers (60s)"
          value={num(s.unique_signers_60s)}
          sub={`fees ${s.fees_60s_sol.toFixed(4)} SOL`}
        />
      </div>

      <div className="grid gap-4 xl:grid-cols-2">
        <Panel title="Transactions per second">
          <div className="p-3">
            {data.series.length > 1 ? <ActivityChart points={data.series} /> : <Empty title="Collecting data" />}
          </div>
        </Panel>
        <Panel title="Average compute units per transaction">
          <div className="p-3">
            {data.series.length > 1 ? (
              <LineChart
                points={data.series}
                value={(p) => (p.tx ? p.avg_cu : null)}
                label="avg CU"
                baseline={s.baseline_avg_cu || undefined}
                format={(v) => num(v)}
              />
            ) : (
              <Empty title="Collecting data" />
            )}
          </div>
        </Panel>
      </div>

      <Panel title="Instructions (last 5 min)">
        {s.instructions.length === 0 ? (
          <Empty title="No instructions yet" />
        ) : (
          <div className="overflow-x-auto">
            <table className="table">
              <thead>
                <tr>
                  <th>Instruction</th>
                  <th className="w-1/4">Share of transactions</th>
                  <th className="text-right">Transactions</th>
                  <th className="text-right">Failure rate</th>
                  <th className="text-right">Avg compute</th>
                </tr>
              </thead>
              <tbody>
                {s.instructions.map((ix) => (
                  <tr key={ix.name}>
                    <td className="font-medium">{ix.name}</td>
                    <td>
                      <div className="flex items-center gap-2">
                        <ShareBar share={ix.share} tone="accent" />
                        <span className="num text-xs w-10 text-right">{(ix.share * 100).toFixed(0)}%</span>
                      </div>
                    </td>
                    <td className="num text-right">{num(ix.tx)}</td>
                    <td className={`num text-right ${ix.failure_rate >= Math.max(10, s.baseline_failure_rate * 2) && ix.tx >= 10 ? 'text-crit font-medium' : ''}`}>
                      {pct(ix.failure_rate)}
                    </td>
                    <td className="num text-right">{compact(ix.avg_cu)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Panel>

      <Panel title="Why transactions fail (last 60s)">
        {s.top_errors.length === 0 ? (
          <Empty title="No failures in the last minute" />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Error</th>
                <th>Raised by</th>
                <th className="w-1/3">Share of failures</th>
                <th className="text-right">Count</th>
              </tr>
            </thead>
            <tbody>
              {s.top_errors.map((e) => (
                <tr key={e.key}>
                  <td className="font-medium">
                    {e.error}
                    {e.code !== null && <span className="ml-1.5 num text-xs text-ink-3">#{e.code}</span>}
                  </td>
                  <td className="text-ink-2">
                    {e.program_name}
                    {e.instruction && <span className="text-ink-3">::{e.instruction}</span>}
                  </td>
                  <td>
                    <div className="flex items-center gap-2">
                      <ShareBar share={e.share} />
                      <span className="num text-xs w-10 text-right">{(e.share * 100).toFixed(0)}%</span>
                    </div>
                  </td>
                  <td className="num text-right">{num(e.count)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Panel>

      <Panel
        title="Live transactions"
        action={
          <div className="flex items-center gap-2">
            <label className="flex items-center gap-1.5 text-xs text-ink-2">
              <input type="checkbox" checked={failedOnly} onChange={(e) => setFailedOnly(e.target.checked)} />
              Failed only
            </label>
            <button className="btn h-8 text-xs" onClick={() => setPaused((p) => !p)} aria-pressed={paused}>
              {paused ? <Play className="size-3.5" aria-hidden /> : <Pause className="size-3.5" aria-hidden />}
              {paused ? 'Resume' : 'Pause'}
            </button>
          </div>
        }
      >
        {rows.length === 0 ? (
          <Empty title={failedOnly ? 'No failed transactions yet' : 'Waiting for transactions'}>
            Transactions appear here the moment Vortex receives them from the stream.
          </Empty>
        ) : (
          <TxTable rows={rows.slice(0, 100)} fresh={fresh} />
        )}
      </Panel>

      {data.incidents.some((i) => i.status === 'resolved') && (
        <Panel title="Incident history" action={<Link to={`/incidents?program=${id}`} className="link text-xs">All</Link>}>
          <IncidentList incidents={data.incidents.filter((i) => i.status === 'resolved').slice(0, 8)} showProgram={false} />
        </Panel>
      )}
    </div>
  );
}
