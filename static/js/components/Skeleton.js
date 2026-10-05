import { html } from "../lib/html.js";

export function Skeleton({ lines = 3, height = 14 }) {
  return html`
    <div class="skeleton-wrap" role="status">
      <span class="sr-only">Loading…</span>
      <div class="skeleton-bars" aria-hidden="true">
        ${Array.from({ length: lines }).map(
          () => html`<div class="skeleton" style=${`height:${height}px`}></div>`,
        )}
      </div>
    </div>`;
}
