import { useFetch } from '../lib/api';
import { ago, explorerAccount, num } from '../lib/format';
import { Address, Panel, Skeleton } from './ui';
import { ShareBar } from './charts';

interface Dependencies {
  dependencies: {
    program_id: string;
    label: string;
    calls: number;
    failed: number;
    share: number;
    last_seen: string;
    watched_for_upgrades: boolean;
  }[];
}

/** Programs this one calls. The busiest are watched, so an upgrade of one is an incident here. */
export function DependenciesCard({ programId }: { programId: string }) {
  const { data, error, loading } = useFetch<Dependencies>(`/api/programs/${programId}/dependencies`);
  return (
    <Panel title="Programs it calls">
      {loading && !data ? (
        <div className="p-4"><Skeleton className="h-12" /></div>
      ) : error || !data ? (
        <p className="p-4 text-sm text-ink-3">Couldn&apos;t load dependencies{error ? `: ${error}` : ''}.</p>
      ) : data.dependencies.length === 0 ? (
        <p className="p-4 text-sm text-ink-3">
          None seen yet. Sentinel learns which programs this one calls (oracles, AMMs, routers) from its transactions, ignoring the system and token programs, and watches the busiest for upgrades.
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table className="table">
            <thead><tr><th>Program</th><th className="w-1/4">Share of calls</th><th className="text-right">Calls</th><th className="text-right">In failed tx</th><th>Upgrades</th></tr></thead>
            <tbody>
              {data.dependencies.map((d) => (
                <tr key={d.program_id}>
                  <td>
                    <span className="font-medium">{d.label}</span>{' '}
                    <a className="link" href={explorerAccount(d.program_id)} target="_blank" rel="noreferrer"><Address value={d.program_id} n={4} copy={false} /></a>
                  </td>
                  <td><div className="flex items-center gap-2"><ShareBar share={d.share} tone="accent" /><span className="num text-xs w-10 text-right">{(d.share * 100).toFixed(0)}%</span></div></td>
                  <td className="num text-right">{num(d.calls)}</td>
                  <td className="num text-right">{num(d.failed)}</td>
                  <td className="text-xs text-ink-2">{d.watched_for_upgrades ? 'watched' : 'not yet (seen too rarely)'} · seen {ago(d.last_seen)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Panel>
  );
}
