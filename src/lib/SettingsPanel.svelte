<script lang="ts">
  import { onMount } from "svelte";
  import { enable, disable, isEnabled } from "@tauri-apps/plugin-autostart";
  import { getConfig, setConfig, testNotification, getHistoryStats, type Config, type HistoryStats } from "$lib/usage";

  let cfg = $state<Config | null>(null);
  let autostart = $state(false);
  let stats = $state<HistoryStats | null>(null);

  onMount(() => {
    getConfig().then((c) => (cfg = c));
    getHistoryStats().then((s) => (stats = s));
    isEnabled().then((v) => (autostart = v)).catch(() => {});
  });

  async function saveCfg() {
    if (cfg) await setConfig($state.snapshot(cfg));
  }

  async function toggleAutostart() {
    try {
      if (autostart) await enable();
      else await disable();
    } catch {
      // revert on failure
      autostart = !autostart;
    }
  }

  // ---- History store size / retention estimates ----
  const MB = 1024 * 1024;
  const spanDays = $derived(
    stats && stats.oldest_ts != null && stats.newest_ts != null
      ? Math.max((stats.newest_ts - stats.oldest_ts) / 86_400_000, 0)
      : 0,
  );
  // Bytes/day at the observed rate. Needs a bit of history to be meaningful.
  const bytesPerDay = $derived(stats && spanDays >= 0.25 ? stats.bytes / spanDays : null);

  function fmtBytes(b: number): string {
    if (b >= MB) return `${(b / MB).toFixed(1)} MB`;
    if (b >= 1024) return `${(b / 1024).toFixed(0)} KB`;
    return `${b} B`;
  }
  function fmtDays(d: number): string {
    if (d >= 365) return `${(d / 365).toFixed(1)} yr`;
    if (d >= 1) return `${Math.round(d)} days`;
    return "<1 day";
  }

  const currentSize = $derived(stats ? fmtBytes(stats.bytes) : "—");
  const sinceLabel = $derived(
    stats && stats.oldest_ts != null ? new Date(stats.oldest_ts).toLocaleDateString() : null,
  );
  // Estimated on-disk size if kept for the configured number of days.
  const timeEstimate = $derived(
    cfg && bytesPerDay != null ? fmtBytes(bytesPerDay * cfg.history_retention_days) : null,
  );
  // Estimated days of history that fit in the configured size cap.
  const sizeEstimate = $derived(
    cfg && bytesPerDay != null ? fmtDays((cfg.history_retention_mb * MB) / bytesPerDay) : null,
  );
</script>

