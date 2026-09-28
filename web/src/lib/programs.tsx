import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from 'react';
import { api } from './api';
import { useLive } from './live';
import type { ProgramSnapshot, StreamHealth } from './types';

interface Ctx {
  programs: ProgramSnapshot[];
  stream: StreamHealth | null;
  loading: boolean;
  error: string | null;
  reload: () => Promise<void>;
}

const ProgramsContext = createContext<Ctx>({
  programs: [],
  stream: null,
  loading: true,
  error: null,
  reload: async () => {},
});

/** Program snapshots + stream health, loaded once and kept live over SSE. */
export function ProgramsProvider({ children }: { children: ReactNode }) {
  const [programs, setPrograms] = useState<ProgramSnapshot[]>([]);
  const [stream, setStream] = useState<StreamHealth | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const s = await api<{ programs: ProgramSnapshot[]; stream: StreamHealth }>('/api/status');
      setPrograms(s.programs);
      setStream(s.stream);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  useLive((e) => {
    if (e.type === 'stream') setStream(e.health);
    if (e.type === 'metrics') {
      setPrograms((prev) => {
        const i = prev.findIndex((p) => p.program_id === e.program_id);
        if (i === -1) return prev;
        const next = prev.slice();
        next[i] = e.snapshot;
        return next;
      });
    }
  });

  return (
    <ProgramsContext.Provider value={{ programs, stream, loading, error, reload }}>
      {children}
    </ProgramsContext.Provider>
  );
}

export const usePrograms = () => useContext(ProgramsContext);
