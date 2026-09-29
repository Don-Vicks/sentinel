import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import type { SeriesPoint } from '../lib/types';
import { compact, num } from '../lib/format';

const H = 180;
const PAD = { top: 12, right: 16, bottom: 22, left: 46 };

function niceMax(v: number) {
  if (v <= 0) return 1;
  const p = Math.pow(10, Math.floor(Math.log10(v)));
  const m = v / p;
  return (m <= 1 ? 1 : m <= 2 ? 2 : m <= 5 ? 5 : 10) * p;
}

/** Measures the container before paint, so charts never render at a guessed width. */
function useWidth() {
  const ref = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setW(el.getBoundingClientRect().width);
    if (typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(([e]) => setW(e.contentRect.width));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, Math.max(240, w)] as const;
}

const TICK_STEPS = [5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600];

/** Round-number time ticks (≈5 across) aligned to wall-clock boundaries. */
function timeTicks(t0: number, t1: number) {
  const span = Math.max(1, t1 - t0);
  const step = TICK_STEPS.find((s) => span / s <= 6) ?? 3600;
  const first = Math.ceil(t0 / step) * step;
  const ticks: number[] = [];
  for (let t = first; t <= t1; t += step) ticks.push(t);
  const withSeconds = step < 60;
  return {
    ticks,
    label: (t: number) =>
      new Date(t * 1000).toLocaleTimeString(undefined, {
        hour12: false,
        hour: '2-digit',
        minute: '2-digit',
        ...(withSeconds ? { second: '2-digit' } : {}),
      }),
  };
}

function clock(t: number) {
  return new Date(t * 1000).toLocaleTimeString(undefined, { hour12: false });
}

function YAxis({ max, width, y, format }: { max: number; width: number; y: (v: number) => number; format: (v: number) => string }) {
  return (
    <>
      {[0, max / 2, max].map((t) => (
        <g key={t}>
          <line x1={PAD.left} x2={width - PAD.right} y1={y(t)} y2={y(t)} stroke="var(--color-line)" />
          <text x={PAD.left - 8} y={y(t) + 3.5} textAnchor="end" className="fill-ink-3 text-[10px] num">
            {format(t)}
          </text>
        </g>
      ))}
    </>
  );
}

function XAxis({ t0, t1, x, width }: { t0: number; t1: number; x: (t: number) => number; width: number }) {
  const { ticks, label } = timeTicks(t0, t1);
  return (
    <>
      {ticks.map((t) => {
        const px = x(t);
        // Keep labels inside the plot: the last one hangs left of its tick.
        const anchor = px > width - PAD.right - 30 ? 'end' : px < PAD.left + 20 ? 'start' : 'middle';
        return (
          <text key={t} x={px} y={H - 6} textAnchor={anchor} className="fill-ink-3 text-[10px] num">
            {label(t)}
          </text>
        );
      })}
    </>
  );
}

function Tooltip({ x, width, children, w = 176 }: { x: number; width: number; children: ReactNode; w?: number }) {
  const left = Math.min(Math.max(x - w / 2, 0), width - w);
  return (
    <div
      className="pointer-events-none absolute top-1 z-10 rounded-md border border-line-strong bg-surface px-2.5 py-2 text-xs"
      style={{ left, width: w }}
    >
      {children}
    </div>
  );
}

/**
 * Transactions per second over a fixed window ending now, succeeded and failed
 * stacked. Bars sit at their real time, so the axis doesn't stretch as history
 * accumulates.
 */
