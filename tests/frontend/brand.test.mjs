import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const asset = (name) => new URL(`../../static/brand/${name}`, import.meta.url);

test("brand marks are theme-specific, text-free vectors using palette tokens", () => {
  const dark = readFileSync(asset("turaes-mark-dark.svg"), "utf8");
  const light = readFileSync(asset("turaes-mark-light.svg"), "utf8");
  assert.match(dark, /#818cf8/);
  assert.match(dark, /#67e8f9/);
  assert.match(light, /#4f46e5/);
  assert.match(light, /#067481/);
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
});

test("document links adaptive SVG and theme-appropriate PNG fallbacks", () => {
  const html = readFileSync(new URL("../../static/index.html", import.meta.url), "utf8");
  assert.match(html, /turaes-favicon\.svg/);
  assert.match(html, /turaes-icon-dark-16\.png/);
  assert.match(html, /turaes-icon-light-16\.png/);
  assert.match(html, /turaes-icon-dark-32\.png/);
  assert.match(html, /turaes-icon-light-32\.png/);
  assert.match(html, /turaes-icon-dark-48\.png/);
  assert.match(html, /turaes-icon-light-48\.png/);
  assert.match(html, /apple-touch-icon.*turaes-icon-light-192\.png/);
});
