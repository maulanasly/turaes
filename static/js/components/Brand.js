import { html } from "../lib/html.js";

// Ownable route/process mark: two nodes joined by a path, drawn in the
// theme-aware signal tokens so it stays legible in dark and light mode.
// Decorative — always paired with the "turaes" wordmark text.
export function BrandMark({ size = 22 }) {
  return html`<svg class="brand-mark" width=${size} height=${size} viewBox="0 0 24 24" aria-hidden="true" focusable="false">
    <rect width="24" height="24" rx="6" fill="var(--signal)" />
    <path d="M6.5 17.5 C 11 17.5, 11 6.5, 17.5 6.5" stroke="var(--signal-ink)" stroke-width="2" fill="none" stroke-linecap="round" />
    <circle cx="6.5" cy="17.5" r="2.6" fill="var(--signal-ink)" />
    <circle cx="17.5" cy="6.5" r="2.6" fill="var(--signal-ink)" />
  </svg>`;
}
