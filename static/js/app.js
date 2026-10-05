// turaes dashboard — zero-build Preact + HTM (vendored), no bundler, no CDN.
import { render } from "preact";
import { useEffect, useState, useCallback, useRef } from "preact/hooks";
import { html } from "./lib/html.js";
import { api, setOrg, getOrg, oapi } from "./lib/api.js";
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
import { AboutView } from "./views/AboutView.js";
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
  about: "About",
  notfound: "Not found",
};

function Shell() {
  const route = useRoute();
  const [user, setUser] = useState(undefined);
  const [servers, setServers] = useState([]);
  const [alerts, setAlerts] = useState([]);
  const [health, setHealth] = useState(null);
  const [theme, toggleTheme] = useTheme();
  // Active org lives in Shell state so a switch remounts the org-scoped views
  // (via `key`) instead of a full page reload.
  const [org, setOrgState] = useState(() => getOrg() || "default");
  const [orgs, setOrgs] = useState([]);
  const [menuOpen, setMenuOpen] = useState(false);
  const mainRef = useRef(null);

  // Screen-reader and tab users learn where they are on every navigation.
  // Close the mobile menu and move focus into the new view (without
  // scrolling) so keyboard/SR users land on fresh content.
  useEffect(() => {
    document.title = `turaes — ${VIEW_TITLES[route.view] || "Applications"}`;
    setMenuOpen(false);
    if (mainRef.current) {
      try { mainRef.current.focus({ preventScroll: true }); } catch (e) {}
    }
  }, [route.view]);

  const loadUser = useCallback(async () => {
    try {
      setUser(await api("/auth/me"));
      // Resolve the tenant principal and pin API calls to an org.
      try {
        const me = await api("/api/v1/me");
        const list = me.orgs || [];
        setOrgs(list);
        let stored = null;
        try { stored = localStorage.getItem("turaes-org"); } catch (e) {}
        const pick = list.find((o) => o.slug === stored)
          || list.find((o) => o.slug === "default")
          || list[0];
        if (pick) { setOrg(pick.slug); setOrgState(pick.slug); }
      } catch (e) {}
    } catch { setUser(null); }
  }, []);
  const loadServers = useCallback(async () => {
    try { const r = await api("/api/v1/servers"); setServers(r.servers || []); } catch (e) {}
  }, []);
  const loadHealth = useCallback(async () => {
    try { setHealth(await api("/health")); } catch { setHealth(null); }
  }, []);
  const loadAlerts = useCallback(async () => {
    try { const r = await oapi("/alerts?status=firing&limit=10"); setAlerts(r.alerts || []); } catch { setAlerts([]); }
  }, []);

  useEffect(() => { loadUser(); }, [loadUser]);

  // Only poll once authenticated. Alerts are org-scoped, so refetch when the
  // active org changes.
  useEffect(() => {
    if (!user) return undefined;
    loadServers(); loadHealth(); loadAlerts();
    const t = setInterval(() => {
      if (!document.hidden) { loadServers(); loadHealth(); loadAlerts(); }
    }, 30000);
    return () => clearInterval(t);
  }, [user, org, loadServers, loadHealth, loadAlerts]);

  const changeOrg = useCallback((slug) => {
    try { localStorage.setItem("turaes-org", slug); } catch (e) {}
    setOrg(slug);
    setOrgState(slug);
  }, []);

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

  const attention = alerts.length;
  const onlineServers = servers.filter((s) => s.status === "online").length;
  const crits = alerts.filter((a) => a.severity === "critical");
  const warns = alerts.filter((a) => a.severity !== "critical");

  function AlertSection({ cls, title, items }) {
    if (items.length === 0) return null;
    return html`<section class=${"panel notice " + cls} role="alert">
      <div class="panel-head"><strong>${title}</strong></div>
      ${items.map((a) => html`<div>
        ${a.application_id
          ? html`<a href=${`#/apps/${a.application_id}/overview`}>${a.subject}</a>`
          : a.subject}
      </div>`)}
    </section>`;
  }

  return html`
    <a class="skip-link" href="#main" onClick=${(e) => {
      // Focus without touching the hash: "#main" is not a route.
      e.preventDefault();
      if (mainRef.current) { try { mainRef.current.focus(); } catch (err) {} }
    }}>Skip to content</a>
    <header class="topbar">
      <div class="brand">turaes <span class="muted small">· deploy your apps — no containers</span></div>
      <button class="btn small ghost menu-toggle" aria-expanded=${menuOpen} aria-controls="primary-nav"
        onClick=${() => setMenuOpen((v) => !v)}
        onKeyDown=${(e) => { if (e.key === "Escape") setMenuOpen(false); }}>
        ${menuOpen ? "Close" : "Menu"}</button>
      <nav class=${"nav" + (menuOpen ? " open" : "")} id="primary-nav" aria-label="Primary">
        <span class="nav-group"><span class="nav-label">Operate</span>
          <${NavLink} route=${route} view="apps" match="app" label="Applications" />
          <${NavLink} route=${route} view="servers" match="server" label="Servers" />
        </span>
        <span class="nav-group"><span class="nav-label">Access</span>
          <${NavLink} route=${route} view="tokens" label="Tokens" />
          <${NavLink} route=${route} view="org" label="Organization" />
        </span>
        <span class="nav-group"><span class="nav-label">System</span>
          <${NavLink} route=${route} view="about" label="About" />
        </span>
      </nav>
      <div class="controls">
        ${orgs.length > 1 ? html`<select value=${org} onChange=${(e) => changeOrg(e.target.value)}
          aria-label="Active organization" title="Active organization">
          ${orgs.map((o) => html`<option value=${o.slug}>${o.slug} (${o.role})</option>`)}
        </select>` : orgs.length === 1 ? html`<span class="muted small" title="Active organization">${orgs[0].slug}</span>` : null}
        <span class="fleet-summary" title="Fleet status">
          <strong>${servers.length}</strong>&nbsp;servers · <strong>${onlineServers}</strong>&nbsp;online${attention > 0 ? html` · <strong>${attention}</strong>&nbsp;attention` : null}
        </span>
        <span class=${"dot " + (health ? "ok" : "bad")} aria-hidden="true"></span>
        <span class="small" role="status">${health ? "Healthy" : "Unreachable"}</span>
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

    <main id="main" tabindex="-1" ref=${mainRef}>
      ${crits.length > 0 && html`<${AlertSection} cls="notice-critical"
        title=${`Critical (${crits.length})`} items=${crits} />`}
      ${warns.length > 0 && html`<${AlertSection} cls="notice-warning"
        title=${`Warnings (${warns.length})`} items=${warns} />`}
      ${route.view === "apps" && html`<${AppsView} key=${org} user=${user} servers=${servers} />`}
      ${route.view === "app" && html`<${AppDetailView} key=${org} id=${route.id} tab=${route.tab} range=${route.range} user=${user} servers=${servers} />`}
      ${route.view === "servers" && html`<${ServersView} key=${org} user=${user} onChanged=${loadServers} />`}
      ${route.view === "server" && html`<${ServerDetailView} key=${org} id=${route.id} />`}
      ${route.view === "tokens" && html`<${TokensView} key=${org} user=${user} />`}
      ${route.view === "org" && html`<${OrgView} key=${org} user=${user} onOrgChange=${changeOrg} />`}
      ${route.view === "about" && html`<${AboutView} health=${health} user=${user} />`}
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
