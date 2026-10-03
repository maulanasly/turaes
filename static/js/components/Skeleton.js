import { html } from "../lib/html.js";

export function Skeleton({ lines = 3, height = 14 }) {
  return html`
    <div class="skeleton-wrap" aria-hidden="true">
      ${Array.from({ length: lines }).map(
        () => html`<div class="skeleton" style=${`height:${height}px`}></div>`,
      )}
    </div>`;
}
