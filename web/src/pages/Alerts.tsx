import { useState } from 'react';
import { Link } from 'react-router';
import { BellRing, CheckCircle2, Send, Trash2, XCircle } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import { useLive } from '../lib/live';
import { usePrograms } from '../lib/programs';
import type { AlertExecution, AlertRule, Condition, Metric, Severity } from '../lib/types';
import { ago, clock, short } from '../lib/format';
import { Empty, ErrorState, Panel, Skeleton, Spinner } from '../components/ui';

type Template = 'failure_rate' | 'failed_count' | 'tps' | 'avg_compute' | 'max_compute' | 'transfer' | 'incident';

const TEMPLATES: { id: Template; label: string; unit: string; defaultValue: number }[] = [
  { id: 'failure_rate', label: 'Failure rate is above', unit: '%', defaultValue: 5 },
  { id: 'failed_count', label: 'Failed transactions exceed', unit: 'tx', defaultValue: 3 },
  { id: 'tps', label: 'TPS is above', unit: 'TPS', defaultValue: 50 },
  { id: 'avg_compute', label: 'Average compute is above', unit: 'CU', defaultValue: 200_000 },
  { id: 'max_compute', label: 'Any transaction uses more than', unit: 'CU', defaultValue: 1_000_000 },
  { id: 'transfer', label: 'A single transfer is at least', unit: '', defaultValue: 100_000 },
  { id: 'incident', label: 'An incident opens with severity at least', unit: '', defaultValue: 0 },
];

