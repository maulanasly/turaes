// Pure route parsing (no framework imports, so it is unit-testable in Node).
export const APP_TABS = ["overview", "deployments", "activity", "environment", "logs", "settings"];

export function parseRoute(hash) {
  const path = String(hash || "").replace(/^#/, "").split("?")[0];
  const parts = path.split("/").filter(Boolean);
  if (parts[0] === "servers") {
    return parts[1] ? { view: "server", id: parts[1] } : { view: "servers" };
  }
  if (parts[0] === "apps" && parts[1]) {
    const tab = APP_TABS.includes(parts[2]) ? parts[2] : "overview";
    return { view: "app", id: parts[1], tab };
  }
  return { view: "apps" };
}

export function pathFor(route) {
  if (!route || route.view === "apps") return "#/apps";
  if (route.view === "servers") return "#/servers";
  if (route.view === "server") return `#/servers/${route.id}`;
  return `#/apps/${route.id}/${route.tab || "overview"}`;
}
