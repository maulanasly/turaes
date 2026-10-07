import { html } from "../lib/html.js";

// Product icon mark (icon-only, no baked-in wordmark) served from the
// embedded turaes-web-assets favicon PNGs. Single blue set for both themes —
// the rounded-square icon reads on dark graphite and warm paper alike.
// Decorative — always paired with the lowercase "turaes" wordmark text.
export function BrandMark({ size = 22, theme }) {
  void theme;
  const src = "/brand/turaes-icon-light-48.png";
  const srcset = "/brand/turaes-icon-light-48.png 1x, /brand/turaes-icon-light-192.png 4x";
  return html`<img class="brand-mark" src=${src} srcset=${srcset} width=${size} height=${size} alt="" aria-hidden="true" />`;
}
