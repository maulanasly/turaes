// Contrast audit: parse the design tokens straight out of the stylesheet and
// assert WCAG ratios so a palette change cannot silently regress contrast.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const css = readFileSync(new URL("../../static/css/styles.css", import.meta.url), "utf8");

function tokens(selector) {
  const block = css.match(new RegExp(`${selector}\\s*\\{([^}]*)\\}`));
  assert.ok(block, `missing token block ${selector}`);
  const out = {};
  for (const line of block[1].split(";")) {
    const m = line.match(/--([\w-]+):\s*(#[0-9a-fA-F]{6})/);
    if (m) out[m[1]] = m[2];
  }
  return out;
}

function channel(v) {
  const c = v / 255;
  return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
}

function luminance(hex) {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function ratio(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

const THEMES = { dark: tokens(":root"), light: tokens('\\[data-theme="light"\\]') };

// Pairs that only need 3:1 (non-text UI boundaries, focus rings).
const NON_TEXT = [
  ["control-border", "bg"],
  ["control-border", "panel"],
  ["control-border", "panel-2"],
  ["signal", "panel"],
];

for (const [name, t] of Object.entries(THEMES)) {
  test(`${name}: body text meets AA (4.5:1)`, () => {
    const pairs = [
      ["text", "bg"], ["text", "panel"], ["text", "panel-2"],
      ["muted", "bg"], ["muted", "panel"], ["muted", "panel-2"],
      ["accent", "panel"], ["accent", "panel-2"],
      ["signal-ink", "signal"],
    ];
    for (const [fg, bg] of pairs) {
      const r = ratio(t[fg], t[bg]);
      assert.ok(r >= 4.5, `${name} --${fg} on --${bg} = ${r.toFixed(2)} (< 4.5)`);
    }
  });

  test(`${name}: UI boundaries and focus ring meet 3:1`, () => {
    for (const [fg, bg] of NON_TEXT) {
      const r = ratio(t[fg], t[bg]);
      assert.ok(r >= 3, `${name} --${fg} on --${bg} = ${r.toFixed(2)} (< 3)`);
    }
  });

  test(`${name}: accent focus ring meets 3:1 on the page`, () => {
    const r = ratio(t.accent, t.bg);
    assert.ok(r >= 3, `${name} --accent on --bg = ${r.toFixed(2)} (< 3)`);
  });
}
