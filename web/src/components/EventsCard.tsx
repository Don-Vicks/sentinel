import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { useFetch } from '../lib/api';
import type { ProgramEvent } from '../lib/types';
import { ago, short } from '../lib/format';
import { Empty, Panel, Skeleton } from './ui';

interface Events {
  idl_loaded: boolean;
  declares_events: boolean;
  events: ProgramEvent[];
}

/** A field value short enough for a table cell. */
function cell(v: unknown): string {
  if (typeof v === 'string') return v.length > 24 ? short(v, 5) : v;
  if (Array.isArray(v)) return `[${v.length}]`;
  if (v && typeof v === 'object') return '{…}';
  return String(v);
}

/** The first few fields, which usually say what the event was about. */
function Fields({ fields }: { fields: Record<string, unknown> }) {
  const entries = Object.entries(fields).filter(([, v]) => v !== 0 && v !== null && v !== '' && !(Array.isArray(v) && !v.length)).slice(0, 5);
  if (!entries.length) return <span className="text-ink-3">no non-empty fields</span>;
  return (
    <span className="flex flex-wrap gap-x-3 gap-y-0.5">
      {entries.map(([k, v]) => (
        <span key={k} className="whitespace-nowrap">
          <span className="text-ink-3">{k}</span> <span className="num">{cell(v)}</span>
        </span>
      ))}
    </span>
  );
}

/** What the program said it did: Anchor events decoded with its IDL. */
export function EventsCard({ programId }: { programId: string }) {
  const { data, loading, reload } = useFetch<Events>(`/api/programs/${programId}/events`);
  const [name, setName] = useState('');
  useEffect(() => {
    const t = setInterval(reload, 5_000);
    return () => clearInterval(t);
  }, [reload]);
  const names = [...new Set((data?.events ?? []).map((e) => e.name))];
  const rows = (data?.events ?? []).filter((e) => !name || e.name === name).slice(0, 25);

  return (
    <Panel
      title="Program events"
      action={
        names.length > 1 && (
          <select className="input h-8 w-auto text-xs" aria-label="Event" value={name} onChange={(e) => setName(e.target.value)}>
            <option value="">All events</option>
            {names.map((n) => <option key={n}>{n}</option>)}
          </select>
        )
      }
    >
      {loading && !data ? (
        <div className="p-4"><Skeleton className="h-16" /></div>
      ) : !data?.idl_loaded ? (
        <Empty title="No IDL for this program">Events are decoded with the program's Anchor IDL, and none was found on chain.</Empty>
      ) : !data.declares_events ? (
        <Empty title="This program declares no events">Its IDL has no events, so there is nothing to decode.</Empty>
      ) : rows.length === 0 ? (
        <Empty title="No events yet">Events appear here as the program emits them. Alert on one from the Alerts page.</Empty>
      ) : (
        <div className="overflow-x-auto">
          <table className="table">
            <thead>
              <tr><th>When</th><th>Event</th><th>Fields</th><th>Transaction</th></tr>
            </thead>
            <tbody>
              {rows.map((e, i) => (
                <tr key={`${e.signature}-${i}`}>
                  <td className="num text-xs text-ink-3 whitespace-nowrap">{ago(e.at)}</td>
                  <td className="font-medium whitespace-nowrap">{e.name}</td>
                  <td className="text-xs text-ink-2"><Fields fields={e.fields} /></td>
                  <td><Link className="link font-mono text-xs" to={`/tx/${e.signature}`}>{short(e.signature, 6)}</Link></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Panel>
  );
}
