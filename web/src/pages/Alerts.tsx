import { useState } from 'react';
import { Link } from 'react-router';
import { BellRing, CheckCircle2, Send, Trash2, XCircle } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import { useLive } from '../lib/live';
import { usePrograms } from '../lib/programs';
import { useAuth } from '../lib/auth';
import { RequireAccount } from '../components/SignIn';
import type { AlertExecution, AlertRule, ArgFilter, Condition, FilterOp, InstructionSchema, Metric, Severity } from '../lib/types';
import { ago, clock, short } from '../lib/format';
import { Empty, ErrorState, PageHeader, Panel, Skeleton, Spinner } from '../components/ui';
import { AgentAccess } from '../components/AgentAccess';
import { CHANNELS, ChannelEditor, deliversTo, draftProblem, draftToChannel, type ChannelDraft } from '../components/ChannelEditor';

type Template = 'failure_rate' | 'failed_count' | 'tps' | 'avg_compute' | 'max_compute' | 'transfer' | 'transfer_usd' | 'incident' | 'instruction' | 'system';

const TEMPLATES: { id: Template; label: string; unit: string; defaultValue: number }[] = [
  { id: 'failure_rate', label: 'Failure rate is above', unit: '%', defaultValue: 5 },
  { id: 'failed_count', label: 'Failed transactions exceed', unit: 'tx', defaultValue: 3 },
  { id: 'tps', label: 'TPS is above', unit: 'TPS', defaultValue: 50 },
  { id: 'avg_compute', label: 'Average compute is above', unit: 'CU', defaultValue: 200_000 },
  { id: 'max_compute', label: 'Any transaction uses more than', unit: 'CU', defaultValue: 1_000_000 },
  { id: 'transfer_usd', label: 'A single transfer is worth at least (USD)', unit: 'USD', defaultValue: 100_000 },
  { id: 'transfer', label: 'A single transfer is at least', unit: '', defaultValue: 100_000 },
  { id: 'incident', label: 'An incident opens with severity at least', unit: '', defaultValue: 0 },
  { id: 'instruction', label: 'An instruction is called', unit: '', defaultValue: 0 },
  { id: 'system', label: 'Sentinel can\'t see the chain (feed stalled or RPC failing)', unit: '', defaultValue: 0 },
];

const OPS: { value: FilterOp; label: string }[] = [
  { value: 'gt', label: '>' },
  { value: 'gte', label: '≥' },
  { value: 'lt', label: '<' },
  { value: 'lte', label: '≤' },
  { value: 'eq', label: '=' },
  { value: 'ne', label: '≠' },
  { value: 'contains', label: 'contains' },
];

/** Names that usually mean someone is changing how the program works or taking money out. */
const ADMIN_NAMES = 'set_*|update_*|withdraw*|pause*|unpause*|*authority*|*admin*|upgrade*|close*';

interface FilterDraft {
  path: string;
  op: FilterOp;
  value: string;
}

/** A number stays a number so `amount > 1000000000` compares as one; anything else is text. */
function filterValue(raw: string): ArgFilter['value'] {
  const t = raw.trim();
  return t !== '' && Number.isFinite(Number(t)) ? Number(t) : t;
}

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
    case 'transfer_usd':
      return `transfer worth ≥ $${c.min_usd.toLocaleString()} (Blur price)`;
    case 'transfer':
      return `transfer ≥ ${c.min_amount.toLocaleString()} ${MINTS.find((m) => m.value === (c.mint ?? ''))?.label ?? short(c.mint)}`;
    case 'incident':
      return `incident opens (severity ≥ ${c.min_severity}${c.kinds.length ? `, ${c.kinds.join('/')}` : ''})`;
    case 'system':
      return `Sentinel itself is blind (${c.kinds.length ? c.kinds.join(' / ').replace(/_/g, ' ') : 'feed stalled or RPC failing'}), and when it recovers`;
    case 'instruction': {
      const symbols: Record<FilterOp, string> = { eq: '=', ne: '≠', gt: '>', gte: '≥', lt: '<', lte: '≤', contains: 'contains' };
      const conds = c.filters.map((f) => `${f.path.replace(/^args\./, '')} ${symbols[f.op]} ${f.value}`).join(c.match_mode === 'any' ? ' or ' : ' and ');
      return `${c.name || 'any instruction'} called${conds ? ` where ${conds}` : ''}${c.first_seen_signer ? ', first time for the signer' : ''}`;
    }
  }
}

