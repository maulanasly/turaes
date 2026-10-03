import { html } from "../lib/html.js";
import { useState, useEffect, useCallback } from "preact/hooks";
import { api } from "../lib/api.js";
import { toast } from "../lib/toast.js";
import { confirmAction } from "../lib/confirm.js";
import { fmtTime } from "../lib/format.js";

function statusClass(s) {
  return s === "online" ? "running" : s === "offline" ? "failed" : "stopped";
}

export function ServersView({ user, onChanged }) {
  const [servers, setServers] = useState(null);

  const load = useCallback(async () => {
    try {
      const r = await api("/api/v1/servers");
      setServers(r.servers || []);
    } catch (e) {
      if (e.status === 401) setServers([]);
      else toast.error(e.message);
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  const validate = async (s) => {
    try {
      const r = await api(`/api/v1/servers/${s.id}/validate`, { method: "POST" });
      toast[r.reachable ? "success" : "error"](`${s.name}: ${r.status}`);
      load();
      onChanged?.();
    } catch (e) { toast.error(e.message); }
  };

  const bootstrap = async (s) => {
    if (!(await confirmAction({
      title: `Bootstrap ${s.name}?`,
      body: "Installs and starts the turaes agent on this node over SSH.",
      confirmLabel: "Bootstrap",
    }))) return;
    const id = toast.info(`Bootstrapping ${s.name}…`, 0);
    try {
      await api(`/api/v1/servers/${s.id}/bootstrap`, { method: "POST" });
      toast.success(`${s.name} bootstrapped`);
    } catch (e) {
      toast.error(e.message);
    } finally {
      void id;
      load();
    }
  };

  const remove = async (s) => {
    if (!(await confirmAction({ title: `Remove ${s.name}?`, danger: true, confirmLabel: "Remove" }))) return;
    try {
      await api(`/api/v1/servers/${s.id}`, { method: "DELETE" });
      toast.success(`Removed ${s.name}`);
      load();
      onChanged?.();
    } catch (e) { toast.error(e.message); }
  };

  const add = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const payload = Object.fromEntries([...fd.entries()].filter(([, v]) => v !== ""));
    if (payload.ssh_port) payload.ssh_port = Number(payload.ssh_port);
    try {
      await api("/api/v1/servers", { method: "POST", body: JSON.stringify(payload) });
      toast.success(`Added ${payload.name}`);
      e.target.reset();
      load();
      onChanged?.();
    } catch (err) { toast.error(err.message); }
  };

  return html`
    <section class="panel">
      <div class="panel-head"><h1>Servers</h1></div>
      ${servers === null
        ? html`<p class="muted">Loading…</p>`
        : html`<table>
            <thead><tr><th>Name</th><th>Address</th><th>Status</th><th>Last seen</th><th>Agent</th><th></th></tr></thead>
            <tbody>
              ${servers.map((s) => html`
                <tr>
                  <td><a class="mono" href=${`#/servers/${s.id}`}>${s.name}</a>${s.is_local ? html` <span class="pill">local</span>` : null}</td>
                  <td class="mono">${s.address}</td>
                  <td><span class=${"badge " + statusClass(s.status)}>${s.status}</span></td>
                  <td class="muted">${s.last_seen_at ? fmtTime(s.last_seen_at) : "—"}</td>
                  <td class="muted mono">${s.agent_version || "—"}</td>
                  <td class="controls">
                    ${user && html`<button class="btn small ghost" onClick=${() => validate(s)}>Validate</button>`}
                    ${user && !s.is_local && html`<button class="btn small" onClick=${() => bootstrap(s)}>Bootstrap</button>`}
                    ${user && !s.is_local && html`<button class="btn small ghost" onClick=${() => remove(s)}>Remove</button>`}
                  </td>
                </tr>`)}
            </tbody>
          </table>`}
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
        <div><button class="btn" type="submit">Add server</button></div>
      </form>`}
    </section>`;
}
