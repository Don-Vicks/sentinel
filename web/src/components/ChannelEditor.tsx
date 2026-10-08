import { useState } from 'react';
import { CheckCircle2, Plus, Save, Send, X, XCircle } from 'lucide-react';
import { send } from '../lib/api';
import type { AlertRule, Channel, ChannelTestResult, ChannelType, Severity } from '../lib/types';
import { Spinner } from './ui';

export const CHANNELS: Record<ChannelType, { label: string; hint: string }> = {
  slack_bot: {
    label: 'Slack app',
    hint: 'Bot User OAuth Token (xoxb-…) of a Slack app with the chat:write scope. Invite it to the channel with /invite @YourApp. An incident’s updates stay in one thread.',
  },
  slack: { label: 'Slack webhook', hint: 'Incoming webhook URL from your Slack app (hooks.slack.com/services/…). Simpler, but every update is a separate message.' },
  telegram: { label: 'Telegram', hint: 'Create a bot with @BotFather, add it to your chat, and use the chat id (negative for groups) or @channelname.' },
  pagerduty: { label: 'PagerDuty', hint: 'Integration Key of an Events API v2 integration on your service. Incidents trigger a page and resolve it.' },
  discord: { label: 'Discord', hint: 'Webhook URL from the channel settings → Integrations.' },
  webhook: { label: 'Webhook', hint: 'Any HTTPS endpoint. Receives the JSON payload with a lifecycle field (opened, updated, resolved).' },
};

/** Order the "add" buttons appear in. */
const ORDER: ChannelType[] = ['slack_bot', 'telegram', 'pagerduty', 'discord', 'slack', 'webhook'];

const SEVERITIES: Severity[] = ['info', 'low', 'medium', 'high', 'critical'];

export interface ChannelDraft {
  type: ChannelType;
  url: string;
  bot_token: string;
  chat_id: string;
  routing_key: string;
  channel: string;
  min_severity: Severity | '';
}

export const emptyDraft = (type: ChannelType): ChannelDraft => ({
  type,
  url: '',
  bot_token: '',
  chat_id: '',
  routing_key: '',
  channel: '',
  min_severity: type === 'pagerduty' ? 'high' : '',
});

export function draftToChannel(d: ChannelDraft): Channel {
  const min = d.min_severity ? { min_severity: d.min_severity } : {};
  if (d.type === 'telegram') return { type: 'telegram', bot_token: d.bot_token.trim(), chat_id: d.chat_id.trim(), ...min };
  if (d.type === 'pagerduty') return { type: 'pagerduty', routing_key: d.routing_key.trim(), ...min };
  if (d.type === 'slack_bot') return { type: 'slack_bot', bot_token: d.bot_token.trim(), channel: d.channel.trim(), ...min };
  return { type: d.type, url: d.url.trim(), ...min };
}

/** A problem the user can fix before sending, or null. The server validates the rest. */
export function draftProblem(d: ChannelDraft): string | null {
  const name = CHANNELS[d.type].label;
  if (d.type === 'telegram') return d.bot_token.trim() && d.chat_id.trim() ? null : `${name}: enter the bot token and chat id.`;
  if (d.type === 'pagerduty') return d.routing_key.trim() ? null : `${name}: enter the integration key.`;
  if (d.type === 'slack_bot') return d.bot_token.trim() && d.channel.trim() ? null : `${name}: enter the bot token and the channel.`;
  return d.url.trim() ? null : `${name}: enter the URL.`;
}

