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
  eta_to_100_hours: number | null;
  cap_eta: string | null;
  projected_final_pct: number;
  will_hit_wall: boolean;
  alert_worthy: boolean;
  summary: string;
}

export interface Snapshot {
  generated_at: string;
  plan: string | null;
  tray_percent: number;
  tray_status: string;
  windows: Projection[];
  error: string | null;
}

export interface Config {
  poll_interval_secs: number;
  projection_margin_mins: number;
  velocity_window_hours: number;
  min_elapsed_frac: number;
  well_beyond_pct: number;
  near_cap_pct: number;
  use_api_severity: boolean;
  self_refresh_tokens: boolean;
  notifications_enabled: boolean;
}

export const getUsage = () => invoke<Snapshot | null>("get_usage");
export const refreshNow = () => invoke<void>("refresh_now");
export const testNotification = () => invoke<void>("test_notification");
export const getConfig = () => invoke<Config>("get_config");
export const setConfig = (config: Config) => invoke<void>("set_config", { config });

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
