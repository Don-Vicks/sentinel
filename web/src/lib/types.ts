export type Severity = 'low' | 'medium' | 'high' | 'critical';
export type IncidentStatus = 'open' | 'investigating' | 'resolved';
export type IncidentKind =
  | 'failure_spike'
  | 'activity_spike'
  | 'activity_drop'
  | 'compute_spike'
  | 'large_transfer'
  | 'rule_triggered'
  | 'error_spike'
  | 'authority_change'
  | 'vault_drain';
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
  /** Raised by the monitored program itself rather than another program in the same transactions. */
  own?: boolean;
  program_id: string;
  program_name: string;
  instruction: string | null;
  error: string;
  code: number | null;
  count: number;
  share: number;
}

export interface InstructionStat {
  name: string;
  tx: number;
  failed: number;
  failure_rate: number;
  avg_cu: number;
  share: number;
}

export interface Timeline {
  bucket_secs: number;
  start: number;
  end: number;
  onset: number | null;
  detected: number;
  resolved: number | null;
  fingerprints: { key: string; label: string }[];
  points: {
    t: number;
    tx: number;
    failed: number;
    failure_rate: number;
    tps: number;
    avg_cu: number;
    errors: number[];
  }[];
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
  instructions: InstructionStat[];
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
  evidence: {
    fingerprints?: FingerprintEvidence[];
    fingerprinted?: number;
    /** Set on authority_change incidents: what the upgradeable loader was asked to do. */
    authority?: AuthorityEvidence;
    /** Set when the incident began soon after the program was upgraded. */
    deploy?: DeployEvidence;
    /** Set on vault_drain incidents. */
    vault?: VaultEvidence;
  };
}

export interface VaultEvidence {
  account: string;
  mint: string | null;
  symbol: string;
  outflow: number;
  balance_after: number;
  pct: number;
  usd: number | null;
}

export interface VaultStatus {
  vaults: {
    account: string;
    mint: string | null;
    symbol: string | null;
    balance: number | null;
    balance_usd: number | null;
    net_window: number;
    seen: boolean;
  }[];
  candidates: { account: string; mint: string | null; symbol: string; transactions: number }[];
  window_secs: number;
}

export interface AuthorityEvidence {
  action: 'upgrade' | 'set_authority' | 'close' | 'extend';
  program_id: string;
  programdata: string;
  authority: string | null;
  signature: string;
  slot: number;
  path: string;
  buffer?: string | null;
  new_authority?: string | null;
  recipient?: string | null;
  additional_bytes?: number;
  /** The Squads multisig call that executed it. */
  via?: { program: string; program_id: string; multisig: string; executor: string | null; instruction: string | null } | null;
  /** What that multisig requires, once read from chain. */
  multisig?: MultisigInfo | null;
}

export interface MultisigInfo {
  threshold: number;
  members: number;
  time_lock: number;
}

export interface DeployEvidence {
  incident_id: number;
  signature: string;
  at: string;
  slot: number;
  authority: string | null;
  seconds_before: number;
  note: string;
}

