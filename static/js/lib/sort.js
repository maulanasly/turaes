// Pure list sorting/filter helpers (unit-testable).
export const APP_SORTS = ["name", "status", "server", "port", "recent"];

// Case-insensitive, whitespace-trimmed search across name and primary domain.
// Empty query returns the input unchanged (new array, never mutates).
export function filterApps(apps, q) {
  const needle = String(q || "").trim().toLowerCase();
  if (!needle) return [...(apps || [])];
  return (apps || []).filter(
    (a) =>
      String(a.name || "").toLowerCase().includes(needle) ||
      String(a.domain || "").toLowerCase().includes(needle),
  );
}

function value(app, key, serverNameFn) {
  switch (key) {
    case "status": return app.status || "";
    case "server": return serverNameFn(app.server_id);
    case "port": return app.port || 0;
    case "recent": return app.updated_at || "";
    case "name":
    default: return app.name || "";
  }
}

export function sortApps(apps, key, serverNameFn = (id) => id) {
  const sorted = [...apps].sort((a, b) => {
    const va = value(a, key, serverNameFn);
    const vb = value(b, key, serverNameFn);
    if (typeof va === "number" && typeof vb === "number") return va - vb;
    return String(va).localeCompare(String(vb));
  });
  return key === "recent" ? sorted.reverse() : sorted;
}
