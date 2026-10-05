import { html } from "../lib/html.js";
import { useState, useEffect, useCallback } from "preact/hooks";
import { api } from "../lib/api.js";
import { dismiss, toast } from "../lib/toast.js";
import { confirmAction } from "../lib/confirm.js";
import { fmtTime } from "../lib/format.js";

function statusClass(s) {
  return s === "online" ? "running" : s === "offline" ? "failed" : "stopped";
}

export function ServersView({ user, onChanged }) {
  const [servers, setServers] = useState(null);
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
  }, []);

  useEffect(() => { load(); }, [load]);

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

  return html`
    <section class="panel">
      <div class="panel-head"><h1>Servers</h1></div>
      ${servers !== null && !error ? html`
        <div class="status-strip" aria-label="Fleet status">
          <div class="stat"><span>Fleet</span><span><strong>${servers.length}</strong>&nbsp;server${servers.length === 1 ? "" : "s"}</span></div>
          <div class="stat"><span>Reachable</span><span><strong>${online}</strong>&nbsp;online</span></div>
        </div>` : null}
      ${servers === null
        ? html`<p class="muted">Loading…</p>`
        : servers.length === 0
          ? html`<p class="muted">No servers yet.</p>`
          : html`<div class="table-wrap"><table class="stacked">
            <thead><tr><th>Name</th><th>Address</th><th>Status</th><th>Last seen</th><th>Agent</th><th></th></tr></thead>
            <tbody>
              ${servers.map((s) => html`
                <tr>
                  <td data-label="Name"><a class="mono" href=${`#/servers/${s.id}`}>${s.name}</a>${s.is_local ? html` <span class="pill">local</span>` : null}</td>
                  <td data-label="Address" class="mono">${s.address}</td>
                  <td data-label="Status"><span class=${"badge " + statusClass(s.status)}>${s.status}</span></td>
                  <td data-label="Last seen" class="muted">${s.last_seen_at ? fmtTime(s.last_seen_at) : "—"}</td>
                  <td data-label="Agent" class="muted mono">${s.agent_version || "—"}</td>
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