export function ActivityChart({ points, span = 600 }: { points: SeriesPoint[]; span?: number }) {
  const [ref, width] = useWidth();
  const [hover, setHover] = useState<SeriesPoint | null>(null);
  const innerW = width - PAD.left - PAD.right;
  const innerH = H - PAD.top - PAD.bottom;
  const t1 = (points[points.length - 1]?.t ?? Math.floor(Date.now() / 1000)) + 1;
  const t0 = t1 - span;
  const visible = points.filter((p) => p.t >= t0);
  const max = niceMax(Math.max(1, ...visible.map((p) => p.tx)));
  const slot = innerW / span;
  const barW = Math.max(1, slot - (slot > 3 ? 1 : 0));
  const x = (t: number) => PAD.left + (t - t0) * slot;
  const y = (v: number) => PAD.top + innerH - (v / max) * innerH;

  return (
    <div ref={ref} className="relative">
      <svg
        width={width}
        height={H}
        role="img"
        aria-label="Transactions per second over the last 10 minutes, succeeded and failed"
        onPointerLeave={() => setHover(null)}
        onPointerMove={(e) => {
          const rect = (e.currentTarget as SVGSVGElement).getBoundingClientRect();
          const t = Math.floor(t0 + (e.clientX - rect.left - PAD.left) / slot);
          setHover(visible.find((p) => p.t === t) ?? null);
        }}
      >
        <YAxis max={max} width={width} y={y} format={compact} />
        {visible.map((p) => {
          const ok = p.tx - p.failed;
          const yOk = y(ok);
          const yFail = y(p.tx);
          const dim = hover !== null && hover.t !== p.t;
          return (
            <g key={p.t} opacity={dim ? 0.5 : 1}>
              {ok > 0 && <rect x={x(p.t)} y={yOk} width={barW} height={y(0) - yOk} fill="var(--color-series-ok)" />}
              {p.failed > 0 && (
                <rect
                  x={x(p.t)}
                  y={yFail}
                  width={barW}
                  height={Math.max(1.5, yOk - yFail - (ok > 0 ? 1 : 0))}
                  fill="var(--color-series-fail)"
                />
              )}
            </g>
          );
        })}
        <XAxis t0={t0} t1={t1} x={x} width={width} />
      </svg>
      {hover && (
        <Tooltip x={x(hover.t)} width={width}>
          <div className="text-ink-3 mb-1 num">{clock(hover.t)}</div>
          <div className="flex justify-between"><span className="num font-semibold">{hover.tx}</span><span className="text-ink-2">transactions</span></div>
          <div className="flex justify-between"><span className="num font-semibold">{hover.failed}</span><span className="text-ink-2">failed</span></div>
        </Tooltip>
      )}
      <div className="flex gap-4 px-1 pt-1 text-xs text-ink-2">
        <span className="inline-flex items-center gap-1.5"><span className="size-2.5 rounded-sm bg-series-ok" aria-hidden />Succeeded</span>
        <span className="inline-flex items-center gap-1.5"><span className="size-2.5 rounded-sm bg-series-fail" aria-hidden />Failed</span>
      </div>
    </div>
  );
}

/** One series over a fixed window ending now, with an optional dashed baseline. */
export function LineChart({
  points,
  value,
  label,
  baseline,
  span = 600,
  format = (v: number) => num(v),
}: {
  points: SeriesPoint[];
  value: (p: SeriesPoint) => number | null;
  label: string;
  baseline?: number;
  span?: number;
  format?: (v: number) => string;
}) {
  const [ref, width] = useWidth();
  const [hover, setHover] = useState<SeriesPoint | null>(null);
  const innerW = width - PAD.left - PAD.right;
  const innerH = H - PAD.top - PAD.bottom;
  const t1 = (points[points.length - 1]?.t ?? Math.floor(Date.now() / 1000)) + 1;
  const t0 = t1 - span;
  const visible = points.filter((p) => p.t >= t0);
  const vals = visible.map(value);
  const max = niceMax(Math.max(1, baseline ?? 0, ...vals.filter((v): v is number => v !== null)));
  const x = (t: number) => PAD.left + ((t - t0) / span) * innerW;
  const y = (v: number) => PAD.top + innerH - (v / max) * innerH;

  let d = '';
  vals.forEach((v, i) => {
    if (v === null) return;
    const joined = i > 0 && vals[i - 1] !== null && visible[i].t - visible[i - 1].t <= 2;
    d += `${joined ? 'L' : 'M'}${x(visible[i].t).toFixed(1)},${y(v).toFixed(1)}`;
  });
  const hv = hover ? value(hover) : null;

  return (
    <div ref={ref} className="relative">
      <svg
        width={width}
        height={H}
        role="img"
        aria-label={`${label} over the last 10 minutes`}
        onPointerLeave={() => setHover(null)}
        onPointerMove={(e) => {
          const rect = (e.currentTarget as SVGSVGElement).getBoundingClientRect();
          const t = Math.round(t0 + ((e.clientX - rect.left - PAD.left) / innerW) * span);
          setHover(visible.reduce<SeriesPoint | null>((best, p) => (!best || Math.abs(p.t - t) < Math.abs(best.t - t) ? p : best), null));
        }}
      >
        <YAxis max={max} width={width} y={y} format={compact} />
        {baseline !== undefined && baseline > 0 && (
          <g>
            <line x1={PAD.left} x2={width - PAD.right} y1={y(baseline)} y2={y(baseline)} stroke="var(--color-ink-3)" strokeDasharray="4 4" />
            <text x={PAD.left + 4} y={y(baseline) - 4} className="fill-ink-3 text-[10px]">
              normal {compact(baseline)}
            </text>
          </g>
        )}
        <path d={d} fill="none" stroke="var(--color-series-1)" strokeWidth={2} strokeLinejoin="round" />
        {hover && hv !== null && (
          <g>
            <line x1={x(hover.t)} x2={x(hover.t)} y1={PAD.top} y2={PAD.top + innerH} stroke="var(--color-line-strong)" />
            <circle cx={x(hover.t)} cy={y(hv)} r={4} fill="var(--color-series-1)" stroke="var(--color-surface)" strokeWidth={2} />
          </g>
        )}
        <XAxis t0={t0} t1={t1} x={x} width={width} />
      </svg>
      {hover && (
        <Tooltip x={x(hover.t)} width={width}>
          <div className="text-ink-3 mb-1 num">{clock(hover.t)}</div>
          <div className="flex justify-between gap-2">
            <span className="num font-semibold">{hv === null ? '—' : format(hv)}</span>
            <span className="text-ink-2 truncate">{label}</span>
          </div>
        </Tooltip>
      )}
    </div>
  );
}

