// Formatting helpers (pure, unit-testable).

export function fmtBytes(n) {
  if (!n) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(i > 0 && v < 10 ? 1 : 0)} ${units[i]}`;
}

// SQLite stores "YYYY-MM-DD HH:MM:SS" (UTC).
export function parseTs(s) {
  return Date.parse(String(s).replace(" ", "T") + "Z");
}

export function fmtClock(d) {
  return new Date(d).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

// Full date-time for long chart windows (>= 7d): "Oct 1, 02:30 PM".
export function fmtFullDate(d) {
  const dt = new Date(d);
  if (Number.isNaN(dt.getTime())) return "—";
  return dt.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

// Human label for a monitoring range key ("24h" -> "last 24 hours").
// Unknown keys fall back to the default window.
export function fmtRangeLabel(range) {
  switch (range) {
    case "1h":
      return "last hour";
    case "6h":
      return "last 6 hours";
    case "24h":
      return "last 24 hours";
    case "7d":
      return "last 7 days";
    case "30d":
      return "last 30 days";
    default:
      return "last 24 hours";
  }
}

// Axis tick: clock time for short windows, full date for week+ windows
// so 7d/30d charts stay readable.
export function fmtAxisTick(d, hours) {
  return (hours ?? 24) >= 168 ? fmtFullDate(d) : fmtClock(d);
}

// Local, readable datetime for tables.
export function fmtTime(s) {
  if (!s) return "—";
  const d = new Date(parseTs(s));
  return Number.isNaN(d.getTime()) ? String(s) : d.toLocaleString();
}

export function timeAgo(ms) {
  const secs = Math.max(0, Math.round(ms / 1000));
  if (secs < 5) return "just now";
  if (secs < 60) return `${secs}s ago`;
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d ago`;
  if (days < 30) return `${Math.round(days / 7)}w ago`;
  return `${Math.round(days / 30)}mo ago`;
}

export function shortHash(h) {
  return h ? `${h.slice(7, 15)}…${h.slice(-4)}` : "—";
}

// Resolve a server id to its display name.
export function serverName(servers, id) {
  const s = (servers || []).find((x) => x.id === id);
  return s ? s.name : id || "local";
}

// Human label for a runtime driver.
export function runtimeLabel(runtime) {
  if (runtime === "proc") return "turaes (proc)";
  if (runtime === "systemd") return "systemd";
  return runtime;
}
