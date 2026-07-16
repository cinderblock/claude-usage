<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import SettingsPanel from "$lib/SettingsPanel.svelte";
  import UsageChart, { type ChartSeries } from "$lib/UsageChart.svelte";
  import {
    getUsage,
    getConfig,
    getHistory,
    refreshNow,
    openSettingsWindow,
    prettyKind,
    fmtHours,
    fmtMoney,
    type Snapshot,
    type Projection,
    type Config,
    type Sample,
  } from "$lib/usage";

  const USAGE_SETTINGS_URL = "https://claude.ai/new#settings/usage";

  // This component is the SvelteKit root page for BOTH windows (the popup and
  // the standalone Settings window, which also loads "/") — branch on which
  // one we're in rather than routing, since ssr:false + the SPA fallback give
  // every window the same shell.
  let isSettingsWindow = $state(false);

  let snap = $state<Snapshot | null>(null);
  let cfg = $state<Config | null>(null);
  let now = $state(Date.now());
  let refreshing = $state(false);

  onMount(() => {
    isSettingsWindow = getCurrentWindow().label === "settings";
    if (isSettingsWindow) return; // SettingsPanel owns its own lifecycle

    getUsage().then((s) => (snap = s));
    getConfig().then((c) => (cfg = c));
    const unUsage = listen<Snapshot>("usage-updated", (e) => (snap = e.payload));
    // Settings may be edited live in the other window; pick it up without
    // waiting for the next poll.
    const unCfg = listen<Config>("config-updated", (e) => (cfg = e.payload));
    const tick = setInterval(() => (now = Date.now()), 1000);
    return () => {
      unUsage.then((f) => f());
      unCfg.then((f) => f());
      clearInterval(tick);
    };
  });

  function hoursToReset(p: Projection): number {
    if (!p.resets_at) return p.time_to_reset_hours;
    return (new Date(p.resets_at).getTime() - now) / 3_600_000;
  }

  /** Live hours until the projected cap, from the absolute cap ETA. */
  function hoursToCap(p: Projection): number | null {
    if (!p.cap_eta) return null;
    return (new Date(p.cap_eta).getTime() - now) / 3_600_000;
  }

  type Level = "ok" | "warn" | "crit";
  function level(p: Projection): Level {
    // Red follows the latched alert (sustained past the debounce), so a noisy
    // fit flapping across the confidence bar shows amber, not flickering red.
    if (p.alert_engaged) return "crit";
    const climbing = (p.rate_per_hour ?? 0) > 0.01;
    if (p.will_hit_wall || (cfg && p.percent >= cfg.near_cap_pct && climbing) || p.severity === "warning" || p.severity === "critical")
      return "warn";
    return "ok";
  }

  function enoughSignal(p: Projection): boolean {
    return !cfg || p.elapsed_frac >= cfg.min_elapsed_frac;
  }

  /** Amber pace warning, built as one string — interpolating fragments
   *  through {#if} blocks collapses the whitespace between them ("capin"). */
  function paceNote(m: Projection, ttr: number, multi: boolean): string {
    const prefix = multi ? memberLabel(m) + ": " : "";
    const odds = m.cap_probability != null ? ` (~${(m.cap_probability * 100).toFixed(0)}% odds)` : "";
    const ttc = hoursToCap(m);
    if (ttc == null) return `${prefix}on pace to cap early${odds} — monitoring`;
    const early = fmtHours(Math.max(ttr - ttc, 0));
    // Cap ETA already passed (or is imminent): "in now" is nonsense.
    if (ttc <= 1 / 60) return `${prefix}on pace to cap about now — ${early} before the reset${odds}`;
    return `${prefix}on pace to cap in ${fmtHours(ttc)} — ${early} early${odds}`;
  }

  /** "~118%" or "~95–140%" when the fit gives a meaningful spread. */
  function fmtProjected(p: Projection): string {
    const lo = p.projected_final_low_pct;
    const hi = p.projected_final_high_pct;
    if (lo != null && hi != null && hi - lo >= 2) return `~${lo.toFixed(0)}–${hi.toFixed(0)}%`;
    return `~${p.projected_final_pct.toFixed(0)}%`;
  }

  async function doRefresh() {
    refreshing = true;
    try {
      await refreshNow();
    } finally {
      refreshing = false;
    }
  }

  // ---- Charts ----
  // Windows are grouped so related series share one time chart: the two weekly
  // limits (overall + per-model) sit on the same 7-day axis.
  type GroupId = "session" | "weekly" | "monthly" | string;
  function groupOf(p: Projection): GroupId {
    if (p.kind === "weekly_all" || p.kind === "weekly_scoped") return "weekly";
    if (p.kind === "session") return "session";
    if (p.kind === "monthly_extra") return "monthly";
    return p.kind;
  }
  const GROUP_ORDER: GroupId[] = ["session", "weekly", "monthly"];
  const GROUP_TITLE: Record<string, string> = {
    session: "5-hour",
    weekly: "Weekly",
    monthly: "Usage billing",
  };

  const key = (p: Projection) => `${p.kind}:${p.scope_key}`;

  /** Stable [start,end] ms for a window's full span (independent of `now`). */
  function windowSpan(p: Projection, generatedAt: string): { start: number; end: number } {
    const len = p.window_len_hours * 3_600_000;
    const end = p.resets_at ? new Date(p.resets_at).getTime() : new Date(generatedAt).getTime() + len;
    return { start: end - len, end };
  }

  /** Short per-series label for a legend chip. */
  function memberLabel(p: Projection): string {
    if (p.scope_label) return p.scope_label;
    if (p.kind === "weekly_all") return "All models";
    return prettyKind(p.kind);
  }

  const SERIES_COLORS = ["#6aa9ff", "#e0a458", "#c586e0", "#54c7b0"];
  function memberColor(p: Projection, idxInGroup: number): string {
    return p.scope_key === "all" ? "#3fb950" : SERIES_COLORS[idxInGroup % SERIES_COLORS.length];
  }

  // Per-(kind,scope) history, refetched whenever a new snapshot lands.
  let hist = $state<Record<string, Sample[]>>({});
  $effect(() => {
    const s = snap;
    if (!s) return;
    Promise.all(
      s.windows.map(
        async (p) =>
          [key(p), await getHistory(p.kind, p.scope_key, Math.floor(windowSpan(p, s.generated_at).start))] as const,
      ),
    ).then((entries) => (hist = Object.fromEntries(entries)));
  });

  interface ChartGroup {
    id: GroupId;
    title: string;
    members: Projection[];
    series: ChartSeries[];
    start: number;
    end: number;
    yCap: number;
    level: Level;
  }

  const groups = $derived.by<ChartGroup[]>(() => {
    if (!snap) return [];
    const by = new Map<GroupId, Projection[]>();
    for (const p of snap.windows) {
      const g = groupOf(p);
      (by.get(g) ?? by.set(g, []).get(g)!).push(p);
    }
    const ids = [...by.keys()].sort((a, b) => {
      const ia = GROUP_ORDER.indexOf(a), ib = GROUP_ORDER.indexOf(b);
      return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib);
    });
    return ids.map((id) => {
      // Overall (scope "all") first so it's the visual baseline.
      const members = by.get(id)!.slice().sort((a, b) => (a.scope_key === "all" ? -1 : 0) - (b.scope_key === "all" ? -1 : 0));
      const rep = members[0];
      const { start, end } = windowSpan(rep, snap!.generated_at);
      const series: ChartSeries[] = members.map((p, i) => ({
        label: memberLabel(p),
        color: memberColor(p, i),
        samples: hist[key(p)] ?? [],
        projectedPct: enoughSignal(p) && p.projected_final_pct > p.percent + 0.5 ? p.projected_final_pct : null,
        currentPct: p.percent,
      }));
      const worst = members.reduce<Level>((acc, p) => {
        const l = level(p);
        return l === "crit" || acc === "crit" ? "crit" : l === "warn" || acc === "warn" ? "warn" : "ok";
      }, "ok");
      return {
        id,
        title: GROUP_TITLE[id] ?? prettyKind(rep.kind),
        members,
        series,
        start,
        end,
        yCap: rep.dollars ? 100 : 150,
        level: worst,
      };
    });
  });