/** One-click starting points. Each fills the form; nothing is created until you press Create rule. */
const PRESETS: { label: string; hint: string; template: Template; value: string; window?: string; severity: Severity; name: string }[] = [
  { label: 'Failure rate above 50%', hint: 'Fires on busy programs within seconds', template: 'failure_rate', value: '50', window: '60', severity: 'high', name: 'Failure rate above 50%' },
  { label: 'Traffic above 300 TPS', hint: 'A surge in activity', template: 'tps', value: '300', window: '10', severity: 'medium', name: 'Traffic above 300 TPS' },
  { label: 'Transfer worth $10K+', hint: 'USD priced by Solami Blur', template: 'transfer_usd', value: '10000', severity: 'medium', name: 'Transfer worth $10K or more' },
  { label: 'Any incident', hint: 'Forward every detector incident', template: 'incident', value: '0', severity: 'high', name: 'Every incident' },
  { label: 'Sentinel feed problems', hint: 'Tells you when Sentinel cannot see the chain, and when it can again', template: 'system', value: '0', severity: 'high', name: 'Sentinel feed problems' },
  { label: 'Admin call from a new wallet', hint: 'set_*, update_*, withdraw*, pause, authority changes, from a wallet that has never called them', template: 'instruction', value: '0', severity: 'critical', name: 'Admin instruction from a new wallet' },
];

