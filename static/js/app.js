// turaes dashboard — zero-build Preact + HTM (vendored), no bundler, no CDN.
import { render } from "preact";
import { useEffect, useState, useCallback } from "preact/hooks";
import { html } from "./lib/html.js";
import { api, setOrg } from "./lib/api.js";
import { toast } from "./lib/toast.js";
import { useRoute, pathFor } from "./lib/router.js";
import { Toasts } from "./components/Toasts.js";
import { ConfirmHost } from "./components/ConfirmHost.js";
import { AppsView } from "./views/AppsView.js";
import { AppDetailView } from "./views/AppDetailView.js";
import { ServersView } from "./views/ServersView.js";
import { ServerDetailView } from "./views/ServerDetailView.js";
import { TokensView } from "./views/TokensView.js";
import { OrgView } from "./views/OrgView.js";
import { LoginView } from "./views/LoginView.js";

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

const VIEW_TITLES = {
  apps: "Applications",
  app: "Application",
  servers: "Servers",
  server: "Server",
  tokens: "API tokens",
  org: "Organization",
  notfound: "Not found",
};

function Shell() {
  const route = useRoute();
  const [user, setUser] = useState(undefined);
  const [servers, setServers] = useState([]);
  const [health, setHealth] = useState(null);
  const [theme, toggleTheme] = useTheme();

  // Screen-reader and tab users learn where they are on every navigation.
  useEffect(() => {
    document.title = `turaes — ${VIEW_TITLES[route.view] || "Applications"}`;
  }, [route.view]);

  const loadUser = useCallback(async () => {
    try {
      setUser(await api("/auth/me"));
      // Resolve the tenant principal and pin API calls to an org.
      try {
        const me = await api("/api/v1/me");
        const orgs = me.orgs || [];
        let stored = null;
        try { stored = localStorage.getItem("turaes-org"); } catch (e) {}
        const pick = orgs.find((o) => o.slug === stored)
          || orgs.find((o) => o.slug === "default")
          || orgs[0];
        if (pick) setOrg(pick.slug);
      } catch (e) {}
    } catch { setUser(null); }
  }, []);
  const loadServers = useCallback(async () => {
    try { const r = await api("/api/v1/servers"); setServers(r.servers || []); } catch (e) {}
  }, []);
  const loadHealth = useCallback(async () => {
    try { setHealth(await api("/health")); } catch { setHealth(null); }
  }, []);

  useEffect(() => { loadUser(); }, [loadUser]);

  // Only poll once authenticated.
  useEffect(() => {
    if (!user) return undefined;
    loadServers(); loadHealth();
    const t = setInterval(() => {
      if (!document.hidden) { loadServers(); loadHealth(); }
    }, 30000);
    return () => clearInterval(t);
  }, [user, loadServers, loadHealth]);

  // Remember the intended route across a full-page sign-in redirect.
  useEffect(() => {
    if (user === null) {
      try { sessionStorage.setItem("turaes-next", location.hash || "#/apps"); } catch (e) {}
    }
  }, [user]);
  useEffect(() => {
    if (!user) return;
    try {
      const next = sessionStorage.getItem("turaes-next");
      sessionStorage.removeItem("turaes-next");
      if (next && next !== location.hash) location.hash = next;
    } catch (e) {}
  }, [user]);

  const loginError = (() => {
    try { return new URLSearchParams(location.search).get("login_error"); } catch (e) { return null; }
  })();

  if (user === undefined) {
    return html`<div class="login"><div class="login-card">
      <div class="login-brand">turaes</div><div class="spinner"></div></div></div>`;
  }
  if (user === null) {
    return html`<${LoginView} error=${loginError} theme=${theme} onToggleTheme=${toggleTheme} />`;
  }

  return html`
    <header class="topbar">
      <div class="brand">turaes <span class="muted small">· deploy your apps — no containers</span></div>
      <nav class="nav" aria-label="Primary">
        <${NavLink} route=${route} view="apps" match="app" label="Applications" />
        <${NavLink} route=${route} view="servers" match="server" label="Servers" />
        <${NavLink} route=${route} view="tokens" label="Tokens" />
        <${NavLink} route=${route} view="org" label="Organization" />
      </nav>
      <div class="controls">
        <span class=${"dot " + (health ? "ok" : "bad")}
          title=${health ? "healthy" : "unreachable"} aria-label=${health ? "healthy" : "unreachable"}></span>
        <button class="btn small ghost" onClick=${toggleTheme}
          title="Toggle theme" aria-label="Toggle theme">${theme === "dark" ? "☾" : "☀"}</button>
        <span class="muted small">${user.login}</span>
        <button class="btn small ghost" onClick=${async () => {
          try {
            await api("/auth/logout", { method: "POST" });
          } catch (e) {
            toast.error(e.message);
            return;
          }
          location.assign("/");
        }}>Sign out</button>
      </div>
    </header>

    <main>
      ${route.view === "apps" && html`<${AppsView} user=${user} servers=${servers} />`}
      ${route.view === "app" && html`<${AppDetailView} id=${route.id} tab=${route.tab} user=${user} servers=${servers} />`}
      ${route.view === "servers" && html`<${ServersView} user=${user} onChanged=${loadServers} />`}
      ${route.view === "server" && html`<${ServerDetailView} id=${route.id} />`}
      ${route.view === "tokens" && html`<${TokensView} user=${user} />`}
      ${route.view === "org" && html`<${OrgView} user=${user} />`}
      ${route.view === "notfound" && html`<section class="panel">
        <h1>Not found</h1>
        <p class="muted">No view matches <span class="mono">${route.path || ""}</span>.</p>
        <div class="controls">
          <a class="btn ghost" href="#/apps">Applications</a>
          <a class="btn ghost" href="#/servers">Servers</a>
        </div>
      </section>`}
    </main>

    <${Toasts} />
    <${ConfirmHost} />`;
}

render(html`<${Shell} />`, document.getElementById("app"));
