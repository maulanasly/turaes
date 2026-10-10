// turaes dashboard — zero-build Preact + HTM (vendored), no bundler, no CDN.
import { render } from "preact";
import { useEffect, useState, useCallback, useRef } from "preact/hooks";
import { html } from "./lib/html.js";
import { api, setOrg, getOrg, oapi, onUnauthorized } from "./lib/api.js";
import { toast } from "./lib/toast.js";
import { useRoute, pathFor } from "./lib/router.js";
import { Toasts } from "./components/Toasts.js";
import { ConfirmHost } from "./components/ConfirmHost.js";
import { BrandMark } from "./components/Brand.js";
import { AppsView } from "./views/AppsView.js";
import { AppDetailView } from "./views/AppDetailView.js";
import { ServersView } from "./views/ServersView.js";
import { ServerDetailView } from "./views/ServerDetailView.js";
import { TokensView } from "./views/TokensView.js";
import { OrgView } from "./views/OrgView.js";
import { AboutView } from "./views/AboutView.js";
import { CatalogView } from "./views/CatalogView.js";
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

function NavLink({ route, view, match, label, badge }) {
  const active = route.view === view || (match && route.view === match);
  return html`<a href=${pathFor({ view })} class=${active ? "active" : ""}
    aria-current=${active ? "page" : null}>${label}${badge > 0
      ? html`<span class="nav-badge" aria-label=${`${badge} firing alerts`}>${badge}</span>`
      : null}</a>`;
}

function ThemeIcon({ theme }) {
  if (theme === "dark") {
    return html`<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" focusable="false"><path d="M12 11A5.5 5.5 0 0 1 5 4a5.5 5.5 0 1 0 7 7z" fill="currentColor" /></svg>`;
  }
  return html`<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" focusable="false"><circle cx="8" cy="8" r="3.5" fill="none" stroke="currentColor" stroke-width="1.5" /><path d="M8 1v2M8 13v2M1 8h2M13 8h2M3 3l1.4 1.4M11.6 11.6L13 13M13 3l-1.4 1.4M4.4 11.6L3 13" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>`;
}

