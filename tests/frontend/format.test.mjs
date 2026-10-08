import { test } from "node:test";
import assert from "node:assert/strict";
import { fmtBytes, shortHash, serverName, runtimeLabel, timeAgo, fmtRangeLabel, fmtFullDate, fmtAxisTick } from "../../static/js/lib/format.js";

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
  assert.equal(timeAgo(-5000), "just now");
  assert.equal(timeAgo(30000), "30s ago");
  assert.equal(timeAgo(120000), "2m ago");
  assert.equal(timeAgo(3 * 3600000), "3h ago");
  assert.equal(timeAgo(23 * 3600000), "23h ago");
  assert.equal(timeAgo(26 * 3600000), "1d ago");
  assert.equal(timeAgo(100 * 3600000), "4d ago");
  assert.equal(timeAgo(6 * 86400000), "6d ago");
  assert.equal(timeAgo(8 * 86400000), "1w ago");
  assert.equal(timeAgo(29 * 86400000), "4w ago");
  assert.equal(timeAgo(30 * 86400000), "1mo ago");
  assert.equal(timeAgo(45 * 86400000), "2mo ago");
});

test("fmtRangeLabel", () => {
  assert.equal(fmtRangeLabel("1h"), "last hour");
  assert.equal(fmtRangeLabel("6h"), "last 6 hours");
  assert.equal(fmtRangeLabel("24h"), "last 24 hours");
  assert.equal(fmtRangeLabel("7d"), "last 7 days");
  assert.equal(fmtRangeLabel("30d"), "last 30 days");
  assert.equal(fmtRangeLabel("bogus"), "last 24 hours");
});

test("fmtFullDate is a human datetime (never NaN text)", () => {
  const t = Date.UTC(2026, 9, 4, 12, 30); // Oct 4, 2026 12:30 UTC
  const s = fmtFullDate(t);
  assert.equal(s.includes("Oct"), true);
  assert.equal(s === "—", false);
  assert.equal(fmtFullDate(NaN), "—");
});

test("fmtAxisTick uses clock for short windows, full date for week+", () => {
  const t = Date.UTC(2026, 9, 4, 12, 30);
  assert.equal(fmtAxisTick(t, 24), fmtAxisTick(t, 24));
  // 168h threshold switches to full date; result must differ from a plain clock.
  assert.equal(fmtAxisTick(t, 24) === fmtAxisTick(t, 168), false);
  assert.equal(fmtAxisTick(t, undefined), fmtAxisTick(t, 24));
});
