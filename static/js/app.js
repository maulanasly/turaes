// turaes dashboard — zero-build Preact + HTM (vendored), no bundler, no CDN.
import { render } from "preact";
import { useEffect, useState, useCallback } from "preact/hooks";
import { html } from "./lib/html.js";
import { api } from "./lib/api.js";
import { toast } from "./lib/toast.js";
import { useRoute, pathFor } from "./lib/router.js";
import { Toasts } from "./components/Toasts.js";
import { ConfirmHost } from "./components/ConfirmHost.js";
import { AppsView } from "./views/AppsView.js";
import { AppDetailView } from "./views/AppDetailView.js";
import { ServersView } from "./views/ServersView.js";
import { ServerDetailView } from "./views/ServerDetailView.js";

function useTheme() {
  const [theme, setTheme] = useState(() => document.documentElement.dataset.theme || "dark");
  const toggle = () => {
    const next = theme === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    try { localStorage.setItem("turaes-theme", next); } catch (e) {}
    setTheme(next);
  };
  return [theme, toggle];
}

function NavLink({ route, view, match, label }) {
  const active = route.view === view || (match && route.view === match);
  return html`<a href=${pathFor({ view })} class=${active ? "active" : ""}
    aria-current=${active ? "page" : null}>${label}</a>`;
}

function Shell() {
  const route = useRoute();
  const [user, setUser] = useState(undefined);
  const [servers, setServers] = useState([]);
  const [health, setHealth] = useState(null);
  const [theme, toggleTheme] = useTheme();

  const loadUser = useCallback(async () => {
    try { setUser(await api("/auth/me")); } catch { setUser(null); }
  }, []);
  const loadServers = useCallback(async () => {
    try { const r = await api("/api/v1/servers"); setServers(r.servers || []); } catch (e) {}
  }, []);
  const loadHealth = useCallback(async () => {
    try { setHealth(await api("/health")); } catch { setHealth(null); }
  }, []);

  useEffect(() => {
    loadUser(); loadServers(); loadHealth();
    const t = setInterval(() => {
      if (!document.hidden) { loadServers(); loadHealth(); }
    }, 30000);
    return () => clearInterval(t);
  }, [loadUser, loadServers, loadHealth]);

  return html`
    <header class="topbar">
      <div class="brand">turaes <span class="muted small">· deploy your apps — no containers</span></div>
      <nav class="nav" aria-label="Primary">
        <${NavLink} route=${route} view="apps" match="app" label="Applications" />
        <${NavLink} route=${route} view="servers" match="server" label="Servers" />
      </nav>
      <div class="controls">
        <span class=${"dot " + (health ? "ok" : "bad")}
          title=${health ? "healthy" : "unreachable"} aria-label=${health ? "healthy" : "unreachable"}></span>
        <button class="btn small ghost" onClick=${toggleTheme}
          title="Toggle theme" aria-label="Toggle theme">${theme === "dark" ? "☾" : "☀"}</button>
        ${user === undefined ? null : user
          ? html`<span class="muted small">${user.login}</span>
              <button class="btn small ghost" onClick=${async () => {
                try {
                  await api("/auth/logout", { method: "POST" });
                } catch (e) {
                  toast.error(e.message);
                  return;
                }
                location.assign("/");
              }}>Sign out</button>`
          : html`<a class="btn small" href="/auth/login">Sign in</a>`}
      </div>
    </header>

    <main>
      ${route.view === "apps" && html`<${AppsView} user=${user} servers=${servers} />`}
      ${route.view === "app" && html`<${AppDetailView} id=${route.id} tab=${route.tab} user=${user} servers=${servers} />`}
      ${route.view === "servers" && html`<${ServersView} user=${user} onChanged=${loadServers} />`}
      ${route.view === "server" && html`<${ServerDetailView} id=${route.id} />`}
    </main>

    <${Toasts} />
    <${ConfirmHost} />`;
}

render(html`<${Shell} />`, document.getElementById("app"));