const VIEW_TITLES = {
  apps: "Applications",
  app: "Application",
  catalog: "Catalog",
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
  const [fleetKnown, setFleetKnown] = useState(false);
  const [fleetFailed, setFleetFailed] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [userMenuOpen, setUserMenuOpen] = useState(false);
  const mainRef = useRef(null);
  const menuBtnRef = useRef(null);
  const navRef = useRef(null);
  const userMenuBtnRef = useRef(null);
  const userMenuRef = useRef(null);

  // Screen-reader and tab users learn where they are on every navigation.
  // Close both menus and move focus into the new view (without
  // scrolling) so keyboard/SR users land on fresh content. Keyed on id/tab as
  // well as view so app→app and tab→tab moves are announced too.
  useEffect(() => {
    document.title = `turaes — ${VIEW_TITLES[route.view] || "Applications"}`;
    setMenuOpen(false);
    setUserMenuOpen(false);
    if (mainRef.current) {
      try { mainRef.current.focus({ preventScroll: true }); } catch (e) {}
    }
  }, [route.view, route.id, route.tab]);

  // Mobile nav is a disclosure: focus the first link when it opens, let Escape
  // close it from anywhere, and return focus to the toggle.
  useEffect(() => {
    if (!menuOpen) return undefined;
    const first = navRef.current && navRef.current.querySelector("a");
    if (first && first.focus) first.focus();
    const onKey = (e) => {
      if (e.key === "Escape") {
        setMenuOpen(false);
        if (menuBtnRef.current) menuBtnRef.current.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [menuOpen]);

  // Account menu is a separate disclosure: Escape closes it, outside clicks
  // dismiss it, and focus returns to the avatar button.
  useEffect(() => {
    if (!userMenuOpen) return undefined;
    const onKey = (e) => {
      if (e.key === "Escape") {
        setUserMenuOpen(false);
        if (userMenuBtnRef.current) userMenuBtnRef.current.focus();
      }
    };
    const onPointer = (e) => {
      const el = userMenuRef.current;
      const btn = userMenuBtnRef.current;
      if (el && !el.contains(e.target) && btn && !btn.contains(e.target)) {
        setUserMenuOpen(false);
      }
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onPointer);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onPointer);
    };
  }, [userMenuOpen]);

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
    try {
      const r = await api("/api/v1/servers");
      setServers(r.servers || []);
      setFleetFailed(false);
    } catch (e) {
      // Never render a failed poll as an empty fleet.
      setFleetFailed(true);
    } finally {
      setFleetKnown(true);
    }
  }, []);
  const loadHealth = useCallback(async () => {
    try { setHealth(await api("/health")); } catch { setHealth(null); }
  }, []);
  const loadAlerts = useCallback(async () => {
    try { const r = await oapi("/alerts?status=firing&limit=10"); setAlerts(r.alerts || []); } catch { setAlerts([]); }
  }, []);

  useEffect(() => { loadUser(); }, [loadUser]);

  // Session-expiry fan-out: any view hitting 401 re-checks the session.
  // A dead session flips user to null (LoginView); a live one keeps state.
  // The ref avoids resubscribing (and stampeding loadUser) on every render.
  const userRef = useRef(user);
  userRef.current = user;
  useEffect(() => onUnauthorized(() => {
    if (userRef.current) loadUser();
  }), [loadUser]);

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
  const offlineServers = servers.filter((s) => s.status !== "online").length;
  const crits = alerts.filter((a) => a.severity === "critical");
  const warns = alerts.filter((a) => a.severity !== "critical");
  // Servers is operator-gated and Tokens is org-admin-gated server-side; the
  // nav mirrors that so restricted users never hit a dead end.
  const isOperator = orgs.some((o) => o.role === "admin" || o.role === "owner");
  const orgRole = (orgs.find((o) => o.slug === org) || {}).role;
  const canManageTokens = orgRole === "admin" || orgRole === "owner";

  const fleetItems = html`
    <span class="fleet-item"><strong>${servers.length}</strong><span>servers</span></span>
    <span class="fleet-separator" aria-hidden="true">·</span>
    <span class="fleet-item"><strong>${onlineServers}</strong><span>online</span></span>
    ${offlineServers > 0 ? html`<span class="fleet-separator" aria-hidden="true">·</span>
      <span class="fleet-item"><strong>${offlineServers}</strong><span>offline</span></span>` : null}
    ${attention > 0 ? html`<span class="fleet-separator" aria-hidden="true">·</span>
      <span class="fleet-item"><strong>${attention}</strong><span>attention</span></span>` : null}`;

  // Single status signal: API reachability wins over fleet counts so the
  // header never shows "Healthy" while servers are offline.
  const degraded = offlineServers > 0 || attention > 0;
  const statusPill = !health
    ? html`<span class="fleet-summary is-bad" role="status">API unreachable</span>`
    : !fleetKnown ? null
      : fleetFailed
        ? html`<span class="fleet-summary fleet-unavailable" title="Fleet status">Fleet status unavailable</span>`
        : html`<span class=${"fleet-summary" + (degraded ? " is-warn" : "")} title="Fleet status">
            ${isOperator
              ? html`<a class="fleet-link" href="#/servers" title="Open Servers">${fleetItems}</a>`
              : fleetItems}
          </span>`;

  const initials = (user.login || "?").slice(0, 2).toUpperCase();
  const inAccountSection = route.view === "org" || route.view === "tokens" || route.view === "about";
  const signOut = async () => {
    try {
      await api("/auth/logout", { method: "POST" });
    } catch (e) {
      toast.error(e.message);
      return;
    }
    location.assign("/");
  };

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
      <a class="brand" href="#/apps" aria-label="turaes home"><${BrandMark} size=${22} theme=${theme} /><span>turaes</span></a>
      <button ref=${menuBtnRef} class="btn small ghost menu-toggle" aria-expanded=${menuOpen} aria-controls="primary-nav"
        onClick=${() => setMenuOpen((v) => !v)}>
        ${menuOpen ? "✕ Close" : "☰ Menu"}</button>
      <nav ref=${navRef} class=${"nav" + (menuOpen ? " open" : "")} id="primary-nav" aria-label="Primary"
        onClick=${(e) => { if (e.target && e.target.closest && e.target.closest("a")) setMenuOpen(false); }}>
        <ul class="nav-list">
          <li><${NavLink} route=${route} view="apps" match="app" label="Applications" badge=${attention} /></li>
          <li><${NavLink} route=${route} view="catalog" label="Catalog" /></li>
          ${isOperator && html`<li><${NavLink} route=${route} view="servers" match="server" label="Servers" /></li>`}
        </ul>
      </nav>
      <div class="controls">
        ${statusPill}
        <button class="btn small ghost theme-toggle" onClick=${toggleTheme}
          title="Toggle theme" aria-label="Toggle theme" aria-pressed=${theme === "light"}><${ThemeIcon} theme=${theme} /></button>
        <div class="avatar-wrap">
          <button ref=${userMenuBtnRef} class=${"avatar-button" + (inAccountSection ? " active" : "")} aria-haspopup="menu"
            aria-expanded=${userMenuOpen} aria-label="Account menu" title=${user.login}
            onClick=${() => setUserMenuOpen((v) => !v)}>${initials}</button>
          ${userMenuOpen && html`<div ref=${userMenuRef} class="avatar-menu" role="menu" aria-label="Account">
            <div class="avatar-head"><strong>${user.login}</strong>
              <span class="muted small">${orgRole ? `${org} · ${orgRole}` : org}</span></div>
            ${orgs.length > 1 && html`<label class="avatar-org"><span class="avatar-org-label">Active organization</span>
              <select value=${org} onChange=${(e) => { changeOrg(e.target.value); setUserMenuOpen(false); }}>
                ${orgs.map((o) => html`<option value=${o.slug}>${o.slug} — ${o.role}</option>`)}
              </select></label>`}
            <a role="menuitem" href="#/org" class=${route.view === "org" ? "active" : ""}
              onClick=${() => setUserMenuOpen(false)}>Organization</a>
            ${canManageTokens && html`<a role="menuitem" href="#/tokens" class=${route.view === "tokens" ? "active" : ""}
              onClick=${() => setUserMenuOpen(false)}>API tokens</a>`}
            <a role="menuitem" href="#/about" class=${route.view === "about" ? "active" : ""}
              onClick=${() => setUserMenuOpen(false)}>About</a>
            <button role="menuitem" class="avatar-signout" onClick=${signOut}>Sign out</button>
          </div>`}
        </div>
      </div>
    </header>

    <main id="main" tabindex="-1" ref=${mainRef}>
      ${crits.length > 0 && html`<${AlertSection} cls="notice-critical"
        title=${`Critical (${crits.length})`} items=${crits} />`}
      ${warns.length > 0 && html`<${AlertSection} cls="notice-warning"
        title=${`Warnings (${warns.length})`} items=${warns} />`}
      ${route.view === "apps" && html`<${AppsView} key=${org} user=${user} servers=${servers} />`}
      ${route.view === "app" && html`<${AppDetailView} key=${org} id=${route.id} tab=${route.tab} range=${route.range} user=${user} servers=${servers} />`}
      ${route.view === "catalog" && html`<${CatalogView} key=${org} user=${user} servers=${servers} />`}
      ${route.view === "servers" && html`<${ServersView} key=${org} user=${user} onChanged=${loadServers} />`}
      ${route.view === "server" && html`<${ServerDetailView} key=${org} id=${route.id} />`}
      ${route.view === "tokens" && html`<${TokensView} key=${org} user=${user} />`}
      ${route.view === "org" && html`<${OrgView} key=${org} user=${user} onOrgChange=${changeOrg} />`}
      ${route.view === "about" && html`<${AboutView} health=${health} user=${user} theme=${theme} />`}
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
