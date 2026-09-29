import { Link, useParams } from 'react-router';
import { ArrowRight, CheckCircle2, ExternalLink, RotateCcw, XCircle } from 'lucide-react';
import { useFetch } from '../lib/api';
import type { CallNode, Party, TransactionDetail } from '../lib/types';
import { clock, compact, explorer, explorerAccount, num, short, sol, usd } from '../lib/format';
import { Address, CopyButton, Empty, ErrorState, PageHeader, PageSkeleton, Panel, Stat } from '../components/ui';

function PartyPill({ p, address }: { p?: Party; address: string }) {
  const label = p?.label ?? short(address);
  const program = p?.roles.includes('monitored program') || p?.roles.includes('program-owned');
  return (
    <span
      className={`inline-flex min-w-0 max-w-full items-center rounded-md border px-2 py-1 text-xs ${
        program ? 'border-accent/40 bg-accent-soft text-ink' : 'border-line bg-sunken text-ink'
      }`}
      title={`${address}${p?.owner_program_name ? ` · owned by ${p.owner_program_name}` : ''}`}
    >
      <span className="truncate">{label}</span>
    </span>
  );
}

function CallTree({ nodes, total }: { nodes: CallNode[]; total: number }) {
  return (
    <ol className="divide-y divide-line">
      {nodes.map((n) => (
        <li key={n.index} className="px-4 py-2">
          <div className="flex items-center gap-2" style={{ paddingLeft: `${(n.depth - 1) * 20}px` }}>
            {n.success === false ? (
              <XCircle className="size-4 text-crit shrink-0" aria-label="failed" />
            ) : (
              <CheckCircle2 className="size-4 text-good shrink-0" aria-label="succeeded" />
            )}
            <span className="truncate">
              <span className="font-medium">{n.program_name}</span>
              {n.instruction && <span className="text-ink-2">::{n.instruction}</span>}
            </span>
            <span className="ml-auto flex items-center gap-2 shrink-0">
              {n.compute_consumed !== null && total > 0 && n.depth === 1 && (
                <span className="hidden sm:block h-1.5 w-24 rounded-full bg-sunken overflow-hidden" aria-hidden>
                  <span className="block h-full bg-series-1" style={{ width: `${(n.compute_consumed / total) * 100}%` }} />
                </span>
              )}
              <span className="num text-xs text-ink-3 w-20 text-right">
                {n.compute_consumed !== null ? `${num(n.compute_consumed)} CU` : ''}
              </span>
            </span>
          </div>
          {n.failure && (
            <div className="mt-1 text-xs text-crit font-mono" style={{ paddingLeft: `${(n.depth - 1) * 20 + 24}px` }}>
              {n.failure}
            </div>
          )}
        </li>
      ))}
    </ol>
  );
}

