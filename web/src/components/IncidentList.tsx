import { Link } from 'react-router';
import type { Incident, Severity } from '../lib/types';
import { ago, KIND_LABEL } from '../lib/format';
import { SeverityBadge, StatusBadge } from './ui';

const STRIPE: Record<Severity, string> = {
  critical: 'border-l-crit',
  high: 'border-l-serious',
  medium: 'border-l-warn',
  low: 'border-l-line-strong',
};

export function IncidentList({ incidents, showProgram = true }: { incidents: Incident[]; showProgram?: boolean }) {
  return (
    <ul className="divide-y divide-line">
      {incidents.map((i) => (
        <li key={i.id}>
          <Link
            to={`/incidents/${i.id}`}
            className={`grid grid-cols-[auto_1fr_auto] items-center gap-x-3 gap-y-0.5 border-l-2 px-4 py-3 hover:bg-sunken ${
              i.status === 'resolved' ? 'border-l-transparent' : STRIPE[i.severity]
            }`}
          >
            <span className="num text-xs text-ink-3 w-12">#{i.id}</span>
            <span className="min-w-0">
              <span className="flex items-center gap-2 flex-wrap">
                <SeverityBadge severity={i.severity} />
                <span className="font-medium text-ink">{KIND_LABEL[i.kind] ?? i.kind}</span>
                {showProgram && <span className="text-ink-3 truncate">· {i.title.split(' · ').slice(1).join(' · ')}</span>}
              </span>
              <span className="block text-ink-2 truncate mt-0.5">{i.summary}</span>
            </span>
            <span className="flex flex-col items-end gap-1">
              <StatusBadge status={i.status} />
              <span className="text-xs text-ink-3 num">{ago(i.detected_at)}</span>
            </span>
          </Link>
        </li>
      ))}
    </ul>
  );
}

/** Applies a live incident event to a list, newest first. */
export function mergeIncident(list: Incident[], incident: Incident, filterProgram?: string) {
  if (filterProgram && incident.program_id !== filterProgram) return list;
  const i = list.findIndex((x) => x.id === incident.id);
  if (i === -1) return [incident, ...list];
  const next = list.slice();
  next[i] = incident;
  return next;
}
