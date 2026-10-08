import { Link } from 'react-router';
import { ShieldAlert, ShieldCheck } from 'lucide-react';
import { useFetch } from '../lib/api';
import { explorerAccount } from '../lib/format';
import type { Incident, Posture } from '../lib/types';
import { Address, Panel, SeverityBadge, Skeleton } from './ui';

const KIND: Record<Posture['authority_kind'], { label: string; good: boolean }> = {
  none: { label: 'Immutable: nobody can upgrade it', good: true },
  program_controlled: { label: 'Program-controlled (multisig, DAO or similar)', good: true },
  single_key: { label: 'A single wallet can upgrade it', good: false },
};

/**
 * Who can change this program's code, read from chain, plus recent upgrades
 * and authority changes Sentinel has seen.
 */
export function PostureCard({ programId, incidents }: { programId: string; incidents: Incident[] }) {
  const { data, error, loading } = useFetch<Posture>(`/api/programs/${programId}/posture`);
  const changes = incidents.filter((i) => i.kind === 'authority_change').slice(0, 3);

  return (
    <Panel title="Code and authority">
      {loading && !data ? (
        <div className="p-4"><Skeleton className="h-16" /></div>
      ) : error || !data ? (
        <p className="p-4 text-sm text-ink-3">
          Couldn&apos;t read the program&apos;s upgrade authority{error ? `: ${error}` : ''}. Upgrades and authority changes are still detected from the live stream.
        </p>
      ) : (
        <div className="p-4 grid gap-3 text-sm">
          <p className="flex items-center gap-2 font-medium">
            {KIND[data.authority_kind].good ? (
              <ShieldCheck className="size-4 text-good" aria-hidden />
            ) : (
              <ShieldAlert className="size-4 text-crit" aria-hidden />
            )}
            {data.upgradeable ? KIND[data.authority_kind].label : 'Not upgradeable (older loader)'}
          </p>
          <dl className="grid gap-x-6 gap-y-1.5 sm:grid-cols-[max-content_1fr]">
            {data.authority && (
              <>
                <dt className="text-ink-3">Upgrade authority</dt>
                <dd><a className="link" href={explorerAccount(data.authority)} target="_blank" rel="noreferrer"><Address value={data.authority} n={6} copy={false} /></a></dd>
              </>
            )}
            {data.last_deployed_slot !== null && (
              <>
                <dt className="text-ink-3">Last deployed at slot</dt>
                <dd className="num">{data.last_deployed_slot.toLocaleString()}</dd>
              </>
            )}
            {data.code_bytes !== null && (
              <>
                <dt className="text-ink-3">Code size</dt>
                <dd className="num">{(data.code_bytes / 1024).toFixed(0)} KB</dd>
              </>
            )}
          </dl>
          {data.controller && (
            <p className="text-ink-2">
              Upgrades have been executed by a {data.controller.name} multisig{' '}
              <a className="link" href={explorerAccount(data.controller.multisig)} target="_blank" rel="noreferrer"><Address value={data.controller.multisig} n={6} copy={false} /></a>
              {data.controller.requires && ` that needs ${data.controller.requires.threshold} of ${data.controller.requires.members} signatures`}.
            </p>
          )}
          {data.risks.map((r) => (
            <p key={r.text} className="text-ink-2 flex gap-2">
              <SeverityBadge severity={r.level} />
              <span>{r.text}</span>
            </p>
          ))}
        </div>
      )}
      {changes.length > 0 && (
        <div className="border-t border-line px-4 py-3 text-sm">
          <p className="label">Recent changes seen</p>
          <ul className="grid gap-1">
            {changes.map((i) => (
              <li key={i.id} className="flex flex-wrap items-center gap-2">
                <SeverityBadge severity={i.severity} />
                <Link className="link" to={`/incidents/${i.id}`}>{i.summary}</Link>
              </li>
            ))}
          </ul>
        </div>
      )}
    </Panel>
  );
}
