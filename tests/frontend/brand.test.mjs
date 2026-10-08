import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const asset = (name) => new URL(`../../static/brand/${name}`, import.meta.url);

test("brand marks are theme-specific, text-free vectors using palette tokens", () => {
  const dark = readFileSync(asset("turaes-mark-dark.svg"), "utf8");
  const light = readFileSync(asset("turaes-mark-light.svg"), "utf8");
  // Dark: teal body + cream accents; light: lagoon body + dark ink.
  assert.match(dark, /#17c3b2/);
  assert.match(dark, /#fef9ef/);
  assert.match(light, /#227c9d/);
  assert.ok(dark !== light, "dark/light marks must diverge");
  assert.doesNotMatch(dark, /<text\b/);
  assert.doesNotMatch(light, /<text\b/);
});

test("favicon assets include square PNG sizes for browser and touch use", () => {
  for (const variant of ["dark", "light"]) {
    for (const size of [16, 32, 48, 192, 512]) {
      const png = readFileSync(asset(`turaes-icon-${variant}-${size}.png`));
      assert.equal(png.toString("hex", 0, 8), "89504e470d0a1a0a");
      assert.equal(png.readUInt32BE(16), size);
      assert.equal(png.readUInt32BE(20), size);
    }
  }
  // Favicon-derived sizes are theme-neutral; the 512 icon is the themed mark.
  for (const size of [16, 32, 48, 192]) {
    const dark = readFileSync(asset(`turaes-icon-dark-${size}.png`));
    const light = readFileSync(asset(`turaes-icon-light-${size}.png`));
    assert.ok(dark.equals(light), `dark/light ${size}px diverge; single set expected`);
  }
  const dark512 = readFileSync(asset("turaes-icon-dark-512.png"));
  const light512 = readFileSync(asset("turaes-icon-light-512.png"));
  assert.ok(!dark512.equals(light512), "512px icons must be theme-specific");
});

test("BrandMark renders the theme-specific vector mark", () => {
  const src = readFileSync(new URL("../../static/js/components/Brand.js", import.meta.url), "utf8");
  assert.match(src, /turaes-mark-\$\{variant\}\.svg/);
  assert.doesNotMatch(src, /turaes-icon-light-48\.png/);
});

test("document links adaptive SVG and theme-appropriate PNG fallbacks", () => {
  const html = readFileSync(new URL("../../static/index.html", import.meta.url), "utf8");
  assert.match(html, /turaes-mark-dark\.svg/);
  assert.match(html, /turaes-mark-light\.svg/);
  assert.match(html, /favicon\.ico/);
  assert.match(html, /turaes-icon-light-32\.png/);
  assert.match(html, /turaes-icon-dark-16\.png/);
  assert.match(html, /turaes-icon-light-16\.png/);
  assert.match(html, /turaes-icon-dark-32\.png/);
  assert.match(html, /turaes-icon-light-32\.png/);
  assert.match(html, /turaes-icon-dark-48\.png/);
  assert.match(html, /turaes-icon-light-48\.png/);
  assert.match(html, /apple-touch-icon.*turaes-icon-light-192\.png/);
});
