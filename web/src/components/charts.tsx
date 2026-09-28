import { useMemo, useRef, useState } from 'react';
import type { SeriesPoint } from '../lib/types';
import { compact, num } from '../lib/format';

const H = 180;
const PAD = { top: 12, right: 12, bottom: 22, left: 40 };

function niceMax(v: number) {
  if (v <= 0) return 1;
  const p = Math.pow(10, Math.floor(Math.log10(v)));
  const m = v / p;
  return (m <= 1 ? 1 : m <= 2 ? 2 : m <= 5 ? 5 : 10) * p;
}

function timeLabel(t: number) {
  return new Date(t * 1000).toLocaleTimeString(undefined, { hour12: false, hour: '2-digit', minute: '2-digit' });
}

function useWidth() {
  const ref = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(600);
  const observer = useMemo(
    () =>
      typeof ResizeObserver === 'undefined'
        ? null
        : new ResizeObserver(([e]) => setW(Math.max(240, e.contentRect.width))),
    [],
  );
  const setRef = (el: HTMLDivElement | null) => {
    if (ref.current && observer) observer.unobserve(ref.current);
    ref.current = el;
    if (el && observer) observer.observe(el);
  };
  return [setRef, w] as const;
}

function Tooltip({ x, width, children }: { x: number; width: number; children: React.ReactNode }) {
  const left = Math.min(Math.max(x - 80, 0), width - 170);
  return (
    <div
      className="pointer-events-none absolute top-1 z-10 w-40 rounded-md border border-line bg-surface px-2.5 py-2 text-xs shadow-sm"
      style={{ left }}
    >
      {children}
    </div>
  );
}

/** Transactions per second, stacked succeeded + failed, one bar per second. */
export function ActivityChart({ points }: { points: SeriesPoint[] }) {
  const [ref, width] = useWidth();
  const [hover, setHover] = useState<number | null>(null);
  const innerW = width - PAD.left - PAD.right;
  const innerH = H - PAD.top - PAD.bottom;
  const max = niceMax(Math.max(1, ...points.map((p) => p.tx)));
  const step = points.length ? innerW / points.length : innerW;
  const barW = Math.max(1, step - (step > 4 ? 1 : 0));
  const y = (v: number) => PAD.top + innerH - (v / max) * innerH;
  const ticks = [0, max / 2, max];
  const labelEvery = Math.max(1, Math.round(points.length / 5));
  const hp = hover !== null ? points[hover] : null;

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
          const i = Math.floor((e.clientX - rect.left - PAD.left) / step);
          setHover(i >= 0 && i < points.length ? i : null);
        }}
      >
        {ticks.map((t) => (
          <g key={t}>
            <line x1={PAD.left} x2={width - PAD.right} y1={y(t)} y2={y(t)} stroke="var(--color-line)" />
            <text x={PAD.left - 6} y={y(t) + 4} textAnchor="end" className="fill-ink-3 text-[10px] num">
              {compact(t)}
            </text>
          </g>
        ))}
        {points.map((p, i) => {
          const x = PAD.left + i * step;
          const ok = p.tx - p.failed;
          const yOk = y(ok);
          const yFail = y(p.tx);
          return (
            <g key={p.t} opacity={hover === null || hover === i ? 1 : 0.55}>
              {ok > 0 && <rect x={x} y={yOk} width={barW} height={y(0) - yOk} fill="var(--color-series-ok)" />}
              {p.failed > 0 && (
                <rect
                  x={x}
                  y={yFail}
                  width={barW}
                  height={Math.max(1, yOk - yFail - (ok > 0 ? 1 : 0))}
                  fill="var(--color-series-fail)"
                />
              )}
            </g>
          );
        })}
        {points.map((p, i) =>
          i % labelEvery === 0 ? (
            <text key={p.t} x={PAD.left + i * step} y={H - 6} className="fill-ink-3 text-[10px] num">
              {timeLabel(p.t)}
            </text>
          ) : null,
        )}
      </svg>
      {hp && hover !== null && (
        <Tooltip x={PAD.left + hover * step} width={width}>
          <div className="text-ink-3 mb-1 num">{new Date(hp.t * 1000).toLocaleTimeString(undefined, { hour12: false })}</div>
          <div className="flex justify-between"><span className="num font-semibold">{hp.tx}</span><span className="text-ink-2">transactions</span></div>
          <div className="flex justify-between"><span className="num font-semibold">{hp.failed}</span><span className="text-ink-2">failed</span></div>
        </Tooltip>
      )}
      <div className="flex gap-4 px-1 pt-1 text-xs text-ink-2">
        <span className="inline-flex items-center gap-1.5"><span className="size-2.5 rounded-sm bg-series-ok" aria-hidden />Succeeded</span>
        <span className="inline-flex items-center gap-1.5"><span className="size-2.5 rounded-sm bg-series-fail" aria-hidden />Failed</span>
      </div>
    </div>
  );
}

