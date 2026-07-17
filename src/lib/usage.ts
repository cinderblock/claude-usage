import { invoke } from "@tauri-apps/api/core";

export interface Projection {
  kind: string;
  scope_key: string;
  scope_label: string | null;
  percent: number;
  severity: string | null;
  resets_at: string | null;
  window_len_hours: number;
  time_to_reset_hours: number;
  elapsed_frac: number;
  rate_per_hour: number | null;
  rate_stderr: number | null;
  eta_to_100_hours: number | null;
  cap_eta: string | null;
  projected_final_pct: number;
  projected_final_low_pct: number | null;
  projected_final_high_pct: number | null;
  cap_probability: number | null;
  will_hit_wall: boolean;
  alert_worthy: boolean;
  alert_engaged: boolean;
  dollars: Dollars | null;
  summary: string;
}

export interface Dollars {
  used: number;
  limit: number;
  currency: string;
  decimals: number;
}

export interface Sample {
  ts: number;
  percent: number;
}

/** One completed/in-progress window instance, keyed by its reset time. */
export interface WindowSummary {
  resets_at: number | null;
  peak_percent: number;
  first_ts: number;
  last_ts: number;
  count: number;
}

export interface HistoryStats {
  rows: number;
  oldest_ts: number | null;
  newest_ts: number | null;
  bytes: number;
}

export type RetentionMode = "unlimited" | "time" | "size";

export interface Snapshot {
  generated_at: string;
  plan: string | null;
  tray_percent: number;
  tray_status: string;
  windows: Projection[];
  session_active: boolean;
  error: string | null;
}

/** One user-defined scheduled message. */
export interface ScheduledMessage {
  id: string;
  enabled: boolean;
  time_of_day: string; // "HH:MM" local
  days: number[]; // 0=Sun … 6=Sat; empty = every day
  message: string;
  model: string;
  only_if_session_inactive: boolean;
}

/** 5-hour-window priming settings. */
export interface PrimingConfig {
  enabled: boolean;
  anchor_time: string; // "HH:MM" local
  windows_per_day: number;
  slot_slack_secs: number;
  model: string;
  end_of_day: string | null; // "HH:MM" local, or null
  prime_prompt: string;
}

/** Result of one send, for the send log. */
export interface SendOutcome {
  ts: number;
  ok: boolean;
  model: string;
  source: string;
  detail: string;
  verified: boolean | null;
}

export interface Config {
  poll_interval_secs: number;
  projection_margin_mins: number;
  velocity_window_hours: number;
  min_elapsed_frac: number;
  well_beyond_pct: number;
  near_cap_pct: number;
  cap_confidence: number;
  alert_sustain_mins: number;
  use_api_severity: boolean;
  self_refresh_tokens: boolean;
  notifications_enabled: boolean;
  history_retention_mode: RetentionMode;
  history_retention_days: number;
  history_retention_mb: number;
  history_downsample: boolean;
  history_downsample_after_days: number;
  claude_binary_path: string;
  scheduled_messages: ScheduledMessage[];
  priming: PrimingConfig;
}

export const getUsage = () => invoke<Snapshot | null>("get_usage");
export const refreshNow = () => invoke<void>("refresh_now");
export const testNotification = () => invoke<void>("test_notification");
export const getConfig = () => invoke<Config>("get_config");
export const setConfig = (config: Config) => invoke<void>("set_config", { config });
export const openSettingsWindow = () => invoke<void>("open_settings_window");
export const openHistoryWindow = () => invoke<void>("open_history_window");
export const getHistory = (kind: string, scope: string, since: number) =>
  invoke<Sample[]>("get_history", { kind, scope, since });
export const getWindowSummaries = (kind: string, scope: string, since: number) =>
  invoke<WindowSummary[]>("get_window_summaries", { kind, scope, since });
export const getHistoryStats = () => invoke<HistoryStats | null>("get_history_stats");
export const sendMessageNow = (message: string, model: string) =>
  invoke<void>("send_message_now", { message, model });
export const getSendLog = () => invoke<SendOutcome[]>("get_send_log");

export function prettyKind(kind: string): string {
  switch (kind) {
    case "session":
      return "5-hour";
    case "weekly_all":
      return "Weekly";
    case "weekly_scoped":
      return "Weekly (model)";
    default:
      return kind.replace(/_/g, " ");
  }
}

/** "$12.02" — a money amount with its currency symbol and fixed decimals. */
export function fmtMoney(amount: number, currency: string, decimals: number): string {
  try {
    return new Intl.NumberFormat(undefined, {
      style: "currency",
      currency,
      minimumFractionDigits: decimals,
      maximumFractionDigits: decimals,
    }).format(amount);
  } catch {
    return `${amount.toFixed(decimals)} ${currency}`;
  }
}

/** Compact "2d 3h" / "4h 10m" / "12m" from fractional hours. */
export function fmtHours(hours: number): string {
  if (hours == null || hours < 0) return "now";
  const total = Math.round(hours * 60);
  const d = Math.floor(total / (60 * 24));
  const h = Math.floor((total % (60 * 24)) / 60);
  const m = total % 60;
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}
