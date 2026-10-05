import { html } from "../lib/html.js";
import { useState, useEffect, useCallback } from "preact/hooks";
import { api, oapi } from "../lib/api.js";
import { dismiss, toast } from "../lib/toast.js";
import { confirmAction } from "../lib/confirm.js";
import { fmtTime, fmtBytes } from "../lib/format.js";

function statusClass(s) {
  return s === "online" ? "running" : s === "offline" ? "failed" : "stopped";
}

// Host capacity sample attached by the API (`null` until the node is sampled).
function loadClass(pct) {
  return pct >= 90 ? "bad" : pct >= 75 ? "warn" : "";
}

function capSummary(cap) {
  if (!cap) return "—";
  const cpu = Math.max(0, Math.min(100, cap.cpu_pct || 0));
  return `${cpu.toFixed(0)}% · ${fmtBytes(cap.mem_bytes)}/${fmtBytes(cap.mem_total_bytes)}`;
}

function Capacity({ cap }) {
  if (!cap) return html`<span class="muted small">no sample</span>`;
  const cpu = Math.max(0, Math.min(100, cap.cpu_pct || 0));
  const memPct = cap.mem_total_bytes > 0
    ? Math.max(0, Math.min(100, (cap.mem_bytes / cap.mem_total_bytes) * 100))
    : 0;
  return html`<div class="capacity" title=${`CPU ${cpu.toFixed(1)}% · Memory ${fmtBytes(cap.mem_bytes)}/${fmtBytes(cap.mem_total_bytes)}`}>
    <div class="cap-row"><span class="muted">CPU</span>
      <div class="cap-track"><div class=${"cap-fill " + loadClass(cpu)} style=${`width:${cpu.toFixed(1)}%`}></div></div>
      <span class="mono">${cpu.toFixed(0)}%</span></div>
    <div class="cap-row"><span class="muted">MEM</span>
      <div class="cap-track"><div class=${"cap-fill mem " + loadClass(memPct)} style=${`width:${memPct.toFixed(1)}%`}></div></div>
      <span class="mono">${fmtBytes(cap.mem_bytes)}</span></div>
  </div>`;
}

