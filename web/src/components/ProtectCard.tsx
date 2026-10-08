import { useState } from 'react';
import { Link } from 'react-router';
import { ShieldCheck } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import { useAuth } from '../lib/auth';
import type { AlertRule } from '../lib/types';
import { CHANNELS, ChannelEditor, draftProblem, draftToChannel, type ChannelDraft } from './ChannelEditor';
import { Panel, Spinner } from './ui';

interface Result {
  created: AlertRule[];
  already_had: string[];
  next_steps: string[];
}

/** The alerts most teams want, created in one go on the channel you choose. */
export function ProtectCard({ programId }: { programId: string }) {
  const { account, watching } = useAuth();
  const rules = useFetch<AlertRule[]>(account ? '/api/rules' : null);
  const [from, setFrom] = useState('');
  const [channels, setChannels] = useState<ChannelDraft[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<Result | null>(null);
  if (!account || !watching.includes(programId)) return null;

  const withChannels = (rules.data ?? []).filter((r) => r.channels?.length || r.webhook_url);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    const problem = from ? null : channels.map(draftProblem).find(Boolean);
    if (problem) return setError(problem);
    if (!from && channels.length === 0) return setError('Choose where alerts should go.');
    setBusy(true);
    try {
      setResult(await send<Result>('POST', `/api/programs/${programId}/protect`, from ? { channels_from_rule: Number(from) } : { channels: channels.map(draftToChannel) }));
      rules.reload();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Panel title="Protect this program">
      <form onSubmit={submit} className="p-4 grid gap-3 text-sm">
        <p className="text-ink-2">
          One step sets up the alerts most teams want: any incident of high severity or above (upgrades, vault outflows, failure spikes), a failure rate above 20%, an admin instruction called by a wallet that never called one before, a health score below 60, and a notice if Sentinel itself can&apos;t see the chain. Rules you already have are left alone.
        </p>
        {withChannels.length > 0 && (
          <div>
            <label className="label" htmlFor="protect-from">Send them to</label>
            <select id="protect-from" className="input" value={from} onChange={(e) => setFrom(e.target.value)}>
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
          <button className="btn-primary" disabled={busy}>
            {busy ? <Spinner /> : <ShieldCheck className="size-4" aria-hidden />} Set up alerts
          </button>
        </div>
        {result && (
          <div className="rounded-md bg-good-soft px-3 py-2 text-good" role="status">
            <p>
              {result.created.length ? `Created ${result.created.length} rule${result.created.length === 1 ? '' : 's'}.` : 'Nothing new to create.'}
              {result.already_had.length > 0 && ` ${result.already_had.length} already existed.`}{' '}
              <Link to="/alerts" className="underline underline-offset-2">See them on the Alerts page</Link>
            </p>
            {result.next_steps.length > 0 && (
              <ul className="list-disc pl-5 mt-1 text-ink-2">
                {result.next_steps.map((n) => <li key={n}>{n}</li>)}
              </ul>
            )}
          </div>
        )}
      </form>
    </Panel>
  );
}
