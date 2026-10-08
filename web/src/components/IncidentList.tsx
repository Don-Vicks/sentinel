import { Link } from 'react-router';
import type { Incident, Severity } from '../lib/types';
import { ago, KIND_LABEL, num } from '../lib/format';
import { SeverityBadge, StatusBadge } from './ui';

const STRIPE: Record<Severity, string> = {
  critical: 'border-l-crit',
  high: 'border-l-serious',
  medium: 'border-l-warn',
  low: 'border-l-line-strong',
  info: 'border-l-transparent',
};

/** A count column in the issue stream: big number, small unit. */
function Metric({ label, value }: { label: string; value: number }) {
  return (
    <span className="hidden md:flex flex-col items-end leading-tight">
      <span className={`num text-sm ${value ? 'text-ink' : 'text-ink-3'}`} aria-label={`${label}: ${value}`}>{num(value)}</span>
    </span>
  );
}

export function IncidentList({ incidents, showProgram = true, counts = true }: { incidents: Incident[]; showProgram?: boolean; counts?: boolean }) {
  return (
    <ul className="divide-y divide-line">
      {counts && (
        <li className="hidden md:grid grid-cols-[auto_1fr_5.5rem_5.5rem_6.5rem] gap-x-3 border-l-2 border-l-transparent bg-sunken/50 px-4 py-1.5 text-[11px] font-medium uppercase tracking-wider text-ink-3" aria-hidden>
          <span className="w-12">ID</span>
          <span>Incident</span>
          <span className="text-right">Txs</span>
          <span className="text-right">Wallets</span>
          <span className="text-right">Status</span>
        </li>
      )}
      {incidents.map((i) => (
        <li key={i.id}>
          <Link
            to={`/incidents/${i.id}`}
            className={`grid ${counts ? 'grid-cols-[auto_1fr_auto] md:grid-cols-[auto_1fr_5.5rem_5.5rem_6.5rem]' : 'grid-cols-[auto_1fr_auto]'} items-center gap-x-3 gap-y-0.5 border-l-2 px-4 py-2.5 hover:bg-sunken ${
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
            {counts && <Metric label="transactions" value={i.affected_count} />}
            {counts && <Metric label="wallets" value={i.affected_wallets} />}
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
