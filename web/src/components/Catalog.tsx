import { useEffect, useMemo, useState } from 'react';
import { Check, Plus, Sparkles } from 'lucide-react';
import { api, send } from '../lib/api';
import { useAuth } from '../lib/auth';
import { usePrograms } from '../lib/programs';
import type { MonitoredProgram } from '../lib/types';
import { short } from '../lib/format';
import { Segmented, Spinner } from './ui';

interface Entry {
  id: string;
  name: string;
  category: string;
  about: string;
  load: 'busy' | 'moderate' | 'quiet';
}
interface CatalogData {
  programs: Entry[];
  showcase: string[];
}

const LOAD: Record<Entry['load'], { label: string; cls: string }> = {
  busy: { label: 'Busy', cls: 'bg-warn-soft text-warn' },
  moderate: { label: 'Steady', cls: 'bg-sunken text-ink-2' },
  quiet: { label: 'Quiet', cls: 'bg-sunken text-ink-3' },
};

/** Well-known programs, one click to watch. Shown on the Overview under the status panel. */
export function Catalog() {
  const { account, watching, requestSignIn, refresh } = useAuth();
  const { reload } = usePrograms();
  const [data, setData] = useState<CatalogData | null>(null);
  const [category, setCategory] = useState('All');
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    api<CatalogData>('/api/catalog').then(setData).catch(() => {});
  }, []);

  const mine = useMemo(() => new Set(watching), [watching]);
  const categories = useMemo(() => ['All', ...Array.from(new Set((data?.programs ?? []).map((p) => p.category)))], [data]);
  if (!data) return null;

  const rows = data.programs.filter((p) => category === 'All' || p.category === category);
  const showcaseTodo = data.showcase.filter((id) => !mine.has(id));

  const watch = async (ids: string[], tag: string) => {
    if (!account) return requestSignIn();
    setBusy(tag);
    setError(null);
    try {
      for (const id of ids) await send<MonitoredProgram>('POST', '/api/programs', { program_id: id });
    } catch (e) {
      setError((e as Error).message);
    } finally {
      await Promise.all([reload(), refresh()]);
      setBusy(null);
    }
  };

  return (
    <section className="panel overflow-hidden" aria-label="Popular programs">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line px-4 py-2.5">
        <div>
          <h2 className="panel-title">Popular programs</h2>
          <p className="text-xs text-ink-3">{data.programs.length} checked on mainnet. One click to watch.</p>
        </div>
        <div className="flex items-center gap-2">
          <button className="btn h-8 text-xs" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
            {open ? 'Hide list' : 'Browse all'}
          </button>
          <button
            className="btn-primary h-8 text-xs"
            disabled={busy !== null || (!!account && showcaseTodo.length === 0)}
            onClick={() => watch(showcaseTodo.length ? showcaseTodo : data.showcase, 'showcase')}
            title="A balanced set across DEXes, lending, perps, oracles, NFTs and infrastructure that one stream carries comfortably"
          >
            {busy === 'showcase' ? <Spinner /> : <Sparkles className="size-3.5" aria-hidden />}
            {account && showcaseTodo.length === 0 ? 'Showcase set watched' : `Watch showcase set (${data.showcase.length})`}
          </button>
        </div>
      </div>
      {error && (
        <p className="border-b border-line bg-crit-soft px-4 py-2 text-xs text-crit" role="alert">
          {error}
        </p>
      )}
      {open && (
        <>
          <div className="overflow-x-auto border-b border-line px-4 py-2">
            <Segmented label="Category" value={category} onChange={setCategory} options={categories.map((c) => ({ value: c, label: c }))} />
          </div>
          <ul className="grid gap-px bg-line md:grid-cols-2">
            {rows.map((p) => (
              <li key={p.id} className="flex items-center gap-3 bg-surface px-4 py-2.5">
                <div className="min-w-0 flex-1">
                  <p className="flex flex-wrap items-center gap-x-2 text-sm font-medium text-ink">
                    {p.name}
                    <span className={`rounded px-1.5 py-px text-[11px] font-medium ${LOAD[p.load].cls}`}>{LOAD[p.load].label}</span>
                  </p>
                  <p className="truncate text-xs text-ink-3" title={p.about}>{p.about}</p>
                  <p className="font-mono text-[11px] text-ink-3">{short(p.id, 6)}</p>
                </div>
                {mine.has(p.id) ? (
                  <span className="inline-flex shrink-0 items-center gap-1 text-xs text-ink-3">
                    <Check className="size-3.5" aria-hidden /> Watching
                  </span>
                ) : (
                  <button className="btn h-8 shrink-0 text-xs" disabled={busy !== null} onClick={() => watch([p.id], p.id)}>
                    {busy === p.id ? <Spinner /> : <Plus className="size-3.5" aria-hidden />}
                    Watch
                  </button>
                )}
              </li>
            ))}
          </ul>
        </>
      )}
    </section>
  );
}
