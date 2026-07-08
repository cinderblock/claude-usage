<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import SettingsPanel from "$lib/SettingsPanel.svelte";
  import {
    getUsage,
    getConfig,
    refreshNow,
    openSettingsWindow,
    prettyKind,
    fmtHours,
    fmtMoney,
    type Snapshot,
    type Projection,
    type Config,
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

  // Bars span 0–150% so an overshooting projection stays visible, with the
  // 100% cap at the 2/3 mark — EXCEPT the usage-billing pool, which has a
  // hard $ cap, so its bar tops out at 100% like a normal meter.
  function barMax(p: Projection): number {
    return p.dollars ? 100 : 150;
  }
  function barX(v: number, max: number): number {
    return (Math.min(Math.max(v, 0), max) / max) * 100;
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
        {#each snap.windows as p (p.kind + p.scope_key)}
          {@const lv = level(p)}
          {@const ttr = hoursToReset(p)}
          {@const max = barMax(p)}
          {@const showProj = enoughSignal(p) && p.projected_final_pct > p.percent + 0.5}
          {@const lo = p.projected_final_low_pct}
          {@const hi = p.projected_final_high_pct}
          <div class="win {lv}">
            <div class="win-head">
              <span class="name">{p.scope_label ?? prettyKind(p.kind)}</span>
              {#if p.dollars}
                <span class="pct">
                  {fmtMoney(p.dollars.used, p.dollars.currency, p.dollars.decimals)}
                  <span class="dim">/ {fmtMoney(p.dollars.limit, p.dollars.currency, p.dollars.decimals)}</span>
                  · {p.percent.toFixed(0)}%
                </span>
              {:else}
                <span class="pct">{p.percent.toFixed(0)}%</span>
              {/if}
            </div>
            <div class="bar">
              {#if !p.dollars}
                <div class="over-zone" style="left:{barX(100, max)}%"></div>
              {/if}
              <div class="fill" style="width:{barX(p.percent, max)}%"></div>
              {#if showProj}
                <div
                  class="proj-fill"
                  style="left:{barX(p.percent, max)}%; width:{barX(p.projected_final_pct, max) - barX(p.percent, max)}%"
                ></div>
                {#if lo != null && hi != null && hi - lo >= 2}
                  <div
                    class="band"
                    style="left:{barX(lo, max)}%; width:{Math.max(barX(hi, max) - barX(lo, max), 0.5)}%"
                    title="likely range at reset (10–90%)"
                  ></div>
                {/if}
                <div class="proj-marker" style="left:{barX(p.projected_final_pct, max)}%" title="projected at reset"></div>
              {/if}
              {#if !p.dollars}
                <div class="tick-100" style="left:{barX(100, max)}%"></div>
              {/if}
            </div>
            <div class="meta">
              {#if p.alert_engaged}
                <span class="warn-text">⚠ {p.summary}</span>
              {:else if p.will_hit_wall}
                <span class="sub soft">
                  on pace to cap early{#if p.cap_probability != null}&nbsp;(~{(p.cap_probability * 100).toFixed(0)}% odds){/if} — monitoring · resets in {fmtHours(ttr)}
                </span>
              {:else}
                <span class="sub">resets in {fmtHours(ttr)}</span>
                {#if showProj && !p.dollars && p.rate_per_hour != null && p.rate_per_hour > 0.01}
                  <span class="sub dim">· {fmtProjected(p)} by reset</span>
                {/if}
              {/if}
              {#if p.dollars}
                {#if showProj && p.rate_per_hour != null && p.rate_per_hour > 0.01}
                  <span class="sub dim">
                    · ~{fmtMoney((p.projected_final_pct / 100) * p.dollars.limit, p.dollars.currency, p.dollars.decimals)} projected
                  </span>
                {/if}
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
    margin-bottom: 5px;
  }
  .name {
    font-weight: 500;
  }
  .pct {
    font-variant-numeric: tabular-nums;
    color: #c7cbd1;
  }
  .bar {
    position: relative;
    height: 7px;
    background: #2c3038;
    border-radius: 4px;
    overflow: visible;
  }
  .over-zone {
    position: absolute;
    top: 0;
    right: 0;
    height: 100%;
    background: rgba(210, 55, 43, 0.16);
    border-radius: 0 4px 4px 0;
  }
  .fill {
    height: 100%;
    border-radius: 4px;
    background: #2ea043;
    transition: width 0.4s ease;
  }
  .win.warn .fill {
    background: #db9a04;
  }
  .win.crit .fill {
    background: #d2372b;
  }
  .proj-fill {
    position: absolute;
    top: 0;
    height: 100%;
    background: rgba(46, 160, 67, 0.35);
    transition: left 0.4s ease, width 0.4s ease;
  }
  .win.warn .proj-fill {
    background: rgba(219, 154, 4, 0.35);
  }
  .win.crit .proj-fill {
    background: rgba(210, 55, 43, 0.35);
  }
  .band {
    position: absolute;
    top: 0;
    height: 100%;
    background: rgba(232, 234, 237, 0.18);
    border-radius: 2px;
  }
  .tick-100 {
    position: absolute;
    top: -2px;
    width: 2px;
    height: 11px;
    margin-left: -1px;
    background: rgba(232, 234, 237, 0.45);
  }
  .proj-marker {
    position: absolute;
    top: -1px;
    width: 2px;
    height: 9px;
    background: #e8eaed;
    opacity: 0.8;
  }
  .meta {
    margin-top: 5px;
    font-size: 12px;
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
