import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";

const asset = (name) => new URL(`../../static/brand/${name}`, import.meta.url);

test("brand icons come from the turaes-web-assets favicon set (single blue set)", () => {
  // No SVG marks remain: the header/about/login mark is the icon-only
  // favicon PNG, identical for both themes (reads on graphite and paper).
  const files = readdirSync(new URL("../../static/brand/", import.meta.url));
  assert.ok(!files.some((f) => f.endsWith(".svg")), `stale SVG brand assets: ${files}`);
  for (const size of [16, 32, 48, 192, 512]) {
    const dark = readFileSync(asset(`turaes-icon-dark-${size}.png`));
    const light = readFileSync(asset(`turaes-icon-light-${size}.png`));
    assert.equal(dark.toString("hex", 0, 8), "89504e470d0a1a0a");
    assert.equal(dark.readUInt32BE(16), size);
    assert.equal(dark.readUInt32BE(20), size);
    assert.ok(dark.equals(light), `dark/light ${size}px diverge; single set expected`);
  }
});

test("BrandMark renders the PNG icon with no baked-in wordmark", () => {
  const src = readFileSync(new URL("../../static/js/components/Brand.js", import.meta.url), "utf8");
  assert.doesNotMatch(src, /turaes-mark-/);
  assert.match(src, /turaes-icon-light-48\.png/);
  assert.match(src, /turaes-icon-light-192\.png/);
});

test("document links PNG favicons with no SVG fallback", () => {
  const html = readFileSync(new URL("../../static/index.html", import.meta.url), "utf8");
  assert.doesNotMatch(html, /turaes-favicon\.svg/);
  assert.match(html, /turaes-icon-light-32\.png/);
  assert.match(html, /turaes-icon-dark-16\.png/);
  assert.match(html, /turaes-icon-light-16\.png/);
  assert.match(html, /turaes-icon-dark-32\.png/);
  assert.match(html, /turaes-icon-dark-48\.png/);
  assert.match(html, /turaes-icon-light-48\.png/);
  assert.match(html, /apple-touch-icon.*turaes-icon-light-192\.png/);
});
