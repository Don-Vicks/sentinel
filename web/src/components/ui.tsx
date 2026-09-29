import { useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import {
  AlertOctagon,
  AlertTriangle,
  Check,
  CheckCircle2,
  CircleDashed,
  Copy,
  Info,
  Loader2,
  Search,
} from 'lucide-react';
import type { Health, IncidentStatus, Severity } from '../lib/types';
import { short } from '../lib/format';

const SEVERITY: Record<Severity, { cls: string; label: string; Icon: typeof Info }> = {
  critical: { cls: 'bg-crit-soft text-crit', label: 'Critical', Icon: AlertOctagon },
  high: { cls: 'bg-serious-soft text-serious', label: 'High', Icon: AlertTriangle },
  medium: { cls: 'bg-warn-soft text-warn', label: 'Medium', Icon: AlertTriangle },
  low: { cls: 'bg-sunken text-ink-2', label: 'Low', Icon: Info },
};

export function SeverityBadge({ severity }: { severity: Severity }) {
  const s = SEVERITY[severity];
  return (
    <span className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-xs font-medium ${s.cls}`}>
      <s.Icon className="size-3.5" aria-hidden />
      {s.label}
    </span>
  );
}

const STATUS: Record<IncidentStatus, { cls: string; label: string; Icon: typeof Info }> = {
  open: { cls: 'text-crit', label: 'Open', Icon: AlertOctagon },
  investigating: { cls: 'text-warn', label: 'Investigating', Icon: Search },
  resolved: { cls: 'text-good', label: 'Resolved', Icon: CheckCircle2 },
};

export function StatusBadge({ status }: { status: IncidentStatus }) {
  const s = STATUS[status];
  return (
    <span className={`inline-flex items-center gap-1 text-xs font-medium ${s.cls}`}>
      <s.Icon className="size-3.5" aria-hidden />
      {s.label}
    </span>
  );
}

const HEALTH: Record<Health, { dot: string; text: string; label: string }> = {
  healthy: { dot: 'bg-good', text: 'text-good', label: 'Healthy' },
  degraded: { dot: 'bg-warn', text: 'text-warn', label: 'Degraded' },
  critical: { dot: 'bg-crit', text: 'text-crit', label: 'Incident' },
  warming_up: { dot: 'bg-accent', text: 'text-accent', label: 'Learning baseline' },
  idle: { dot: 'bg-ink-3', text: 'text-ink-3', label: 'Waiting for traffic' },
};

export function HealthDot({ health, withLabel = false }: { health: Health; withLabel?: boolean }) {
  const h = HEALTH[health];
  return (
    <span className={`inline-flex items-center gap-1.5 text-xs font-medium ${h.text}`}>
      <span className={`size-2 rounded-full ${h.dot} ${health !== 'healthy' && health !== 'idle' ? 'pulse-dot' : ''}`} aria-hidden />
      {withLabel ? h.label : <span className="sr-only">{h.label}</span>}
    </span>
  );
}

export function Stat({
  label,
  value,
  sub,
  tone,
  spark,
  badge,
}: {
  label: string;
  value: ReactNode;
  sub?: ReactNode;
  tone?: 'crit' | 'warn' | 'good';
  /** Small trend under the number. */
  spark?: ReactNode;
  /** Comparison chip, e.g. "3.1× normal". */
  badge?: { text: string; tone: 'crit' | 'warn' | 'muted' } | null;
}) {
  const toneCls = tone === 'crit' ? 'text-crit' : tone === 'warn' ? 'text-warn' : tone === 'good' ? 'text-good' : 'text-ink';
  const badgeCls =
    badge?.tone === 'crit' ? 'bg-crit-soft text-crit' : badge?.tone === 'warn' ? 'bg-warn-soft text-warn' : 'bg-sunken text-ink-3';
  return (
    <div className="panel px-4 pt-3 pb-3 min-w-0 flex flex-col">
      <span className="text-xs text-ink-3 truncate">{label}</span>
      <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1">
        <span className={`num text-[22px] leading-7 font-semibold tracking-tight truncate ${toneCls}`}>{value}</span>
        {badge && <span className={`rounded px-1.5 py-px num text-[11px] font-medium whitespace-nowrap ${badgeCls}`}>{badge.text}</span>}
      </div>
      {sub && <div className="text-xs text-ink-3 mt-0.5 truncate" title={typeof sub === 'string' ? sub : undefined}>{sub}</div>}
      {spark && <div className="mt-2 -mx-1">{spark}</div>}
    </div>
  );
}

/** Title row with breadcrumbs and actions. */
export function PageHeader({
  crumbs = [],
  title,
  meta,
  actions,
}: {
  crumbs?: { label: string; to: string }[];
  title: ReactNode;
  meta?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <header className="flex flex-wrap items-end justify-between gap-3">
      <div className="min-w-0">
        {crumbs.length > 0 && (
          <nav aria-label="Breadcrumb" className="mb-1 flex items-center gap-1.5 text-xs text-ink-3">
            {crumbs.map((c) => (
              <span key={c.to} className="inline-flex items-center gap-1.5">
                <Link to={c.to} className="hover:text-ink">{c.label}</Link>
                <span aria-hidden>/</span>
              </span>
            ))}
          </nav>
        )}
        <h1 className="text-xl font-semibold tracking-tight text-ink">{title}</h1>
        {meta && <div className="mt-1 text-sm text-ink-2">{meta}</div>}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </header>
  );
}

export function CopyButton({ text, label = 'Copy' }: { text: string; label?: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      type="button"
      className="inline-flex size-7 items-center justify-center rounded text-ink-3 hover:text-ink hover:bg-sunken"
      aria-label={label}
      onClick={() => {
        navigator.clipboard?.writeText(text);
        setDone(true);
        setTimeout(() => setDone(false), 1200);
      }}
    >
      {done ? <Check className="size-3.5" aria-hidden /> : <Copy className="size-3.5" aria-hidden />}
    </button>
  );
}

export function Address({ value, n = 4, copy = true }: { value: string; n?: number; copy?: boolean }) {
  return (
    <span className="inline-flex items-center gap-0.5 font-mono text-xs" title={value}>
      {short(value, n)}
      {copy && <CopyButton text={value} label={`Copy ${value}`} />}
    </span>
  );
}

export function Empty({ title, children, icon }: { title: string; children?: ReactNode; icon?: ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center text-center px-6 py-10 gap-2">
      <div className="text-ink-3">{icon ?? <CircleDashed className="size-6" aria-hidden />}</div>
      <div className="font-medium text-ink">{title}</div>
      {children && <div className="text-ink-3 max-w-sm text-sm">{children}</div>}
    </div>
  );
}

export function ErrorState({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <div className="panel flex flex-col items-center gap-3 px-6 py-10 text-center" role="alert">
      <AlertTriangle className="size-6 text-crit" aria-hidden />
      <div className="text-ink">{message}</div>
      {onRetry && (
        <button className="btn" onClick={onRetry}>
          Try again
        </button>
      )}
    </div>
  );
}

export function Skeleton({ className = '' }: { className?: string }) {
  return <div className={`animate-pulse rounded bg-sunken ${className}`} aria-hidden />;
}

export function PageSkeleton() {
  return (
    <div className="space-y-4" aria-busy="true" aria-label="Loading">
      <Skeleton className="h-8 w-64" />
      <div className="grid grid-cols-2 lg:grid-cols-5 gap-3">
        {Array.from({ length: 5 }).map((_, i) => (
          <Skeleton key={i} className="h-20" />
        ))}
      </div>
      <Skeleton className="h-64" />
    </div>
  );
}

export function Spinner() {
  return <Loader2 className="size-4 animate-spin" aria-hidden />;
}

export function Panel({
  title,
  action,
  children,
  className = '',
}: {
  title: ReactNode;
  action?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={`panel min-w-0 ${className}`}>
      <div className="panel-head">
        <h2 className="panel-title">{title}</h2>
        {action}
      </div>
      {children}
    </section>
  );
}