</script>

{#if isSettingsWindow}
  <SettingsPanel />
{:else}
  <div class="app" data-tauri-drag-region>
    <header data-tauri-drag-region>
      <div class="title">Claude Usage</div>
      {#if snap?.plan}<span class="plan">{snap.plan}</span>{/if}
      <div class="spacer"></div>
      <button class="icon" title="Refresh" onclick={doRefresh} class:spin={refreshing}>⟳</button>
      <button class="icon" title="Settings" onclick={() => openSettingsWindow()}>⚙</button>
    </header>

    {#if snap?.error}
      <div class="error">
        <strong>Can't read usage</strong>
        <div>{snap.error}</div>
        {#if snap.error.toLowerCase().includes("token")}
          <div class="hint">If the token expired, run Claude Code once to refresh it.</div>
        {/if}
      </div>
    {/if}

    <main>
      {#if !snap}
        <div class="loading">Loading…</div>
      {:else}
        {#each groups as g (g.id)}
          {@const ttr = hoursToReset(g.members[0])}
          {@const single = g.members.length === 1 ? g.members[0] : null}
          <div class="win {g.level}">
            <div class="win-head">
              <span class="name">{g.title}</span>
              {#if single?.dollars}
                <span class="pct">
                  {fmtMoney(single.dollars.used, single.dollars.currency, single.dollars.decimals)}
                  <span class="dim">/ {fmtMoney(single.dollars.limit, single.dollars.currency, single.dollars.decimals)}</span>
                  · {single.percent.toFixed(0)}%
                </span>
              {:else if single}
                <span class="pct">{single.percent.toFixed(0)}%</span>
              {:else}
                <span class="legend">
                  {#each g.members as m, i (key(m))}
                    <span class="chip" style="--c:{memberColor(m, i)}">{memberLabel(m)} {m.percent.toFixed(0)}%</span>
                  {/each}
                </span>
              {/if}
            </div>

            <UsageChart series={g.series} startMs={g.start} endMs={g.end} nowMs={now} yCap={g.yCap} />

            <div class="meta">
              <span class="sub">resets in {fmtHours(ttr)}</span>
              {#each g.members as m (key(m))}
                {#if m.alert_engaged}
                  <div class="note warn-text">⚠ {m.summary}</div>
                {:else if m.will_hit_wall}
                  <div class="note soft">{paceNote(m, ttr, g.members.length > 1)}</div>
                {:else if enoughSignal(m) && m.projected_final_pct > m.percent + 0.5 && m.rate_per_hour != null && m.rate_per_hour > 0.01}
                  <div class="note sub dim">
                    {g.members.length > 1 ? memberLabel(m) + ": " : ""}{#if m.dollars}~{fmtMoney((m.projected_final_pct / 100) * m.dollars.limit, m.dollars.currency, m.dollars.decimals)}{:else}{fmtProjected(m)}{/if} by reset
                  </div>
                {/if}
              {/each}
              {#if single?.dollars}
                <button class="link" onclick={() => openUrl(USAGE_SETTINGS_URL)} title="Change your limit on claude.ai">
                  Change limit ↗
                </button>
              {/if}
            </div>
          </div>
        {/each}
        <div class="foot">
          updated {fmtHours((now - new Date(snap.generated_at).getTime()) / 3_600_000)} ago
        </div>
      {/if}
    </main>
  </div>
{/if}

<style>
  /* Shared by both windows (this component is the SvelteKit root page for
     each) — kept minimal so the resizable, scrollable Settings window isn't
     clipped by rules meant for the frameless popup below. */
  :global(html),
  :global(body) {
    margin: 0;
    padding: 0;
    background: transparent;
  }
  .app {
    font-family: "Segoe UI", system-ui, sans-serif;
    color: #e8eaed;
    background: #1c1f24;
    height: 100vh;
    display: flex;
    flex-direction: column;
    font-size: 13px;
    overflow: hidden;
    user-select: none;
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid #2c3038;
  }
  .title {
    font-weight: 600;
  }
  .plan {
    font-size: 11px;
    color: #9aa0a6;
    background: #2c3038;
    padding: 1px 6px;
    border-radius: 999px;
  }
  .spacer {
    flex: 1;
  }
  .icon {
    background: none;
    border: none;
    color: #9aa0a6;
    font-size: 15px;
    cursor: pointer;
    padding: 2px 4px;
    border-radius: 4px;
  }
  .icon:hover {
    color: #e8eaed;
    background: #2c3038;
  }
  .spin {
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  main {
    flex: 1;
    overflow-y: auto;
    padding: 10px 12px;
  }
  .win {
    padding: 8px 0;
    border-bottom: 1px solid #23272e;
  }
  .win:last-child {
    border-bottom: none;
  }
  .win-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 8px;
    margin-bottom: 4px;
  }
  .name {
    font-weight: 500;
  }
  .pct {
    font-variant-numeric: tabular-nums;
    color: #c7cbd1;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
    justify-content: flex-end;
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .chip {
    color: #c7cbd1;
    display: inline-flex;
    align-items: center;
  }
  .chip::before {
    content: "";
    width: 8px;
    height: 8px;
    border-radius: 2px;
    background: var(--c);
    margin-right: 4px;
  }
  .meta {
    margin-top: 4px;
    font-size: 12px;
  }
  .note {
    margin-top: 2px;
  }
  .sub {
    color: #9aa0a6;
  }
  .dim {
    opacity: 0.75;
  }
  .soft {
    color: #d9a441;
  }
  .warn-text {
    color: #f0b429;
  }
  .link {
    background: none;
    border: none;
    color: #6aa9ff;
    font-size: 12px;
    cursor: pointer;
    padding: 0;
    margin-left: 6px;
  }
  .link:hover {
    text-decoration: underline;
  }
  .foot {
    text-align: center;
    color: #6b7280;
    font-size: 11px;
    margin-top: 10px;
  }
  .error {
    margin: 10px 12px;
    padding: 8px 10px;
    background: #3a1d1b;
    border: 1px solid #d2372b;
    border-radius: 6px;
    font-size: 12px;
  }
  .hint {
    color: #c7cbd1;
    margin-top: 4px;
  }
  .loading {
    text-align: center;
    color: #9aa0a6;
    padding: 30px;
  }
</style>
