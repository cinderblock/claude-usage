<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { enable, disable, isEnabled } from "@tauri-apps/plugin-autostart";
  import PrimingTimeline from "$lib/PrimingTimeline.svelte";
  import {
    getConfig,
    setConfig,
    testNotification,
    getHistoryStats,
    sendMessageNow,
    getSendLog,
    getUsage,
    type Config,
    type HistoryStats,
    type ScheduledMessage,
    type SendOutcome,
    type Snapshot,
  } from "$lib/usage";

  let cfg = $state<Config | null>(null);
  let autostart = $state(false);
  let stats = $state<HistoryStats | null>(null);
  let sendLog = $state<SendOutcome[]>([]);
  let snapshot = $state<Snapshot | null>(null);
  let sendBusy = $state<Record<string, boolean>>({});

  const MODELS = ["haiku", "sonnet", "opus"];
  const DOW = ["S", "M", "T", "W", "T", "F", "S"];

  onMount(() => {
    getConfig().then((c) => (cfg = c));
    getHistoryStats().then((s) => (stats = s));
    getSendLog().then((l) => (sendLog = l));
    getUsage().then((s) => (snapshot = s));
    isEnabled()
      .then((v) => (autostart = v))
      .catch(() => {});
    const un1 = listen("send-log-updated", () => getSendLog().then((l) => (sendLog = l)));
    const un2 = listen<Snapshot>("usage-updated", (e) => (snapshot = e.payload));
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
    };
  });

  // Current live 5h window, for the priming timeline (green band).
  const sessionWindow = $derived.by(() => {
    if (!snapshot || !snapshot.session_active) return null;
    const s = snapshot.windows.find((w) => w.kind === "session");
    if (!s || !s.resets_at) return null;
    const endMs = Date.parse(s.resets_at);
    if (!Number.isFinite(endMs)) return null;
    return { startMs: endMs - 5 * 3600 * 1000, endMs };
  });

  function genId(): string {
    try {
      return crypto.randomUUID();
    } catch {
      return `m${Date.now()}${Math.random().toString(36).slice(2, 8)}`;
    }
  }

  function addMessage() {
    if (!cfg) return;
    cfg.scheduled_messages = [
      ...cfg.scheduled_messages,
      {
        id: genId(),
        enabled: true,
        time_of_day: "09:00",
        days: [],
        message: "",
        model: "haiku",
        only_if_session_inactive: false,
      },
    ];
    saveCfg();
  }

  function removeMessage(id: string) {
    if (!cfg) return;
    cfg.scheduled_messages = cfg.scheduled_messages.filter((m) => m.id !== id);
    saveCfg();
  }

  // Weekday chips: empty `days` means "every day". Toggling from that state
  // starts from all-selected so unchecking one leaves the other six.
  function toggleDay(m: ScheduledMessage, d: number) {
    let cur = m.days.length === 0 ? [0, 1, 2, 3, 4, 5, 6] : [...m.days];
    cur = cur.includes(d) ? cur.filter((x) => x !== d) : [...cur, d];
    cur.sort((a, b) => a - b);
    m.days = cur.length === 7 ? [] : cur;
    saveCfg();
  }

  async function sendNow(message: string, model: string, key: string) {
    sendBusy = { ...sendBusy, [key]: true };
    try {
      await sendMessageNow(message, model);
    } catch {
      // The failure is also recorded in the send log below.
    } finally {
      sendBusy = { ...sendBusy, [key]: false };
    }
  }

  function fmtTime(ts: number): string {
    return new Date(ts).toLocaleTimeString();
  }
  function outcomeLabel(o: SendOutcome): string {
    if (!o.ok) return "error";
    if (o.verified === true) return "primed ✓";
    if (o.verified === false) return "sent, no window seen";
    return "ok";
  }

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

      <!-- Scheduled messages -->
      <div class="section">Scheduled messages</div>
      <p class="hint">Send a prompt to Claude on a schedule via the local Claude Code CLI (<code>claude -p</code>). Each send counts toward your usage like normal Claude Code use.</p>

      <div class="field">
        <div class="row"><span>Claude CLI path</span>
          <input class="path" type="text" placeholder="autodetect" bind:value={cfg.claude_binary_path} onchange={saveCfg} /></div>
        <p class="hint">Blank = autodetect the <code>claude</code> command on your PATH. Set this only if it isn't found.</p>
      </div>

      {#each cfg.scheduled_messages as m (m.id)}
        <div class="sched">
          <div class="sched-head">
            <label class="check">
              <input type="checkbox" bind:checked={m.enabled} onchange={saveCfg} />
              <span>On</span>
            </label>
            <input class="time" type="time" bind:value={m.time_of_day} onchange={saveCfg} />
            <select bind:value={m.model} onchange={saveCfg}>
              {#each MODELS as mo}<option value={mo}>{mo}</option>{/each}
            </select>
            <button class="icon-btn" onclick={() => removeMessage(m.id)} aria-label="Remove message">✕</button>
          </div>
          <div class="days">
            {#each DOW as label, d}
              <button type="button" class="day" class:on={m.days.length === 0 || m.days.includes(d)} onclick={() => toggleDay(m, d)}>{label}</button>
            {/each}
            {#if m.days.length === 0}<span class="days-hint">every day</span>{/if}
          </div>
          <textarea class="msg" rows="2" placeholder="Message to send…" bind:value={m.message} onchange={saveCfg}></textarea>
          <div class="sched-foot">
            <label class="check">
              <input type="checkbox" bind:checked={m.only_if_session_inactive} onchange={saveCfg} />
              <span>Skip if a 5h window is active</span>
            </label>
            <button class="test-btn small" disabled={!m.message.trim() || sendBusy[m.id]} onclick={() => sendNow(m.message, m.model, m.id)}>
              {sendBusy[m.id] ? "Sending…" : "Send now"}
            </button>
          </div>
        </div>
      {/each}
      <button class="test-btn" onclick={addMessage}>+ Add scheduled message</button>

      <!-- 5-hour window priming -->
      <div class="section">5-hour window priming</div>
      <label class="check">
        <input type="checkbox" bind:checked={cfg.priming.enabled} onchange={saveCfg} />
        <span>Auto-prime 5-hour windows</span>
      </label>
      <p class="hint">Sends a tiny {cfg.priming.model} message at each anchor so a fresh 5-hour window starts early — letting you line windows up with your day and fit 3 in a day instead of 2. A slot is skipped when a window is already running.</p>

      <PrimingTimeline
        bind:anchor={cfg.priming.anchor_time}
        windowsPerDay={cfg.priming.windows_per_day}
        slotSlackSecs={cfg.priming.slot_slack_secs}
        session={sessionWindow}
        onchange={saveCfg}
      />
      <p class="hint">Drag to set when the first window starts. Blue = primed windows · green = your current live window · dashed = now.</p>

      <div class="field">
        <div class="row"><span>First window starts</span>
          <input class="time" type="time" bind:value={cfg.priming.anchor_time} onchange={saveCfg} /></div>
      </div>
      <div class="field">
        <div class="row"><span>Windows per day</span>
          <input type="number" min="1" max="5" step="1" bind:value={cfg.priming.windows_per_day} onchange={saveCfg} /></div>
      </div>
      <div class="field">
        <div class="row"><span>Prime model</span>
          <select bind:value={cfg.priming.model} onchange={saveCfg}>
            {#each MODELS as mo}<option value={mo}>{mo}</option>{/each}
          </select></div>
      </div>
      <div class="field">
        <div class="row"><span>Slot slack (s)</span>
          <input type="number" min="0" max="120" step="1" bind:value={cfg.priming.slot_slack_secs} onchange={saveCfg} /></div>
        <p class="hint">Extra seconds added to each 5h step so a prime lands just after the previous window resets, never on the boundary.</p>
      </div>
      <label class="check">
        <input type="checkbox" checked={cfg.priming.end_of_day !== null}
          onchange={(e) => { if (cfg) { cfg.priming.end_of_day = (e.target as HTMLInputElement).checked ? "22:00" : null; saveCfg(); } }} />
        <span>No primes after a cutoff time</span>
      </label>
      {#if cfg.priming.end_of_day !== null}
        <div class="field">
          <div class="row"><span>Cutoff time</span>
            <input class="time" type="time" bind:value={cfg.priming.end_of_day} onchange={saveCfg} /></div>
        </div>
      {/if}

      {#if sendLog.length}
        <div class="section">Recent sends</div>
        <div class="sendlog">
          {#each [...sendLog].reverse().slice(0, 8) as o}
            <div class="logrow" class:err={!o.ok}>
              <span class="logtime">{fmtTime(o.ts)}</span>
              <span class="logsrc">{o.source}</span>
              <span class="logstat" class:ok={o.ok}>{outcomeLabel(o)}</span>
              <span class="logdetail">{o.detail}</span>
            </div>
          {/each}
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
  .settings input[type="time"],
  .settings input[type="text"].path {
    background: #12151a;
    border: 1px solid #2c3038;
    color: #e8eaed;
    border-radius: 4px;
    padding: 4px 6px;
    font-size: 12px;
  }
  .settings input[type="text"].path {
    width: 150px;
  }
  code {
    background: #12151a;
    border: 1px solid #2c3038;
    border-radius: 3px;
    padding: 0 3px;
    font-size: 11px;
  }

  /* Scheduled message rows */
  .sched {
    display: grid;
    gap: 6px;
    padding: 8px;
    border: 1px solid #2c3038;
    border-radius: 6px;
    background: #191c22;
  }
  .sched-head {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .sched-head .time {
    margin-left: auto;
  }
  .icon-btn {
    background: transparent;
    border: none;
    color: #8b9096;
    cursor: pointer;
    font-size: 13px;
    padding: 2px 4px;
    border-radius: 4px;
  }
  .icon-btn:hover {
    background: #2c3038;
    color: #e8eaed;
  }
  .days {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .day {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: 1px solid #2c3038;
    background: #12151a;
    color: #6b7178;
    font-size: 11px;
    cursor: pointer;
    padding: 0;
  }
  .day.on {
    background: #2c4a7a;
    border-color: #3b82f6;
    color: #e8eaed;
  }
  .days-hint {
    font-size: 11px;
    color: #8b9096;
    margin-left: 4px;
  }
  .msg {
    background: #12151a;
    border: 1px solid #2c3038;
    color: #e8eaed;
    border-radius: 4px;
    padding: 5px 6px;
    font-size: 12px;
    font-family: inherit;
    resize: vertical;
  }
  .sched-foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .test-btn.small {
    margin-top: 0;
    padding: 4px 8px;
    font-size: 11px;
  }
  .test-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  /* Send log */
  .sendlog {
    display: grid;
    gap: 3px;
  }
  .logrow {
    display: flex;
    gap: 8px;
    font-size: 11px;
    color: #9aa0a6;
    align-items: baseline;
  }
  .logtime {
    color: #6b7178;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .logsrc {
    color: #c7cbd1;
    white-space: nowrap;
  }
  .logstat {
    color: #f59e0b;
    white-space: nowrap;
  }
  .logstat.ok {
    color: #22c55e;
  }
  .logrow.err .logstat {
    color: #ef4444;
  }
  .logdetail {
    color: #8b9096;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