export function ServersView({ user, onChanged }) {
  const [servers, setServers] = useState(null);
  const [apps, setApps] = useState([]);
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(null);

  const load = useCallback(async () => {
    try {
      const r = await api("/api/v1/servers");
      setServers(r.servers || []);
      setError(null);
    } catch (e) {
      if (e.status === 401) { setServers([]); setError(null); }
      else { setError(e.message); toast.error(e.message); }
    }
    try {
      const a = await oapi("/apps");
      setApps(a.applications || []);
    } catch { /* placement counts are advisory; never block the list */ }
  }, []);

  useEffect(() => { load(); }, [load]);

  const appsOn = (id) => apps.filter((a) => a.server_id === id);
  const appDot = (a) =>
    a.status === "running" ? "ok"
    : (a.status === "unhealthy" || a.status === "failed") ? "bad" : "muted";

  const retry = () => { setError(null); load(); };

  const validate = async (s) => {
    setBusy(`validate:${s.id}`);
    try {
      const r = await api(`/api/v1/servers/${s.id}/validate`, { method: "POST" });
      toast[r.reachable ? "success" : "error"](`${s.name}: ${r.status}`);
      load();
      onChanged?.();
    } catch (e) { toast.error(e.message); } finally { setBusy(null); }
  };

  const bootstrap = async (s) => {
    if (!(await confirmAction({
      title: `Bootstrap ${s.name}?`,
      body: "Installs and starts the turaes agent on this node over SSH.",
      confirmLabel: "Bootstrap",
    }))) return;
    const id = toast.info(`Bootstrapping ${s.name}…`, 0);
    setBusy(`bootstrap:${s.id}`);
    try {
      await api(`/api/v1/servers/${s.id}/bootstrap`, { method: "POST" });
      toast.success(`${s.name} bootstrapped`);
    } catch (e) {
      toast.error(e.message);
    } finally {
      dismiss(id);
      setBusy(null);
      load();
    }
  };

  const remove = async (s) => {
    if (!(await confirmAction({ title: `Remove ${s.name}?`, danger: true, confirmLabel: "Remove" }))) return;
    setBusy(`remove:${s.id}`);
    try {
      await api(`/api/v1/servers/${s.id}`, { method: "DELETE" });
      toast.success(`Removed ${s.name}`);
      load();
      onChanged?.();
    } catch (e) { toast.error(e.message); } finally { setBusy(null); }
  };

  const add = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const payload = Object.fromEntries([...fd.entries()].filter(([, v]) => v !== ""));
    if (payload.ssh_port) payload.ssh_port = Number(payload.ssh_port);
    setBusy("add");
    try {
      await api("/api/v1/servers", { method: "POST", body: JSON.stringify(payload) });
      toast.success(`Added ${payload.name}`);
      e.target.reset();
      load();
      onChanged?.();
    } catch (err) { toast.error(err.message); } finally { setBusy(null); }
  };

  if (error) {
    return html`<section class="panel">
      <div class="panel-head"><h1>Servers</h1></div>
      <p class="muted">Could not load servers: ${error}</p>
      <div><button class="btn" onClick=${retry}>Retry</button></div>
    </section>`;
  }

  const online = (servers || []).filter((s) => s.status === "online").length;
  const offline = (servers || []).filter((s) => s.status !== "online");
  const affectedApps = offline.flatMap((s) => appsOn(s.id));

  return html`
    <section class="panel">
      <div class="panel-head"><h1>Servers</h1></div>
      ${servers !== null && !error ? html`
        <div class="status-strip" aria-label="Fleet status">
          <div class="stat"><span>Fleet</span><span><strong>${servers.length}</strong>&nbsp;server${servers.length === 1 ? "" : "s"}</span></div>
          <div class="stat"><span>Reachable</span><span><strong>${online}</strong>&nbsp;online</span></div>
          <div class="stat"><span>Placed apps</span><span><strong>${apps.length}</strong></span></div>
          ${offline.length > 0 ? html`<div class="stat"><span>Offline</span><span><strong>${offline.length}</strong>&nbsp;server${offline.length === 1 ? "" : "s"}</span></div>` : null}
        </div>` : null}
      ${offline.length > 0 ? html`<section class="panel notice notice-critical" role="alert" aria-label="Offline servers">
        <div class="panel-head"><strong>Offline: ${offline.map((s) => s.name).join(", ")}</strong></div>
        ${affectedApps.length > 0
          ? html`<div>Affected apps: ${affectedApps.map((a, i) => html`${i > 0 ? ", " : ""}<a class="mono" href=${`#/apps/${a.id}/overview`}>${a.name}</a>`)}
            </div>`
          : html`<div class="muted">No apps placed on the offline servers.</div>`}
      </section>` : null}
      ${servers !== null && !error && servers.length > 0 ? html`
        <div class="section-band"><h2>Placement</h2><span class="muted small">apps per server</span></div>
        <div class="grid">
          ${servers.map((s) => html`
            <div class=${"server-block" + (s.status !== "online" ? " offline" : "")}>
              <div><a class="mono" href=${`#/servers/${s.id}`}><strong>${s.name}</strong></a>${s.is_local ? html`<span class="muted small"> · local</span>` : null}</div>
              <div class="muted small">${s.status}${appsOn(s.id).length > 0 ? ` · ${appsOn(s.id).length} app${appsOn(s.id).length === 1 ? "" : "s"}` : " · empty"}</div>
              <${Capacity} cap=${s.capacity} />
              ${appsOn(s.id).length > 0 ? html`<div class="server-apps">
                ${appsOn(s.id).map((a) => html`<div class="server-app">
                  <span class=${"dot " + appDot(a)} aria-hidden="true"></span>
                  <a href=${`#/apps/${a.id}/overview`}>${a.name}</a>
                </div>`)}
              </div>` : null}
            </div>`)}
        </div>` : null}
      ${servers === null
        ? html`<p class="muted">Loading…</p>`
        : servers.length === 0
          ? html`<p class="muted">No servers yet.</p>`
          : html`<div class="section-band"><h2>All servers</h2></div>
            <div class="table-wrap"><table class="stacked">
            <thead><tr><th>Status</th><th>Name</th><th>Apps</th><th>Load</th><th>Agent</th><th>Last seen</th><th></th></tr></thead>
            <tbody>
              ${servers.map((s) => html`
                <tr>
                  <td data-label="Status"><span class=${"badge " + statusClass(s.status)}>${s.status}</span></td>
                  <td data-label="Name"><a class="mono" href=${`#/servers/${s.id}`}>${s.name}</a>${s.is_local ? html`<span class="muted small"> · local</span>` : null}</td>
                  <td data-label="Apps"><strong>${appsOn(s.id).length}</strong></td>
                  <td data-label="Load" class="mono small">${capSummary(s.capacity)}</td>
                  <td data-label="Agent" class="muted mono">${s.agent_version || "—"}</td>
                  <td data-label="Last seen" class="muted">${s.last_seen_at ? fmtTime(s.last_seen_at) : "—"}</td>
                  <td class="controls no-label">
                    ${user && html`<button class="btn small ghost" disabled=${busy !== null} onClick=${() => validate(s)}>
                      ${busy === `validate:${s.id}` ? "Validating…" : "Validate"}</button>`}
                    ${user && !s.is_local && html`<button class="btn small" disabled=${busy !== null} onClick=${() => bootstrap(s)}>
                      ${busy === `bootstrap:${s.id}` ? "Bootstrapping…" : "Bootstrap"}</button>`}
                    ${user && !s.is_local && html`<button class="btn small ghost" disabled=${busy !== null} onClick=${() => remove(s)}>Remove</button>`}
                  </td>
                </tr>`)}
            </tbody>
          </table></div>`}
      ${user && html`<form class="form" style="margin-top:14px" onSubmit=${add}>
        <h3>Add server</h3>
        <div class="row">
          <label>Name <input name="name" placeholder="worker-1" required /></label>
          <label>Address <input name="address" placeholder="10.0.0.12" required /></label>
        </div>
        <div class="row">
          <label>SSH host <input name="ssh_host" placeholder="10.0.0.12" /></label>
          <label>SSH user <input name="ssh_user" placeholder="root" /></label>
        </div>
        <div><button class="btn" type="submit" disabled=${busy === "add"}>${busy === "add" ? "Adding…" : "Add server"}</button></div>
      </form>`}
    </section>`;
}
