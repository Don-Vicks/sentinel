import { Plus, X } from 'lucide-react';
import type { AlertRule, Channel, ChannelType, Severity } from '../lib/types';

export const CHANNELS: Record<ChannelType, { label: string; hint: string }> = {
  slack: { label: 'Slack', hint: 'Incoming webhook URL from your Slack app (hooks.slack.com/services/…).' },
  telegram: { label: 'Telegram', hint: 'Create a bot with @BotFather, add it to your chat, and use the chat id (negative for groups) or @channelname.' },
  pagerduty: { label: 'PagerDuty', hint: 'Integration Key of an Events API v2 integration on your service. Incidents trigger a page and resolve it.' },
  discord: { label: 'Discord', hint: 'Webhook URL from the channel settings → Integrations.' },
  webhook: { label: 'Webhook', hint: 'Any HTTPS endpoint. Receives the JSON payload with a lifecycle field (opened, updated, resolved).' },
};

const SEVERITIES: Severity[] = ['low', 'medium', 'high', 'critical'];

export interface ChannelDraft {
  type: ChannelType;
  url: string;
  bot_token: string;
  chat_id: string;
  routing_key: string;
  min_severity: Severity | '';
}

export const emptyDraft = (type: ChannelType): ChannelDraft => ({
  type,
  url: '',
  bot_token: '',
  chat_id: '',
  routing_key: '',
  min_severity: type === 'pagerduty' ? 'high' : '',
});

export function draftToChannel(d: ChannelDraft): Channel {
  const min = d.min_severity ? { min_severity: d.min_severity } : {};
  if (d.type === 'telegram') return { type: 'telegram', bot_token: d.bot_token.trim(), chat_id: d.chat_id.trim(), ...min };
  if (d.type === 'pagerduty') return { type: 'pagerduty', routing_key: d.routing_key.trim(), ...min };
  return { type: d.type, url: d.url.trim(), ...min };
}

/** A problem the user can fix before sending, or null. The server validates the rest. */
export function draftProblem(d: ChannelDraft): string | null {
  const name = CHANNELS[d.type].label;
  if (d.type === 'telegram') return d.bot_token.trim() && d.chat_id.trim() ? null : `${name}: enter the bot token and chat id.`;
  if (d.type === 'pagerduty') return d.routing_key.trim() ? null : `${name}: enter the integration key.`;
  return d.url.trim() ? null : `${name}: enter the URL.`;
}

export function ChannelEditor({
  value,
  onChange,
  exclude = [],
  emptyHint = 'No channel: the rule only opens incidents on the dashboard. Add one to be notified.',
}: {
  value: ChannelDraft[];
  onChange: (next: ChannelDraft[]) => void;
  /** Channel types that don't make sense here (PagerDuty for reports). */
  exclude?: ChannelType[];
  /** Shown when no channel has been added. */
  emptyHint?: string;
}) {
  const update = (i: number, patch: Partial<ChannelDraft>) => onChange(value.map((d, j) => (j === i ? { ...d, ...patch } : d)));
  const remaining = (Object.keys(CHANNELS) as ChannelType[]).filter((t) => !exclude.includes(t));

  return (
    <div className="grid gap-3">
      {value.length === 0 && (
        <p className="text-xs text-ink-3">
          {emptyHint}
        </p>
      )}
      {value.map((d, i) => (
        <div key={i} className="rounded-md border border-line p-3 grid gap-3 md:grid-cols-[1fr_9rem_auto] md:items-end">
          <div className="grid gap-3 md:col-span-1">
            <p className="text-sm font-medium">{CHANNELS[d.type].label}</p>
            {d.type === 'telegram' ? (
              <div className="grid gap-3 sm:grid-cols-2">
                <div>
                  <label className="label" htmlFor={`ch-${i}-token`}>Bot token</label>
                  <input id={`ch-${i}-token`} className="input font-mono" type="password" value={d.bot_token} onChange={(e) => update(i, { bot_token: e.target.value })} placeholder="123456:ABC-DEF…" autoComplete="off" />
                </div>
                <div>
                  <label className="label" htmlFor={`ch-${i}-chat`}>Chat id</label>
                  <input id={`ch-${i}-chat`} className="input font-mono" value={d.chat_id} onChange={(e) => update(i, { chat_id: e.target.value })} placeholder="-1001234567890" autoComplete="off" />
                </div>
              </div>
            ) : d.type === 'pagerduty' ? (
              <div>
                <label className="label" htmlFor={`ch-${i}-key`}>Integration key</label>
                <input id={`ch-${i}-key`} className="input font-mono" type="password" value={d.routing_key} onChange={(e) => update(i, { routing_key: e.target.value })} placeholder="32-character Events API v2 key" autoComplete="off" />
              </div>
            ) : (
              <div>
                <label className="label" htmlFor={`ch-${i}-url`}>URL</label>
                <input id={`ch-${i}-url`} className="input font-mono" type="url" inputMode="url" value={d.url} onChange={(e) => update(i, { url: e.target.value })} placeholder={d.type === 'slack' ? 'https://hooks.slack.com/services/…' : d.type === 'discord' ? 'https://discord.com/api/webhooks/…' : 'https://…'} autoComplete="off" />
              </div>
            )}
            <p className="text-xs text-ink-3">{CHANNELS[d.type].hint}</p>
          </div>
          <div>
            <label className="label" htmlFor={`ch-${i}-sev`}>Only if severity ≥</label>
            <select id={`ch-${i}-sev`} className="input" value={d.min_severity} onChange={(e) => update(i, { min_severity: e.target.value as Severity | '' })}>
              <option value="">any</option>
              {SEVERITIES.map((s) => <option key={s} value={s}>{s}</option>)}
            </select>
          </div>
          <button type="button" className="btn h-9 w-9 px-0" onClick={() => onChange(value.filter((_, j) => j !== i))} aria-label={`Remove ${CHANNELS[d.type].label} channel`}>
            <X className="size-4" aria-hidden />
          </button>
        </div>
      ))}
      {value.length < 5 && (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs text-ink-3 inline-flex items-center gap-1"><Plus className="size-3.5" aria-hidden />Add</span>
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
