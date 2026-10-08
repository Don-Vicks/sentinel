import { useState } from 'react';
import { KeyRound, Trash2 } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import { ago } from '../lib/format';
import type { ApiToken } from '../lib/types';
import { CopyButton, Empty, Panel, Spinner } from './ui';

interface Created {
  token: ApiToken;
  secret: string;
}

/**
 * Tokens for agents (Claude, Cursor, ...) to use Sentinel through MCP. Read
 * tokens look at everything the account can; write tokens can also change things.
 */
export function AgentAccess() {
  const tokens = useFetch<ApiToken[]>('/api/tokens');
  const [name, setName] = useState('');
  const [scope, setScope] = useState<'read' | 'write'>('read');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<Created | null>(null);
  const origin = window.location.origin;

  const create = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      setCreated(await send<Created>('POST', '/api/tokens', { name: name.trim() || 'agent', scope }));
      setName('');
      tokens.reload();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const revoke = async (t: ApiToken) => {
    if (!confirm(`Revoke "${t.name}"? Agents using it lose access immediately.`)) return;
    await send('DELETE', `/api/tokens/${t.id}`);
    if (created?.token.id === t.id) setCreated(null);
    tokens.reload();
  };

  const secret = created?.secret ?? 'snt_…';
  const claudeCode = `claude mcp add --transport http sentinel ${origin}/mcp --header "Authorization: Bearer ${secret}"`;
  const desktop = JSON.stringify({ mcpServers: { sentinel: { command: 'sentinel-mcp', env: { SENTINEL_URL: origin, SENTINEL_TOKEN: secret } } } }, null, 2);

  return (
    <Panel title="Connect an agent (MCP)">
      <div className="p-4 grid gap-4 text-sm">
        <p className="text-ink-2">
          Let Claude, Cursor or any MCP client read your programs, diagnose incidents and, with a write token, set up alerts. Everything an agent does goes through the same checks as this dashboard.
        </p>
        {created && (
          <div className="rounded-md border border-good/40 bg-good-soft p-3 grid gap-2">
            <p className="font-medium">Copy this token now. It is not shown again.</p>
            <div className="flex items-center gap-2">
              <code className="font-mono text-xs break-all">{created.secret}</code>
              <CopyButton text={created.secret} label="Copy token" />
            </div>
            <p className="text-xs text-ink-2">Claude Code:</p>
            <div className="flex items-start gap-2">
              <code className="font-mono text-xs break-all">{claudeCode}</code>
              <CopyButton text={claudeCode} label="Copy command" />
            </div>
            <p className="text-xs text-ink-2">Clients that start a local command (Claude Desktop, Cursor), using the <code>sentinel-mcp</code> bridge:</p>
            <div className="flex items-start gap-2">
              <pre className="font-mono text-xs whitespace-pre-wrap break-all">{desktop}</pre>
              <CopyButton text={desktop} label="Copy config" />
            </div>
          </div>
        )}
        {tokens.data && tokens.data.length > 0 ? (
          <ul className="divide-y divide-line">
            {tokens.data.map((t) => (
              <li key={t.id} className="flex flex-wrap items-center gap-x-3 gap-y-1 py-2">
                <KeyRound className="size-4 text-ink-3" aria-hidden />
                <span className="font-medium">{t.name}</span>
                <span className={`rounded px-1.5 py-px text-[11px] font-medium ${t.scope === 'write' ? 'bg-warn-soft text-warn' : 'bg-sunken text-ink-3'}`}>{t.scope}</span>
                <span className="text-xs text-ink-3">{t.last_used_at ? `used ${ago(t.last_used_at)}` : 'never used'}</span>
                <button className="btn h-8 w-8 px-0 ml-auto" onClick={() => revoke(t)} aria-label={`Revoke ${t.name}`}>
                  <Trash2 className="size-3.5" aria-hidden />
                </button>
              </li>
            ))}
          </ul>
        ) : (
          !tokens.loading && <Empty title="No tokens yet">Create one to connect an agent.</Empty>
        )}
        <form onSubmit={create} className="grid gap-3 sm:grid-cols-[1fr_9rem_auto] sm:items-end">
          <div>
            <label className="label" htmlFor="token-name">Name</label>
            <input id="token-name" className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="Claude on my laptop" autoComplete="off" />
          </div>
          <div>
            <label className="label" htmlFor="token-scope">Access</label>
            <select id="token-scope" className="input" value={scope} onChange={(e) => setScope(e.target.value as 'read' | 'write')}>
              <option value="read">Read only</option>
              <option value="write">Read and change</option>
            </select>
          </div>
          <button className="btn-primary" disabled={busy}>{busy ? <Spinner /> : <KeyRound className="size-4" aria-hidden />} Create token</button>
        </form>
        {error && <p className="text-crit" role="alert">{error}</p>}
      </div>
    </Panel>
  );
}
