import { html } from "../lib/html.js";
import { useEffect, useState, useCallback } from "preact/hooks";
import { api } from "../lib/api.js";
import { toast } from "../lib/toast.js";
import { serverName, runtimeLabel } from "../lib/format.js";
import { sortApps } from "../lib/sort.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { Skeleton } from "../components/Skeleton.js";

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
  const submit = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const payload = Object.fromEntries([...fd.entries()].filter(([, v]) => v !== ""));
    if (payload.port) payload.port = Number(payload.port);
    setBusy(true);
    try {
      const r = await api("/api/v1/apps", { method: "POST", body: JSON.stringify(payload) });
      toast.success(`Created ${r.application.name}`);
      e.target.reset();
      onCreated();
    } catch (err) {
      toast.error(err.message);
    } finally {
      setBusy(false);
    }
  };
  return html`
    <form class="form" onSubmit=${submit}>
      <div class="row">
        <label>Name <input name="name" placeholder="beruang" required /></label>
        <label>Port <input name="port" type="number" placeholder="8000" required /></label>
      </div>
      <label>Binary path on the server
        <input name="binary_path" placeholder="/srv/beruang/target/release/beruang-gateway" required />
      </label>
      <div class="row">
        <label>Domain <input name="domain" placeholder="app.rayakala.ink" /></label>
        <label>Server
          <select name="server_id">
            ${servers.map((s) => html`<option value=${s.id}>${s.name}</option>`)}
          </select>
        </label>
      </div>
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
  const [showAdd, setShowAdd] = useState(false);
  const [q, setQ] = useState("");
  const [sortKey, setSortKey] = useState("name");

  const load = useCallback(async () => {
    try {
      const r = await api("/api/v1/apps");
      setApps(r.applications || []);
    } catch (e) {
      if (e.status === 401) setApps([]);
      else toast.error(e.message);
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  const filtered = sortApps(
    (apps || []).filter((a) => !q || a.name.includes(q) || (a.domain || "").includes(q)),
    sortKey,
    (id) => serverName(servers, id),
  );

  return html`
    <section class="panel">
      <div class="panel-head">
        <h1>Applications</h1>
        <div class="controls">
          <input class="search" placeholder="Search…" value=${q}
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
      ${showAdd && user && html`<div class="form-wrap"><${NewAppForm} servers=${servers} onCreated=${load} /></div>`}
      ${apps === null
        ? html`<${Skeleton} lines={4} height=${64} />`
        : apps.length === 0
          ? html`<p class="muted">No applications yet.${user ? "" : " Sign in to create one."}</p>`
          : html`<div class="grid">
              ${filtered.map((a) => html`<${AppCard} app=${a} servers=${servers} />`)}
            </div>`}
    </section>`;
}
