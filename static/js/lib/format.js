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
  return `${Math.round(mins / 60)}h ago`;
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
