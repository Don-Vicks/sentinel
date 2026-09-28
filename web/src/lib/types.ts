export type Severity = 'low' | 'medium' | 'high' | 'critical';
export type IncidentStatus = 'open' | 'investigating' | 'resolved';
export type IncidentKind =
  | 'failure_spike'
  | 'activity_spike'
  | 'activity_drop'
  | 'compute_spike'
  | 'large_transfer'
  | 'rule_triggered';
export type Health = 'healthy' | 'degraded' | 'critical' | 'warming_up' | 'idle';

export interface LargestTransfer {
  amount: number;
  symbol: string;
  usd: number | null;
}

export interface TxSummary {
  signature: string;
  slot: number;
  received_at: string;
  success: boolean;
  error: string | null;
  fingerprint: string | null;
  compute_units: number | null;
  fee: number;
  fee_payer: string | null;
  instructions: string[];
  transfers: number;
  largest_transfer: LargestTransfer | null;
}

export interface SeriesPoint {
  t: number;
  tx: number;
  failed: number;
  avg_cu: number;
  max_cu: number;
}

export interface ErrorCount {
  key: string;
  program_id: string;
  program_name: string;
  instruction: string | null;
  error: string;
  code: number | null;
  count: number;
  share: number;
}

export interface ProgramSnapshot {
  program_id: string;
  label: string;
  health: Health;
  at: string;
  warmup_remaining_secs: number;
  total_tx: number;
  total_failed: number;
  tx_60s: number;
  failed_60s: number;
  failure_rate_60s: number;
  tps_10s: number;
  tps_60s: number;
  avg_cu_60s: number;
  max_cu_60s: number;
  unique_signers_60s: number;
  fees_60s_sol: number;
  baseline_failure_rate: number;
  baseline_tps: number;
  baseline_avg_cu: number;
  top_errors: ErrorCount[];
  open_incidents: number;
  last_tx_at: string | null;
  point: SeriesPoint | null;
}

export interface FingerprintEvidence {
  key: string;
  program_id: string | null;
  program_name: string | null;
  instruction: string | null;
  error: string | null;
  code: number | null;
  count: number;
  share: number;
  samples: string[];
}

export interface Incident {
  id: number;
  program_id: string;
  kind: IncidentKind;
  severity: Severity;
  status: IncidentStatus;
  title: string;
  summary: string;
  explanation: string;
  source: string;
  metric: string | null;
  observed: number | null;
  peak: number | null;
  baseline: number | null;
  threshold: number | null;
  onset_at: string | null;
  detected_at: string;
  updated_at: string;
  resolved_at: string | null;
  detection_latency_ms: number | null;
  affected_count: number;
  affected_wallets: number;
  evidence: { fingerprints?: FingerprintEvidence[]; fingerprinted?: number };
}

export interface StreamHealth {
  connected: boolean;
  transactions_received: number;
  ingest_tps: number;
  last_slot: number;
  slot_lag: number;
  last_transaction_age_ms: number | null;
  dropped: number;
  programs_streamed: string[];
  uptime_secs: number;
  pricing: {
    enabled: boolean;
    priced_mints: number;
    tracked_mints: number;
    last_refresh: string | null;
    last_error: string | null;
  };
}

export type Metric =
  | 'failure_rate'
  | 'failed_count'
  | 'tps'
  | 'tx_count'
  | 'avg_compute'
  | 'max_compute'
  | 'unique_signers';

export type Condition =
  | { type: 'metric'; metric: Metric; op: '>' | '>=' | '<' | '<='; value: number; window_secs: number }
  | { type: 'transfer'; mint: string | null; min_amount: number }
  | { type: 'transfer_usd'; min_usd: number }
  | { type: 'incident'; kinds: IncidentKind[]; min_severity: Severity };

export interface AlertRule {
  id: number;
  name: string;
  program_id: string | null;
  condition: Condition;
  create_incident: boolean;
  severity: Severity;
  webhook_url: string | null;
  enabled: boolean;
  cooldown_secs: number;
  created_at: string;
  last_fired_at: string | null;
}

export interface AlertExecution {
  id: number;
  rule_id: number;
  rule_name: string;
  program_id: string;
  fired_at: string;
  message: string;
  incident_id: number | null;
  webhook_url: string | null;
  delivered: boolean;
  status_code: number | null;
  error: string | null;
  latency_ms: number | null;
  attempts: number;
}

export interface MonitoredProgram {
  program_id: string;
  label: string;
  created_at: string;
}

// --- Transaction (Vortex model) -------------------------------------------

export interface AccountRef {
  pubkey: string;
  signer: boolean;
  writable: boolean;
  from_lookup_table: boolean;
  pre_lamports: number;
  post_lamports: number;
}

export interface Instruction {
  path: string;
  top_index: number;
  inner_index: number | null;
  stack_height: number;
  program_id: string;
  program_name: string | null;
  accounts: string[];
  data: string;
  name: string | null;
  parsed: unknown;
}

export interface VortexTransaction {
  signature: string;
  slot: number;
  index: number;
  received_at: string;
  success: boolean;
  error: {
    message: string;
    instruction_index: number | null;
    custom_code: number | null;
    program_id: string | null;
    name: string | null;
    class: string;
  } | null;
  fee: number;
  compute_units: number | null;
  compute_unit_limit: number | null;
  compute_unit_price: number | null;
  accounts: AccountRef[];
  instructions: Instruction[];
  logs: string[];
  logs_truncated: boolean;
}

export interface Party {
  address: string;
  label: string;
  roles: string[];
  owner_program: string | null;
  owner_program_name: string | null;
}

export interface Flow {
  from: string;
  to: string;
  amount: number;
  symbol: string;
  mint: string | null;
  kind: 'sol' | 'token' | 'mint' | 'burn';
  instruction: string;
  from_account: string | null;
  to_account: string | null;
  usd: number | null;
}

export interface CallNode {
  index: number;
  parent: number | null;
  depth: number;
  program_id: string;
  program_name: string | null;
  instruction: string | null;
  compute_consumed: number | null;
  success: boolean | null;
  failure: string | null;
  logs: string[];
}

export interface Trace {
  parties: Party[];
  flows: Flow[];
  balance_changes: { owner: string; symbol: string; mint: string | null; delta: number; usd: number | null }[];
  state_changes: {
    account: string;
    owner: string | null;
    kind: 'token' | 'lamports' | 'data';
    symbol: string;
    before: number;
    after: number;
    delta: number;
    note: string | null;
  }[];
  call_tree: CallNode[];
  narrative: string[];
  reverted: boolean;
}

export interface TransactionDetail {
  transaction: VortexTransaction;
  trace: Trace;
  monitored_programs: string[];
  program_labels: Record<string, string>;
  incidents: number[];
}