/** The fields for one channel, without the severity filter or the buttons around it. */
function Fields({ d, id, update }: { d: ChannelDraft; id: string; update: (patch: Partial<ChannelDraft>) => void }) {
  if (d.type === 'telegram') {
    return (
      <div className="grid gap-3 sm:grid-cols-2">
        <div>
          <label className="label" htmlFor={`${id}-token`}>Bot token</label>
          <input id={`${id}-token`} className="input font-mono" type="password" value={d.bot_token} onChange={(e) => update({ bot_token: e.target.value })} placeholder="123456:ABC-DEF…" autoComplete="off" />
        </div>
        <div>
          <label className="label" htmlFor={`${id}-chat`}>Chat id</label>
          <input id={`${id}-chat`} className="input font-mono" value={d.chat_id} onChange={(e) => update({ chat_id: e.target.value })} placeholder="-1001234567890" autoComplete="off" />
        </div>
      </div>
    );
  }
  if (d.type === 'slack_bot') {
    return (
      <div className="grid gap-3 sm:grid-cols-2">
        <div>
          <label className="label" htmlFor={`${id}-token`}>Bot token</label>
          <input id={`${id}-token`} className="input font-mono" type="password" value={d.bot_token} onChange={(e) => update({ bot_token: e.target.value })} placeholder="xoxb-…" autoComplete="off" />
        </div>
        <div>
          <label className="label" htmlFor={`${id}-channel`}>Channel</label>
          <input id={`${id}-channel`} className="input font-mono" value={d.channel} onChange={(e) => update({ channel: e.target.value })} placeholder="#alerts or C0123456789" autoComplete="off" />
        </div>
      </div>
    );
  }
  if (d.type === 'pagerduty') {
    return (
      <div>
        <label className="label" htmlFor={`${id}-key`}>Integration key</label>
        <input id={`${id}-key`} className="input font-mono" type="password" value={d.routing_key} onChange={(e) => update({ routing_key: e.target.value })} placeholder="32-character Events API v2 key" autoComplete="off" />
      </div>
    );
  }
  return (
    <div>
      <label className="label" htmlFor={`${id}-url`}>URL</label>
      <input
        id={`${id}-url`}
        className="input font-mono"
        type="url"
        inputMode="url"
        value={d.url}
        onChange={(e) => update({ url: e.target.value })}
        placeholder={d.type === 'slack' ? 'https://hooks.slack.com/services/…' : d.type === 'discord' ? 'https://discord.com/api/webhooks/…' : 'https://…'}
        autoComplete="off"
      />
    </div>
  );
}

/** Sends one real test message and says what happened, before the channel is relied on. */
export function TestButton({ draft }: { draft: ChannelDraft }) {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ChannelTestResult | { error: string; delivered: false } | null>(null);
  const run = async () => {
    const problem = draftProblem(draft);
    if (problem) return setResult({ delivered: false, error: problem });
    setBusy(true);
    setResult(null);
    try {
      setResult(await send<ChannelTestResult>('POST', '/api/channels/test', draftToChannel(draft)));
    } catch (e) {
      setResult({ delivered: false, error: (e as Error).message });
    } finally {
      setBusy(false);
    }
  };
  return (
    <span className="inline-flex flex-wrap items-center gap-2">
      <button type="button" className="btn h-8 text-xs" onClick={run} disabled={busy}>
        {busy ? <Spinner /> : <Send className="size-3.5" aria-hidden />} Send test
      </button>
      {result && (
        <span role="status" className={`inline-flex items-center gap-1 text-xs ${result.delivered ? 'text-good' : 'text-crit'}`}>
          {result.delivered ? <CheckCircle2 className="size-3.5" aria-hidden /> : <XCircle className="size-3.5" aria-hidden />}
          {result.delivered ? 'Delivered. Check the channel.' : (result.error ?? 'Not delivered')}
        </span>
      )}
    </span>
  );
}

