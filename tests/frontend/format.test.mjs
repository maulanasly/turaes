import { test } from "node:test";
import assert from "node:assert/strict";
import { fmtBytes, shortHash, serverName, runtimeLabel, timeAgo } from "../../static/js/lib/format.js";

test("fmtBytes scales", () => {
  assert.equal(fmtBytes(0), "0 B");
  assert.equal(fmtBytes(512), "512 B");
  assert.equal(fmtBytes(2048), "2.0 KB");
  assert.equal(fmtBytes(1572864), "1.5 MB");
});

test("shortHash trims the sha256 prefix", () => {
  const h = "sha256:" + "a".repeat(64);
  assert.equal(shortHash(h).startsWith("aaaaaaa"), true);
  assert.equal(shortHash(null), "—");
});

test("serverName resolves id -> name", () => {
  const servers = [{ id: "s1", name: "worker-1" }];
  assert.equal(serverName(servers, "s1"), "worker-1");
  assert.equal(serverName(servers, "local"), "local");
  assert.equal(serverName([], "abc"), "abc");
});

test("runtimeLabel is human", () => {
  assert.equal(runtimeLabel("systemd"), "systemd");
  assert.equal(runtimeLabel("proc"), "turaes (proc)");
});

test("timeAgo", () => {
  assert.equal(timeAgo(1000), "just now");
  assert.equal(timeAgo(30000), "30s ago");
  assert.equal(timeAgo(120000), "2m ago");
});
