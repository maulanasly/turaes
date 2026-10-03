import { html } from "../lib/html.js";

const LABELS = {
  running: "Running",
  stopped: "Stopped",
  unhealthy: "Unhealthy",
  failed: "Failed",
  deploying: "Deploying",
  unknown: "Unknown",
};

export function statusLabel(status) {
  return LABELS[status] || status || "Unknown";
}

export function StatusBadge({ status }) {
  return html`<span class=${"badge " + (status || "unknown")}>${statusLabel(status)}</span>`;
}
