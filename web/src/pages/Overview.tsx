import { useState } from 'react';
import { Link, useNavigate } from 'react-router';
import { Plus, Radar } from 'lucide-react';
import { send, useFetch } from '../lib/api';
import { usePrograms } from '../lib/programs';
import { useLive } from '../lib/live';
import type { Incident, MonitoredProgram, ProgramSnapshot } from '../lib/types';
import { compact, pct, short } from '../lib/format';
import { Empty, ErrorState, HealthDot, Panel, Skeleton, Spinner } from '../components/ui';
import { IncidentList, mergeIncident } from '../components/IncidentList';

const SUGGESTED = [
  { id: '6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P', label: 'Pump.fun' },
  { id: 'pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA', label: 'PumpSwap AMM' },
  { id: 'JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4', label: 'Jupiter v6' },
  { id: 'whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc', label: 'Orca Whirlpool' },
];

function AddProgram() {
  const { reload, programs } = usePrograms();
  const navigate = useNavigate();
  const [id, setId] = useState('');
  const [label, setLabel] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const add = async (programId: string, programLabel?: string) => {
    setBusy(true);
    setError(null);
    try {
      const p = await send<MonitoredProgram>('POST', '/api/programs', {
        program_id: programId.trim(),
        label: programLabel?.trim() || undefined,
      });
      await reload();
      navigate(`/programs/${p.program_id}`);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const watched = new Set(programs.map((p) => p.program_id));
  return (
    <Panel title="Monitor a program">
      <form
        className="p-4 grid gap-3 md:grid-cols-[2fr_1fr_auto] md:items-end"
        onSubmit={(e) => {
          e.preventDefault();
          if (id.trim()) add(id, label);
        }}
      >
        <div>
          <label className="label" htmlFor="program-id">Program ID</label>
          <input
            id="program-id"
            className="input font-mono"
            value={id}
            onChange={(e) => setId(e.target.value)}
            placeholder="e.g. 6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"
            autoComplete="off"
            spellCheck={false}
            aria-invalid={!!error}
            aria-describedby={error ? 'program-error' : undefined}
          />
        </div>
        <div>
          <label className="label" htmlFor="program-label">Name (optional)</label>
          <input id="program-label" className="input" value={label} onChange={(e) => setLabel(e.target.value)} autoComplete="off" />
        </div>
        <button className="btn-primary" disabled={busy || !id.trim()}>
          {busy ? <Spinner /> : <Plus className="size-4" aria-hidden />}
          Start monitoring
        </button>
        {error && (
          <p id="program-error" className="md:col-span-3 text-sm text-crit" role="alert">
            {error}
          </p>
        )}
      </form>
      <div className="px-4 pb-4 flex flex-wrap items-center gap-2 text-xs">
        <span className="text-ink-3">Try a busy mainnet program:</span>
        {SUGGESTED.filter((s) => !watched.has(s.id)).map((s) => (
          <button key={s.id} type="button" className="btn h-8 text-xs" disabled={busy} onClick={() => add(s.id, s.label)}>
            {s.label}
          </button>
        ))}
      </div>
    </Panel>
  );
}

function ProgramCard({ p }: { p: ProgramSnapshot }) {
  const failureTone = p.failure_rate_60s > Math.max(5, p.baseline_failure_rate * 2) ? 'text-crit' : 'text-ink';
  return (
    <Link to={`/programs/${p.program_id}`} className="panel block p-4 hover:border-line-strong">
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="font-medium text-ink truncate">{p.label}</div>
          <div className="font-mono text-xs text-ink-3">{short(p.program_id, 6)}</div>
        </div>
        <HealthDot health={p.health} withLabel />
      </div>
      <dl className="mt-4 grid grid-cols-3 gap-2">
        <div>
          <dt className="text-xs text-ink-3">TPS</dt>
          <dd className="num font-semibold">{compact(p.tps_10s)}</dd>
        </div>
        <div>
          <dt className="text-xs text-ink-3">Failure rate</dt>
          <dd className={`num font-semibold ${failureTone}`}>{pct(p.failure_rate_60s)}</dd>
        </div>
        <div>
          <dt className="text-xs text-ink-3">Avg CU</dt>
          <dd className="num font-semibold">{compact(p.avg_cu_60s)}</dd>
        </div>
      </dl>
      {p.open_incidents > 0 && (
        <div className="mt-3 text-xs text-crit font-medium">
          {p.open_incidents} open incident{p.open_incidents > 1 ? 's' : ''}
        </div>
      )}
    </Link>
  );
}

export function Overview() {
  const { programs, loading, error, reload } = usePrograms();
  const incidents = useFetch<Incident[]>('/api/incidents?limit=15');
  useLive((e) => {
    if (e.type === 'incident') incidents.setData((prev) => mergeIncident(prev ?? [], e.incident).slice(0, 15));
  });

  return (
    <div className="space-y-5">
      <header>
        <h1 className="text-xl font-semibold">Overview</h1>
        <p className="text-ink-2 mt-1">
          Live health of the programs you watch, streamed from mainnet by Vortex over Solami gRPC.
        </p>
      </header>

      <AddProgram />

      {error ? (
        <ErrorState message={`Couldn't reach Sentinel: ${error}`} onRetry={reload} />
      ) : loading ? (
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          {[0, 1, 2].map((i) => <Skeleton key={i} className="h-36" />)}
        </div>
      ) : programs.length === 0 ? (
        <div className="panel">
          <Empty title="No programs monitored" icon={<Radar className="size-6" aria-hidden />}>
            Add a program ID above. Sentinel starts streaming its transactions immediately and learns a baseline
            over the first two minutes.
          </Empty>
        </div>
      ) : (
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          {programs.map((p) => <ProgramCard key={p.program_id} p={p} />)}
        </div>
      )}

      <Panel title="Recent incidents" action={<Link to="/incidents" className="link text-xs">View all</Link>}>
        {incidents.loading && !incidents.data ? (
          <div className="p-4 space-y-2">{[0, 1].map((i) => <Skeleton key={i} className="h-12" />)}</div>
        ) : incidents.error ? (
          <div className="p-4 text-crit text-sm">{incidents.error}</div>
        ) : incidents.data?.length ? (
          <IncidentList incidents={incidents.data} />
        ) : (
          <Empty title="No incidents">Incidents appear here the moment a detector or alert rule fires.</Empty>
        )}
      </Panel>
    </div>
  );
}