<div class="settings-app" data-tauri-drag-region>
  {#if !cfg}
    <div class="loading">Loading…</div>
  {:else}
    <div class="settings">
      <div class="field">
        <div class="row"><span>Poll interval (s)</span>
          <input type="number" min="30" step="30" bind:value={cfg.poll_interval_secs} onchange={saveCfg} /></div>
      </div>
      <div class="field">
        <div class="row"><span>Warn if capping ≥ (min) early</span>
          <input type="number" min="0" step="15" bind:value={cfg.projection_margin_mins} onchange={saveCfg} /></div>
        <p class="hint">Only warn when projected to hit the cap at least this many minutes before the window resets — smaller gaps are noise.</p>
      </div>
      <div class="field">
        <div class="row"><span>Velocity window (h)</span>
          <input type="number" min="1" step="0.5" bind:value={cfg.velocity_window_hours} onchange={saveCfg} /></div>
        <p class="hint">Trailing span used to estimate current burn rate.</p>
      </div>
      <div class="field">
        <div class="row"><span>Quiet early phase (0–1)</span>
          <input type="number" min="0" max="1" step="0.05" bind:value={cfg.min_elapsed_frac} onchange={saveCfg} /></div>
        <p class="hint">Suppress projection warnings until this fraction of the window has elapsed (early velocity is too noisy to trust).</p>
      </div>
      <div class="field">
        <div class="row"><span>Well-beyond (%)</span>
          <input type="number" min="0" max="100" bind:value={cfg.well_beyond_pct} onchange={saveCfg} /></div>
        <p class="hint">…but warn anyway once usage is already this high, however little time has passed.</p>
      </div>
      <div class="field">
        <div class="row"><span>Near-cap nudge (%)</span>
          <input type="number" min="50" max="100" bind:value={cfg.near_cap_pct} onchange={saveCfg} /></div>
        <p class="hint">Secondary nudge: warn if already at/above this % and still climbing.</p>
      </div>
      <div class="field">
        <div class="row"><span>Alert confidence (0–1)</span>
          <input type="number" min="0" max="1" step="0.05" bind:value={cfg.cap_confidence} onchange={saveCfg} /></div>
        <p class="hint">Only alert when the odds of capping early are at least this — from the spread of the recent burn-rate fit, not just its average.</p>
      </div>
      <div class="field">
        <div class="row"><span>Sustain before alert (min)</span>
          <input type="number" min="0" step="1" bind:value={cfg.alert_sustain_mins} onchange={saveCfg} /></div>
        <p class="hint">An alert must hold continuously this long before it notifies, and be clear this long before it re-arms. Debounces noisy projections.</p>
      </div>

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

      <!-- History retention -->
      <div class="section">History</div>
      <p class="hint stat">
        Stored: {currentSize}{#if stats} · {stats.rows.toLocaleString()} samples{/if}{#if sinceLabel} · since {sinceLabel}{/if}
      </p>

      <div class="field">
        <div class="row"><span>Keep history</span>
          <select bind:value={cfg.history_retention_mode} onchange={saveCfg}>
            <option value="unlimited">Forever</option>
            <option value="time">By age</option>
            <option value="size">By size</option>
          </select></div>
        <p class="hint">How the long-term store is bounded. Every window (5-hour, weekly, billing) is kept for graphing in the History window.</p>
      </div>

      {#if cfg.history_retention_mode === "time"}
        <div class="field">
          <div class="row"><span>Keep for (days)</span>
            <input type="number" min="1" step="1" bind:value={cfg.history_retention_days} onchange={saveCfg} /></div>
          <p class="hint">{#if timeEstimate}≈ {timeEstimate} on disk at the current rate.{:else}Collecting data to estimate size…{/if}</p>
        </div>
      {:else if cfg.history_retention_mode === "size"}
        <div class="field">
          <div class="row"><span>Cap size (MB)</span>
            <input type="number" min="1" step="1" bind:value={cfg.history_retention_mb} onchange={saveCfg} /></div>
          <p class="hint">{#if sizeEstimate}≈ {sizeEstimate} of history at the current rate.{:else}Collecting data to estimate span…{/if}</p>
        </div>
      {/if}

      <label class="check">
        <input type="checkbox" bind:checked={cfg.history_downsample} onchange={saveCfg} />
        <span>Downsample old data</span>
      </label>
      <p class="hint">Off keeps every sample. On thins samples older than the cutoff to one peak-preserving point per hour, shrinking the store (and lowering the estimates above).</p>
      {#if cfg.history_downsample}
        <div class="field">
          <div class="row"><span>Downsample after (days)</span>
            <input type="number" min="1" step="1" bind:value={cfg.history_downsample_after_days} onchange={saveCfg} /></div>
          <p class="hint">Recent data stays at full poll fidelity; only older data is thinned.</p>
        </div>
      {/if}
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
  .field {
    display: grid;
    gap: 3px;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 12px;
    color: #c7cbd1;
  }
  .hint {
    margin: 0;
    font-size: 11px;
    line-height: 1.35;
    color: #8b9096;
  }
  .settings label.check {
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: 8px;
    font-size: 12px;
    color: #c7cbd1;
  }
  .section {
    margin-top: 8px;
    padding-top: 10px;
    border-top: 1px solid #2c3038;
    font-weight: 600;
    font-size: 12px;
    color: #e8eaed;
  }
  .stat {
    color: #9aa0a6;
    font-variant-numeric: tabular-nums;
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
  .settings select {
    background: #12151a;
    border: 1px solid #2c3038;
    color: #e8eaed;
    border-radius: 4px;
    padding: 4px 6px;
    font-size: 12px;
  }
</style>