export interface Posture {
  program_id: string;
  programdata: string | null;
  upgradeable: boolean;
  authority: string | null;
  authority_kind: 'none' | 'single_key' | 'program_controlled';
  last_deployed_slot: number | null;
  code_bytes: number | null;
  /** A Squads multisig seen executing upgrades with this authority. */
  controller?: { name: string; multisig: string; requires: MultisigInfo | null } | null;
  risks: { level: Severity; text: string }[];
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
  /** Which Solami transport carries the stream; mirage means gRPC failed over. */
  transport?: 'grpc' | 'mirage';
  /** The chain tip stopped advancing: no data is arriving, so detectors are paused. */
  stalled?: boolean;
  /** Slots between the real chain tip (over RPC) and the newest slot the stream delivered. */
  behind_chain_slots?: number | null;
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
  | { type: 'incident'; kinds: IncidentKind[]; min_severity: Severity }
  | { type: 'system'; kinds: ('feed_stalled' | 'rpc_failing')[] }
  | {
      type: 'instruction';
      name: string;
      program_id?: string | null;
      filters: ArgFilter[];
      match_mode: 'all' | 'any';
      success_only: boolean;
      first_seen_signer: boolean;
    };

export type FilterOp = 'eq' | 'ne' | 'gt' | 'gte' | 'lt' | 'lte' | 'contains';

export interface ArgFilter {
  /** `args.amount`, `accounts.authority`, `signer` or `instruction`. */
  path: string;
  op: FilterOp;
  value: string | number | boolean;
}

export interface InstructionSchema {
  name: string;
  accounts: string[];
  args: { path: string; type: string }[];
}

export type ChannelType = 'slack' | 'telegram' | 'pagerduty' | 'discord' | 'webhook';

/** Secrets (bot token, routing key, webhook paths) come back masked and are kept when sent back unchanged. */
export type Channel = (
  | { type: 'slack' | 'discord' | 'webhook'; url: string }
  | { type: 'telegram'; bot_token: string; chat_id: string }
  | { type: 'pagerduty'; routing_key: string }
) & { min_severity?: Severity | null };

export interface AlertRule {
  id: number;
  name: string;
  program_id: string | null;
  condition: Condition;
  create_incident: boolean;
  severity: Severity;
  webhook_url: string | null;
  channels: Channel[];
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
  /** Secret-free description of where it went. */
  webhook_url: string | null;
  channel?: ChannelType | null;
  event?: 'opened' | 'updated' | 'resolved' | 'test' | null;
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
  decoded: DecodedView[];
  error_detail: { code: number; name: string; message: string | null } | null;
}

export interface DecodedView {
  path: string;
  program_id: string;
  idl_name: string | null;
  name: string;
  args: Record<string, unknown>;
  accounts: { name: string; pubkey: string }[];
  partial: boolean;
}

export interface TransactionDetail {
  transaction: VortexTransaction;
  trace: Trace;
  monitored_programs: string[];
  program_labels: Record<string, string>;
  incidents: number[];
  beam?: { landing: BeamLanding | null; tip: { lamports: number; address: string } | null };
}

/** How Solami Beam delivered a transaction it carried. */
export interface BeamLanding {
  is_landed: boolean;
  landed_via_jito: boolean;
  rebroadcasted: boolean;
  region: string | null;
  tip_lamports: number | null;
  tip_address: string | null;
  first_seen_ms: number | null;
  forwarded_ms: number | null;
}

export interface Candidate {
  program_id: string;
  name: string | null;
  infra: boolean;
}

export interface Resolution {
  kind: 'program' | 'authority' | 'transaction' | 'account';
  subject: string;
  headline: string;
  programs: Candidate[];
  /** Programs still being resolved in the background; the finder asks again. */
  pending?: number;
}

/** What each Solami product is doing for this instance. */
export interface SolamiStatus {
  grpc: { connected: boolean; transport: 'grpc' | 'mirage'; tx_per_sec: number; transactions: number; behind_chain_slots: number | null };
  rpc: { enabled: boolean; idls_loaded: number };
  blur: { enabled: boolean; priced_mints: number; error: string | null };
  mirage: { configured: boolean; active: boolean };
  beam: { lookups: number; carried: number };
}

// --- Health and summaries ---------------------------------------------------

export interface HealthCheck {
  id: string;
  label: string;
  status: 'pass' | 'warn' | 'fail' | 'unknown';
  score: number | null;
  weight: number;
  /** The rule applied and the numbers behind it. */
  detail: string;
}

export interface HealthReport {
  program_id: string;
  score: number | null;
  status: 'healthy' | 'degraded' | 'critical' | 'learning';
  headline: string;
  checks: HealthCheck[];
}

export interface SummaryActivity {
  tx: number;
  failed: number;
  success_rate: number | null;
  unique_wallets: number;
  peak_tps: number;
  peak_at: number | null;
  fees_sol: number;
  avg_compute: number | null;
}

export interface BigMove {
  signature: string;
  at: number;
  amount: number;
  symbol: string;
  usd: number | null;
}

export interface IncidentRef {
  id: number;
  kind: IncidentKind;
  severity: Severity;
  status: IncidentStatus;
  title: string;
  summary: string;
  detected_at: number;
  resolved_at: number | null;
}

export interface HourPoint {
  hour: number;
  tx: number;
  failed: number;
  usd_volume: number;
}

export interface Summary {
  program_id: string;
  label: string;
  from: number;
  to: number;
  period_secs: number;
  headline: string;
  activity: SummaryActivity;
  previous: SummaryActivity;
  previous_usd_volume: number;
  value: { usd_volume: number; sol_volume: number; largest: BigMove[] };
  top_instructions: { name: string; tx: number; share: number; failure_rate: number }[];
  top_errors: { label: string; count: number; share: number }[];
  busiest_hour: HourPoint | null;
  hourly: HourPoint[];
  reliability: {
    opened: number;
    resolved: number;
    open_now: number;
    minutes_in_incident: number;
    mttd_secs: number | null;
    mttr_secs: number | null;
    worst: IncidentRef | null;
    incidents: IncidentRef[];
  };
  program_changes: IncidentRef[];
  next_actions: string[];
  coverage: number;
}

export interface SummarySchedule {
  id: number;
  program_id: string;
  period: 'daily' | 'weekly';
  hour_utc: number;
  channels: Channel[];
  enabled: boolean;
  created_at: string;
  last_sent_at: string | null;
}

export interface ApiToken {
  id: number;
  name: string;
  scope: 'read' | 'write';
  created_at: string;
  last_used_at: string | null;
}
