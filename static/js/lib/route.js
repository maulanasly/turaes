// Monitoring time ranges for the Overview charts. Single selector drives
// CPU, memory and visitor series together so KPIs share one window.
// Backend clamps `?hours=` to 1..720 (30d retention); presets stay inside it.
export const RANGES = ["1h", "6h", "24h", "7d", "30d"];
export const DEFAULT_RANGE = "24h";
export const RANGE_HOURS = { "1h": 1, "6h": 6, "24h": 24, "7d": 168, "30d": 720 };

// Pure route parsing (no framework imports, so it is unit-testable in Node).
export const APP_TABS = ["overview", "deployments", "activity", "environment", "logs", "settings"];

export function normalizeRange(v) {
  return RANGES.includes(v) ? v : DEFAULT_RANGE;
}

export function rangeToHours(range) {
  return RANGE_HOURS[normalizeRange(range)] ?? 24;
}

function parseRangeParam(query) {
  try {
    const params = new URLSearchParams(query || "");
    const r = params.get("range");
    if (!r) return undefined;
    const norm = normalizeRange(r);
    // Absence means DEFAULT_RANGE, so an explicit default (and any unknown
    // value that normalizes to it) is treated as no override.
    return norm === DEFAULT_RANGE ? undefined : norm;
  } catch {
    return undefined;
  }
}

export function parseRoute(hash) {
  const raw = String(hash || "").replace(/^#/, "");
  const qIndex = raw.indexOf("?");
  const path = (qIndex >= 0 ? raw.slice(0, qIndex) : raw).split("?")[0];
  const query = qIndex >= 0 ? raw.slice(qIndex + 1) : "";
  const parts = path.split("/").filter(Boolean);
  if (parts.length === 0 || (parts[0] === "apps" && !parts[1])) {
    return { view: "apps" };
  }
  if (parts[0] === "servers") {
    return parts[1] ? { view: "server", id: parts[1] } : { view: "servers" };
  }
  if (parts[0] === "tokens") {
    return { view: "tokens" };
  }
  if (parts[0] === "org") {
    return { view: "org" };
  }
  if (parts[0] === "apps" && parts[1]) {
    // Unknown tabs pass through raw; the detail view names them instead of
    // silently showing the wrong tab. `?range=` is preserved for the overview
    // time filter so chart windows are deep-linkable and back-button safe.
    const tab = parts[2] || "overview";
    const range = parseRangeParam(query);
    return range ? { view: "app", id: parts[1], tab, range } : { view: "app", id: parts[1], tab };
  }
  return { view: "notfound", path: `/${parts.join("/")}` };
}

export function pathFor(route) {
  if (!route || route.view === "apps") return "#/apps";
  if (route.view === "servers") return "#/servers";
  if (route.view === "server") return `#/servers/${route.id}`;
  if (route.view === "tokens") return "#/tokens";
  if (route.view === "org") return "#/org";
  if (route.view === "app") {
    const base = `#/apps/${route.id}/${route.tab || "overview"}`;
    // Only non-default ranges are serialized to keep URLs clean; absence
    // means DEFAULT_RANGE on parse.
    if (route.range && route.range !== DEFAULT_RANGE && RANGES.includes(route.range)) {
      return `${base}?range=${route.range}`;
    }
    return base;
  }
  return "#/apps";
}
