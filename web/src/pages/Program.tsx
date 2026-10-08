import { useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router';
import { ArrowRight, ExternalLink, Pause, Play, Star } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import { useLive } from '../lib/live';
import { usePrograms } from '../lib/programs';
import { useAuth } from '../lib/auth';
import type { Incident, MonitoredProgram, ProgramSnapshot, SeriesPoint, Severity, TxSummary } from '../lib/types';
import { compact, duration, KIND_LABEL, num, pct } from '../lib/format';
import { ActivityChart, LineChart, ShareBar, Sparkline } from '../components/charts';
import { Address, Empty, ErrorState, HealthDot, PageHeader, PageSkeleton, Panel, SeverityBadge, Segmented, ShowMore, Stat, Tabs } from '../components/ui';
import { IncidentList, mergeIncident } from '../components/IncidentList';
import { TxTable } from '../components/TxTable';
import { PostureCard } from '../components/PostureCard';
import { HealthCard } from '../components/HealthCard';
import { FundsCard } from '../components/FundsCard';
import { DependenciesCard } from '../components/DependenciesCard';
import { MuteControl } from '../components/MuteControl';
import { ProtectCard } from '../components/ProtectCard';
import { SummaryView } from './Summary';

const TABS = ['overview', 'activity', 'security', 'summary'] as const;
type Tab = (typeof TABS)[number];

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
  const [params, setParams] = useSearchParams();
  const tabParam = params.get('tab') as Tab | null;
  const tab: Tab = tabParam && TABS.includes(tabParam) ? tabParam : 'overview';
  const setTab = (t: Tab) => setParams(t === 'overview' ? {} : { tab: t }, { replace: true });
  const { reload: reloadPrograms } = usePrograms();
  const { account, watching, refresh: refreshAuth, requestSignIn } = useAuth();
  const [watchBusy, setWatchBusy] = useState(false);
  const [watchError, setWatchError] = useState<string | null>(null);
  const { data, setData, error, loading, reload } = useFetch<Detail>(`/api/programs/${id}`);
  const [paused, setPaused] = useState(false);
  const [failedOnly, setFailedOnly] = useState(false);
  const [ixShown, setIxShown] = useState(6);
  const [errShown, setErrShown] = useState(6);
  const [errScope, setErrScope] = useState<'own' | 'all'>('own');
  const [txShown, setTxShown] = useState(25);
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
  const errRows = errScope === 'own' ? s.top_errors.filter((e) => e.own) : s.top_errors;
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

  const isWatching = watching.includes(id);
  const toggleWatch = async () => {
    if (!account) return requestSignIn();
    setWatchBusy(true);
    setWatchError(null);
    try {
      if (isWatching) {
        await send('DELETE', `/api/programs/${id}`);
      } else {
        await send('POST', '/api/programs', { program_id: id });
      }
      await Promise.all([refreshAuth(), reloadPrograms()]);
      // Leaving the last watch stops monitoring; the page has nothing left to show.
      if (isWatching) {
        const still = await fetch(`/api/programs/${id}`);
        if (!still.ok) navigate('/');
      }
    } catch (e) {
      setWatchError((e as Error).message);
    } finally {
      setWatchBusy(false);
    }
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
          <span className="flex flex-col gap-y-1 text-ink-3 sm:flex-row sm:flex-wrap sm:items-center sm:gap-x-2">
            <Address value={id} n={8} />
            <span aria-hidden className="hidden sm:inline">·</span>
            <span className="text-xs num">{num(s.total_tx)} transactions observed</span>
            {s.health === 'warming_up' && (
              <>
                <span aria-hidden className="hidden sm:inline">·</span>
                <span className="text-xs text-accent">
                  learning the normal baseline, detectors arm in {duration(s.warmup_remaining_secs * 1000)}
                </span>
              </>
            )}
          </span>
        }
        actions={
          <>
          <Link to={`/status/${id}`} className="btn" target="_blank" title="A public page for this program, with an embeddable badge">
            <ExternalLink className="size-4" aria-hidden /> Status page
          </Link>
          <button
            className={isWatching ? 'btn' : 'btn-primary'}
            onClick={toggleWatch}
            disabled={watchBusy}
            aria-pressed={isWatching}
            title={isWatching ? 'Remove from your watchlist' : 'Add to your watchlist to get alerts for it'}
          >
            <Star className={`size-4 ${isWatching ? 'fill-current text-accent' : ''}`} aria-hidden />
            {isWatching ? 'Watching' : 'Watch'}
          </button>
          </>
        }
      />

      {watchError && (
        <p className="rounded-md bg-crit-soft px-3 py-2 text-sm text-crit" role="alert">
          {watchError}
        </p>
      )}

      <MuteControl program={data.program} onChange={(p) => setData((d) => (d ? { ...d, program: p } : d))} />

      {open.length > 0 && <IncidentBanner incidents={open} />}

      <Tabs
        label="Program sections"
        value={tab}
        onChange={setTab}
        tabs={[
          { value: 'overview', label: 'Overview', badge: open.length, tone: 'crit' },
          { value: 'activity', label: 'Activity' },
          { value: 'security', label: 'Security' },
          { value: 'summary', label: 'Summary' },
        ]}
      />

      {tab === 'overview' && (
        <div role="tabpanel" id="panel-overview" aria-labelledby="tab-overview" className="space-y-6">
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

        <div className="grid items-start gap-4 xl:grid-cols-2">
        <HealthCard programId={id} />
        {data.incidents.some((i) => i.status === 'resolved') && (
          <Panel title="Incident history" action={<Link to={`/incidents?program=${id}`} className="link text-xs">All</Link>}>
            <IncidentList incidents={data.incidents.filter((i) => i.status === 'resolved').slice(0, 8)} showProgram={false} />
          </Panel>
        )}
        </div>
        </div>
      )}

      {tab === 'activity' && (
        <div role="tabpanel" id="panel-activity" aria-labelledby="tab-activity" className="space-y-6">
        <Panel title="Instructions (last 5 min)">
          {s.instructions.length === 0 ? (
            <Empty title="No instructions yet" />
          ) : (
            <div className="overflow-x-auto">
              <table className="table table-stack">
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
                  {s.instructions.slice(0, ixShown).map((ix) => (
                    <tr key={ix.name}>
                      <td data-primary className="font-medium">
                        {ix.name === '(unnamed)' ? (
                          <span title="Transactions that touch this program but whose logs name no instruction, usually bots whose own program fails first">
                            No instruction named
                          </span>
                        ) : (
                          ix.name
                        )}
                      </td>
                      <td data-label="Share of transactions" data-wide>
                        <div className="flex items-center gap-2">
                          <ShareBar share={ix.share} tone="accent" />
                          <span className="num text-xs w-10 text-right">{(ix.share * 100).toFixed(0)}%</span>
                        </div>
                      </td>
                      <td data-label="Transactions" className="num text-right">{num(ix.tx)}</td>
                      <td data-label="Failure rate" className={`num text-right ${ix.failure_rate >= Math.max(10, s.baseline_failure_rate * 2) && ix.tx >= 10 ? 'text-crit font-medium' : ''}`}>
                        {pct(ix.failure_rate)}
                      </td>
                      <td data-label="Avg compute" className="num text-right">{compact(ix.avg_cu)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
          <ShowMore shown={ixShown} total={s.instructions.length} step={20} onMore={() => setIxShown((n) => n + 20)} />
        </Panel>

        <Panel
          title="Why transactions fail (last 60s)"
          action={
            s.top_errors.length > 0 && (
              <Segmented
                label="Which errors to show"
                value={errScope}
                onChange={setErrScope}
                options={[
                  { value: 'own', label: `${data.program.label} (${s.top_errors.filter((e) => e.own).length})` },
                  { value: 'all', label: `All (${s.top_errors.length})` },
                ]}
              />
            )
          }
        >
          {s.top_errors.length === 0 ? (
            <Empty title="No failures in the last minute" />
          ) : errRows.length === 0 ? (
            <Empty title={`No errors raised by ${data.program.label} itself`}>
              {s.top_errors.length} come from other programs in the same transactions. They count toward the failure rate but don't open
              incidents. Switch to "All" to see them.
            </Empty>
          ) : (
            <table className="table table-stack">
              <thead>
                <tr>
                  <th>Error</th>
                  <th>Raised by</th>
                  <th className="w-1/3">Share of failures</th>
                  <th className="text-right">Count</th>
                </tr>
              </thead>
              <tbody>
                {errRows.slice(0, errShown).map((e) => (
                  <tr key={e.key} className={e.own === false ? 'opacity-60' : ''} title={e.own === false ? 'Raised by another program in the same transactions; counts toward the failure rate but opens no incident' : undefined}>
                    <td data-primary className="font-medium">
                      {e.error}
                      {e.code !== null && <span className="ml-1.5 num text-xs text-ink-3">#{e.code}</span>}
                    </td>
                    <td data-label="Raised by" data-wide className="text-ink-2">
                      {e.program_name}
                      {e.instruction && <span className="text-ink-3">::{e.instruction}</span>}
                    </td>
                    <td data-label="Share of failures" data-wide>
                      <div className="flex items-center gap-2">
                        <ShareBar share={e.share} />
                        <span className="num text-xs w-10 text-right">{(e.share * 100).toFixed(0)}%</span>
                      </div>
                    </td>
                    <td data-label="Count" className="num text-right">{num(e.count)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <ShowMore shown={errShown} total={errRows.length} step={20} onMore={() => setErrShown((n) => n + 20)} />
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
            <TxTable rows={rows.slice(0, txShown)} fresh={fresh} />
          )}
          <ShowMore shown={txShown} total={Math.min(rows.length, 100)} step={25} onMore={() => setTxShown((n) => n + 25)} />
        </Panel>

        </div>
      )}

      {tab === 'security' && (
        <div role="tabpanel" id="panel-security" aria-labelledby="tab-security" className="space-y-6">
        <PostureCard programId={id} incidents={data.incidents} />
        <ProtectCard programId={id} />

        <div className="grid gap-4 xl:grid-cols-2">
          <FundsCard programId={id} />
          <DependenciesCard programId={id} />
        </div>

        </div>
      )}

      {tab === 'summary' && (
        <div role="tabpanel" id="panel-summary" aria-labelledby="tab-summary">
          <SummaryView id={id} />
        </div>
      )}
    </div>
  );
}
