import { html } from "../lib/html.js";
import { fmtClock } from "../lib/format.js";

// Dependency-free SVG line/area chart with an accessible summary.
export function Chart({ title, points, color, formatY, height = 150 }) {
  const w = 600;
  const pad = { l: 58, r: 8, t: 8, b: 24 };
  const has = points && points.length > 0;

  if (!has) {
    return html`<div class="chart">
      <div class="chart-title"><span>${title}</span></div>
      <div class="empty">No data yet</div>
    </div>`;
  }

  const xs = points.map((p) => p.x);
  const ys = points.map((p) => p.y);
  let x0 = Math.min(...xs), x1 = Math.max(...xs);
  let y0 = Math.min(...ys, 0), y1 = Math.max(...ys);
  if (x1 === x0) x1 = x0 + 1;
  if (y1 === y0) y1 = y0 + 1;
  const X = (x) => pad.l + ((x - x0) / (x1 - x0)) * (w - pad.l - pad.r);
  const Y = (y) => height - pad.b - ((y - y0) / (y1 - y0)) * (height - pad.t - pad.b);
  const path = points.map((p, i) => `${i ? "L" : "M"}${X(p.x).toFixed(1)},${Y(p.y).toFixed(1)}`).join(" ");
  const area = `${path} L${X(x1).toFixed(1)},${height - pad.b} L${X(x0).toFixed(1)},${height - pad.b} Z`;
  const last = points[points.length - 1];
  const desc = `${title}. Latest ${formatY(last.y)}, peak ${formatY(y1)}.`;

  return html`
    <div class="chart">
      <div class="chart-title">
        <span>${title}</span>
        <span class="muted">latest ${formatY(last.y)} · peak ${formatY(y1)}</span>
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
        <span>${fmtClock(x0)}</span>
        <span>${fmtClock(x1)}</span>
      </div>
    </div>`;
}
