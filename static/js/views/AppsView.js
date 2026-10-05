import { html } from "../lib/html.js";
import { useEffect, useState, useCallback } from "preact/hooks";
import { navigate } from "../lib/router.js";
import { oapi } from "../lib/api.js";
import { toast } from "../lib/toast.js";
import { serverName, runtimeLabel, fmtTime } from "../lib/format.js";
import { sortApps, filterApps } from "../lib/sort.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { Skeleton } from "../components/Skeleton.js";

function AppList({ apps, servers }) {
  return html`
    <div class="table-wrap"><table class="stacked">
      <thead><tr><th>Status</th><th>Name</th><th>Server</th><th>Route</th><th>Updated</th></tr></thead>
      <tbody>
        ${apps.map((a) => html`
          <tr>
            <td data-label="Status"><${StatusBadge} status=${a.status} /></td>
            <td data-label="Name"><a class="mono" href=${`#/apps/${a.id}/overview`}>${a.name}</a></td>
            <td data-label="Server">${serverName(servers, a.server_id)}</td>
            <td data-label="Route" class="mono small break">${a.domain || "—"}</td>
            <td data-label="Updated" class="muted">${fmtTime(a.updated_at)}</td>
          </tr>`)}
      </tbody>
    </table></div>`;
}

function AppCard({ app, servers }) {
  return html`
    <a class="card" href=${`#/apps/${app.id}/overview`}>
      <h3>
        <span class="mono">${app.name}</span>
        <${StatusBadge} status=${app.status} />
      </h3>
      <div class="stat"><span>Domain</span><span class="mono">${app.domain || "—"}</span></div>
      <div class="stat"><span>Server</span><span class="mono">${serverName(servers, app.server_id)}</span></div>
      <div class="stat"><span>Port</span><span class="mono">${app.port}</span></div>
      <div class="stat"><span>Managed by</span><span>${runtimeLabel(app.runtime)}</span></div>
    </a>`;
}

function NewAppForm({ servers, onCreated }) {
  const [busy, setBusy] = useState(false);
  const [review, setReview] = useState(null);
  const startReview = (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const payload = Object.fromEntries([...fd.entries()].filter(([, v]) => v !== ""));
    if (payload.port) payload.port = Number(payload.port);
    setReview(payload);
  };
  const confirm = async () => {
    setBusy(true);
    try {
      const r = await oapi("/apps", { method: "POST", body: JSON.stringify(review) });
      toast.success(`Created ${r.application.name}`);
      setReview(null);
      onCreated(r.application);
    } catch (err) {
      toast.error(err.message);
    } finally {
      setBusy(false);
    }
  };
  if (review) {
    const server = servers.find((s) => s.id === review.server_id);
    return html`
      <div>
        <div class="section-band"><h2>Review</h2><span class="muted small">check before creating</span></div>
        <table><tbody>
          <tr><th>Name</th><td class="mono">${review.name}</td></tr>
          <tr><th>Binary</th><td class="mono break">${review.binary_path}</td></tr>
          <tr><th>Port</th><td class="mono">${review.port}</td></tr>
          <tr><th>Domain</th><td class="mono">${review.domain || "—"}</td></tr>
          <tr><th>Server</th><td>${server ? server.name : review.server_id || "—"}</td></tr>
          <tr><th>Managed by</th><td>${runtimeLabel(review.runtime)}</td></tr>
        </tbody></table>
        <p class="muted small">This creates the app record; deploy it from its detail page to start it.</p>
        <div class="controls">
          <button class="btn ghost" disabled=${busy} onClick=${() => setReview(null)}>Back</button>
          <button class="btn" disabled=${busy} onClick=${confirm}>${busy ? "Creating…" : "Create app"}</button>
        </div>
      </div>`;
  }
  return html`
    <form class="form" onSubmit=${startReview}>
      <div class="section-band"><h2>Identity</h2><span class="muted small">name becomes the unit + install name</span></div>
      <div class="row">
        <label>Name <input name="name" placeholder="beruang" required /></label>
        <label>Port <input name="port" type="number" placeholder="8000" required /></label>
      </div>
      <label>Binary path on the server
        <input name="binary_path" placeholder="/srv/beruang/target/release/beruang-gateway" required />
        <span class="muted small">A prebuilt binary — turaes never builds or containers anything.</span>
      </label>
      <div class="section-band"><h2>Placement</h2><span class="muted small">where it runs and how it is reached</span></div>
      <div class="row">
        <label>Domain <input name="domain" placeholder="app.rayakala.ink" /></label>
        <label>Server
          <select name="server_id">
            ${servers.map((s) => html`<option value=${s.id}>${s.name}</option>`)}
          </select>
        </label>
      </div>
      <div class="section-band"><h2>Runtime</h2><span class="muted small">supervision behavior</span></div>
      <div class="row">
        <label>Managed by
          <select name="runtime">
            <option value="systemd">systemd</option>
            <option value="proc">turaes (proc)</option>
          </select>
        </label>
      </div>
      <div><button class="btn" type="submit" disabled=${busy}>${busy ? "Creating…" : "Create"}</button></div>
    </form>`;
}

export function AppsView({ user, servers }) {
  const [apps, setApps] = useState(null);
  const [error, setError] = useState(null);
  const [showAdd, setShowAdd] = useState(false);
  const [q, setQ] = useState(() => {
    try { return sessionStorage.getItem("turaes-apps-q") || ""; } catch { return ""; }
  });
  const [sortKey, setSortKey] = useState(() => {
    try { return sessionStorage.getItem("turaes-apps-sort") || "name"; } catch { return "name"; }
  });
  const [filter, setFilter] = useState("all");
  const [view, setView] = useState(() => {
    try { return sessionStorage.getItem("turaes-apps-view") || "list"; } catch { return "list"; }
  });

  const needsAttention = (a) =>
    a.status === "unhealthy" || a.status === "failed" || a.status === "deploying" || a.status === "unknown";

  // List state survives list → detail → back (the views unmount on route change).
  useEffect(() => {
    try { sessionStorage.setItem("turaes-apps-q", q); } catch {}
  }, [q]);
  useEffect(() => {
    try { sessionStorage.setItem("turaes-apps-sort", sortKey); } catch {}
  }, [sortKey]);
  useEffect(() => {
    try { sessionStorage.setItem("turaes-apps-view", view); } catch {}
  }, [view]);

  const load = useCallback(async () => {
    try {
      const r = await oapi("/apps");
      setApps(r.applications || []);
      setError(null);
    } catch (e) {
      if (e.status === 401) { setApps([]); setError(null); }
      else { setError(e.message); toast.error(e.message); }
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  const retry = () => { setError(null); load(); };

  const created = (app) => {
    setShowAdd(false);
    load();
    navigate(`#/apps/${app.id}/overview`);
  };

  const filtered = sortApps(
    filterApps(apps, q),
    sortKey,
    (id) => serverName(servers, id),
  ).filter((a) => filter === "all" || (filter === "attention" && needsAttention(a)));
  const attentionCount = (apps || []).filter(needsAttention).length;

  const grouped = sortKey === "server" ? (() => {
    const groups = new Map();
    for (const a of filtered) {
      const key = serverName(servers, a.server_id);
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(a);
    }
    return [...groups.entries()];
  })() : null;

  return html`
    <section class="panel">
      <div class="panel-head">
        <h1>Applications</h1>
        <div class="controls">
          <div class="seg" role="radiogroup" aria-label="Application filter">
            <button type="button" role="radio" aria-checked=${filter === "all"}
              class=${filter === "all" ? "active" : ""} onClick=${() => setFilter("all")}>All</button>
            <button type="button" role="radio" aria-checked=${filter === "attention"}
              class=${filter === "attention" ? "active" : ""} onClick=${() => setFilter("attention")}>
              Attention${attentionCount > 0 ? ` (${attentionCount})` : ""}</button>
          </div>
          <div class="seg" role="radiogroup" aria-label="Applications view">
            <button type="button" role="radio" aria-checked=${view === "list"}
              class=${view === "list" ? "active" : ""} onClick=${() => setView("list")}>List</button>
            <button type="button" role="radio" aria-checked=${view === "cards"}
              class=${view === "cards" ? "active" : ""} onClick=${() => setView("cards")}>Cards</button>
          </div>
          <input class="search" placeholder="Search…" aria-label="Search applications" value=${q}
            onInput=${(e) => setQ(e.target.value)} />
          <select value=${sortKey} onChange=${(e) => setSortKey(e.target.value)} aria-label="Sort by">
            <option value="name">Sort: name</option>
            <option value="status">Sort: status</option>
            <option value="server">Sort: server</option>
            <option value="port">Sort: port</option>
            <option value="recent">Sort: recently updated</option>
          </select>
          ${user && html`<button class="btn" onClick=${() => setShowAdd((v) => !v)}>
            ${showAdd ? "Close" : "+ New app"}
          </button>`}
        </div>
      </div>
      ${showAdd && user && html`<div class="form-wrap"><${NewAppForm} servers=${servers} onCreated=${created} /></div>`}
      ${error
        ? html`<p class="muted">Could not load applications: ${error}</p>
          <div><button class="btn" onClick=${retry}>Retry</button></div>`
        : apps === null
        ? html`<${Skeleton} lines={4} height=${64} />`
        : apps.length === 0
          ? html`<p class="muted">No applications yet.${user ? "" : " Sign in to create one."}</p>`
          : filtered.length === 0
          ? html`<p class="muted">No applications match this filter.</p>`
          : view === "list"
          ? (grouped
            ? html`${grouped.map(([server, items]) => html`
                <div class="section-band"><h2>${server}</h2><span class="muted small">${items.length} app${items.length === 1 ? "" : "s"}</span></div>
                <${AppList} apps=${items} servers=${servers} />`)}`
            : html`<${AppList} apps=${filtered} servers=${servers} />`)
          : grouped
          ? html`${grouped.map(([server, items]) => html`
              <div class="section-band"><h2>${server}</h2><span class="muted small">${items.length} app${items.length === 1 ? "" : "s"}</span></div>
              <div class="grid">
                ${items.map((a) => html`<${AppCard} app=${a} servers=${servers} />`)}
              </div>`)}`
          : html`<div class="grid">
              ${filtered.map((a) => html`<${AppCard} app=${a} servers=${servers} />`)}
            </div>`}
    </section>`;
}
