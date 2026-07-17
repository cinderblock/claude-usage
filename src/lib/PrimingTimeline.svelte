<script lang="ts">
  import { onMount } from "svelte";

  interface SessionWindow {
    startMs: number;
    endMs: number;
  }

  let {
    anchor = $bindable(),
    windowsPerDay,
    slotSlackSecs = 15,
    session = null,
    onchange,
  }: {
    anchor: string;
    windowsPerDay: number;
    slotSlackSecs?: number;
    session?: SessionWindow | null;
    onchange?: () => void;
  } = $props();

  // Geometry. 0..1440 local minutes map across the inner width.
  const W = 520;
  const H = 78;
  const PAD = 10;
  const innerW = W - PAD * 2;
  const trackY = 30;
  const trackH = 24;

  const minToX = (m: number) => PAD + (Math.max(0, Math.min(1440, m)) / 1440) * innerW;
  const WINDOW_MIN = 5 * 60; // a 5h window, for block width

  function parseHHMM(s: string): number {
    const [h, m] = (s ?? "").split(":").map((x) => parseInt(x, 10));
    return (Number.isFinite(h) ? h : 0) * 60 + (Number.isFinite(m) ? m : 0);
  }
  function fmtHHMM(mins: number): string {
    mins = ((Math.round(mins) % 1440) + 1440) % 1440;
    const h = Math.floor(mins / 60);
    const m = mins % 60;
    return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}`;
  }

  const anchorMin = $derived(parseHHMM(anchor));
  const stepMin = $derived(WINDOW_MIN + slotSlackSecs / 60);

  // Slots for today (those whose start lands before midnight). Later ones spill
  // into tomorrow and aren't drawn on today's bar.
  const slots = $derived(
    Array.from({ length: Math.max(0, windowsPerDay) }, (_, k) => ({
      k,
      startMin: anchorMin + k * stepMin,
    })).filter((s) => s.startMin < 1440),
  );

  const sessionMins = $derived(
    session
      ? {
          startMin: localMinutes(session.startMs),
          endMin: localMinutes(session.endMs),
        }
      : null,
  );

  function localMinutes(ms: number): number {
    const d = new Date(ms);
    return d.getHours() * 60 + d.getMinutes();
  }

  // Live "now" marker, refreshed each minute.
  let nowMin = $state(nowLocalMinutes());
  function nowLocalMinutes(): number {
    const d = new Date();
    return d.getHours() * 60 + d.getMinutes();
  }
  onMount(() => {
    const t = setInterval(() => (nowMin = nowLocalMinutes()), 30_000);
    return () => clearInterval(t);
  });

  let svgEl: SVGSVGElement;
  let dragging = $state(false);

  function xToMin(clientX: number): number {
    const rect = svgEl.getBoundingClientRect();
    const frac = (clientX - rect.left - PAD) / innerW;
    const m = Math.round((frac * 1440) / 5) * 5; // snap to 5 minutes
    return Math.max(0, Math.min(1435, m));
  }
  function onPointerDown(e: PointerEvent) {
    dragging = true;
    svgEl.setPointerCapture(e.pointerId);
    anchor = fmtHHMM(xToMin(e.clientX));
    e.preventDefault();
  }
  function onPointerMove(e: PointerEvent) {
    if (!dragging) return;
    anchor = fmtHHMM(xToMin(e.clientX));
  }
  function onPointerUp(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    try {
      svgEl.releasePointerCapture(e.pointerId);
    } catch {}
    onchange?.();
  }

  const hourTicks = [0, 3, 6, 9, 12, 15, 18, 21, 24];
</script>

<svg
  bind:this={svgEl}
  viewBox={`0 0 ${W} ${H}`}
  class="timeline"
  role="slider"
  aria-label="First 5-hour window start time"
  aria-valuemin={0}
  aria-valuemax={1435}
  aria-valuenow={anchorMin}
  aria-valuetext={anchor}
  tabindex="0"
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={onPointerUp}
  onpointercancel={onPointerUp}
>
  <!-- Hour grid + labels -->
  {#each hourTicks as h}
    <line x1={minToX(h * 60)} y1={trackY - 4} x2={minToX(h * 60)} y2={trackY + trackH + 4} class="grid" />
    <text x={minToX(h * 60)} y={trackY - 8} class="hourlabel">{h}</text>
  {/each}

  <!-- Track -->
  <rect x={PAD} y={trackY} width={innerW} height={trackH} rx="4" class="track" />

  <!-- Current session window (if any) -->
  {#if sessionMins}
    <rect
      x={minToX(sessionMins.startMin)}
      y={trackY + 2}
      width={Math.max(2, minToX(sessionMins.endMin) - minToX(sessionMins.startMin))}
      height={trackH - 4}
      rx="3"
      class="session"
    />
  {/if}

  <!-- Prime window blocks -->
  {#each slots as s}
    <rect
      x={minToX(s.startMin)}
      y={trackY + 2}
      width={Math.max(2, minToX(Math.min(1440, s.startMin + WINDOW_MIN)) - minToX(s.startMin))}
      height={trackH - 4}
      rx="3"
      class="block"
    />
    <text x={minToX(s.startMin) + 3} y={trackY + trackH + 13} class="blocklabel">
      {fmtHHMM(s.startMin)}–{fmtHHMM(s.startMin + WINDOW_MIN)}
    </text>
  {/each}

  <!-- Now marker -->
  <line x1={minToX(nowMin)} y1={trackY - 6} x2={minToX(nowMin)} y2={trackY + trackH + 6} class="now" />

  <!-- Anchor handle -->
  <line x1={minToX(anchorMin)} y1={trackY - 7} x2={minToX(anchorMin)} y2={trackY + trackH + 7} class="anchor" />
  <circle cx={minToX(anchorMin)} cy={trackY - 7} r="5" class="anchor-knob" />
</svg>

<style>
  .timeline {
    width: 100%;
    height: auto;
    touch-action: none;
    cursor: ew-resize;
    user-select: none;
    display: block;
  }
  .track {
    fill: #12151a;
    stroke: #2c3038;
  }
  .grid {
    stroke: #2c3038;
    stroke-width: 1;
  }
  .hourlabel {
    fill: #6b7178;
    font-size: 9px;
    text-anchor: middle;
    font-family: "Segoe UI", system-ui, sans-serif;
  }
  .block {
    fill: #3b82f6;
    fill-opacity: 0.55;
    stroke: #3b82f6;
    stroke-opacity: 0.9;
  }
  .blocklabel {
    fill: #9aa0a6;
    font-size: 9px;
    font-variant-numeric: tabular-nums;
    font-family: "Segoe UI", system-ui, sans-serif;
  }
  .session {
    fill: #22c55e;
    fill-opacity: 0.35;
    stroke: #22c55e;
    stroke-opacity: 0.8;
  }
  .now {
    stroke: #e8eaed;
    stroke-width: 1;
    stroke-dasharray: 2 2;
    opacity: 0.6;
  }
  .anchor {
    stroke: #f59e0b;
    stroke-width: 2;
  }
  .anchor-knob {
    fill: #f59e0b;
  }
</style>