/** A single series over time, with a dashed baseline reference. */
export function LineChart({
  points,
  value,
  label,
  baseline,
  format = (v: number) => num(v),
}: {
  points: SeriesPoint[];
  value: (p: SeriesPoint) => number | null;
  label: string;
  baseline?: number;
  format?: (v: number) => string;
}) {
  const [ref, width] = useWidth();
  const [hover, setHover] = useState<number | null>(null);
  const innerW = width - PAD.left - PAD.right;
  const innerH = H - PAD.top - PAD.bottom;
  const vals = points.map(value);
  const max = niceMax(Math.max(1, baseline ?? 0, ...vals.filter((v): v is number => v !== null)));
  const x = (i: number) => PAD.left + (points.length <= 1 ? 0 : (i / (points.length - 1)) * innerW);
  const y = (v: number) => PAD.top + innerH - (v / max) * innerH;

  let d = '';
  vals.forEach((v, i) => {
    if (v === null) return;
    const prev = i > 0 ? vals[i - 1] : null;
    d += `${prev === null ? 'M' : 'L'}${x(i).toFixed(1)},${y(v).toFixed(1)}`;
  });
  const hv = hover !== null ? vals[hover] : null;
  const labelEvery = Math.max(1, Math.round(points.length / 5));

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
          const rel = (e.clientX - rect.left - PAD.left) / innerW;
          const i = Math.round(rel * (points.length - 1));
          setHover(i >= 0 && i < points.length ? i : null);
        }}
      >
        {[0, max / 2, max].map((t) => (
          <g key={t}>
            <line x1={PAD.left} x2={width - PAD.right} y1={y(t)} y2={y(t)} stroke="var(--color-line)" />
            <text x={PAD.left - 6} y={y(t) + 4} textAnchor="end" className="fill-ink-3 text-[10px] num">
              {compact(t)}
            </text>
          </g>
        ))}
        {baseline !== undefined && baseline > 0 && (
          <g>
            <line x1={PAD.left} x2={width - PAD.right} y1={y(baseline)} y2={y(baseline)} stroke="var(--color-ink-3)" strokeDasharray="4 4" />
            <text x={width - PAD.right} y={y(baseline) - 4} textAnchor="end" className="fill-ink-3 text-[10px]">
              baseline
            </text>
          </g>
        )}
        <path d={d} fill="none" stroke="var(--color-series-1)" strokeWidth={2} strokeLinejoin="round" />
        {hover !== null && hv !== null && (
          <g>
            <line x1={x(hover)} x2={x(hover)} y1={PAD.top} y2={PAD.top + innerH} stroke="var(--color-line-strong)" />
            <circle cx={x(hover)} cy={y(hv)} r={4} fill="var(--color-series-1)" stroke="var(--color-surface)" strokeWidth={2} />
          </g>
        )}
        {points.map((p, i) =>
          i % labelEvery === 0 ? (
            <text key={p.t} x={x(i)} y={H - 6} className="fill-ink-3 text-[10px] num">
              {timeLabel(p.t)}
            </text>
          ) : null,
        )}
      </svg>
      {hover !== null && points[hover] && (
        <Tooltip x={x(hover)} width={width}>
          <div className="text-ink-3 mb-1 num">
            {new Date(points[hover].t * 1000).toLocaleTimeString(undefined, { hour12: false })}
          </div>
          <div className="flex justify-between gap-2">
            <span className="num font-semibold">{hv === null ? '—' : format(hv)}</span>
            <span className="text-ink-2 truncate">{label}</span>
          </div>
        </Tooltip>
      )}
    </div>
  );
}

/** Horizontal share bars for a ranked breakdown. */
export function ShareBar({ share, tone = 'fail' }: { share: number; tone?: 'fail' | 'accent' }) {
  return (
    <div className="h-2 w-full rounded-full bg-sunken overflow-hidden" aria-hidden>
      <div
        className={`h-full rounded-full ${tone === 'fail' ? 'bg-series-fail' : 'bg-series-1'}`}
        style={{ width: `${Math.max(2, Math.min(100, share * 100))}%` }}
      />
    </div>
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
  const labelEvery = Math.max(1, Math.round(times.length / 5));

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
            <text x={PAD.left - 6} y={y(t) + 4} textAnchor="end" className="fill-ink-3 text-[10px] num">
              {format(t)}
            </text>
          </g>
        ))}
        {markers
          .filter((m) => m.t >= t0 && m.t <= t1)
          .map((m, i) => (
            <g key={m.label}>
              <line x1={x(m.t)} x2={x(m.t)} y1={top - 4} y2={top + innerH} stroke="var(--color-ink-3)" strokeDasharray="2 3" />
              <text
                x={x(m.t)}
                y={top - 8 - (i % 2) * 0}
                textAnchor={x(m.t) > width - 80 ? 'end' : 'start'}
                className="fill-ink-2 text-[10px]"
              >
                {m.label}
              </text>
            </g>
          ))}
        {refs.map((r) => (
          <g key={r.label}>
            <line x1={PAD.left} x2={width - PAD.right} y1={y(r.value)} y2={y(r.value)} stroke="var(--color-ink-3)" strokeDasharray="5 4" />
            <text x={width - PAD.right - 2} y={y(r.value) - 4} textAnchor="end" className="fill-ink-3 text-[10px]">
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
        {times.map((t, i) =>
          i % labelEvery === 0 ? (
            <text key={t} x={x(t)} y={H - 6} className="fill-ink-3 text-[10px] num">
              {timeLabel(t)}
            </text>
          ) : null,
        )}
      </svg>
      {hover !== null && (
        <div
          className="pointer-events-none absolute top-6 z-10 w-56 rounded-md border border-line bg-surface px-2.5 py-2 text-xs shadow-sm"
          style={{ left: Math.min(Math.max(x(times[hover]) - 112, 0), width - 230) }}
        >
          <div className="text-ink-3 mb-1 num">{new Date(times[hover] * 1000).toLocaleTimeString(undefined, { hour12: false })}</div>
          {series.map((s) => (
            <div key={s.label} className="flex items-center gap-2">
              <span className="size-2 rounded-sm shrink-0" style={{ background: s.color }} aria-hidden />
              <span className="num font-semibold">{s.values[hover] === null ? '—' : format(s.values[hover]!)}</span>
              <span className="text-ink-2 truncate">{s.label}</span>
            </div>
          ))}
        </div>
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
