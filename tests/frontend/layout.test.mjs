// Layout audit: parse the stylesheet and assert the spacing rules that keep
// label/value rows (status strips, cards, fleet strips) from overlapping, so
// a future cleanup cannot silently reintroduce the collision.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const css = readFileSync(new URL("../../static/css/styles.css", import.meta.url), "utf8");

function rule(selector) {
  const block = css.match(new RegExp(`${selector}\\s*\\{([^}]*)\\}`));
  assert.ok(block, `missing rule ${selector}`);
  return block[1];
}

test("stat rows keep a gap between label and value", () => {
  assert.match(rule("\\.stat"), /gap:\s*8px/);
});

test("status-strip values shrink and wrap instead of overlapping", () => {
  const body = rule("\\.status-strip \\.stat span:last-child");
  assert.match(body, /min-width:\s*0/);
  assert.match(body, /overflow-wrap:\s*anywhere/);
});

test("panel-head blocks can shrink so strips wrap under actions", () => {
  assert.match(rule("\\.panel-head > div"), /min-width:\s*0/);
});

test("More disclosure matches sibling header buttons (not small)", () => {
  const src = readFileSync(
    new URL("../../static/js/views/AppDetailView.js", import.meta.url),
    "utf8"
  );
  assert.doesNotMatch(src, /<summary class="btn small ghost"/);
  assert.match(src, /<summary class="btn ghost" aria-label="More actions">/);
});

test("non-running apps explain the maintenance page", () => {
  const src = readFileSync(
    new URL("../../static/js/views/AppDetailView.js", import.meta.url),
    "utf8"
  );
  assert.match(src, /Visitors see the maintenance page/);
});
