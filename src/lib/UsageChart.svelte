<script lang="ts">
  import type { Sample } from "$lib/usage";

  export interface ChartSeries {
    label: string;
    color: string;
    samples: Sample[];
    projectedPct: number | null;
    currentPct: number;
  }

  let {
    series,
    startMs,
    endMs,
    nowMs,
    markDays = false,
  }: {
    series: ChartSeries[];
    startMs: number;
    endMs: number;
    nowMs: number;
    /** Draw local-date boundaries and weekend shading (for multi-day windows). */
    markDays?: boolean;
  } = $props();

  // Measured pixel width → crisp strokes (no viewBox distortion).
  let w = $state(300);
  const H = 70;

  const span = $derived(Math.max(endMs - startMs, 1));

  // Y-axis tops out at 100% — the meaningful ceiling for every window. Overshoot
  // (usage or projection past 100) rides the top edge rather than wasting the
  // chart on empty headroom above the cap.
  const yMax = 100;

  const xOf = (ts: number) => ((ts - startMs) / span) * w;
  const yOf = (pct: number) => H - (Math.min(Math.max(pct, 0), yMax) / yMax) * H;
  const clampX = (x: number) => Math.max(0, Math.min(w, x));

  // Local-date segments spanning the window: one per calendar day (DST-aware via
  // the Date constructor), tagged as weekend for background shading.
  const days = $derived.by(() => {
    if (!markDays) return [] as { start: number; end: number; weekend: boolean }[];
    const midnight = (ts: number) => {
      const d = new Date(ts);
      return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
    };
    const segs: { start: number; end: number; weekend: boolean }[] = [];
    for (let t = midnight(startMs); t < endMs; ) {
      const d = new Date(t);
      const next = new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1).getTime();
      const dow = d.getDay(); // 0 Sun … 6 Sat
      segs.push({ start: t, end: next, weekend: dow === 0 || dow === 6 });
      t = next;
    }
    return segs;
  });

  function linePath(samples: Sample[]): string {
    if (!samples.length) return "";
    return samples.map((p, i) => `${i ? "L" : "M"}${xOf(p.ts).toFixed(1)},${yOf(p.percent).toFixed(1)}`).join("");
  }

  const nowX = $derived(xOf(nowMs));
</script>

<div class="chart" bind:clientWidth={w}>
  <svg width={w} height={H} role="img">
    <!-- weekend shading + local-date boundaries (multi-day windows only) -->
    {#each days as d (d.start)}
      {#if d.weekend}
        <rect x={clampX(xOf(d.start))} y="0" width={clampX(xOf(d.end)) - clampX(xOf(d.start))} height={H} class="weekend" />
      {/if}
    {/each}
    {#each days as d (d.start)}
      {@const x = xOf(d.start)}
      {#if x > 0.5 && x < w - 0.5}
        <line x1={x} y1="0" x2={x} y2={H} class="day-edge" />
      {/if}
    {/each}
    <!-- even-pace reference: 0% at window start → 100% at reset -->
    <line x1={xOf(startMs)} y1={yOf(0)} x2={xOf(endMs)} y2={yOf(100)} class="pace" />
    <!-- now -->
    {#if nowX >= 0 && nowX <= w}
      <line x1={nowX} y1="0" x2={nowX} y2={H} class="now" />
    {/if}

    {#each series as s (s.label)}
      {@const last = s.samples.at(-1)}
      <!-- projection: last sample → projected at reset -->
      {#if last && s.projectedPct != null && s.projectedPct > last.percent + 0.5}
        <line
          x1={xOf(last.ts)}
          y1={yOf(last.percent)}
          x2={xOf(endMs)}
          y2={yOf(s.projectedPct)}
          stroke={s.color}
          class="proj"
        />
      {/if}
      <!-- actual usage -->
      <path d={linePath(s.samples)} fill="none" stroke={s.color} stroke-width="1.75" />
      {#if last}
        <circle cx={xOf(last.ts)} cy={yOf(last.percent)} r="2.4" fill={s.color} />
      {/if}
    {/each}
  </svg>
</div>

<style>
  .chart {
    width: 100%;
  }
  svg {
    display: block;
    overflow: visible;
  }
  .weekend {
    fill: #ffffff;
    opacity: 0.04;
  }
  .day-edge {
    stroke: #9aa0aa;
    stroke-width: 1;
    stroke-dasharray: 2 3;
    opacity: 0.18;
  }
  .pace {
    stroke: #6b7280;
    stroke-width: 1;
    stroke-dasharray: 3 3;
    opacity: 0.6;
  }
  .now {
    stroke: #e8eaed;
    stroke-width: 1;
    opacity: 0.25;
  }
  .proj {
    stroke-width: 1.5;
    stroke-dasharray: 3 2;
    opacity: 0.55;
  }
</style>
