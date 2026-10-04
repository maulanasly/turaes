// Pure route parsing (no framework imports, so it is unit-testable in Node).
export const APP_TABS = ["overview", "deployments", "activity", "environment", "logs", "settings"];

export function parseRoute(hash) {
  const path = String(hash || "").replace(/^#/, "").split("?")[0];
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
    // silently showing the wrong tab.
    return { view: "app", id: parts[1], tab: parts[2] || "overview" };
  }
  return { view: "notfound", path: `/${parts.join("/")}` };
}

export function pathFor(route) {
  if (!route || route.view === "apps") return "#/apps";
  if (route.view === "servers") return "#/servers";
  if (route.view === "server") return `#/servers/${route.id}`;
  if (route.view === "tokens") return "#/tokens";
  if (route.view === "org") return "#/org";
  if (route.view === "app") return `#/apps/${route.id}/${route.tab || "overview"}`;
  return "#/apps";
}
