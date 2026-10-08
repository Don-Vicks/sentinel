import { useState } from 'react';
import { Link, useParams, useSearchParams } from 'react-router';
import { CheckCircle2, ClipboardCopy, Rocket, Search } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import { useLive } from '../lib/live';
import { useAuth } from '../lib/auth';
import type { AuthorityEvidence, Incident as IncidentT, IncidentStatus, TxSummary } from '../lib/types';
import { clock, compact, duration, explorer, explorerAccount, KIND_LABEL, num, pct, short, usd } from '../lib/format';
import { ShareBar } from '../components/charts';
import { Address, Empty, ErrorState, PageHeader, PageSkeleton, Panel, SeverityBadge, ShowMore, Spinner, Stat, StatusBadge } from '../components/ui';
import { IncidentList, mergeIncident } from '../components/IncidentList';
import { TxTable } from '../components/TxTable';
import { IncidentTimeline } from '../components/IncidentTimeline';

function fmtMetric(metric: string | null, v: number | null) {
  if (v === null) return '—';
  if (metric === 'failure_rate') return pct(v);
  if (metric === 'tps') return `${v.toFixed(2)} TPS`;
  if (metric === 'avg_compute') return `${compact(v)} CU`;
  if (metric === 'vault_outflow_pct') return pct(v, 0);
  if (metric === 'error_count') return `${Number.isInteger(v) ? num(v) : num(v, 1)} / 60s`;
  if (metric === 'transfer_usd') return usd(v) ?? '—';
  return compact(v);
}

/** Copies the incident's markdown post-mortem, ready to paste into a doc or a channel. */
function ReportButton({ id }: { id: number }) {
  const [state, setState] = useState<'idle' | 'busy' | 'copied' | 'failed'>('idle');
  const copy = async () => {
    setState('busy');
    try {
      const res = await fetch(`/api/incidents/${id}/report`);
      if (!res.ok) throw new Error('report unavailable');
      await navigator.clipboard.writeText(await res.text());
      setState('copied');
    } catch {
      setState('failed');
    }
    setTimeout(() => setState('idle'), 2000);
  };
  return (
    <button className="btn" onClick={copy} disabled={state === 'busy'} title="Copy a markdown post-mortem">
      {state === 'busy' ? <Spinner /> : <ClipboardCopy className="size-4" aria-hidden />}
      {state === 'copied' ? 'Copied' : state === 'failed' ? 'Could not copy' : 'Post-mortem'}
    </button>
  );
}

const ACTION_LABEL: Record<AuthorityEvidence['action'], string> = {
  upgrade: 'Program code replaced',
  set_authority: 'Upgrade authority changed',
  close: 'Program closed',
  extend: 'Program data extended',
};

/** What the upgradeable loader was asked to do, and by whom. */
function AuthorityPanel({ ev }: { ev: AuthorityEvidence }) {
  const rows: [string, React.ReactNode][] = [
    ['Action', ACTION_LABEL[ev.action]],
    ['Signed by', ev.authority ? <Address value={ev.authority} n={6} /> : '—'],
  ];
  if (ev.action === 'upgrade') rows.push(['New code from buffer', ev.buffer ? <Address value={ev.buffer} n={6} /> : '—']);
  if (ev.action === 'set_authority')
    rows.push(['New authority', ev.new_authority ? <Address value={ev.new_authority} n={6} /> : 'none: the program is now immutable']);
  if (ev.action === 'close' && ev.recipient) rows.push(['Funds to', <Address value={ev.recipient} n={6} />]);
  if (ev.action === 'extend') rows.push(['Added', `${num(ev.additional_bytes ?? 0)} bytes`]);
  rows.push(['Executed via', ev.path.includes('.') ? 'another program (CPI), such as a multisig vote' : 'a direct instruction']);
  rows.push(['Transaction', <a className="link font-mono text-xs" href={explorer(ev.signature)} target="_blank" rel="noreferrer">{short(ev.signature, 6)}</a>]);
  rows.push(['Slot', num(ev.slot)]);
  return (
    <Panel title="What changed">
      <dl className="grid gap-x-6 gap-y-2 p-4 sm:grid-cols-[max-content_1fr] text-sm">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-ink-3">{k}</dt>
            <dd>{v}</dd>
          </div>
        ))}
      </dl>
    </Panel>
  );
}