const MINTS = [
  { value: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v', label: 'USDC' },
  { value: 'Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB', label: 'USDT' },
  { value: '', label: 'SOL' },
];

function describe(c: Condition) {
  switch (c.type) {
    case 'metric': {
      const names: Record<Metric, string> = {
        failure_rate: 'failure rate',
        failed_count: 'failed tx',
        tps: 'TPS',
        tx_count: 'tx count',
        avg_compute: 'avg CU',
        max_compute: 'max CU',
        unique_signers: 'unique signers',
      };
      return `${names[c.metric]} ${c.op} ${c.value.toLocaleString()}${c.metric === 'failure_rate' ? '%' : ''} over ${c.window_secs}s`;
    }
    case 'transfer':
      return `transfer ≥ ${c.min_amount.toLocaleString()} ${MINTS.find((m) => m.value === (c.mint ?? ''))?.label ?? short(c.mint)}`;
    case 'incident':
      return `incident opens (severity ≥ ${c.min_severity}${c.kinds.length ? `, ${c.kinds.join('/')}` : ''})`;
  }
}

function RuleForm({ onCreated }: { onCreated: () => void }) {
  const { programs } = usePrograms();
  const [name, setName] = useState('');
  const [program, setProgram] = useState('');
  const [template, setTemplate] = useState<Template>('failure_rate');
  const [value, setValue] = useState('5');
  const [windowSecs, setWindowSecs] = useState('60');
  const [mint, setMint] = useState(MINTS[0].value);
  const [minSeverity, setMinSeverity] = useState<Severity>('medium');
  const [severity, setSeverity] = useState<Severity>('high');
  const [createIncident, setCreateIncident] = useState(true);
  const [webhook, setWebhook] = useState('');
  const [cooldown, setCooldown] = useState('300');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const tpl = TEMPLATES.find((t) => t.id === template)!;

  const condition = (): Condition => {
    const v = Number(value);
    if (template === 'transfer') return { type: 'transfer', mint: mint || null, min_amount: v };
    if (template === 'incident') return { type: 'incident', kinds: [], min_severity: minSeverity };
    return { type: 'metric', metric: template as Metric, op: '>', value: v, window_secs: Number(windowSecs) || 60 };
  };

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    if (template !== 'incident' && !(Number(value) > 0)) {
      setError('Enter a threshold greater than zero.');
      return;
    }
    setBusy(true);
    try {
      await send('POST', '/api/rules', {
        name: name.trim() || `${tpl.label} ${template === 'incident' ? minSeverity : value}${tpl.unit}`,
        program_id: program || null,
        condition: condition(),
        create_incident: template === 'incident' ? false : createIncident,
        severity,
        webhook_url: webhook.trim() || null,
        cooldown_secs: Number(cooldown) || 0,
      });
      setName('');
      onCreated();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={submit} className="p-4 grid gap-4">
      <div className="grid gap-3 md:grid-cols-2">
        <div>
          <label className="label" htmlFor="rule-name">Name</label>
          <input id="rule-name" className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="Page on failure spikes" autoComplete="off" />
        </div>
        <div>
          <label className="label" htmlFor="rule-program">Program</label>
          <select id="rule-program" className="input" value={program} onChange={(e) => setProgram(e.target.value)}>
            <option value="">All monitored programs</option>
            {programs.map((p) => <option key={p.program_id} value={p.program_id}>{p.label}</option>)}
          </select>
        </div>
      </div>

      <fieldset className="grid gap-3 md:grid-cols-[2fr_1fr_1fr] md:items-end">
        <legend className="label">When</legend>
        <div>
          <label className="sr-only" htmlFor="rule-template">Condition</label>
          <select
            id="rule-template"
            className="input"
            value={template}
            onChange={(e) => {
              const t = TEMPLATES.find((x) => x.id === e.target.value)!;
              setTemplate(t.id);
              setValue(String(t.defaultValue));
            }}
          >
            {TEMPLATES.map((t) => <option key={t.id} value={t.id}>{t.label}</option>)}
          </select>
        </div>
        {template === 'incident' ? (
          <div className="md:col-span-2">
            <label className="sr-only" htmlFor="rule-min-sev">Minimum severity</label>
            <select id="rule-min-sev" className="input" value={minSeverity} onChange={(e) => setMinSeverity(e.target.value as Severity)}>
              {(['low', 'medium', 'high', 'critical'] as const).map((s) => <option key={s} value={s}>{s}</option>)}
            </select>
          </div>
        ) : (
          <>
            <div className="flex items-center gap-2">
              <label className="sr-only" htmlFor="rule-value">Threshold</label>
              <input id="rule-value" className="input num" type="number" inputMode="decimal" min="0" step="any" value={value} onChange={(e) => setValue(e.target.value)} />
              {tpl.unit && <span className="text-ink-3 text-xs">{tpl.unit}</span>}
            </div>
            {template === 'transfer' ? (
              <div>
                <label className="sr-only" htmlFor="rule-mint">Asset</label>
                <select id="rule-mint" className="input" value={mint} onChange={(e) => setMint(e.target.value)}>
                  {MINTS.map((m) => <option key={m.label} value={m.value}>{m.label}</option>)}
                </select>
              </div>
            ) : (
              <div className="flex items-center gap-2">
                <span className="text-ink-3 text-xs whitespace-nowrap">over</span>
                <label className="sr-only" htmlFor="rule-window">Window seconds</label>
                <input id="rule-window" className="input num" type="number" min="5" value={windowSecs} onChange={(e) => setWindowSecs(e.target.value)} />
                <span className="text-ink-3 text-xs">s</span>
              </div>
            )}
          </>
        )}
      </fieldset>

      <fieldset className="grid gap-3 md:grid-cols-[2fr_1fr_1fr] md:items-end">
        <legend className="label">Then</legend>
        <div>
          <label className="label" htmlFor="rule-webhook">Webhook URL (Discord, Slack or any HTTPS endpoint)</label>
          <input id="rule-webhook" className="input font-mono" type="url" inputMode="url" value={webhook} onChange={(e) => setWebhook(e.target.value)} placeholder="https://discord.com/api/webhooks/…" autoComplete="off" />
        </div>
        <div>
          <label className="label" htmlFor="rule-severity">Severity</label>
          <select id="rule-severity" className="input" value={severity} onChange={(e) => setSeverity(e.target.value as Severity)}>
            {(['low', 'medium', 'high', 'critical'] as const).map((s) => <option key={s} value={s}>{s}</option>)}
          </select>
        </div>
        <div>
          <label className="label" htmlFor="rule-cooldown">Cooldown (s)</label>
          <input id="rule-cooldown" className="input num" type="number" min="0" value={cooldown} onChange={(e) => setCooldown(e.target.value)} />
        </div>
        {template !== 'incident' && (
          <label className="flex items-center gap-2 text-sm text-ink-2 md:col-span-3">
            <input type="checkbox" checked={createIncident} onChange={(e) => setCreateIncident(e.target.checked)} />
            Open an incident with the matching transactions
          </label>
        )}
      </fieldset>

      {error && <p className="text-sm text-crit" role="alert">{error}</p>}
      <div>
        <button className="btn-primary" disabled={busy}>
          {busy ? <Spinner /> : <BellRing className="size-4" aria-hidden />} Create rule
        </button>
      </div>
    </form>
  );
}

export function Alerts() {
  const rules = useFetch<AlertRule[]>('/api/rules');
  const executions = useFetch<AlertExecution[]>('/api/alerts');
  const { programs } = usePrograms();
  const [testing, setTesting] = useState<number | null>(null);
  const label = (id: string | null) => (id ? programs.find((p) => p.program_id === id)?.label ?? short(id) : 'All programs');

  useLive((e) => {
    if (e.type === 'alert') executions.setData((prev) => [e.execution, ...(prev ?? [])].slice(0, 100));
  });

  const toggle = async (r: AlertRule) => {
    await send('PATCH', `/api/rules/${r.id}`, { ...r, enabled: !r.enabled });
    rules.reload();
  };
  const remove = async (r: AlertRule) => {
    if (!confirm(`Delete rule "${r.name}"?`)) return;
    await send('DELETE', `/api/rules/${r.id}`);
    rules.reload();
  };
  const test = async (r: AlertRule) => {
    setTesting(r.id);
    try {
      await send('POST', `/api/rules/${r.id}/test`);
    } finally {
      setTimeout(() => setTesting(null), 800);
    }
  };

  return (
    <div className="space-y-5">
      <header>
        <h1 className="text-xl font-semibold">Alerts</h1>
        <p className="text-ink-2 mt-1">Rules run against the live stream every second and deliver to your webhook.</p>
      </header>

      <Panel title="New rule">
        <RuleForm onCreated={rules.reload} />
      </Panel>

      <Panel title="Rules">
        {rules.error ? (
          <div className="p-4"><ErrorState message={rules.error} onRetry={rules.reload} /></div>
        ) : rules.loading && !rules.data ? (
          <div className="p-4"><Skeleton className="h-16" /></div>
        ) : !rules.data?.length ? (
          <Empty title="No rules yet">Create one above. Built-in detectors run regardless.</Empty>
        ) : (
          <div className="overflow-x-auto">
            <table className="table">
              <thead>
                <tr><th>Rule</th><th>Program</th><th>Condition</th><th>Delivers to</th><th>Last fired</th><th><span className="sr-only">Actions</span></th></tr>
              </thead>
              <tbody>
                {rules.data.map((r) => (
                  <tr key={r.id} className={r.enabled ? '' : 'opacity-60'}>
                    <td className="font-medium">{r.name}</td>
                    <td className="text-ink-2">{label(r.program_id)}</td>
                    <td className="text-ink-2 text-xs">{describe(r.condition)}{r.create_incident && r.condition.type !== 'incident' ? ' → incident' : ''}</td>
                    <td className="font-mono text-xs text-ink-2 max-w-48 truncate">{r.webhook_url ? new URL(r.webhook_url).host : '—'}</td>
                    <td className="text-xs text-ink-3 num">{ago(r.last_fired_at)}</td>
                    <td className="whitespace-nowrap text-right">
                      <button className="btn h-8 text-xs mr-1" onClick={() => toggle(r)} aria-pressed={r.enabled}>
                        {r.enabled ? 'Disable' : 'Enable'}
                      </button>
                      {r.webhook_url && (
                        <button className="btn h-8 text-xs mr-1" onClick={() => test(r)} disabled={testing === r.id}>
                          {testing === r.id ? <Spinner /> : <Send className="size-3.5" aria-hidden />} Test
                        </button>
                      )}
                      <button className="btn h-8 w-8 px-0" onClick={() => remove(r)} aria-label={`Delete ${r.name}`}>
                        <Trash2 className="size-3.5" aria-hidden />
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Panel>

      <Panel title="Deliveries">
        {executions.loading && !executions.data ? (
          <div className="p-4"><Skeleton className="h-16" /></div>
        ) : !executions.data?.length ? (
          <Empty title="Nothing delivered yet">Each webhook call shows up here live, with its HTTP status and latency.</Empty>
        ) : (
          <div className="overflow-x-auto">
            <table className="table">
              <thead>
                <tr><th>Time</th><th>Rule</th><th>Message</th><th>Result</th><th className="text-right">Latency</th><th>Incident</th></tr>
              </thead>
              <tbody>
                {executions.data.map((x) => (
                  <tr key={x.id}>
                    <td className="num text-xs text-ink-2 whitespace-nowrap">{clock(x.fired_at)}</td>
                    <td className="font-medium whitespace-nowrap">{x.rule_name}</td>
                    <td className="text-ink-2 text-xs max-w-md truncate" title={x.message}>{x.message}</td>
                    <td className="whitespace-nowrap text-xs">
                      {x.delivered ? (
                        <span className="inline-flex items-center gap-1 text-good"><CheckCircle2 className="size-3.5" aria-hidden />{x.status_code ?? 'OK'}</span>
                      ) : (
                        <span className="inline-flex items-center gap-1 text-crit" title={x.error ?? undefined}><XCircle className="size-3.5" aria-hidden />{x.error ?? 'Failed'}</span>
                      )}
                      {x.attempts > 1 && <span className="text-ink-3"> · {x.attempts} tries</span>}
                    </td>
                    <td className="num text-xs text-right">{x.latency_ms != null ? `${x.latency_ms} ms` : '—'}</td>
                    <td>{x.incident_id ? <Link className="link text-xs" to={`/incidents/${x.incident_id}`}>#{x.incident_id}</Link> : '—'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Panel>
    </div>
  );
}
