import { useState } from 'react';
import { BellOff, BellRing } from 'lucide-react';
import { send } from '../lib/api';
import { useAuth } from '../lib/auth';
import type { MonitoredProgram } from '../lib/types';
import { clock } from '../lib/format';

const WINDOWS = [
  { minutes: 30, label: '30 minutes' },
  { minutes: 60, label: '1 hour' },
  { minutes: 240, label: '4 hours' },
  { minutes: 1440, label: '24 hours' },
];

/**
 * A maintenance window: notifications for the program are held (and logged as held) while it
 * lasts. Incidents are still recorded, and the program's settings are shared by everyone watching it.
 */
export function MuteControl({ program, onChange }: { program: MonitoredProgram; onChange: (p: MonitoredProgram) => void }) {
  const { account, watching, requestSignIn } = useAuth();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const muted = !!program.muted_until && new Date(program.muted_until).getTime() > Date.now();
  const canEdit = !!account && watching.includes(program.program_id);

  const set = async (minutes: number) => {
    if (!account) return requestSignIn();
    setBusy(true);
    setError(null);
    try {
      onChange(await send<MonitoredProgram>('PUT', `/api/programs/${program.program_id}/mute`, { minutes, reason: minutes ? 'Maintenance window' : undefined }));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  if (muted) {
    return (
      <div className="flex flex-wrap items-center gap-2 rounded-md bg-warn-soft px-3 py-2 text-sm text-warn" role="status">
        <BellOff className="size-4" aria-hidden />
        <span>
          Notifications are held until {clock(program.muted_until)} UTC{program.mute_reason ? ` (${program.mute_reason})` : ''}. Incidents are still recorded.
        </span>
        {canEdit && (
          <button className="btn h-8 text-xs ml-auto" onClick={() => set(0)} disabled={busy}>
            <BellRing className="size-3.5" aria-hidden /> Resume now
          </button>
        )}
        {error && <span className="text-crit">{error}</span>}
      </div>
    );
  }
  if (!canEdit) return null;
  return (
    <label className="inline-flex items-center gap-2 text-xs text-ink-3">
      <BellOff className="size-3.5" aria-hidden />
      <select
        className="input h-8 w-auto text-xs"
        value=""
        disabled={busy}
        onChange={(e) => e.target.value && set(Number(e.target.value))}
        aria-label="Mute notifications for a maintenance window"
      >
        <option value="">Mute for a deploy…</option>
        {WINDOWS.map((w) => <option key={w.minutes} value={w.minutes}>{w.label}</option>)}
      </select>
      {error && <span className="text-crit">{error}</span>}
    </label>
  );
}
