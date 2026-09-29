export const short = (s: string | null | undefined, n = 4) =>
  !s ? '—' : s.length <= n * 2 + 1 ? s : `${s.slice(0, n)}…${s.slice(-n)}`;

export function num(x: number | null | undefined, digits = 0) {
  if (x === null || x === undefined || Number.isNaN(x)) return '—';
  return x.toLocaleString(undefined, { maximumFractionDigits: digits, minimumFractionDigits: digits });
}

export function compact(x: number | null | undefined) {
  if (x === null || x === undefined) return '—';
  const a = Math.abs(x);
  if (a >= 1e9) return `${(x / 1e9).toFixed(2)}B`;
  if (a >= 1e6) return `${(x / 1e6).toFixed(2)}M`;
  if (a >= 1e5) return `${(x / 1e3).toFixed(0)}K`;
  if (a >= 1e4) return `${(x / 1e3).toFixed(1)}K`;
  if (a >= 100) return x.toFixed(0);
  if (a >= 1) return Number.isInteger(x) ? String(x) : x.toFixed(2);
  if (a === 0) return '0';
  return x.toPrecision(3);
}

/** USD with sensible precision; `null` when unpriced. */
export function usd(x: number | null | undefined) {
  if (x === null || x === undefined || !Number.isFinite(x)) return null;
  const a = Math.abs(x);
  const sign = x < 0 ? '-' : '';
  if (a >= 1e9) return `${sign}$${(a / 1e9).toFixed(2)}B`;
  if (a >= 1e6) return `${sign}$${(a / 1e6).toFixed(2)}M`;
  if (a >= 1e4) return `${sign}$${(a / 1e3).toFixed(1)}K`;
  if (a >= 1) return `${sign}$${a.toFixed(2)}`;
  if (a >= 0.01) return `${sign}$${a.toFixed(2)}`;
  return a === 0 ? '$0' : `<$0.01`;
}

export const pct = (x: number | null | undefined, digits = 1) =>
  x === null || x === undefined ? '—' : `${x.toFixed(digits)}%`;

export function ago(iso: string | null | undefined, now = Date.now()) {
  if (!iso) return '—';
  const s = Math.max(0, Math.round((now - new Date(iso).getTime()) / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export function clock(iso: string | null | undefined) {
  if (!iso) return '—';
  return new Date(iso).toLocaleTimeString(undefined, { hour12: false });
}

export function duration(ms: number) {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}

export const sol = (lamports: number) => lamports / 1e9;

export const KIND_LABEL: Record<string, string> = {
  failure_spike: 'Failure spike',
  activity_spike: 'Activity spike',
  activity_drop: 'Activity stopped',
  compute_spike: 'Compute spike',
  large_transfer: 'Large transfer',
  rule_triggered: 'Rule triggered',
  error_spike: 'Error spike',
};

export const explorer = (sig: string) => `https://solscan.io/tx/${sig}`;
export const explorerAccount = (a: string) => `https://solscan.io/account/${a}`;
