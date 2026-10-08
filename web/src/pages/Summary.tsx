import { useState } from 'react';
import { Link } from 'react-router';
import { CalendarClock, Pause, Play, Send, Trash2 } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import type { Summary, SummaryActivity, SummarySchedule } from '../lib/types';
import { ago, compact, duration, explorer, KIND_LABEL, num, pct, short, usd } from '../lib/format';
import { HourlyBars, ShareBar, Sparkline } from '../components/charts';
import { Empty, ErrorState, PageSkeleton, Panel, SeverityBadge, Segmented, Spinner, Stat } from '../components/ui';
import { RequireAccount } from '../components/SignIn';
import { CHANNELS, ChannelEditor, draftProblem, draftToChannel, type ChannelDraft } from '../components/ChannelEditor';

const PERIODS = [
  { value: '1h', label: '1 hour' },
  { value: '24h', label: '24 hours' },
  { value: '7d', label: '7 days' },
  { value: '30d', label: '30 days' },
] as const;
type Period = (typeof PERIODS)[number]['value'];

/** "+12% vs previous", or nothing when there is nothing to compare with. */
function delta(now: number, before: number) {
  if (before <= 0) return null;
  const p = ((now - before) * 100) / before;
  return { text: `${p >= 0 ? '+' : ''}${p.toFixed(0)}%`, tone: 'muted' as const };
}

const utc = (secs: number) => new Date(secs * 1000).toISOString().slice(11, 16) + ' UTC';

function Tiles({ s }: { s: Summary }) {
  const a: SummaryActivity = s.activity;
  const r = s.reliability;
  return (
    <div className="grid grid-cols-2 lg:grid-cols-3 xl:grid-cols-6 gap-3">
      <Stat label="Transactions" value={compact(a.tx)} badge={delta(a.tx, s.previous.tx)} sub={`${num(a.failed)} failed`} />
      <Stat
        label="Success rate"
        value={a.success_rate === null ? '—' : pct(a.success_rate, 2)}
        tone={a.success_rate !== null && a.success_rate < 90 ? 'crit' : undefined}
        sub={s.previous.success_rate === null ? undefined : `previous ${pct(s.previous.success_rate, 2)}`}
      />
      <Stat label="Unique wallets" value={`~${compact(a.unique_wallets)}`} badge={delta(a.unique_wallets, s.previous.unique_wallets)} sub="estimated (±3%)" />
      <Stat
        label="Value moved"
        value={s.value.usd_volume > 0 ? usd(s.value.usd_volume) ?? '—' : s.value.sol_volume > 0 ? `${compact(s.value.sol_volume)} SOL` : '—'}
        badge={delta(s.value.usd_volume, s.previous_usd_volume)}
        sub="priced tokens + SOL, Solami Blur"
      />
      <Stat label="Peak" value={a.peak_at ? `${compact(a.peak_tps)} TPS` : '—'} sub={a.peak_at ? utc(a.peak_at) : undefined} />
      <Stat
        label="Incidents"
        value={num(r.opened)}
        tone={r.open_now > 0 ? 'crit' : undefined}
        sub={
          r.opened === 0
            ? 'none'
            : [r.open_now ? `${r.open_now} open` : null, r.mttd_secs !== null ? `found in ${duration(r.mttd_secs * 1000)}` : null, r.mttr_secs !== null ? `fixed in ${duration(r.mttr_secs * 1000)}` : null]
                .filter(Boolean)
                .join(' · ')
        }
      />
    </div>
  );
}