/** Tiny trend line for tiles and cards. Scales to its container; the number beside it carries the value. */
export function Sparkline({
  values,
  tone = 'accent',
  height = 28,
  className = '',
}: {
  values: (number | null)[];
  tone?: 'accent' | 'fail' | 'muted';
  height?: number;
  className?: string;
}) {
  const nums = values.filter((v): v is number => v !== null);
  if (nums.length < 2) return <div className={className} style={{ height }} aria-hidden />;
  const W = 100;
  const max = Math.max(1e-9, ...nums);
  const step = W / (values.length - 1);
  const y = (v: number) => height - 1.5 - (v / max) * (height - 3);
  let d = '';
  values.forEach((v, i) => {
    if (v === null) return;
    d += `${i > 0 && values[i - 1] !== null ? 'L' : 'M'}${(i * step).toFixed(2)},${y(v).toFixed(2)}`;
  });
  const color = tone === 'fail' ? 'var(--color-series-fail)' : tone === 'muted' ? 'var(--color-ink-3)' : 'var(--color-series-1)';
  return (
    <svg
      viewBox={`0 0 ${W} ${height}`}
      preserveAspectRatio="none"
      className={`block w-full ${className}`}
      style={{ height }}
      aria-hidden
    >
      <path d={`${d}L${W},${height}L0,${height}Z`} fill={color} opacity={0.12} />
      <path d={d} fill="none" stroke={color} strokeWidth={1.5} strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

export interface Series {
  label: string;
  values: (number | null)[];
  color: string;
}

export interface Marker {
  t: number;
  label: string;
}

/**
 * Time series with up to a few lines, dashed reference values (baseline,
 * threshold) and vertical event markers (onset, detected, resolved).
 */
export function TimelineChart({
  times,
  series,
  refs = [],
  markers = [],
  format = (v: number) => compact(v),
  label,
}: {
  times: number[];
  series: Series[];
  refs?: { value: number; label: string }[];
  markers?: Marker[];
  format?: (v: number) => string;
  label: string;
}) {
  const [ref, width] = useWidth();
  const [hover, setHover] = useState<number | null>(null);
  const top = 22;
  const innerW = width - PAD.left - PAD.right;
  const innerH = H - top - PAD.bottom;
  const all = series.flatMap((s) => s.values).filter((v): v is number => v !== null);
  const max = niceMax(Math.max(1e-9, ...all, ...refs.map((r) => r.value)));
  const t0 = times[0] ?? 0;
  const t1 = times[times.length - 1] ?? 1;
  const x = (t: number) => PAD.left + (t1 === t0 ? 0 : ((t - t0) / (t1 - t0)) * innerW);
  const y = (v: number) => top + innerH - (v / max) * innerH;

  const path = (vals: (number | null)[]) => {
    let d = '';
    vals.forEach((v, i) => {
      if (v === null) return;
      d += `${i > 0 && vals[i - 1] !== null ? 'L' : 'M'}${x(times[i]).toFixed(1)},${y(v).toFixed(1)}`;
    });
    return d;
  };

  return (
    <div ref={ref} className="relative">
      <svg
        width={width}
        height={H}
        role="img"
        aria-label={label}
        onPointerLeave={() => setHover(null)}
        onPointerMove={(e) => {
          const rect = (e.currentTarget as SVGSVGElement).getBoundingClientRect();
          const rel = (e.clientX - rect.left - PAD.left) / innerW;
          const i = Math.round(rel * (times.length - 1));
          setHover(i >= 0 && i < times.length ? i : null);
        }}
      >
        {[0, max / 2, max].map((t) => (
          <g key={t}>
            <line x1={PAD.left} x2={width - PAD.right} y1={y(t)} y2={y(t)} stroke="var(--color-line)" />
            <text x={PAD.left - 8} y={y(t) + 3.5} textAnchor="end" className="fill-ink-3 text-[10px] num">
              {format(t)}
            </text>
          </g>
        ))}
        {markers
          .filter((m) => m.t >= t0 && m.t <= t1)
          .sort((a, b) => a.t - b.t)
          .map((m, i, all) => {
            const px = x(m.t);
            // Close markers (onset → detected is often seconds): first label
            // hangs left of its line, the rest to the right.
            const crowded = all.length > 1 && all.some((o) => o !== m && Math.abs(x(o.t) - px) < 70);
            const anchor =
              px > width - 80 ? 'end' : px < PAD.left + 50 ? 'start' : crowded && i === 0 ? 'end' : 'start';
            return (
              <g key={m.label}>
                <line x1={px} x2={px} y1={top - 4} y2={top + innerH} stroke="var(--color-ink-3)" strokeDasharray="2 3" />
                <text x={anchor === 'end' ? px - 3 : px + 3} y={top - 8} textAnchor={anchor} className="fill-ink-2 text-[10px]">
                  {m.label}
                </text>
              </g>
            );
          })}
        {refs.map((r, i) => (
          <g key={r.label}>
            <line x1={PAD.left} x2={width - PAD.right} y1={y(r.value)} y2={y(r.value)} stroke="var(--color-ink-3)" strokeDasharray="5 4" />
            {/* Alternate sides so nearby reference labels don't collide. */}
            <text
              x={i % 2 === 0 ? PAD.left + 4 : width - PAD.right - 2}
              y={y(r.value) - 4}
              textAnchor={i % 2 === 0 ? 'start' : 'end'}
              className="fill-ink-3 text-[10px]"
            >
              {r.label} {format(r.value)}
            </text>
          </g>
        ))}
        {series.map((s) => (
          <path key={s.label} d={path(s.values)} fill="none" stroke={s.color} strokeWidth={2} strokeLinejoin="round" />
        ))}
        {hover !== null && (
          <g>
            <line x1={x(times[hover])} x2={x(times[hover])} y1={top} y2={top + innerH} stroke="var(--color-line-strong)" />
            {series.map((s) =>
              s.values[hover] === null ? null : (
                <circle key={s.label} cx={x(times[hover])} cy={y(s.values[hover]!)} r={4} fill={s.color} stroke="var(--color-surface)" strokeWidth={2} />
              ),
            )}
          </g>
        )}
        <XAxis t0={t0} t1={t1} x={x} width={width} />
      </svg>
      {hover !== null && (
        <Tooltip x={x(times[hover])} width={width} w={232}>
          <div className="text-ink-3 mb-1 num">{clock(times[hover])}</div>
          {series.map((s) => (
            <div key={s.label} className="flex items-center gap-2">
              <span className="size-2 rounded-sm shrink-0" style={{ background: s.color }} aria-hidden />
              <span className="num font-semibold">{s.values[hover] === null ? '—' : format(s.values[hover]!)}</span>
              <span className="text-ink-2 truncate">{s.label}</span>
            </div>
          ))}
        </Tooltip>
      )}
      {series.length > 1 && (
        <div className="flex flex-wrap gap-x-4 gap-y-1 px-1 pt-1 text-xs text-ink-2">
          {series.map((s) => (
            <span key={s.label} className="inline-flex items-center gap-1.5 min-w-0">
              <span className="size-2.5 rounded-sm shrink-0" style={{ background: s.color }} aria-hidden />
              <span className="truncate">{s.label}</span>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

/** Horizontal share bars for a ranked breakdown. */
export function ShareBar({ share, tone = 'fail' }: { share: number; tone?: 'fail' | 'accent' }) {
  return (
    <div className="h-1.5 w-full rounded-full bg-sunken overflow-hidden" aria-hidden>
      <div
        className={`h-full rounded-full ${tone === 'fail' ? 'bg-series-fail' : 'bg-series-1'}`}
        style={{ width: `${Math.max(2, Math.min(100, share * 100))}%` }}
      />
    </div>
  );
}
