import { html } from "../lib/html.js";

// Product icon mark (icon-only, no baked-in wordmark) served from the
// embedded brand assets. Decorative — always paired with the lowercase
// "turaes" wordmark text.
export function BrandMark({ size = 22 }) {
  const src = size <= 32 ? "/brand/favicon-32.png" : "/brand/favicon-48.png";
  return html`<img class="brand-mark" src=${src} width=${size} height=${size} alt="" aria-hidden="true" />`;
}
