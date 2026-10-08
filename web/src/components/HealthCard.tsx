import { CheckCircle2, CircleHelp, TriangleAlert, XCircle } from 'lucide-react';
import { useFetch } from '../lib/api';
import { useLive } from '../lib/live';
import type { HealthCheck, HealthReport } from '../lib/types';
import { Panel, Skeleton } from './ui';

const STATUS = {
  healthy: { label: 'Healthy', text: 'text-good', bg: 'bg-good-soft' },
  degraded: { label: 'Degraded', text: 'text-warn', bg: 'bg-warn-soft' },
  critical: { label: 'Critical', text: 'text-crit', bg: 'bg-crit-soft' },
  learning: { label: 'Learning', text: 'text-ink-3', bg: 'bg-sunken' },
} as const;

function CheckIcon({ status }: { status: HealthCheck['status'] }) {
  const cls = 'size-4 shrink-0 mt-0.5';
  if (status === 'pass') return <CheckCircle2 className={`${cls} text-good`} aria-label="Passing" />;
  if (status === 'warn') return <TriangleAlert className={`${cls} text-warn`} aria-label="Warning" />;
  if (status === 'fail') return <XCircle className={`${cls} text-crit`} aria-label="Failing" />;
  return <CircleHelp className={`${cls} text-ink-3`} aria-label="Not enough data" />;
}

/** A 0-100 score and the checks behind it; every row states the rule and the numbers. */
export function HealthCard({ programId }: { programId: string }) {
  const { data, error, loading, reload } = useFetch<HealthReport>(`/api/programs/${programId}/health`);
  // Incidents opening or closing change the score straight away.
  useLive((e) => {
    if (e.type === 'incident' && e.incident.program_id === programId) reload();
  });
  const s = data ? STATUS[data.status] : null;

  return (
    <Panel title="Health check">
      {loading && !data ? (
        <div className="p-4"><Skeleton className="h-24" /></div>
      ) : error || !data || !s ? (
        <p className="p-4 text-sm text-ink-3">Couldn&apos;t run the health check{error ? `: ${error}` : ''}.</p>
      ) : (
        <div className="p-4 grid gap-4">
          <div className="flex items-center gap-4">
            <div className={`rounded-lg px-4 py-2 text-center ${s.bg}`}>
              <div className={`num text-3xl font-semibold leading-none ${s.text}`}>{data.score ?? '–'}</div>
              <div className={`text-xs font-medium mt-1 ${s.text}`}>{s.label}</div>
            </div>
            <p className="text-sm text-ink-2 min-w-0">{data.headline}</p>
          </div>
          <ul className="grid gap-2">
            {data.checks.map((c) => (
              <li key={c.id} className="flex gap-2 text-sm">
                <CheckIcon status={c.status} />
                <div className="min-w-0">
                  <span className="font-medium">{c.label}</span>
                  {c.score !== null && <span className="num text-xs text-ink-3"> · {c.score}/100</span>}
                  <p className="text-xs text-ink-3">{c.detail}</p>
                </div>
              </li>
            ))}
          </ul>
        </div>
      )}
    </Panel>
  );
}