function Schedules({ programId }: { programId: string }) {
  const all = useFetch<SummarySchedule[]>('/api/summary-schedules');
  const mine = (all.data ?? []).filter((x) => x.program_id === programId);
  const [period, setPeriod] = useState<'daily' | 'weekly'>('daily');
  const [hour, setHour] = useState('9');
  const [channels, setChannels] = useState<ChannelDraft[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sent, setSent] = useState<number | null>(null);

  const create = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    if (!channels.length) return setError('Add a channel to send the summary to.');
    const problem = channels.map(draftProblem).find(Boolean);
    if (problem) return setError(problem);
    setBusy(true);
    try {
      await send('POST', '/api/summary-schedules', { program_id: programId, period, hour_utc: Number(hour), channels: channels.map(draftToChannel) });
      setChannels([]);
      all.reload();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const toggle = async (x: SummarySchedule) => {
    await send('PATCH', `/api/summary-schedules/${x.id}`, { ...x, enabled: !x.enabled });
    all.reload();
  };
  const remove = async (x: SummarySchedule) => {
    if (!confirm('Delete this scheduled summary?')) return;
    await send('DELETE', `/api/summary-schedules/${x.id}`);
    all.reload();
  };
  const sendNow = async (x: SummarySchedule) => {
    setSent(x.id);
    try {
      await send('POST', `/api/summary-schedules/${x.id}/send`);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setTimeout(() => setSent(null), 1000);
    }
  };

  return (
    <Panel title="Send this summary on a schedule">
      {mine.length > 0 && (
        <ul className="divide-y divide-line">
          {mine.map((x) => (
            <li key={x.id} className={`flex flex-wrap items-center gap-x-3 gap-y-1 px-4 py-3 text-sm ${x.enabled ? '' : 'opacity-60'}`}>
              <span className="font-medium">{x.period === 'daily' ? 'Every day' : 'Every Monday'} at {String(x.hour_utc).padStart(2, '0')}:00 UTC</span>
              <span className="text-ink-2">→ {x.channels.map((c) => CHANNELS[c.type].label).join(', ')}</span>
              <span className="text-xs text-ink-3">{x.last_sent_at ? `last sent ${ago(x.last_sent_at)}` : 'not sent yet'}</span>
              <span className="ml-auto flex gap-1">
                <button className="btn h-8 text-xs" onClick={() => sendNow(x)} disabled={sent === x.id}>
                  {sent === x.id ? <Spinner /> : <Send className="size-3.5" aria-hidden />} Send now
                </button>
                <button className="btn h-8 w-8 px-0" onClick={() => toggle(x)} aria-label={x.enabled ? 'Pause' : 'Resume'}>
                  {x.enabled ? <Pause className="size-3.5" aria-hidden /> : <Play className="size-3.5" aria-hidden />}
                </button>
                <button className="btn h-8 w-8 px-0" onClick={() => remove(x)} aria-label="Delete">
                  <Trash2 className="size-3.5" aria-hidden />
                </button>
              </span>
            </li>
          ))}
        </ul>
      )}
      <form onSubmit={create} className="p-4 grid gap-3">
        <div className="grid gap-3 sm:grid-cols-[1fr_8rem]">
          <div>
            <label className="label" htmlFor="sched-period">Send</label>
            <select id="sched-period" className="input" value={period} onChange={(e) => setPeriod(e.target.value as 'daily' | 'weekly')}>
              <option value="daily">Every day: the last 24 hours</option>
              <option value="weekly">Every Monday: the last 7 days</option>
            </select>
          </div>
          <div>
            <label className="label" htmlFor="sched-hour">At (UTC)</label>
            <select id="sched-hour" className="input" value={hour} onChange={(e) => setHour(e.target.value)}>
              {Array.from({ length: 24 }, (_, h) => <option key={h} value={h}>{String(h).padStart(2, '0')}:00</option>)}
            </select>
          </div>
        </div>
        <ChannelEditor value={channels} onChange={setChannels} exclude={['pagerduty']} emptyHint="Add the channel the summary should be sent to." />
        {error && <p className="text-sm text-crit" role="alert">{error}</p>}
        <div>
          <button className="btn-primary" disabled={busy}>
            {busy ? <Spinner /> : <CalendarClock className="size-4" aria-hidden />} Schedule summary
          </button>
        </div>
      </form>
    </Panel>
  );
}

/** What the program did over a period, with the schedules that deliver it. Lives in the program's Summary tab. */
export function SummaryView({ id }: { id: string }) {
  const [period, setPeriod] = useState<Period>('24h');
  const { data, error, loading, reload } = useFetch<Summary>(`/api/programs/${id}/summary?period=${period}`);

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-sm text-ink-2">{data?.headline ?? 'What this program did'}</p>
        <Segmented label="Period" value={period} options={[...PERIODS]} onChange={setPeriod} />
      </div>
      {error ? (
        <ErrorState message={error} onRetry={reload} />
      ) : loading && !data ? (
        <PageSkeleton />
      ) : data ? (
        <>
          {data.activity.tx === 0 ? (
            <div className="panel"><Empty title="No transactions observed in this period">Sentinel only counts what it was watching. Try a shorter period.</Empty></div>
          ) : (
            <>
              <Tiles s={data} />
              <div>
                <Panel title="Transactions per hour">
                  <div className="p-3">
                    <HourlyBars points={data.hourly} />
                    <p className="text-xs text-ink-3 mt-1">
                      {data.busiest_hour ? `Busiest hour ${new Date(data.busiest_hour.hour * 1000).toISOString().slice(11, 16)} UTC with ${num(data.busiest_hour.tx)} transactions. ` : ''}
                      Red is failed. Sentinel saw traffic in {Math.round(data.coverage * 100)}% of the hours in this period.
                    </p>
                    {data.hourly.filter((h) => h.health_avg !== null).length >= 3 && (
                      <div className="mt-3">
                        <Sparkline values={data.hourly.map((h) => h.health_avg)} height={36} />
                        <p className="text-xs text-ink-3 mt-1">
                          Health score by hour, lowest {Math.min(...data.hourly.map((h) => h.health_min ?? 100))}/100. It is sampled about once a minute.
                        </p>
                      </div>
                    )}
                  </div>
                </Panel>
              </div>
              <div className="grid gap-4 xl:grid-cols-2">
                <Panel title="Top instructions">
                  <table className="table">
                    <thead><tr><th>Instruction</th><th className="w-1/3">Share</th><th className="text-right">Transactions</th><th className="text-right">Failing</th></tr></thead>
                    <tbody>
                      {data.top_instructions.map((i) => (
                        <tr key={i.name}>
                          <td className="font-medium">{i.name}</td>
                          <td><div className="flex items-center gap-2"><ShareBar share={i.share} tone="accent" /><span className="num text-xs w-10 text-right">{(i.share * 100).toFixed(0)}%</span></div></td>
                          <td className="num text-right">{num(i.tx)}</td>
                          <td className={`num text-right ${i.failure_rate >= 10 ? 'text-crit' : ''}`}>{pct(i.failure_rate)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </Panel>
                <Panel title="Top errors">
                  {data.top_errors.length === 0 ? (
                    <Empty title="No failed transactions" />
                  ) : (
                    <table className="table">
                      <thead><tr><th>Error</th><th className="w-1/4">Share of failures</th><th className="text-right">Count</th></tr></thead>
                      <tbody>
                        {data.top_errors.map((e) => (
                          <tr key={e.label}>
                            <td className="font-medium">{e.label}</td>
                            <td><div className="flex items-center gap-2"><ShareBar share={e.share} /><span className="num text-xs w-10 text-right">{(e.share * 100).toFixed(0)}%</span></div></td>
                            <td className="num text-right">{num(e.count)}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  )}
                </Panel>
              </div>
              {data.value.largest.length > 0 && (
                <Panel title="Largest transfers">
                  <table className="table">
                    <thead><tr><th>Amount</th><th>Value</th><th>When</th><th>Transaction</th></tr></thead>
                    <tbody>
                      {data.value.largest.map((m) => (
                        <tr key={m.signature}>
                          <td className="num">{compact(m.amount)} {m.symbol}</td>
                          <td className="num">{m.usd !== null ? usd(m.usd) : '—'}</td>
                          <td className="text-xs text-ink-2">{utc(m.at)}</td>
                          <td><Link className="link font-mono text-xs" to={`/tx/${m.signature}`}>{short(m.signature, 6)}</Link>{' '}<a className="text-xs text-ink-3" href={explorer(m.signature)} target="_blank" rel="noreferrer">↗</a></td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </Panel>
              )}
            </>
          )}
          {(data.reliability.incidents.length > 0 || data.program_changes.length > 0) && (
            <Panel title="Incidents and program changes">
              <ul className="divide-y divide-line">
                {[...data.reliability.incidents, ...data.program_changes].map((i) => (
                  <li key={i.id} className="flex flex-wrap items-center gap-2 px-4 py-2.5 text-sm">
                    <SeverityBadge severity={i.severity} />
                    <Link className="link" to={`/incidents/${i.id}`}>{KIND_LABEL[i.kind]}</Link>
                    <span className="text-ink-2 min-w-0 truncate">{i.summary}</span>
                    <span className="ml-auto text-xs text-ink-3">{i.status === 'resolved' ? 'resolved' : 'open'}</span>
                  </li>
                ))}
              </ul>
            </Panel>
          )}
          {data.next_actions.length > 0 && (
            <Panel title="Worth doing">
              <ul className="list-disc pl-9 pr-4 py-3 text-sm space-y-1">
                {data.next_actions.map((a) => <li key={a}>{a}</li>)}
              </ul>
            </Panel>
          )}
        </>
      ) : null}
      <RequireAccount what="schedule this summary">
        <Schedules programId={id} />
      </RequireAccount>
    </div>
  );
}