function RuleForm({ onCreated }: { onCreated: () => void }) {
  const { programs: all } = usePrograms();
  const { watching } = useAuth();
  const programs = all.filter((p) => watching.includes(p.program_id));
  const [name, setName] = useState('');
  const [program, setProgram] = useState('');
  const [template, setTemplate] = useState<Template>('failure_rate');
  const [value, setValue] = useState('5');
  const [windowSecs, setWindowSecs] = useState('60');
  const [mint, setMint] = useState(MINTS[0].value);
  const [minSeverity, setMinSeverity] = useState<Severity>('medium');
  const [severity, setSeverity] = useState<Severity>('high');
  const [createIncident, setCreateIncident] = useState(true);
  const [channels, setChannels] = useState<ChannelDraft[]>([]);
  const [ixName, setIxName] = useState('');
  const [ixFilters, setIxFilters] = useState<FilterDraft[]>([]);
  const [ixMode, setIxMode] = useState<'all' | 'any'>('all');
  const [ixSuccess, setIxSuccess] = useState(true);
  const [ixFirstSeen, setIxFirstSeen] = useState(false);
  const idl = useFetch<{ loaded: boolean; instructions: InstructionSchema[] }>(
    template === 'instruction' && program ? `/api/programs/${program}/idl` : null,
  );
  const [cooldown, setCooldown] = useState('300');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const tpl = TEMPLATES.find((t) => t.id === template)!;
  const applyPreset = (p: (typeof PRESETS)[number]) => {
    setName(p.name);
    setTemplate(p.template);
    setValue(p.value);
    if (p.window) setWindowSecs(p.window);
    setSeverity(p.severity);
    if (p.template === 'instruction') {
      setIxName(ADMIN_NAMES);
      setIxFirstSeen(true);
      setIxFilters([]);
    }
    setError(null);
  };

  const condition = (): Condition => {
    const v = Number(value);
    if (template === 'transfer') return { type: 'transfer', mint: mint || null, min_amount: v };
    if (template === 'transfer_usd') return { type: 'transfer_usd', min_usd: v };
    if (template === 'incident') return { type: 'incident', kinds: [], min_severity: minSeverity };
    if (template === 'system') return { type: 'system', kinds: [] };
    if (template === 'instruction') {
      return {
        type: 'instruction',
        name: ixName.trim(),
        filters: ixFilters.filter((f) => f.path.trim() && f.value.trim() !== '').map((f) => ({ path: f.path.trim(), op: f.op, value: filterValue(f.value) })),
        match_mode: ixMode,
        success_only: ixSuccess,
        first_seen_signer: ixFirstSeen,
      };
    }
    return { type: 'metric', metric: template as Metric, op: '>', value: v, window_secs: Number(windowSecs) || 60 };
  };

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    if (template === 'instruction' && !ixName.trim() && !ixFirstSeen && !ixFilters.some((f) => f.path.trim() && f.value.trim())) {
      setError('Name an instruction, add a condition, or choose first-time signers; otherwise this would fire on every call.');
      return;
    }
    if (template !== 'incident' && template !== 'instruction' && template !== 'system' && !(Number(value) > 0)) {
      setError('Enter a threshold greater than zero.');
      return;
    }
    const problem = channels.map(draftProblem).find(Boolean);
    if (problem) {
      setError(problem);
      return;
    }
    setBusy(true);
    try {
      await send('POST', '/api/rules', {
        name: name.trim() || (template === 'instruction' ? `${ixName.trim() || 'Instruction'} called` : `${tpl.label} ${template === 'incident' ? minSeverity : value}${tpl.unit}`),
        program_id: program || null,
        condition: condition(),
        create_incident: template === 'incident' || template === 'system' ? false : createIncident,
        severity,
        channels: channels.map(draftToChannel),
        cooldown_secs: Number(cooldown) || 0,
      });
      setName('');
      setChannels([]);
      onCreated();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={submit} className="p-4 grid gap-4">
      {programs.length === 0 && (
        <p className="rounded-md bg-warn-soft px-3 py-2 text-sm text-warn" role="status">
          You aren't watching any programs yet, so rules have nothing to fire on.{' '}
          <Link to="/" className="underline underline-offset-2">Watch a program</Link> first.
        </p>
      )}
      <div>
        <p className="label">Quick start</p>
        <div className="flex flex-wrap gap-2">
          {PRESETS.map((p) => (
            <button key={p.label} type="button" className="btn h-8 text-xs" title={p.hint} onClick={() => applyPreset(p)}>
              {p.label}
            </button>
          ))}
        </div>
      </div>
      <div className="grid gap-3 md:grid-cols-2">
        <div>
          <label className="label" htmlFor="rule-name">Name</label>
          <input id="rule-name" className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="Page on failure spikes" autoComplete="off" />
        </div>
        <div>
          <label className="label" htmlFor="rule-program">Program</label>
          <select id="rule-program" className="input" value={program} onChange={(e) => setProgram(e.target.value)}>
            <option value="">All programs I watch ({programs.length})</option>
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
        {template === 'system' ? (
          <p className="md:col-span-2 text-xs text-ink-3">
            Fires when the chain tip stops advancing on Sentinel's stream (detectors pause, so a quiet program isn't reported as down) or when RPC calls keep failing, and again when it recovers.
          </p>
        ) : template === 'instruction' ? (
          <div className="md:col-span-2 text-xs text-ink-3">
            {program
              ? idl.data?.loaded
                ? 'Names, arguments and accounts below come from this program\'s on-chain IDL.'
                : idl.loading
                  ? 'Reading the program\'s IDL…'
                  : 'No Anchor IDL found for this program. You can still match instruction names; arguments and accounts need the IDL.'
              : 'Pick a program to get suggestions from its IDL.'}
          </div>
        ) : template === 'incident' ? (
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
            {template === 'transfer_usd' ? (
              <p className="text-xs text-ink-3">Priced by Solami Blur. Tokens under $10K liquidity are ignored.</p>
            ) : template === 'transfer' ? (
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

      {template === 'instruction' && (
        <div className="grid gap-3 rounded-md border border-line p-3">
          <div>
            <label className="label" htmlFor="ix-name">Instruction name</label>
            <input
              id="ix-name"
              className="input font-mono"
              list="ix-names"
              value={ixName}
              onChange={(e) => setIxName(e.target.value)}
              placeholder="withdraw, or set_*|update_*|*authority*"
              autoComplete="off"
            />
            <datalist id="ix-names">
              {(idl.data?.instructions ?? []).map((i) => <option key={i.name} value={i.name} />)}
            </datalist>
            <p className="text-xs text-ink-3 mt-1">
              Separate several with <code>|</code>. <code>*</code> matches any text. Case and underscores are ignored, so <code>SetAuthority</code> matches <code>set_authority</code>. Leave empty to match every instruction.{' '}
              <button type="button" className="link" onClick={() => setIxName(ADMIN_NAMES)}>Use common admin names</button>
            </p>
          </div>
          <div className="grid gap-2">
            <div className="flex items-center justify-between gap-2">
              <p className="label !mb-0">Only when</p>
              {ixFilters.length > 1 && (
                <select className="input h-8 w-auto text-xs" value={ixMode} onChange={(e) => setIxMode(e.target.value as 'all' | 'any')} aria-label="Match mode">
                  <option value="all">all of these hold</option>
                  <option value="any">any of these holds</option>
                </select>
              )}
            </div>
            <datalist id="ix-paths">
              <option value="signer" />
              {(idl.data?.instructions ?? [])
                .filter((i) => !ixName.trim() || i.name.replace(/_/g, '').toLowerCase() === ixName.trim().replace(/_/g, '').toLowerCase() || ixName.includes('*') || ixName.includes('|'))
                .flatMap((i) => [...i.args.map((a) => a.path), ...i.accounts.map((a) => `accounts.${a}`)])
                .filter((v, k, all) => all.indexOf(v) === k)
                .map((p) => <option key={p} value={p} />)}
            </datalist>
            {ixFilters.map((f, i) => (
              <div key={i} className="grid gap-2 sm:grid-cols-[2fr_7rem_2fr_auto]">
                <input className="input font-mono" list="ix-paths" aria-label="Field" value={f.path} onChange={(e) => setIxFilters(ixFilters.map((x, j) => (j === i ? { ...x, path: e.target.value } : x)))} placeholder="args.amount" autoComplete="off" />
                <select className="input" aria-label="Comparison" value={f.op} onChange={(e) => setIxFilters(ixFilters.map((x, j) => (j === i ? { ...x, op: e.target.value as FilterOp } : x)))}>
                  {OPS.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
                </select>
                <input className="input font-mono" aria-label="Value" value={f.value} onChange={(e) => setIxFilters(ixFilters.map((x, j) => (j === i ? { ...x, value: e.target.value } : x)))} placeholder="1000000000 or an address" autoComplete="off" />
                <button type="button" className="btn h-9 w-9 px-0" onClick={() => setIxFilters(ixFilters.filter((_, j) => j !== i))} aria-label="Remove condition">×</button>
              </div>
            ))}
            <div>
              <button type="button" className="btn h-8 text-xs" onClick={() => setIxFilters([...ixFilters, { path: '', op: 'gt', value: '' }])}>+ Add a condition</button>
              <span className="text-xs text-ink-3 ml-2">on an argument (<code>args.amount</code>), an account (<code>accounts.authority</code>) or the <code>signer</code>. Amounts are in the token's smallest unit.</span>
            </div>
          </div>
          <label className="flex items-center gap-2 text-sm text-ink-2">
            <input type="checkbox" checked={ixFirstSeen} onChange={(e) => setIxFirstSeen(e.target.checked)} />
            Only when the signer has never called a matching instruction before (learned over the first minutes after Sentinel starts watching)
          </label>
          <label className="flex items-center gap-2 text-sm text-ink-2">
            <input type="checkbox" checked={ixSuccess} onChange={(e) => setIxSuccess(e.target.checked)} />
            Only successful calls
          </label>
        </div>
      )}

      <fieldset className="grid gap-3 md:grid-cols-[2fr_1fr_1fr] md:items-end">
        <legend className="label">Then</legend>
        <div className="md:col-span-1">
          <p className="label">Notify</p>
          <p className="text-xs text-ink-3">Slack, Telegram, PagerDuty, Discord or a webhook. Alerts follow the incident: a message when it opens, replies when it escalates and when it resolves.</p>
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
        <div className="md:col-span-3">
          <ChannelEditor value={channels} onChange={setChannels} />
        </div>
        {template !== "incident" && template !== "system" && (
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
  return (
    <div className="space-y-6">
      <PageHeader
        title="Alerts"
        meta="Rules run against the live stream every second and notify Slack, Telegram, PagerDuty, Discord or your webhook, following each incident from open to resolved."
      />
      <RequireAccount what="create alerts">
        <AlertsBody />
      </RequireAccount>
    </div>
  );
}

function AlertsBody() {
  const rules = useFetch<AlertRule[]>('/api/rules');
  const executions = useFetch<AlertExecution[]>('/api/alerts');
  const { programs } = usePrograms();
  const [testing, setTesting] = useState<number | null>(null);
  const label = (id: string | null) => (id ? programs.find((p) => p.program_id === id)?.label ?? short(id) : 'All programs');

  useLive((e) => {
    if (e.type === 'alert') {
      executions.setData((prev) => [e.execution, ...(prev ?? [])].slice(0, 100));
      // "Last fired" changed on the rule too.
      rules.reload();
    }
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
    <div className="space-y-6">

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
                    <td className="text-xs text-ink-2">
                      {deliversTo(r).length ? (
                        <span className="flex flex-wrap gap-x-2 gap-y-0.5">
                          {deliversTo(r).map((d, i) => (
                            <span key={i} title={d.min ? `severity ≥ ${d.min}` : undefined}>{d.label}{d.min ? ` ≥ ${d.min}` : ''}</span>
                          ))}
                        </span>
                      ) : (
                        '—'
                      )}
                    </td>
                    <td className="text-xs text-ink-3 num">{ago(r.last_fired_at)}</td>
                    <td className="whitespace-nowrap text-right">
                      <button className="btn h-8 text-xs mr-1" onClick={() => toggle(r)} aria-pressed={r.enabled}>
                        {r.enabled ? 'Disable' : 'Enable'}
                      </button>
                      {deliversTo(r).length > 0 && (
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

      <AgentAccess />

      <Panel title="Deliveries">
        {executions.loading && !executions.data ? (
          <div className="p-4"><Skeleton className="h-16" /></div>
        ) : !executions.data?.length ? (
          <Empty title="Nothing delivered yet">Each delivery shows up here live, with its channel, HTTP status and latency.</Empty>
        ) : (
          <div className="overflow-x-auto">
            <table className="table">
              <thead>
                <tr><th>Time</th><th>Rule</th><th>Channel</th><th>Message</th><th>Result</th><th className="text-right">Latency</th><th>Incident</th></tr>
              </thead>
              <tbody>
                {executions.data.map((x) => (
                  <tr key={x.id}>
                    <td className="num text-xs text-ink-2 whitespace-nowrap">{clock(x.fired_at)}</td>
                    <td className="font-medium whitespace-nowrap">{x.rule_name}</td>
                    <td className="text-xs whitespace-nowrap text-ink-2">
                      {x.channel ? CHANNELS[x.channel].label : '—'}
                      {x.event && x.event !== 'opened' && <span className="text-ink-3"> · {x.event}</span>}
                    </td>
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
