import { useEffect, useRef, useState } from 'react';
import { Link } from 'react-router';
import { send, useFetch } from '../lib/api';
import { useAuth } from '../lib/auth';
import type { ProgramEvent } from '../lib/types';
import { ago, short } from '../lib/format';
import { Empty, Panel, Skeleton, Spinner } from './ui';

interface Events {
  idl_loaded: boolean;
  custom_idl: boolean;
  declares_events: boolean;
  names: { name: string; count: number }[];
  events: ProgramEvent[];
}

/** Supply the program's IDL when it has none on chain, or replace the one that is. */
function IdlUpload({ programId, custom, onChange }: { programId: string; custom: boolean; onChange: () => void }) {
  const { account, watching } = useAuth();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const file = useRef<HTMLInputElement>(null);
  if (!account || !watching.includes(programId)) return null;

  const upload = async (f: File) => {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const idl = JSON.parse(await f.text());
      const r = await send<{ name: string | null; instructions: number; events: number }>('PUT', `/api/programs/${programId}/idl`, idl);
      setNote(`Using ${r.name ?? 'your IDL'}: ${r.instructions} instructions, ${r.events} events.`);
      onChange();
    } catch (e) {
      setError(e instanceof SyntaxError ? 'That file is not JSON.' : (e as Error).message);
    } finally {
      setBusy(false);
      if (file.current) file.current.value = '';
    }
  };
  const remove = async () => {
    setBusy(true);
    try {
      await send('DELETE', `/api/programs/${programId}/idl`);
      setNote('Back to the IDL on chain, if there is one.');
      onChange();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-wrap items-center gap-2 border-t border-line px-4 py-3 text-xs text-ink-2">
      <input ref={file} type="file" accept="application/json,.json" className="sr-only" id="idl-file" onChange={(e) => e.target.files?.[0] && upload(e.target.files[0])} />
      <label htmlFor="idl-file" className="btn h-8 cursor-pointer text-xs">
        {busy ? <Spinner /> : null} {custom ? 'Replace IDL' : 'Upload IDL'}
      </label>
      {custom && <button type="button" className="btn h-8 text-xs" onClick={remove} disabled={busy}>Use the IDL on chain</button>}
      <span className="text-ink-3">
        {custom ? 'Using an IDL you supplied.' : 'The JSON `anchor build` writes (target/idl/<name>.json), for programs that keep no IDL on chain.'}
      </span>
      {note && <span className="text-good" role="status">{note}</span>}
      {error && <span className="text-crit" role="alert">{error}</span>}
    </div>
  );
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
  const [name, setName] = useState('');
  const { data, loading, reload } = useFetch<Events>(`/api/programs/${programId}/events${name ? `?name=${encodeURIComponent(name)}` : ''}`);
  useEffect(() => {
    const t = setInterval(reload, 5_000);
    return () => clearInterval(t);
  }, [reload]);
  const names = data?.names ?? [];
  const rows = (data?.events ?? []).slice(0, 25);

  return (
    <Panel
      title="Program events"
      action={
        names.length > 1 && (
          <select className="input h-8 w-auto text-xs" aria-label="Event" value={name} onChange={(e) => setName(e.target.value)}>
            <option value="">All events</option>
            {names.map((n) => <option key={n.name} value={n.name}>{n.name} ({n.count})</option>)}
          </select>
        )
      }
    >
      {loading && !data ? (
        <div className="p-4"><Skeleton className="h-16" /></div>
      ) : !data?.idl_loaded ? (
        <Empty title="No IDL for this program">Events are decoded with the program's Anchor IDL, and none was found on chain. If you have it, sign in, watch the program and upload it here.</Empty>
      ) : !data.declares_events ? (
        <Empty title="This program declares no events">Its IDL has no events, so there is nothing to decode.</Empty>
      ) : rows.length === 0 ? (
        <Empty title="No events yet">Events appear here as the program emits them, and are kept for a week. Alert on one from the Alerts page.</Empty>
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
      <IdlUpload programId={programId} custom={!!data?.custom_idl} onChange={reload} />
    </Panel>
  );
}
