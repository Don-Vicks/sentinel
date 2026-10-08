import { useState } from 'react';
import { Check, Send, Trash2 } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import type { ChannelTestResult, Destination } from '../lib/types';
import { CHANNELS, ChannelEditor, draftToChannel, type ChannelDraft } from './ChannelEditor';
import { Empty, Panel, Spinner } from './ui';

/** Saved destinations, with a way to add one from a rule form or the Destinations tab. */
export function useDestinations() {
  const list = useFetch<Destination[]>('/api/destinations');
  const save = async (name: string, d: ChannelDraft) => {
    await send('POST', '/api/destinations', { name, ...draftToChannel(d) });
    list.reload();
  };
  return { ...list, save };
}

/** Pick saved destinations for a rule: one tap each, nothing to retype. */
export function DestinationPicker({
  destinations,
  selected,
  onChange,
}: {
  destinations: Destination[];
  selected: number[];
  onChange: (ids: number[]) => void;
}) {
  if (destinations.length === 0) return null;
  const toggle = (id: number) => onChange(selected.includes(id) ? selected.filter((x) => x !== id) : [...selected, id]);
  return (
    <div>
      <p className="label">Your saved destinations</p>
      <div className="flex flex-wrap gap-2" role="group" aria-label="Saved destinations">
        {destinations.map((d) => {
          const on = selected.includes(d.id);
          return (
            <button
              key={d.id}
              type="button"
              aria-pressed={on}
              onClick={() => toggle(d.id)}
              className={`inline-flex h-9 items-center gap-2 rounded-md border px-3 text-sm ${on ? 'border-brand bg-brand/10 text-ink' : 'border-line-strong bg-sunken/40 text-ink-2 hover:text-ink'}`}
            >
              {on && <Check className="size-3.5" aria-hidden />}
              <span className="font-medium">{d.name}</span>
              <span className="text-xs text-ink-3">{CHANNELS[d.type].label}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

/** The Destinations tab: add, test and remove saved channels. */
export function DestinationsTab() {
  const dest = useDestinations();
  const [draft, setDraft] = useState<ChannelDraft[]>([]);
  const [testing, setTesting] = useState<number | null>(null);
  const [results, setResults] = useState<Record<number, ChannelTestResult | { delivered: false; error: string }>>({});

  const test = async (d: Destination) => {
    setTesting(d.id);
    try {
      setResults((r) => ({ ...r, [d.id]: { ...({} as ChannelTestResult) } }));
      const res = await send<ChannelTestResult>('POST', `/api/destinations/${d.id}/test`);
      setResults((r) => ({ ...r, [d.id]: res }));
    } catch (e) {
      setResults((r) => ({ ...r, [d.id]: { delivered: false, error: (e as Error).message } }));
    } finally {
      setTesting(null);
    }
  };
  const remove = async (d: Destination) => {
    if (!confirm(`Delete "${d.name}"? Rules that already use it keep working.`)) return;
    await send('DELETE', `/api/destinations/${d.id}`);
    dest.reload();
  };

  return (
    <div className="space-y-6">
      <Panel title="Saved destinations">
        {!dest.data?.length ? (
          <Empty title="No saved destinations yet">Add Slack, Telegram, PagerDuty or a webhook once below, test it, and pick it in any rule.</Empty>
        ) : (
          <ul className="divide-y divide-line">
            {dest.data.map((d) => {
              const r = results[d.id];
              return (
                <li key={d.id} className="flex flex-wrap items-center gap-3 px-4 py-3">
                  <div className="min-w-48 flex-1">
                    <p className="text-sm font-medium">{d.name}</p>
                    <p className="text-xs text-ink-3">
                      {CHANNELS[d.type].label}
                      {'channel' in d ? ` · ${d.channel}` : ''}
                      {'chat_id' in d ? ` · ${d.chat_id}` : ''}
                    </p>
                  </div>
                  {r && 'delivered' in r && (r.error || r.delivered) && (
                    <span role="status" className={`text-xs ${r.delivered ? 'text-good' : 'text-crit'}`}>
                      {r.delivered ? 'Delivered. Check the channel.' : r.error}
                    </span>
                  )}
                  <button className="btn h-8 text-xs" onClick={() => test(d)} disabled={testing === d.id}>
                    {testing === d.id ? <Spinner /> : <Send className="size-3.5" aria-hidden />} Send test
                  </button>
                  <button className="btn h-8 w-8 px-0" onClick={() => remove(d)} aria-label={`Delete ${d.name}`}>
                    <Trash2 className="size-3.5" aria-hidden />
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </Panel>
      <Panel title="Add a destination">
        <div className="p-4">
          <ChannelEditor
            value={draft}
            onChange={setDraft}
            emptyHint="Choose where alerts should go. Slack app is the best for Slack: it keeps an incident's updates in one thread."
            onSave={async (name, d) => {
              await dest.save(name, d);
              setDraft((prev) => prev.filter((x) => x !== d));
            }}
          />
        </div>
      </Panel>
    </div>
  );
}
