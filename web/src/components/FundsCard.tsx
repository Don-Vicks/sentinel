import { useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import { useLive } from '../lib/live';
import { useAuth } from '../lib/auth';
import { compact, explorerAccount, usd } from '../lib/format';
import type { VaultStatus } from '../lib/types';
import { Address, Panel, Skeleton, Spinner } from './ui';

/**
 * Vault and treasury accounts to watch for drains. The list is part of the
 * program's shared settings, so anyone watching the program can edit it.
 */
export function FundsCard({ programId }: { programId: string }) {
  const { data, error, loading, reload, setData } = useFetch<VaultStatus>(`/api/programs/${programId}/vaults`);
  const { account, watching, requestSignIn } = useAuth();
  const [input, setInput] = useState('');
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const canEdit = !!account && watching.includes(programId);

  // Balances change with the stream.
  useLive((e) => {
    if (e.type === 'incident' && e.incident.program_id === programId && e.incident.kind === 'vault_drain') reload();
  });

  const save = async (vaults: string[]) => {
    if (!account) return requestSignIn();
    setBusy(true);
    setProblem(null);
    try {
      setData(await send<VaultStatus>('PUT', `/api/programs/${programId}/vaults`, { vaults }));
      setInput('');
    } catch (e) {
      setProblem((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const current = data?.vaults.map((v) => v.account) ?? [];

  return (
    <Panel title="Funds">
      {loading && !data ? (
        <div className="p-4"><Skeleton className="h-12" /></div>
      ) : error || !data ? (
        <p className="p-4 text-sm text-ink-3">Couldn&apos;t load vaults{error ? `: ${error}` : ''}.</p>
      ) : (
        <div className="grid gap-3 p-4 text-sm">
          {data.vaults.length === 0 ? (
            <p className="text-ink-3">
              No vaults watched. Add the program&apos;s treasury or vault accounts (token accounts or SOL accounts) and Sentinel opens an incident when one loses a large share of its balance within {Math.round(data.window_secs / 60)} minutes.
            </p>
          ) : (
            <ul className="divide-y divide-line">
              {data.vaults.map((v) => (
                <li key={v.account} className="flex flex-wrap items-center gap-x-4 gap-y-1 py-2">
                  <a className="link" href={explorerAccount(v.account)} target="_blank" rel="noreferrer"><Address value={v.account} n={6} copy={false} /></a>
                  {v.seen ? (
                    <>
                      <span className="num">{v.balance !== null ? `${compact(v.balance)} ${v.symbol ?? ''}` : '—'}{v.balance_usd !== null ? ` · ${usd(v.balance_usd)}` : ''}</span>
                      <span className={`num text-xs ${v.net_window < 0 ? 'text-crit' : 'text-ink-3'}`}>
                        {v.net_window >= 0 ? '+' : ''}{compact(v.net_window)} in the last {Math.round(data.window_secs / 60)} min
                      </span>
                    </>
                  ) : (
                    <span className="text-xs text-ink-3">no activity seen yet</span>
                  )}
                  {canEdit && (
                    <button className="btn h-8 w-8 px-0 ml-auto" onClick={() => save(current.filter((a) => a !== v.account))} disabled={busy} aria-label="Stop watching this vault">
                      <Trash2 className="size-3.5" aria-hidden />
                    </button>
                  )}
                </li>
              ))}
            </ul>
          )}
          {data.candidates.length > 0 && canEdit && (
            <div>
              <p className="label">Looks like a vault</p>
              <ul className="grid gap-1">
                {data.candidates.map((c) => (
                  <li key={c.account} className="flex flex-wrap items-center gap-2">
                    <Address value={c.account} n={6} />
                    <span className="text-xs text-ink-3">{c.symbol} · moved in {c.transactions} recent transactions, owner never signs</span>
                    <button className="btn h-7 text-xs ml-auto" onClick={() => save([...current, c.account])} disabled={busy}>
                      <Plus className="size-3.5" aria-hidden /> Watch
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          )}
          {problem && <p className="text-crit" role="alert">{problem}</p>}
          {canEdit ? (
            <form
              className="flex gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                if (input.trim()) save([...current, input.trim()]);
              }}
            >
              <label className="sr-only" htmlFor="vault-add">Vault address</label>
              <input id="vault-add" className="input font-mono" value={input} onChange={(e) => setInput(e.target.value)} placeholder="Vault or treasury account address" autoComplete="off" />
              <button className="btn" disabled={busy || !input.trim()}>{busy ? <Spinner /> : <Plus className="size-4" aria-hidden />} Watch</button>
            </form>
          ) : (
            <p className="text-xs text-ink-3">
              {account ? 'Watch this program to edit its vaults.' : <button className="link" onClick={requestSignIn}>Sign in</button>}
              {!account && ' and watch the program to edit its vaults.'}
            </p>
          )}
        </div>
      )}
    </Panel>
  );
}
