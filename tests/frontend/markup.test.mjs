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