export function Incident() {
  const { id } = useParams();
  const { data, setData, error, loading, reload } = useFetch<{
    incident: IncidentT;
    program_label: string | null;
    transactions: TxSummary[];
  }>(`/api/incidents/${id}`);
  const [txShown, setTxShown] = useState(25);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const { account, requestSignIn } = useAuth();

  useLive((e) => {
    if (e.type === 'incident' && String(e.incident.id) === id) {
      const grew = data && e.incident.affected_count !== data.incident.affected_count;
      setData((d) => (d ? { ...d, incident: e.incident } : d));
      if (grew) reload();
    }
  });

  if (error) return <ErrorState message={error} onRetry={reload} />;
  if (loading && !data) return <PageSkeleton />;
  if (!data) return null;
  const inc = data.incident;
  const fps = inc.evidence.fingerprints ?? [];
  const ongoing = inc.status !== 'resolved';
  const end = inc.resolved_at ? new Date(inc.resolved_at).getTime() : Date.now();

  const setStatus = async (status: IncidentStatus) => {
    if (!account) return requestSignIn();
    setBusy(true);
    setActionError(null);
    try {
      const updated = await send<IncidentT>('PATCH', `/api/incidents/${inc.id}`, { status });
      setData((d) => (d ? { ...d, incident: updated } : d));
    } catch (e) {
      setActionError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-6">
      <PageHeader
        crumbs={[
          { label: 'Incidents', to: '/incidents' },
          { label: data.program_label ?? short(inc.program_id), to: `/programs/${inc.program_id}` },
        ]}
        title={
          <span className="flex flex-wrap items-center gap-2">
            {KIND_LABEL[inc.kind]}
            <span className="num text-ink-3 font-normal">#{inc.id}</span>
          </span>
        }
        meta={
          <span className="flex flex-wrap items-center gap-2">
            <SeverityBadge severity={inc.severity} />
            <StatusBadge status={inc.status} />
            <span className="text-ink-2">{inc.summary}</span>
          </span>
        }
        actions={
          <>
            <ReportButton id={inc.id} />
            {ongoing && (
            <>
              {inc.status === 'open' && (
                <button className="btn" disabled={busy} onClick={() => setStatus('investigating')}>
                  <Search className="size-4" aria-hidden /> Investigating
                </button>
              )}
              <button className="btn-primary" disabled={busy} onClick={() => setStatus('resolved')}>
                {busy ? <Spinner /> : <CheckCircle2 className="size-4" aria-hidden />} Resolve
              </button>
            </>
            )}
          </>
        }
      />

      {actionError && (
        <p className="rounded-md bg-crit-soft px-3 py-2 text-sm text-crit" role="alert">
          {actionError}
        </p>
      )}

      <div className="panel p-4">
        <h2 className="panel-title mb-2">Why this fired</h2>
        <p className="text-ink leading-relaxed">{inc.explanation}</p>
        <p className="text-xs text-ink-3 mt-2">Source: {inc.source === 'detector' ? 'built-in detector' : inc.source.replace(':', ' #')}</p>
      </div>

      {inc.evidence.deploy && (
        <div className="rounded-md border border-warn/40 bg-warn-soft px-4 py-3 text-sm" role="note">
          <p className="font-medium text-warn flex items-center gap-2"><Rocket className="size-4" aria-hidden /> Started after a program upgrade</p>
          <p className="text-ink-2 mt-1">
            {inc.evidence.deploy.note}{' '}
            <Link className="link" to={`/incidents/${inc.evidence.deploy.incident_id}`}>See the upgrade</Link>
            {' · '}
            <a className="link" href={explorer(inc.evidence.deploy.signature)} target="_blank" rel="noreferrer">transaction</a>
          </p>
        </div>
      )}

      {inc.evidence.authority && <AuthorityPanel ev={inc.evidence.authority} />}

      {inc.evidence.vault && (
        <Panel title="Vault">
          <dl className="grid gap-x-6 gap-y-2 p-4 sm:grid-cols-[max-content_1fr] text-sm">
            <dt className="text-ink-3">Account</dt>
            <dd><a className="link" href={explorerAccount(inc.evidence.vault.account)} target="_blank" rel="noreferrer"><Address value={inc.evidence.vault.account} n={6} copy={false} /></a></dd>
            <dt className="text-ink-3">Net outflow</dt>
            <dd className="num">{compact(inc.evidence.vault.outflow)} {inc.evidence.vault.symbol}{inc.evidence.vault.usd !== null ? ` (${usd(inc.evidence.vault.usd)})` : ''}</dd>
            <dt className="text-ink-3">Share of balance</dt>
            <dd className="num">{pct(inc.evidence.vault.pct, 0)}</dd>
            <dt className="text-ink-3">Left in the vault</dt>
            <dd className="num">{compact(inc.evidence.vault.balance_after)} {inc.evidence.vault.symbol}</dd>
          </dl>
        </Panel>
      )}

      <div className={`grid grid-cols-2 md:grid-cols-3 xl:grid-cols-6 gap-3 ${inc.kind === 'authority_change' ? 'hidden' : ''}`}>
        <Stat label="Observed" value={fmtMetric(inc.metric, inc.observed)} tone={ongoing ? 'crit' : undefined} />
        <Stat label="Peak" value={fmtMetric(inc.metric, inc.peak)} />
        <Stat label="Normal baseline" value={fmtMetric(inc.metric, inc.baseline)} sub={inc.threshold !== null ? `threshold ${fmtMetric(inc.metric, inc.threshold)}` : undefined} />
        <Stat label="Affected transactions" value={num(inc.affected_count)} sub={`${num(inc.affected_wallets)} wallets`} />
        <Stat
          label="Detected"
          value={clock(inc.detected_at)}
          sub={inc.onset_at ? `onset ${clock(inc.onset_at)}` : undefined}
        />
        <Stat
          label={ongoing ? 'Ongoing for' : 'Lasted'}
          value={duration(end - new Date(inc.onset_at ?? inc.detected_at).getTime())}
          sub={inc.detection_latency_ms !== null ? `detected ${(inc.detection_latency_ms / 1000).toFixed(2)}s after last tx` : undefined}
        />
      </div>

      <IncidentTimeline incident={inc} />

      {fps.length > 0 && (
        <Panel title="Failure fingerprints">
          <table className="table table-stack">
            <thead>
              <tr>
                <th>Error</th>
                <th>Raised by</th>
                <th className="w-1/4">Share</th>
                <th className="text-right">Count</th>
                <th>Examples</th>
              </tr>
            </thead>
            <tbody>
              {fps.map((f) => (
                <tr key={f.key}>
                  <td data-primary className="font-medium">
                    {f.error}
                    {f.code !== null && <span className="ml-1.5 num text-xs text-ink-3">#{f.code}</span>}
                  </td>
                  <td data-label="Raised by" data-wide className="text-ink-2">
                    {f.program_name}
                    {f.instruction && <span className="text-ink-3">::{f.instruction}</span>}
                  </td>
                  <td data-label="Share" data-wide>
                    <div className="flex items-center gap-2">
                      <ShareBar share={f.share} />
                      <span className="num text-xs w-10 text-right">{(f.share * 100).toFixed(0)}%</span>
                    </div>
                  </td>
                  <td data-label="Count" className="num text-right">{num(f.count)}</td>
                  <td data-label="Examples" data-wide className="space-x-2 sm:whitespace-nowrap">
                    {f.samples.slice(0, 3).map((s) => (
                      <Link key={s} to={`/tx/${s}`} className="link font-mono text-xs">
                        {short(s, 4)}
                      </Link>
                    ))}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </Panel>
      )}

      <Panel title={`Affected transactions (${num(data.transactions.length)}${inc.affected_count > data.transactions.length ? ` of ${num(inc.affected_count)}` : ''})`}>
        {data.transactions.length ? (
          <TxTable rows={data.transactions.slice(0, txShown)} />
        ) : (
          <Empty title="No transactions linked yet" />
        )}
        <ShowMore shown={txShown} total={data.transactions.length} step={50} onMore={() => setTxShown((n) => n + 50)} />
      </Panel>
    </div>
  );
}

export function Incidents() {
  const [params] = useSearchParams();
  const program = params.get('program');
  const { data, setData, error, loading, reload } = useFetch<IncidentT[]>(
    `/api/incidents?limit=200${program ? `&program=${program}` : ''}`,
  );
  const [filter, setFilter] = useState<'active' | 'all'>('all');
  useLive((e) => {
    if (e.type === 'incident') setData((prev) => mergeIncident(prev ?? [], e.incident, program ?? undefined));
  });
  const list = (data ?? []).filter((i) => filter === 'all' || i.status !== 'resolved');

  return (
    <div className="space-y-6">
      <PageHeader
        title="Incidents"
        meta="Every detector and rule that fired, linked to the transactions behind it."
        actions={
        <div className="inline-flex rounded-md border border-line-strong p-0.5" role="group" aria-label="Filter">
          {(['all', 'active'] as const).map((f) => (
            <button
              key={f}
              className={`h-8 px-3 rounded text-sm ${filter === f ? 'bg-sunken font-medium' : 'text-ink-2'}`}
              aria-pressed={filter === f}
              onClick={() => setFilter(f)}
            >
              {f === 'all' ? 'All' : 'Active'}
            </button>
          ))}
        </div>
        }
      />
      {error ? (
        <ErrorState message={error} onRetry={reload} />
      ) : loading && !data ? (
        <PageSkeleton />
      ) : (
        <div className="panel">
          {list.length ? (
            <IncidentList incidents={list} />
          ) : (
            <Empty title={filter === 'active' ? 'Nothing is on fire' : 'No incidents yet'}>
              Sentinel opens an incident when a detector or alert rule fires.
            </Empty>
          )}
        </div>
      )}
    </div>
  );
}
