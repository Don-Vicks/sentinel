import { useEffect, useRef, useState } from 'react';
import type {
  AlertExecution,
  Incident,
  ProgramSnapshot,
  StreamHealth,
  TxSummary,
} from './types';

export type LiveEvent =
  | { type: 'transactions'; program_id: string; items: TxSummary[] }
  | { type: 'metrics'; program_id: string; snapshot: ProgramSnapshot }
  | { type: 'incident'; change: 'opened' | 'updated' | 'resolved'; incident: Incident }
  | { type: 'alert'; execution: AlertExecution }
  | { type: 'stream'; health: StreamHealth };

type Listener = (e: LiveEvent) => void;

/** One EventSource for the whole app, fanned out to subscribers. */
class LiveBus {
  private source: EventSource | null = null;
  private listeners = new Set<Listener>();
  private statusListeners = new Set<(open: boolean) => void>();
  open = false;

  subscribe(fn: Listener) {
    this.listeners.add(fn);
    this.connect();
    return () => {
      this.listeners.delete(fn);
    };
  }

  onStatus(fn: (open: boolean) => void) {
    this.statusListeners.add(fn);
    fn(this.open);
    return () => {
      this.statusListeners.delete(fn);
    };
  }

  private setOpen(open: boolean) {
    this.open = open;
    this.statusListeners.forEach((f) => f(open));
  }

  private connect() {
    if (this.source) return;
    const es = new EventSource('/api/stream');
    this.source = es;
    const handle = (msg: MessageEvent) => {
      try {
        const ev = JSON.parse(msg.data) as LiveEvent;
        this.listeners.forEach((l) => l(ev));
      } catch {
        /* ignore malformed frames */
      }
    };
    for (const name of ['transactions', 'metrics', 'incident', 'alert', 'stream']) {
      es.addEventListener(name, handle as EventListener);
    }
    es.onopen = () => this.setOpen(true);
    // EventSource reconnects on its own; just reflect the state.
    es.onerror = () => this.setOpen(false);
  }
}

export const live = new LiveBus();

export function useLive(fn: Listener) {
  const ref = useRef(fn);
  ref.current = fn;
  useEffect(() => live.subscribe((e) => ref.current(e)), []);
}

export function useLiveStatus() {
  const [open, setOpen] = useState(live.open);
  useEffect(() => live.onStatus(setOpen), []);
  return open;
}

export function useStreamHealth(initial?: StreamHealth | null) {
  const [health, setHealth] = useState<StreamHealth | null>(initial ?? null);
  useLive((e) => {
    if (e.type === 'stream') setHealth(e.health);
  });
  return health;
}
