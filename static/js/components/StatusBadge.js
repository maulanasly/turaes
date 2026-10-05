import { html } from "../lib/html.js";

const LABELS = {
  running: "Running",
  stopped: "Stopped",
  unhealthy: "Unhealthy",
  failed: "Failed",
  deploying: "Deploying",
  unknown: "Unknown",
};

const GLYPHS = {
  running: "●",
  stopped: "■",
  unhealthy: "▲",
  failed: "✕",
  deploying: "◐",
  unknown: "?",
};

export function statusLabel(status) {
  return LABELS[status] || status || "Unknown";
}

export function StatusBadge({ status }) {
  const key = LABELS[status] ? status : "unknown";
  return html`<span class=${"badge " + key}><span aria-hidden="true" class="badge-glyph">${GLYPHS[key]}</span>${statusLabel(status)}</span>`;
}