export function Transaction() {
  const { sig = '' } = useParams();
  const { data, error, loading, reload } = useFetch<TransactionDetail>(`/api/transactions/${sig}`);

  if (error) {
    return (
      <div className="space-y-3">
        <ErrorState message={error} onRetry={reload} />
        <a className="link text-sm inline-flex items-center gap-1" href={explorer(sig)} target="_blank" rel="noreferrer">
          Open in Solscan <ExternalLink className="size-3.5" aria-hidden />
        </a>
      </div>
    );
  }
  if (loading && !data) return <PageSkeleton />;
  if (!data) return null;

  const tx = data.transaction;
  const trace = data.trace;
  const parties = new Map(trace.parties.map((p) => [p.address, p]));
  const decodedByPath = new Map(trace.decoded.map((d) => [d.path, d]));
  const labelOf = (a: string) => parties.get(a)?.label ?? short(a);
  const topTotal = trace.call_tree.filter((n) => n.depth === 1).reduce((s, n) => s + (n.compute_consumed ?? 0), 0);
  const priorityFee =
    tx.compute_unit_price && tx.compute_unit_limit ? (tx.compute_unit_price * tx.compute_unit_limit) / 1e6 : null;

  return (
    <div className="space-y-6">
      <PageHeader
        crumbs={[
          ...data.monitored_programs.slice(0, 1).map((pid) => ({
            label: data.program_labels[pid] ?? short(pid),
            to: `/programs/${pid}`,
          })),
          ...data.incidents.slice(0, 1).map((iid) => ({ label: `Incident #${iid}`, to: `/incidents/${iid}` })),
        ]}
        title={
          <span className="flex flex-wrap items-center gap-3">
            Transaction
            {tx.success ? (
              <span className="inline-flex items-center gap-1 rounded bg-good-soft px-1.5 py-0.5 text-xs font-medium text-good">
                <CheckCircle2 className="size-3.5" aria-hidden /> Succeeded
              </span>
            ) : (
              <span className="inline-flex items-center gap-1 rounded bg-crit-soft px-1.5 py-0.5 text-xs font-medium text-crit">
                <XCircle className="size-3.5" aria-hidden /> Failed
              </span>
            )}
          </span>
        }
        meta={
          <span className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <span className="inline-flex items-center gap-1 font-mono text-xs text-ink-2 break-all">
              {tx.signature}
              <CopyButton text={tx.signature} label="Copy signature" />
            </span>
            {data.incidents.map((id) => (
              <Link key={id} to={`/incidents/${id}`} className="link text-xs">
                Incident #{id}
              </Link>
            ))}
          </span>
        }
        actions={
          <a className="btn" href={explorer(tx.signature)} target="_blank" rel="noreferrer">
            Solscan <ExternalLink className="size-3.5" aria-hidden />
          </a>
        }
      />

      <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-6 gap-3">
        <Stat label="Slot" value={num(tx.slot)} sub={`received ${clock(tx.received_at)}`} />
        <Stat label="Compute" value={compact(tx.compute_units)} sub={tx.compute_unit_limit ? `limit ${compact(tx.compute_unit_limit)}` : 'default limit'} />
        <Stat label="Fee" value={`${sol(tx.fee).toFixed(6)}`} sub={priorityFee !== null ? `priority ≈ ${num(priorityFee)} lamports` : 'SOL'} />
        <Stat label="Instructions" value={num(tx.instructions.filter((i) => i.inner_index === null).length)} sub={`${tx.instructions.length} incl. CPIs`} />
        <Stat label="Accounts" value={num(tx.accounts.length)} sub={`${tx.accounts.filter((a) => a.writable).length} writable`} />
        <Stat label="Transfers" value={num(trace.flows.length)} />
      </div>

      {tx.error && (
        <div className="panel border-crit/40 p-4">
          <h2 className="panel-title text-crit">Error</h2>
          <p className="mt-1 font-medium">
            {trace.error_detail?.name ?? tx.error.name ?? tx.error.message}
            {(trace.error_detail?.code ?? tx.error.custom_code) != null && (
              <span className="ml-1.5 num text-xs text-ink-3">#{trace.error_detail?.code ?? tx.error.custom_code}</span>
            )}
          </p>
          {trace.error_detail?.message && (
            <p className="mt-1 text-ink-2">
              {trace.error_detail.message} <span className="text-xs text-ink-3">(from the program's IDL)</span>
            </p>
          )}
          <p className="mt-1 font-mono text-xs text-ink-2 break-all">{tx.error.message}</p>
        </div>
      )}

      <div className="panel p-4">
        <h2 className="panel-title mb-2">What happened</h2>
        <ol className="space-y-1.5 list-decimal list-inside marker:text-ink-3">
          {trace.narrative.map((l, i) => <li key={i}>{l}</li>)}
        </ol>
      </div>

      <div className="grid gap-4 xl:grid-cols-2">
        <Panel
          title={trace.flows.some((f) => f.usd !== null) ? 'Value flow (USD via Solami Blur)' : 'Value flow'}
          action={trace.reverted ? (
            <span className="inline-flex items-center gap-1 text-xs text-warn"><RotateCcw className="size-3.5" aria-hidden />Rolled back</span>
          ) : undefined}
        >
          {trace.flows.length === 0 ? (
            <Empty title="No SOL or token movement" />
          ) : (
            <ul className="divide-y divide-line">
              {trace.flows.map((f, i) => (
                <li key={i} className={`grid grid-cols-[1fr_auto_1fr] items-center gap-2 px-4 py-2.5 ${trace.reverted ? 'opacity-60' : ''}`}>
                  <PartyPill p={parties.get(f.from)} address={f.from} />
                  <span className="flex flex-col items-center text-xs">
                    <span className="num font-semibold whitespace-nowrap">
                      {compact(f.amount)} {f.symbol}
                    </span>
                    {usd(f.usd) && <span className="num text-ink-2">{usd(f.usd)}</span>}
                    <ArrowRight className="size-4 text-ink-3" aria-hidden />
                    <span className="text-ink-3 num">ix {f.instruction}</span>
                  </span>
                  <span className="flex justify-end min-w-0"><PartyPill p={parties.get(f.to)} address={f.to} /></span>
                </li>
              ))}
            </ul>
          )}
        </Panel>

        <Panel title="Net balance changes">
          {trace.balance_changes.length === 0 ? (
            <Empty title="No balance changes" />
          ) : (
            <table className="table">
              <thead>
                <tr><th>Owner</th><th>Asset</th><th className="text-right">Change</th><th className="text-right">USD</th></tr>
              </thead>
              <tbody>
                {trace.balance_changes.map((b, i) => (
                  <tr key={i}>
                    <td className="truncate max-w-56" title={b.owner}>{labelOf(b.owner)}</td>
                    <td className="text-ink-2">{b.symbol}</td>
                    <td className={`num text-right ${b.delta < 0 ? 'text-crit' : 'text-good'}`}>
                      {b.delta > 0 ? '+' : ''}{compact(b.delta)}
                    </td>
                    <td className="num text-right text-ink-2">{usd(b.usd) ?? '—'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </Panel>
      </div>

      <Panel title="Program call tree">
        {trace.call_tree.length ? <CallTree nodes={trace.call_tree} total={topTotal} /> : <Empty title="No invocation logs" />}
      </Panel>

      <Panel title="Account state changes">
        <div className="overflow-x-auto">
          <table className="table">
            <thead>
              <tr><th>Account</th><th>Kind</th><th className="text-right">Before</th><th className="text-right">After</th><th className="text-right">Change</th><th>Note</th></tr>
            </thead>
            <tbody>
              {trace.state_changes.map((c) => (
                <tr key={c.account}>
                  <td>
                    <a className="link font-mono text-xs" href={explorerAccount(c.account)} target="_blank" rel="noreferrer">{short(c.account, 5)}</a>
                    {c.owner && <span className="text-xs text-ink-3"> · {labelOf(c.owner)}</span>}
                  </td>
                  <td className="text-xs text-ink-2">{c.kind === 'data' ? 'data' : c.symbol}</td>
                  <td className="num text-xs text-right">{c.kind === 'data' ? '' : compact(c.before)}</td>
                  <td className="num text-xs text-right">{c.kind === 'data' ? '' : compact(c.after)}</td>
                  <td className={`num text-xs text-right ${c.delta < 0 ? 'text-crit' : c.delta > 0 ? 'text-good' : ''}`}>
                    {c.kind === 'data' ? '' : `${c.delta > 0 ? '+' : ''}${compact(c.delta)}`}
                  </td>
                  <td className="text-xs text-ink-3">{c.note}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Panel>

      <Panel title={trace.decoded.length ? 'Instructions (decoded with on-chain IDLs)' : 'Instructions'}>
        <ul className="divide-y divide-line">
          {tx.instructions.map((ix) => {
            const dec = decodedByPath.get(ix.path);
            return (
            <li key={ix.path}>
              <details className="group">
                <summary className="flex cursor-pointer items-center gap-3 px-4 py-2 hover:bg-sunken list-none">
                  <span className="num text-xs text-ink-3 w-10">{ix.path}</span>
                  <span className="font-medium truncate" style={{ paddingLeft: `${(ix.stack_height - 1) * 16}px` }}>
                    {ix.program_name ?? data.program_labels[ix.program_id] ?? short(ix.program_id)}
                  </span>
                  <span className="text-ink-2 truncate">{dec?.name ?? ix.name ?? ''}</span>
                  <span className="ml-auto text-xs text-ink-3">{ix.accounts.length} accounts</span>
                </summary>
                <div className="px-4 pb-3 pl-16 space-y-2 text-xs">
                  <div><span className="text-ink-3">Program </span><Address value={ix.program_id} n={8} /></div>
                  {dec ? (
                    <>
                      {Object.keys(dec.args).length > 0 && (
                        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 rounded bg-sunken p-2">
                          {Object.entries(dec.args).map(([k, v]) => (
                            <div key={k} className="contents">
                              <dt className="text-ink-3">{k}</dt>
                              <dd className="font-mono break-all">{typeof v === 'object' ? JSON.stringify(v) : String(v)}</dd>
                            </div>
                          ))}
                        </dl>
                      )}
                      {dec.partial && <p className="text-warn">Some arguments could not be decoded.</p>}
                      <table className="w-full">
                        <tbody>
                          {dec.accounts.map((a, i) => (
                            <tr key={i}>
                              <td className="pr-3 py-0.5 text-ink-3 whitespace-nowrap align-top">{a.name}</td>
                              <td className="py-0.5 font-mono text-ink-2 break-all">{a.pubkey}</td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </>
                  ) : (
                    <>
                      {ix.parsed != null && <pre className="rounded bg-sunken p-2 overflow-x-auto">{JSON.stringify(ix.parsed, null, 2)}</pre>}
                      <div className="font-mono break-all text-ink-2"><span className="text-ink-3 font-sans">Data (base64) </span>{ix.data || '—'}</div>
                      <ol className="list-decimal list-inside font-mono text-ink-2 space-y-0.5">
                        {ix.accounts.map((a, i) => <li key={i}>{a}</li>)}
                      </ol>
                    </>
                  )}
                </div>
              </details>
            </li>
            );
          })}
        </ul>
      </Panel>

      <Panel title={`Logs${tx.logs_truncated ? ' (truncated by the runtime)' : ''}`}>
        <pre className="max-h-96 overflow-auto p-4 font-mono text-xs leading-relaxed text-ink-2 whitespace-pre-wrap break-all">
          {tx.logs.join('\n') || 'No logs'}
        </pre>
      </Panel>
    </div>
  );
}
