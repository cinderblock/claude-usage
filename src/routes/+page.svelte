<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { enable, disable, isEnabled } from "@tauri-apps/plugin-autostart";
  import {
    getUsage,
    getConfig,
    setConfig,
    refreshNow,
    testNotification,
    prettyKind,
    fmtHours,
    type Snapshot,
    type Projection,
    type Config,
  } from "$lib/usage";

  let snap = $state<Snapshot | null>(null);
  let cfg = $state<Config | null>(null);
  let showSettings = $state(false);
  let now = $state(Date.now());
  let refreshing = $state(false);
  let autostart = $state(false);

  onMount(() => {
    getUsage().then((s) => (snap = s));
    getConfig().then((c) => (cfg = c));
    isEnabled().then((v) => (autostart = v)).catch(() => {});
    const un = listen<Snapshot>("usage-updated", (e) => (snap = e.payload));
    const tick = setInterval(() => (now = Date.now()), 1000);
    return () => {
      un.then((f) => f());
      clearInterval(tick);
    };
  });

  function hoursToReset(p: Projection): number {
    if (!p.resets_at) return p.time_to_reset_hours;
    return (new Date(p.resets_at).getTime() - now) / 3_600_000;
  }

  type Level = "ok" | "warn" | "crit";
  function level(p: Projection): Level {
    if (p.alert_worthy) return "crit";
    const climbing = (p.rate_per_hour ?? 0) > 0.01;
    // A projected wall that's merely early (not alert-worthy) shows amber, not red.
    if (p.will_hit_wall || (cfg && p.percent >= cfg.near_cap_pct && climbing) || p.severity === "warning" || p.severity === "critical")
      return "warn";
    return "ok";
  }

  function enoughSignal(p: Projection): boolean {
    return !cfg || p.elapsed_frac >= cfg.min_elapsed_frac;
  }

  // Bars span 0–150% so an overshooting projection stays visible;
  // the 100% cap sits at the 2/3 mark.
  const BAR_MAX = 150;
  function barX(v: number): number {
    return (Math.min(Math.max(v, 0), BAR_MAX) / BAR_MAX) * 100;
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

  async function saveCfg() {
    if (cfg) await setConfig($state.snapshot(cfg));
  }

  async function toggleAutostart() {
    try {
      if (autostart) await enable();
      else await disable();
    } catch (e) {
      // revert on failure
      autostart = !autostart;
    }
  }
</script>

<div class="app" data-tauri-drag-region>
  <header data-tauri-drag-region>
    <div class="title">Claude Usage</div>
    {#if snap?.plan}<span class="plan">{snap.plan}</span>{/if}
    <div class="spacer"></div>
    <button class="icon" title="Refresh" onclick={doRefresh} class:spin={refreshing}>⟳</button>
    <button class="icon" title="Settings" onclick={() => (showSettings = !showSettings)}>⚙</button>
  </header>

  {#if snap?.error}
    <div class="error">
      <strong>Can't read usage</strong>
      <div>{snap.error}</div>
      <div class="hint">If the token expired, run Claude Code once to refresh it.</div>
    </div>
  {/if}

  {#if showSettings && cfg}
    <div class="settings">
      <label>
        <span>Poll interval (s)</span>
        <input type="number" min="10" bind:value={cfg.poll_interval_secs} onchange={saveCfg} />
      </label>
      <label title="Only warn when projected to hit the cap at least this many minutes before the window resets. Small gaps are noise.">
        <span>Warn if capping ≥ (min) early</span>
        <input type="number" min="0" step="15" bind:value={cfg.projection_margin_mins} onchange={saveCfg} />
      </label>
      <label>
        <span>Velocity window (h)</span>
        <input type="number" min="1" step="0.5" bind:value={cfg.velocity_window_hours} onchange={saveCfg} />
      </label>
      <label title="Suppress projection warnings until this fraction of the window has elapsed">
        <span>Quiet early phase (0–1)</span>
        <input type="number" min="0" max="1" step="0.05" bind:value={cfg.min_elapsed_frac} onchange={saveCfg} />
      </label>
      <label title="…but warn anyway once usage is already this high">
        <span>Well-beyond (%)</span>
        <input type="number" min="0" max="100" bind:value={cfg.well_beyond_pct} onchange={saveCfg} />
      </label>
      <label>
        <span>Near-cap nudge (%)</span>
        <input type="number" min="50" max="100" bind:value={cfg.near_cap_pct} onchange={saveCfg} />
      </label>
      <label title="Only alert when the odds of capping early are at least this. Uses the spread of the recent burn-rate fit, not just its average.">
        <span>Alert confidence (0–1)</span>
        <input type="number" min="0" max="1" step="0.05" bind:value={cfg.cap_confidence} onchange={saveCfg} />
      </label>
      <label class="check">
        <input type="checkbox" bind:checked={cfg.notifications_enabled} onchange={saveCfg} />
        <span>Notifications</span>
      </label>
      <label class="check">
        <input type="checkbox" bind:checked={cfg.use_api_severity} onchange={saveCfg} />
        <span>Use API severity</span>
      </label>
      <label class="check">
        <input type="checkbox" bind:checked={cfg.self_refresh_tokens} onchange={saveCfg} />
        <span>Self-refresh token</span>
      </label>
      <button class="test-btn" onclick={() => testNotification()}>Send test notification</button>
    </div>
  {/if}

  <main>
    {#if !snap}
      <div class="loading">Loading…</div>
    {:else}
      {#each snap.windows as p (p.kind + p.scope_key)}
        {@const lv = level(p)}
        {@const ttr = hoursToReset(p)}
        {@const showProj = enoughSignal(p) && p.projected_final_pct > p.percent + 0.5}
        {@const lo = p.projected_final_low_pct}
        {@const hi = p.projected_final_high_pct}
        <div class="win {lv}">
          <div class="win-head">
            <span class="name">{p.scope_label ?? prettyKind(p.kind)}</span>
            <span class="pct">{p.percent.toFixed(0)}%</span>
          </div>
          <div class="bar">
            <div class="over-zone" style="left:{barX(100)}%"></div>
            <div class="fill" style="width:{barX(p.percent)}%"></div>
            {#if showProj}
              <div
                class="proj-fill"
                style="left:{barX(p.percent)}%; width:{barX(p.projected_final_pct) - barX(p.percent)}%"
              ></div>
              {#if lo != null && hi != null && hi - lo >= 2}
                <div
                  class="band"
                  style="left:{barX(lo)}%; width:{Math.max(barX(hi) - barX(lo), 0.5)}%"
                  title="likely range at reset (10–90%)"
                ></div>
              {/if}
              <div class="proj-marker" style="left:{barX(p.projected_final_pct)}%" title="projected at reset"></div>
            {/if}
            <div class="tick-100" style="left:{barX(100)}%"></div>
          </div>
          <div class="meta">
            {#if p.alert_worthy}
              <span class="warn-text">⚠ {p.summary}</span>
            {:else if p.will_hit_wall}
              <span class="sub soft">
                on pace to cap early{#if p.cap_probability != null}&nbsp;(~{(p.cap_probability * 100).toFixed(0)}% odds){/if} — monitoring · resets in {fmtHours(ttr)}
              </span>
            {:else}
              <span class="sub">resets in {fmtHours(ttr)}</span>
              {#if showProj && p.rate_per_hour != null && p.rate_per_hour > 0.01}
                <span class="sub dim">· {fmtProjected(p)} by reset</span>
              {/if}
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

<style>
  :global(html),
  :global(body) {
    margin: 0;
    padding: 0;
    background: transparent;
    overflow: hidden;
    user-select: none;
  }
  .app {
    font-family: "Segoe UI", system-ui, sans-serif;
    color: #e8eaed;
    background: #1c1f24;
    height: 100vh;
    display: flex;
    flex-direction: column;
    font-size: 13px;
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
  .settings {
    padding: 10px 12px;
    border-bottom: 1px solid #2c3038;
    display: grid;
    gap: 6px;
  }
  .settings label {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 12px;
    color: #c7cbd1;
  }
  .settings label.check {
    justify-content: flex-start;
  }
  .test-btn {
    margin-top: 4px;
    background: #2c3038;
    border: 1px solid #3a3f48;
    color: #e8eaed;
    border-radius: 5px;
    padding: 6px 8px;
    font-size: 12px;
    cursor: pointer;
  }
  .test-btn:hover {
    background: #363b44;
  }
  .settings input[type="number"] {
    width: 70px;
    background: #12151a;
    border: 1px solid #2c3038;
    color: #e8eaed;
    border-radius: 4px;
    padding: 3px 6px;
    font-size: 12px;
  }
</style>