function ChannelRow({
  d,
  i,
  update,
  remove,
  onSave,
}: {
  d: ChannelDraft;
  i: number;
  update: (patch: Partial<ChannelDraft>) => void;
  remove: () => void;
  onSave?: (name: string, d: ChannelDraft) => Promise<void>;
}) {
  const [naming, setNaming] = useState(false);
  const [name, setName] = useState('');
  const [saved, setSaved] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const label = CHANNELS[d.type].label;

  const save = async () => {
    setSaveError(null);
    const problem = draftProblem(d);
    if (problem) return setSaveError(problem);
    try {
      await onSave!(name.trim() || label, d);
      setSaved(true);
      setNaming(false);
    } catch (e) {
      setSaveError((e as Error).message);
    }
  };

  return (
    <div className="rounded-lg border border-line bg-sunken/30 p-4 grid gap-3">
      <div className="flex items-center justify-between gap-2">
        <p className="text-sm font-medium">{label}</p>
        <button type="button" className="btn h-8 w-8 px-0" onClick={remove} aria-label={`Remove ${label} channel`}>
          <X className="size-4" aria-hidden />
        </button>
      </div>
      <Fields d={d} id={`ch-${i}`} update={update} />
      <p className="text-xs text-ink-3">{CHANNELS[d.type].hint}</p>
      <div className="flex flex-wrap items-end gap-x-4 gap-y-3">
        <div className="w-40">
          <label className="label" htmlFor={`ch-${i}-sev`}>Only if severity ≥</label>
          <select id={`ch-${i}-sev`} className="input" value={d.min_severity} onChange={(e) => update({ min_severity: e.target.value as Severity | '' })}>
            <option value="">any</option>
            {SEVERITIES.map((s) => <option key={s} value={s}>{s}</option>)}
          </select>
        </div>
        <TestButton draft={d} />
        {onSave && !saved && !naming && (
          <button type="button" className="btn h-8 text-xs" onClick={() => setNaming(true)}>
            <Save className="size-3.5" aria-hidden /> Save to reuse
          </button>
        )}
        {saved && <span className="text-xs text-good inline-flex items-center gap-1"><CheckCircle2 className="size-3.5" aria-hidden />Saved. Pick it from the list above next time.</span>}
      </div>
      {naming && (
        <div className="flex flex-wrap items-end gap-2">
          <div className="w-64">
            <label className="label" htmlFor={`ch-${i}-name`}>Name</label>
            <input id={`ch-${i}-name`} className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={`e.g. Ops ${label}`} autoComplete="off" />
          </div>
          <button type="button" className="btn-primary h-8 text-xs" onClick={save}>Save</button>
          <button type="button" className="btn h-8 text-xs" onClick={() => setNaming(false)}>Cancel</button>
        </div>
      )}
      {saveError && <p className="text-xs text-crit" role="alert">{saveError}</p>}
    </div>
  );
}

export function ChannelEditor({
  value,
  onChange,
  exclude = [],
  emptyHint = 'No channel: the rule only opens incidents on the dashboard. Add one to be notified.',
  onSave,
}: {
  value: ChannelDraft[];
  onChange: (next: ChannelDraft[]) => void;
  /** Channel types that don't make sense here (PagerDuty for reports). */
  exclude?: ChannelType[];
  /** Shown when no channel has been added. */
  emptyHint?: string;
  /** When given, each channel can be saved under a name and reused. */
  onSave?: (name: string, d: ChannelDraft) => Promise<void>;
}) {
  const update = (i: number, patch: Partial<ChannelDraft>) => onChange(value.map((d, j) => (j === i ? { ...d, ...patch } : d)));
  const remaining = ORDER.filter((t) => !exclude.includes(t));

  return (
    <div className="grid gap-3">
      {value.length === 0 && <p className="text-xs text-ink-3">{emptyHint}</p>}
      {value.map((d, i) => (
        <ChannelRow key={i} d={d} i={i} update={(patch) => update(i, patch)} remove={() => onChange(value.filter((_, j) => j !== i))} onSave={onSave} />
      ))}
      {value.length < 5 && (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs text-ink-3 inline-flex items-center gap-1"><Plus className="size-3.5" aria-hidden />New destination</span>
          {remaining.map((t) => (
            <button key={t} type="button" className="btn h-8 text-xs" onClick={() => onChange([...value, emptyDraft(t)])}>
              {CHANNELS[t].label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** Where a rule delivers, from its channels or its legacy single URL. */
export function deliversTo(rule: AlertRule): { label: string; min?: Severity | null }[] {
  if (rule.channels?.length) return rule.channels.map((c) => ({ label: CHANNELS[c.type].label, min: c.min_severity }));
  if (!rule.webhook_url) return [];
  let host = 'Webhook';
  try {
    host = new URL(rule.webhook_url).host;
  } catch {
    /* keep the generic label */
  }
  return [{ label: host }];
}
