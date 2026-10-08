import { useState } from 'react';
import { Link } from 'react-router';
import { Wand2 } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import { useAuth } from '../lib/auth';
import type { AlertRule, Severity } from '../lib/types';
import { CHANNELS, ChannelEditor, draftProblem, draftToChannel, type ChannelDraft } from './ChannelEditor';
import { Panel, SeverityBadge, Spinner } from './ui';

interface Suggestion {
  id: string;
  title: string;
  why: string;
  severity: Severity;
  matches: string[];
  needs_value: { path: string; label: string } | null;
}

interface Suggestions {
  idl_loaded: boolean;
  suggestions: Suggestion[];
}

interface Applied {
  created: AlertRule[];
  already_had: string[];
}

/** Rules worth having for this program, read from its IDL: pick some and a channel. */
export function SuggestionsCard({ programId }: { programId: string }) {
  const { account, watching } = useAuth();
  const { data } = useFetch<Suggestions>(`/api/programs/${programId}/suggestions`);
  const rules = useFetch<AlertRule[]>(account ? '/api/rules' : null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [values, setValues] = useState<Record<string, string>>({});
  const [from, setFrom] = useState('');
  const [channels, setChannels] = useState<ChannelDraft[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<Applied | null>(null);

  if (!data) return null;
  const mine = !!account && watching.includes(programId);
  const withChannels = (rules.data ?? []).filter((r) => r.channels?.length || r.webhook_url);
  const toggle = (id: string) =>
    setPicked((p) => {
      const n = new Set(p);
      if (!n.delete(id)) n.add(id);
      return n;
    });

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    if (picked.size === 0) return setError('Pick at least one rule.');
    const need = data.suggestions.find((s) => picked.has(s.id) && s.needs_value && !(Number(values[s.id]) > 0));
    if (need) return setError(`"${need.title}" needs a number: ${need.needs_value!.label}.`);
    const problem = from ? null : channels.map(draftProblem).find(Boolean);
    if (problem) return setError(problem);
    if (!from && channels.length === 0) return setError('Choose where alerts should go.');
    setBusy(true);
    try {
      // Keep big integers exact: send the digits as typed.
      const nums = Object.fromEntries([...picked].filter((id) => values[id]).map((id) => [id, JSON.parse(values[id].trim())]));
      setResult(
        await send<Applied>('POST', `/api/programs/${programId}/suggestions`, {
          ids: [...picked],
          values: nums,
          ...(from ? { channels_from_rule: Number(from) } : { channels: channels.map(draftToChannel) }),
        }),
      );
      setPicked(new Set());
      rules.reload();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Panel title="Suggested rules for this program">
      {!data.idl_loaded ? (
        <p className="p-4 text-sm text-ink-3">No Anchor IDL was found for this program, so there is nothing to read rules from. With one, Sentinel proposes alerts for authority changes, pausing, funds leaving and configuration changes.</p>
      ) : data.suggestions.length === 0 ? (
        <p className="p-4 text-sm text-ink-3">Nothing in this program&apos;s IDL looks like an authority change, pause control, withdrawal or setting.</p>
      ) : (
        <form onSubmit={submit} className="p-4 grid gap-3 text-sm">
          <p className="text-ink-2">Read from the program&apos;s IDL: the instructions and events that change who is in charge, stop it, move funds out or change its settings. Pick what you want to be told about.</p>
          <ul className="grid gap-2">
            {data.suggestions.map((s) => (
              <li key={s.id} className="rounded-md border border-line p-3">
                <label className="flex items-start gap-3">
                  <input type="checkbox" className="mt-1" checked={picked.has(s.id)} onChange={() => toggle(s.id)} disabled={!mine} />
                  <span className="min-w-0 flex-1">
                    <span className="flex flex-wrap items-center gap-2">
                      <span className="font-medium">{s.title}</span>
                      <SeverityBadge severity={s.severity} />
                    </span>
                    <span className="block text-xs text-ink-3 mt-0.5">{s.why}</span>
                  </span>
                </label>
                {s.needs_value && picked.has(s.id) && (
                  <div className="mt-2 ml-7 flex items-center gap-2">
                    <label className="text-xs text-ink-2" htmlFor={`sg-${s.id}`}>{s.needs_value.label}</label>
                    <input id={`sg-${s.id}`} className="input num w-48" inputMode="numeric" value={values[s.id] ?? ''} onChange={(e) => setValues({ ...values, [s.id]: e.target.value.replace(/[^\d.]/g, '') })} placeholder="smallest unit" />
                  </div>
                )}
              </li>
            ))}
          </ul>
          {!mine ? (
            <p className="text-xs text-ink-3">Watch this program (signed in) to create these.</p>
          ) : (
            <>
              {withChannels.length > 0 && (
                <div>
                  <label className="label" htmlFor="sg-from">Send them to</label>
                  <select id="sg-from" className="input" value={from} onChange={(e) => setFrom(e.target.value)}>
                    <option value="">A new channel…</option>
                    {withChannels.map((r) => (
                      <option key={r.id} value={r.id}>
                        Same as “{r.name}” ({(r.channels?.length ? r.channels.map((c) => CHANNELS[c.type].label) : ['webhook']).join(', ')})
                      </option>
                    ))}
                  </select>
                </div>
              )}
              {!from && <ChannelEditor value={channels} onChange={setChannels} emptyHint="Add the channel alerts should go to." />}
              {error && <p className="text-crit" role="alert">{error}</p>}
              <div>
                <button className="btn-primary" disabled={busy || picked.size === 0}>
                  {busy ? <Spinner /> : <Wand2 className="size-4" aria-hidden />} Create {picked.size || ''} rule{picked.size === 1 ? '' : 's'}
                </button>
              </div>
            </>
          )}
          {result && (
            <p className="rounded-md bg-good-soft px-3 py-2 text-good" role="status">
              {result.created.length ? `Created ${result.created.length} rule${result.created.length === 1 ? '' : 's'}.` : 'Nothing new to create.'}
              {result.already_had.length > 0 && ` ${result.already_had.length} already existed.`}{' '}
              <Link to="/alerts" className="underline underline-offset-2">See them on the Alerts page</Link>
            </p>
          )}
        </form>
      )}
    </Panel>
  );
}
