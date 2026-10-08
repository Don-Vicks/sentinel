import { useFetch } from '../lib/api';
import { useLive } from '../lib/live';
import type { Incident, Timeline } from '../lib/types';
import { compact, pct } from '../lib/format';
import { TimelineChart, type Marker, type Series } from './charts';
import { Panel, Skeleton } from './ui';

const SERIES = ['var(--color-series-1)', 'var(--color-series-2)', 'var(--color-series-3)'];

/** Picks the metric that best shows why this incident fired. */
function primary(inc: Incident, tl: Timeline) {
  const pts = tl.points;
  const perBucket = tl.bucket_secs;
  switch (inc.kind) {
    case 'error_spike': {
      // Detector numbers are per 60s window; the chart is per bucket.
      const scale = perBucket / 60;
      return {
        title: `${tl.fingerprints[0]?.label ?? 'Error'} per ${perBucket}s`,
        series: [{ label: 'occurrences', values: pts.map((p) => p.errors[0] ?? 0), color: SERIES[0] }],
        refs: [
          ...(inc.baseline ? [{ value: inc.baseline * scale, label: 'normal' }] : []),
          ...(inc.threshold ? [{ value: inc.threshold * scale, label: 'threshold' }] : []),
        ],
        format: (v: number) => compact(v),
      };
    }
    case 'activity_spike':
    case 'activity_drop':
      return {
        title: 'Transactions per second',
        series: [{ label: 'TPS', values: pts.map((p) => p.tps), color: SERIES[0] }],
        refs: [
          ...(inc.baseline ? [{ value: inc.baseline, label: 'normal' }] : []),
          ...(inc.threshold && inc.kind === 'activity_spike' ? [{ value: inc.threshold, label: 'threshold' }] : []),
        ],
        format: (v: number) => compact(v),
      };
    case 'compute_spike':
      return {
        title: 'Average compute units per transaction',
        series: [{ label: 'avg CU', values: pts.map((p) => (p.tx ? p.avg_cu : null)), color: SERIES[0] }],
        refs: [
          ...(inc.baseline ? [{ value: inc.baseline, label: 'normal' }] : []),
          ...(inc.threshold ? [{ value: inc.threshold, label: 'threshold' }] : []),
        ],
        format: (v: number) => compact(v),
      };
    case 'rule_triggered': {
      // Draw the metric the rule watches, with the rule's threshold.
      const ref = inc.threshold ? [{ value: inc.threshold, label: 'threshold' }] : [];
      if (inc.metric === 'tps') {
        return {
          title: 'Transactions per second',
          series: [{ label: 'TPS', values: pts.map((p) => p.tps), color: SERIES[0] }],
          refs: ref,
          format: (v: number) => compact(v),
        };
      }
      if (inc.metric === 'avg_compute' || inc.metric === 'max_compute') {
        return {
          title: 'Average compute units per transaction',
          series: [{ label: 'avg CU', values: pts.map((p) => (p.tx ? p.avg_cu : null)), color: SERIES[0] }],
          refs: inc.metric === 'avg_compute' ? ref : [],
          format: (v: number) => compact(v),
        };
      }
      return {
        title: 'Failure rate',
        series: [{ label: 'failure rate', values: pts.map((p) => (p.tx ? p.failure_rate : null)), color: SERIES[0] }],
        refs: inc.metric === 'failure_rate' ? ref : [],
        format: (v: number) => pct(v, 0),
      };
    }
    case 'large_transfer':
    case 'vault_drain':
    case 'dependency_change':
    case 'bot_activity':
    case 'authority_change':
      return {
        title: 'Transactions per second (context)',
        series: [{ label: 'TPS', values: pts.map((p) => p.tps), color: SERIES[0] }],
        refs: [],
        format: (v: number) => compact(v),
      };
    default:
      return {
        title: 'Failure rate',
        series: [{ label: 'failure rate', values: pts.map((p) => (p.tx ? p.failure_rate : null)), color: SERIES[0] }],
        refs:
          inc.kind === 'failure_spike'
            ? [
                ...(inc.baseline !== null ? [{ value: inc.baseline, label: 'normal' }] : []),
                ...(inc.threshold ? [{ value: inc.threshold, label: 'threshold' }] : []),
              ]
            : [],
        format: (v: number) => pct(v, 0),
      };
  }
}

export function IncidentTimeline({ incident }: { incident: Incident }) {
  const { data, loading, reload } = useFetch<Timeline | null>(`/api/incidents/${incident.id}/timeline`);
  // Extend the chart while the incident is ongoing.
  useLive((e) => {
    if (e.type === 'metrics' && e.program_id === incident.program_id && incident.status !== 'resolved') {
      const last = data?.points[data.points.length - 1]?.t ?? 0;
      if (Date.now() / 1000 - last > (data?.bucket_secs ?? 10) + 1) reload();
    }
  });

  if (loading && !data) return <Skeleton className="h-56" />;
  if (!data || data.points.length < 2) {
    return (
      <Panel title="Timeline">
        <p className="p-4 text-sm text-ink-3">
          No metric history for this incident. History is kept for incidents resolved while Sentinel was running.
        </p>
      </Panel>
    );
  }

  const times = data.points.map((p) => p.t);
  const markers: Marker[] = [
    ...(data.onset !== null ? [{ t: data.onset, label: 'onset' }] : []),
    { t: data.detected, label: 'detected' },
    ...(data.resolved !== null ? [{ t: data.resolved, label: 'resolved' }] : []),
  ];
  const main = primary(incident, data);
  const showErrors = data.fingerprints.length > 0 && incident.kind !== 'error_spike';
  const errorSeries: Series[] = data.fingerprints.map((f, i) => ({
    label: f.label,
    values: data.points.map((p) => p.errors[i] ?? 0),
    color: SERIES[i],
  }));

  return (
    <div className={`grid gap-4 ${showErrors ? 'xl:grid-cols-2' : ''}`}>
      <Panel title={main.title}>
        <div className="p-3">
          <TimelineChart
            times={times}
            series={main.series}
            refs={main.refs}
            markers={markers}
            format={main.format}
            label={`${main.title} around incident ${incident.id}`}
          />
        </div>
      </Panel>
      {showErrors && (
        <Panel title={`Failures by error type per ${data.bucket_secs}s`}>
          <div className="p-3">
            <TimelineChart
              times={times}
              series={errorSeries}
              markers={markers}
              label="Failures by error type around the incident"
            />
          </div>
        </Panel>
      )}
    </div>
  );
}
