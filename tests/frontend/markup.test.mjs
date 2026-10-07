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
