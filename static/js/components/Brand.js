import { html } from "../lib/html.js";

// Product icon mark (icon-only, no baked-in wordmark) served from the
// embedded theme-specific SVG assets. Teal on graphite, lagoon on paper —
// the mark follows the active palette. Decorative — always paired with the
// lowercase "turaes" wordmark text.
export function BrandMark({ size = 22, theme }) {
  const activeTheme = theme || document.documentElement.dataset.theme || "dark";
  const variant = activeTheme === "light" ? "light" : "dark";
  const src = `/brand/turaes-mark-${variant}.svg`;
  return html`<img class="brand-mark" src=${src} width=${size} height=${size} alt="" aria-hidden="true" />`;
}
