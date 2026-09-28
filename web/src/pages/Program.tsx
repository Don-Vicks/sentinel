import { useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { Pause, Play, Trash2 } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import { useLive } from '../lib/live';
import { usePrograms } from '../lib/programs';
import type { Incident, MonitoredProgram, ProgramSnapshot, SeriesPoint, TxSummary } from '../lib/types';
import { compact, duration, num, pct } from '../lib/format';
import { ActivityChart, LineChart, ShareBar } from '../components/charts';
import { Address, Empty, ErrorState, HealthDot, PageSkeleton, Panel, Stat } from '../components/ui';
import { IncidentList, mergeIncident } from '../components/IncidentList';
import { TxTable } from '../components/TxTable';

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
  const base = (v: string) => (learning ? 'baseline: learning' : `baseline ${v}`);
  const open = data.incidents.filter((i) => i.status !== 'resolved');
  const rows = failedOnly ? data.recent.filter((t) => !t.success) : data.recent;

  const remove = async () => {
    if (!confirm(`Stop monitoring ${data.program.label}? Its incidents stay in history.`)) return;
    await send('DELETE', `/api/programs/${id}`);
    await reloadPrograms();
    navigate('/');
  };

  return (
    <div className="space-y-5">
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-3">
            <h1 className="text-xl font-semibold truncate">{data.program.label}</h1>
            <HealthDot health={s.health} withLabel />
          </div>
          <div className="mt-1 flex items-center gap-2 text-ink-3">
            <Address value={id} n={8} />
            <span aria-hidden>·</span>
            <span className="text-xs num">{num(s.total_tx)} transactions observed</span>
          </div>
          {s.health === 'warming_up' && (
            <p className="mt-2 text-xs text-accent">
              Learning the normal baseline; detectors arm in {duration(s.warmup_remaining_secs * 1000)}.
            </p>
          )}
        </div>
        <button className="btn" onClick={remove}>
          <Trash2 className="size-4" aria-hidden />
          Stop monitoring
        </button>
      </header>

      <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-6 gap-3">
        <Stat label="TPS (10s)" value={compact(s.tps_10s)} sub={base(compact(s.baseline_tps))} />
        <Stat label="Failure rate (60s)" value={pct(s.failure_rate_60s)} sub={base(pct(s.baseline_failure_rate))} tone={failTone} />
        <Stat label="Failed (60s)" value={num(s.failed_60s)} sub={`of ${num(s.tx_60s)} tx`} tone={failTone} />
        <Stat label="Avg compute (60s)" value={compact(s.avg_cu_60s)} sub={`max ${compact(s.max_cu_60s)} · ${base(compact(s.baseline_avg_cu))}`} />
        <Stat label="Unique signers (60s)" value={num(s.unique_signers_60s)} />
        <Stat label="Fees (60s)" value={`${s.fees_60s_sol.toFixed(4)}`} sub="SOL" />
      </div>

      {open.length > 0 && (
        <Panel title={`Open incidents (${open.length})`}>
          <IncidentList incidents={open} showProgram={false} />
        </Panel>
      )}

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
