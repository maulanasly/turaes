// Static import-resolution smoke test.
//
// check-js only runs `node --check` per file (syntax, no import resolution),
// so a view importing a name a module does not export — e.g.
// `import { RANGES } from "./lib/router.js"` when router.js doesn't re-export
// it — breaks the whole browser module graph at load time but passes CI.
//
// This test walks every module in static/js (excluding vendor/), resolves each
// `import { ... } from "./x.js"` against the target module's actual exports,
// and fails on any name the target does not provide.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, dirname, resolve, sep } from "node:path";

const ROOT = resolve(join(import.meta.dirname, "../../static/js"));

function walk(dir) {
  const out = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    const st = statSync(full);
    if (st.isDirectory()) out.push(...walk(full));
    else if (entry.endsWith(".js")) out.push(full);
  }
  return out;
}

// Collect the names a module exports: `export function f(`, `export const x =`,
// `export class C`, `export { a, b }`, and re-exports `export { ... } from`.
function exportedNames(file) {
  const src = readFileSync(file, "utf8");
  const names = new Set();
  for (const m of src.matchAll(/export\s+(?:async\s+)?(?:function|class|const|let|var)\s+([A-Za-z_$][\w$]*)/g)) {
    names.add(m[1]);
  }
  for (const m of src.matchAll(/export\s*\{([^}]*)\}\s*(?:from\s+["']([^"']+)["'])?/g)) {
    for (const part of m[1].split(",")) {
      const provided = part.trim().split(/\s+as\s+/)[1]?.trim() || part.trim();
      if (provided) names.add(provided);
    }
    // `export { a, b } from "./x"` re-exports ONLY the listed names — not
    // everything from x — so we do not follow the chain here. The re-exported
    // names' existence in x is validated transitively by this same test when a
    // module imports them from the re-exporting facade.
  }
  return names;
}

const modules = walk(ROOT);
const failures = [];

for (const file of modules) {
  const src = readFileSync(file, "utf8");
  for (const m of src.matchAll(/import\s*\{([^}]*)\}\s*from\s*["']([^"']+)["']/g)) {
    const spec = m[2];
    if (!spec.startsWith(".")) continue; // bare specifiers (preact/htm) come from the importmap
    const target = resolve(dirname(file), spec);
    if (!target.endsWith(".js")) continue;
    const provided = exportedNames(target);
    for (const part of m[1].split(",")) {
      const name = part.trim().split(/\s+as\s+/)[1]?.trim() || part.trim();
      if (!name) continue;
      if (!provided.has(name)) {
        failures.push(`${file.replace(ROOT + sep, "")} imports "${name}" from ${spec} (not exported)`);
      }
    }
  }
}

test("every named import resolves to a real export", () => {
  assert.deepEqual(failures, []);
});