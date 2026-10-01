import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router';
import { Check, Search } from 'lucide-react';
import { send } from '../lib/api';
import { useAuth } from '../lib/auth';
import { usePrograms } from '../lib/programs';
import type { Candidate, MonitoredProgram, Resolution } from '../lib/types';
import { short } from '../lib/format';
import { Spinner } from './ui';

const HINTS: Record<Resolution['kind'], string> = {
  program: 'Program found.',
  authority: 'Upgrade authority. It is only read from the chain; nothing is signed.',
  transaction: 'Transaction found.',
  account: 'Account found.',
};

/** One box: paste a program, an upgrade-authority address, a signature or an explorer link. */
export function Finder({ onDone, initial }: { onDone?: () => void; initial?: string }) {
  const { account, watching, requestSignIn, refresh } = useAuth();
  const { reload } = usePrograms();
  const navigate = useNavigate();
  const [query, setQuery] = useState(initial ?? '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [found, setFound] = useState<Resolution | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());

  // Reading a whole authority off a busy RPC can take a while; say so instead of spinning silently.
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    if (!busy || found) return setSlow(false);
    const t = setTimeout(() => setSlow(true), 3000);
    return () => clearTimeout(t);
  }, [busy, found]);

  const look = async (q = query) => {
    setBusy(true);
    setError(null);
    setFound(null);
    try {
      const r = await send<Resolution>('POST', '/api/resolve', { query: q });
      setFound(r);
      const mine = new Set(watching);
      // Preselect the application programs, not System/Token plumbing.
      setPicked(new Set(r.programs.filter((p) => !p.infra && !mine.has(p.program_id)).map((p) => p.program_id)));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  // Some programs are still being identified on the server; keep asking until they all are.
  const pending = found?.pending ?? 0;
  const subject = found?.subject;
  useEffect(() => {
    if (!pending || !subject) return;
    let stop = false;
    const t = setTimeout(async () => {
      try {
        const r = await send<Resolution>('POST', '/api/resolve', { query: subject });
        if (stop) return;
        const known = new Set(found?.programs.map((p) => p.program_id));
        const mine = new Set(watching);
        setFound(r);
        // New arrivals are preselected like the first batch; earlier choices stay as the user left them.
        setPicked((prev) => {
          const next = new Set(prev);
          r.programs.filter((p) => !known.has(p.program_id) && !p.infra && !mine.has(p.program_id)).forEach((p) => next.add(p.program_id));
          return next;
        });
      } catch (e) {
        if (!stop) setError((e as Error).message);
      }
    }, 2000);
    return () => {
      stop = true;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pending, subject, found]);

  useEffect(() => {
    if (initial) {
      setQuery(initial);
      look(initial);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initial]);

  const watch = async () => {
    if (!account) return requestSignIn();
    setBusy(true);
    setError(null);
    try {
      const added: MonitoredProgram[] = [];
      for (const id of picked) added.push(await send<MonitoredProgram>('POST', '/api/programs', { program_id: id }));
      await Promise.all([reload(), refresh()]);
      onDone?.();
      if (added.length === 1) navigate(`/programs/${added[0].program_id}`);
    } catch (e) {
      setError((e as Error).message);
      await Promise.all([reload(), refresh()]);
    } finally {
      setBusy(false);
    }
  };

  const toggle = (id: string) =>
    setPicked((p) => {
      const n = new Set(p);
      n.has(id) ? n.delete(id) : n.add(id);
      return n;
    });

  const mine = new Set(watching);
  const row = (c: Candidate) => (
    <label key={c.program_id} className={`flex items-center gap-3 px-3 py-2.5 ${mine.has(c.program_id) ? 'opacity-60' : 'cursor-pointer hover:bg-sunken'}`}>
      <input
        type="checkbox"
        className="size-4 accent-[var(--color-accent)]"
        checked={picked.has(c.program_id) || mine.has(c.program_id)}
        disabled={mine.has(c.program_id)}
        onChange={() => toggle(c.program_id)}
      />
      <span className="min-w-0 flex-1">
        <span className="block truncate text-sm font-medium text-ink">{c.name ?? 'Unnamed program'}</span>
        <span className="block truncate font-mono text-xs text-ink-3">{short(c.program_id, 8)}</span>
      </span>
      {mine.has(c.program_id) && (
        <span className="inline-flex items-center gap-1 text-xs text-ink-3">
          <Check className="size-3.5" aria-hidden /> Watching
        </span>
      )}
      {c.infra && <span className="text-[11px] text-ink-3">shared plumbing</span>}
    </label>
  );

  return (
    <div className="p-4 space-y-3">
      <form
        className="flex flex-col gap-2 sm:flex-row"
        onSubmit={(e) => {
          e.preventDefault();
          if (query.trim()) look();
        }}
      >
        <input
          className="input font-mono flex-1"
          aria-label="Program, wallet, transaction or explorer link"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Program, authority, tx or link"
          autoComplete="off"
          spellCheck={false}
          autoFocus={!initial}
        />
        <button className="btn-primary" disabled={busy || !query.trim()}>
          {busy && !found ? <Spinner /> : <Search className="size-4" aria-hidden />}
          Find programs
        </button>
      </form>
      <p className="text-xs text-ink-3">
        Deploying several programs? Paste your upgrade-authority address and we list everything it can upgrade. Everything read is public; no signature from that key.
      </p>
      {slow && (
        <p className="text-xs text-ink-2" role="status">
          Reading the chain. Looking up every program an address can upgrade takes a few seconds the first time.
        </p>
      )}
      {error && (
        <p className="text-sm text-crit" role="alert">
          {error}
        </p>
      )}
      {found && (
        <div className="rounded-md border border-line">
          <div className="border-b border-line px-3 py-2">
            <p className="text-sm font-medium text-ink">{found.headline}</p>
            <p className="text-xs text-ink-3">{HINTS[found.kind]}</p>
          </div>
          <div className="max-h-72 divide-y divide-line overflow-auto">
            {found.programs.map(row)}
            {pending > 0 && (
              <div className="flex items-center gap-2 px-3 py-2.5 text-xs text-ink-2" role="status">
                <Spinner /> Identifying {pending} more program{pending === 1 ? '' : 's'}. Each takes a few seconds the first time.
              </div>
            )}
          </div>
          <div className="flex items-center justify-between gap-3 border-t border-line px-3 py-2">
            <span className="text-xs text-ink-3">{account ? `${picked.size} selected` : 'Sign in with any wallet to get alerts'}</span>
            <button className="btn-primary" disabled={busy || (!!account && picked.size === 0)} onClick={watch}>
              {busy && <Spinner />}
              {account ? `Watch ${picked.size || ''} ${picked.size === 1 ? 'program' : 'programs'}` : 'Sign in to watch'}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
