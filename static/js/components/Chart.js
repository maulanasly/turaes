import { html } from "../lib/html.js";
import { fmtAxisTick } from "../lib/format.js";

// Bucket-average downsampling so 30d (up to ~43k 1-min rollups) stays crisp.
function downsample(points, max = 500) {
  if (!points || points.length <= max) return points || [];
  const bucket = Math.ceil(points.length / max);
  const out = [];
  for (let i = 0; i < points.length; i += bucket) {
    const slice = points.slice(i, i + bucket);
    const x = slice[Math.floor(slice.length / 2)].x;
    const y = slice.reduce((s, p) => s + p.y, 0) / slice.length;
    out.push({ x, y });
  }
  return out;
}

// Dependency-free SVG line/area chart with an accessible summary.
// `subtitle` renders under the latest/peak line (e.g. range + freshness).
// `rangeHours` switches axis ticks to full dates for week+ windows.
// `emptyHint` explains empty windows (retention, downtime) instead of "No data yet".
export function Chart({ title, points, color, formatY, height = 150, subtitle, rangeHours = 24, emptyHint }) {
  const w = 600;
  const pad = { l: 58, r: 8, t: 8, b: 24 };
  const has = points && points.length > 0;

  if (!has) {
    return html`<div class="chart">
      <div class="chart-title"><span>${title}</span>${subtitle && html`<span class="muted">${subtitle}</span>`}</div>
      <div class="empty">${emptyHint || "No data yet"}</div>
    </div>`;
  }

  const sampled = downsample(points);
  const xs = sampled.map((p) => p.x);
  const ys = sampled.map((p) => p.y);
  let x0 = Math.min(...xs), x1 = Math.max(...xs);
  let y0 = Math.min(...ys, 0), y1 = Math.max(...ys);
  if (x1 === x0) x1 = x0 + 1;
  if (y1 === y0) y1 = y0 + 1;
  const X = (x) => pad.l + ((x - x0) / (x1 - x0)) * (w - pad.l - pad.r);
  const Y = (y) => height - pad.b - ((y - y0) / (y1 - y0)) * (height - pad.t - pad.b);
  const path = sampled.map((p, i) => `${i ? "L" : "M"}${X(p.x).toFixed(1)},${Y(p.y).toFixed(1)}`).join(" ");
  const area = `${path} L${X(x1).toFixed(1)},${height - pad.b} L${X(x0).toFixed(1)},${height - pad.b} Z`;
  const last = sampled[sampled.length - 1];
  const desc = `${title}${subtitle ? ` (${subtitle})` : ""}. Latest ${formatY(last.y)}, peak ${formatY(y1)}.`;
  // Screen-reader fallback: latest + peak plus a capped data table.
  const tableRows = sampled.slice(-20);

  return html`
    <div class="chart">
      <div class="chart-title">
        <span>${title}</span>
        <span class="muted">${subtitle ? `${subtitle} · ` : ""}latest ${formatY(last.y)} · peak ${formatY(y1)}</span>
      </div>
      <svg viewBox="0 0 ${w} ${height}" preserveAspectRatio="none" class="spark" role="img" aria-label=${desc}>
        <title>${title}</title>
        <desc>${desc}</desc>
        <line class="grid-line" x1=${pad.l} y1=${Y(y0)} x2=${w - pad.r} y2=${Y(y0)} />
        <line class="grid-line" x1=${pad.l} y1=${Y(y1)} x2=${w - pad.r} y2=${Y(y1)} />
        <path d=${area} fill=${color} opacity="0.12" stroke="none" />
        <path d=${path} fill="none" stroke=${color} stroke-width="2" vector-effect="non-scaling-stroke" />
      </svg>
      <div class="axis">
        <span>${formatY(y0)}</span>
        <span>${fmtAxisTick(x0, rangeHours)}</span>
        <span>${fmtAxisTick(x1, rangeHours)}</span>
      </div>
      <details class="sr-details">
        <summary class="muted small">Data table</summary>
        <table class="sr-only-table">
          <thead><tr><th>Time</th><th>Value</th></tr></thead>
          <tbody>${tableRows.map((p) => html`
            <tr><td>${fmtAxisTick(p.x, rangeHours)}</td><td>${formatY(p.y)}</td></tr>`)}</tbody>
        </table>
      </details>
    </div>`;
}
