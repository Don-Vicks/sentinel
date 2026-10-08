import { useEffect } from 'react';
import { Link, useParams } from 'react-router';
import { useFetch } from '../lib/api';
import { ago, KIND_LABEL, num, pct, duration } from '../lib/format';
import { CopyButton, Empty, ErrorState, PageSkeleton, SeverityBadge } from '../components/ui';
import type { IncidentKind, IncidentStatus, Severity } from '../lib/types';

interface PublicStatus {
  program: { id: string; label: string | null };
  health: { score: number | null; status: 'healthy' | 'degraded' | 'critical' | 'learning'; headline: string };
  uptime_percent: number;
  uptime_days: number;
  coverage: number;
  transactions_7d: number;
  success_rate_7d: number | null;
  incidents_7d: number;
  mean_time_to_resolve_secs: number | null;
  open_incidents: number;
  incidents: { id: number; kind: IncidentKind; severity: Severity; status: IncidentStatus; title: string; summary: string; detected_at: string; resolved_at: string | null }[];
  generated_at: string;
}

const TONE = {
  healthy: { word: 'Operating normally', cls: 'bg-good-soft text-good' },
  degraded: { word: 'Degraded', cls: 'bg-warn-soft text-warn' },
  critical: { word: 'Major problems', cls: 'bg-crit-soft text-crit' },
  learning: { word: 'Still learning this program', cls: 'bg-sunken text-ink-2' },
} as const;

/** A page a team can link to: how the program is doing, and its recent incidents. No sign-in. */
export function StatusPage() {
  const { id = '' } = useParams();
  const { data, error, loading, reload } = useFetch<PublicStatus>(`/api/public/status/${id}`);
  useEffect(() => {
    const t = setInterval(reload, 30_000);
    return () => clearInterval(t);
  }, [reload]);
  const origin = window.location.origin;
  const badge = `[![Sentinel](${origin}/badge/${id}.svg)](${origin}/status/${id})`;

  return (
    <div className="min-h-screen bg-canvas text-ink">
      <div className="mx-auto max-w-3xl px-4 py-8 space-y-6">
        {error ? (
          <ErrorState message={error} onRetry={reload} />
        ) : loading && !data ? (
          <PageSkeleton />
        ) : data ? (
          <>
            <header>
              <p className="text-xs text-ink-3">Status</p>
              <h1 className="text-2xl font-semibold tracking-tight">{data.program.label ?? id}</h1>
            </header>

            <div className={`rounded-lg px-5 py-4 ${TONE[data.health.status].cls}`} role="status">
              <div className="flex flex-wrap items-baseline gap-x-3">
                <span className="text-lg font-semibold">{TONE[data.health.status].word}</span>
                {data.health.score !== null && <span className="num">health {data.health.score}/100</span>}
              </div>
              <p className="text-sm mt-1 opacity-90">{data.health.headline}</p>
            </div>

            <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
              {[
                ['Uptime, 7 days', `${data.uptime_percent.toFixed(2)}%`, data.coverage < 0.9 ? `watched ${Math.round(data.coverage * 100)}% of the period` : undefined],
                ['Transactions', num(data.transactions_7d), data.success_rate_7d !== null ? `${pct(data.success_rate_7d, 2)} succeeded` : undefined],
                ['Incidents', num(data.incidents_7d), data.open_incidents ? `${data.open_incidents} open now` : 'none open'],
                ['Typically fixed in', data.mean_time_to_resolve_secs !== null ? duration(data.mean_time_to_resolve_secs * 1000) : '—', undefined],
              ].map(([label, value, sub]) => (
                <div key={label as string} className="panel px-4 py-3">
                  <div className="text-xs text-ink-3">{label}</div>
                  <div className="num text-xl font-semibold">{value}</div>
                  {sub && <div className="text-xs text-ink-3">{sub}</div>}
                </div>
              ))}
            </div>

            <section className="panel">
              <h2 className="panel-title px-4 pt-3">Recent incidents</h2>
              {data.incidents.length === 0 ? (
                <Empty title="No incidents">Nothing has gone wrong while Sentinel was watching.</Empty>
              ) : (
                <ul className="divide-y divide-line">
                  {data.incidents.map((i) => (
                    <li key={i.id} className="px-4 py-3 text-sm">
                      <div className="flex flex-wrap items-center gap-2">
                        <SeverityBadge severity={i.severity} />
                        <span className="font-medium">{KIND_LABEL[i.kind] ?? i.kind}</span>
                        <span className={i.status === 'resolved' ? 'text-good text-xs' : 'text-crit text-xs'}>{i.status === 'resolved' ? 'Resolved' : 'Ongoing'}</span>
                        <span className="text-xs text-ink-3 ml-auto">{ago(i.detected_at)}</span>
                      </div>
                      <p className="text-ink-2 mt-1">{i.summary}</p>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <footer className="text-xs text-ink-3 space-y-2">
              <p>
                Uptime is the share of the time Sentinel was watching with no open reliability incident (failure or error spikes, stopped activity, rules). Monitored by{' '}
                <Link to="/" className="link">Vortex Sentinel</Link>. Updated {ago(data.generated_at)}.
              </p>
              <p className="flex flex-wrap items-center gap-2">
                Badge for your README: <code className="font-mono break-all">{badge}</code>
                <CopyButton text={badge} label="Copy badge markdown" />
              </p>
            </footer>
          </>
        ) : null}
      </div>
    </div>
  );
}
