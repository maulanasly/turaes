import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../static/js/", import.meta.url));

function jsFiles(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    if (entry.isDirectory() && entry.name === "vendor") return [];
    const path = `${dir}/${entry.name}`;
    if (entry.isDirectory()) return jsFiles(path);
    return entry.isFile() && entry.name.endsWith(".js") ? [path] : [];
  });
}

test("frontend templates do not contain literal non-breaking-space entities", () => {
  const entity = "&" + "nbsp;";
  for (const path of jsFiles(root)) {
    assert.ok(!readFileSync(path, "utf8").includes(entity), `${path} contains a literal ${entity}`);
  }
});

test("error-wired inputs carry names so errors can focus them", () => {
  // Every control that renders aria-invalid for a field error must be
  // reachable through form.elements.namedItem(name).
  for (const file of ["views/AppsView.js", "views/AppDetailView.js"]) {
    const src = readFileSync(new URL(`../../static/js/${file}`, import.meta.url), "utf8");
    for (const match of src.matchAll(/<(input|select|textarea)\b([^>]*?)>/g)) {
      const [tag, attrs] = [match[1], match[2]];
      if (!/aria-invalid/.test(attrs)) continue;
      assert.ok(
        /\bname="/.test(attrs),
        `${file} has an error-wired but nameless <${tag}>: ${match[0].slice(0, 80)}`
      );
    }
  }
});

test("topbar uses a flat primary nav with the account section in an avatar menu", () => {
  const src = readFileSync(new URL("../../static/js/app.js", import.meta.url), "utf8");
  // No grouped clusters: primary nav is Applications + Catalog + Servers.
  assert.ok(src.includes('class="nav-list"'), "primary nav renders a flat nav-list");
  assert.ok(!src.includes("nav-group"), "grouped nav clusters are gone");
  assert.ok(!src.includes("nav-label"), "nav section labels are gone");
  assert.ok(!src.includes("brand-tag"), "header descriptor is out of the topbar");
  // Infrequent destinations live in the account menu, not the primary nav.
  assert.ok(src.includes('class="avatar-menu"'), "account menu exists");
  assert.ok(src.includes('href="#/org"'), "Organization lives in the account menu");
  assert.ok(src.includes('href="#/tokens"') || src.includes("#/tokens"), "Tokens stays reachable");
  assert.ok(src.includes('href="#/about"'), "About lives in the account menu");
  // Single status signal: API reachability wins over fleet counts.
  assert.ok(src.includes("API unreachable"), "API-down has a distinct status");
  assert.ok(!src.includes("${health ? \"Healthy\""), "separate Healthy/Unreachable text is gone");
  // Triage aid: firing alerts badge on Applications.
  assert.ok(src.includes("nav-badge"), "Applications carries an attention badge");
  // Catalog is a first-class peer in the primary nav, not the account menu.
  assert.ok(src.includes('view="catalog"'), "Catalog is a primary nav entry");
  // Disclosures stay accessible.
  assert.ok(src.includes('aria-haspopup="menu"'), "avatar button exposes the menu");
  assert.ok(src.includes('aria-expanded=${userMenuOpen}'), "avatar menu reports expanded state");
});

test("app detail exposes the maintenance toggle and parked hint", () => {
  const src = readFileSync(new URL("../../static/js/views/AppDetailView.js", import.meta.url), "utf8");
  assert.ok(src.includes("/maintenance"), "detail view calls the maintenance endpoint");
  assert.ok(src.includes("Maintenance on") && src.includes("Maintenance off"), "toggle labels both states");
  assert.ok(src.includes("Visitors see the maintenance page"), "parked hint explains visitor impact");
});
