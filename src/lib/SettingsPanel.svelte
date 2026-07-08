<script lang="ts">
  import { onMount } from "svelte";
  import { enable, disable, isEnabled } from "@tauri-apps/plugin-autostart";
  import { getConfig, setConfig, testNotification, type Config } from "$lib/usage";

  let cfg = $state<Config | null>(null);
  let autostart = $state(false);

  onMount(() => {
    getConfig().then((c) => (cfg = c));
    isEnabled().then((v) => (autostart = v)).catch(() => {});
  });

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

<div class="settings-app" data-tauri-drag-region>
  {#if !cfg}
    <div class="loading">Loading…</div>
  {:else}
    <div class="settings">
      <label>
        <span>Poll interval (s)</span>
        <input type="number" min="30" step="30" bind:value={cfg.poll_interval_secs} onchange={saveCfg} />
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
      <label title="An alert must hold continuously this long before it notifies, and be clear this long before it re-arms. Debounces noisy projections.">
        <span>Sustain before alert (min)</span>
        <input type="number" min="0" step="1" bind:value={cfg.alert_sustain_mins} onchange={saveCfg} />
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
      <label class="check">
        <input type="checkbox" checked={autostart} onchange={(e) => { autostart = (e.target as HTMLInputElement).checked; toggleAutostart(); }} />
        <span>Launch at login</span>
      </label>
      <button class="test-btn" onclick={() => testNotification()}>Send test notification</button>
    </div>
  {/if}
</div>

<style>
  :global(html),
  :global(body) {
    margin: 0;
    padding: 0;
    background: #1c1f24;
  }
  .settings-app {
    font-family: "Segoe UI", system-ui, sans-serif;
    color: #e8eaed;
    background: #1c1f24;
    min-height: 100vh;
    font-size: 13px;
  }
  .loading {
    text-align: center;
    color: #9aa0a6;
    padding: 30px;
  }
  .settings {
    padding: 14px 16px;
    display: grid;
    gap: 10px;
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
    margin-top: 6px;
    background: #2c3038;
    border: 1px solid #3a3f48;
    color: #e8eaed;
    border-radius: 5px;
    padding: 7px 10px;
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
    padding: 4px 6px;
    font-size: 12px;
  }
</style>
