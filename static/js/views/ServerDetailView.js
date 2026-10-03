import { html } from "../lib/html.js";
import { useEffect, useState } from "preact/hooks";
import { api } from "../lib/api.js";
import { toast } from "../lib/toast.js";
import { fmtTime, serverName } from "../lib/format.js";
import { StatusBadge } from "../components/StatusBadge.js";

export function ServerDetailView({ id }) {
  const [server, setServer] = useState(null);
  const [apps, setApps] = useState([]);
  const [missing, setMissing] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        const [s, a] = await Promise.all([api(`/api/v1/servers/${id}`), api("/api/v1/apps")]);
        setServer(s.server);
        setApps((a.applications || []).filter((x) => x.server_id === id));
      } catch (e) {
        if (e.status === 404) setMissing(true);
        else toast.error(e.message);
      }
    })();
  }, [id]);

  if (missing) {
    return html`<section class="panel"><p class="muted">Server not found.</p>
      <a href="#/servers">← Back to servers</a></section>`;
  }
  if (!server) return html`<section class="panel"><p class="muted">Loading…</p></section>`;

  return html`
    <section class="panel">
      <a href="#/servers" class="muted">← Servers</a>
      <h1 class="mono">${server.name}</h1>
      <table>
        <tbody>
          <tr><th>Address</th><td class="mono">${server.address}</td></tr>
          <tr><th>Status</th><td>${server.status}</td></tr>
          <tr><th>Last seen</th><td>${server.last_seen_at ? fmtTime(server.last_seen_at) : "—"}</td></tr>
          <tr><th>Agent version</th><td class="mono">${server.agent_version || "—"}</td></tr>
          <tr><th>Role</th><td>${server.is_local ? "control plane" : "worker"}</td></tr>
        </tbody>
      </table>
      <h2 style="margin-top:16px">Applications on this server</h2>
      ${apps.length === 0
        ? html`<p class="muted">No apps placed here.</p>`
        : html`<div class="table-wrap"><table><thead><tr><th>Name</th><th>Status</th></tr></thead>
            <tbody>${apps.map((a) => html`<tr>
              <td><a class="mono" href=${`#/apps/${a.id}/overview`}>${a.name}</a></td>
              <td><${StatusBadge} status=${a.status} /></td></tr>`)}</tbody></table></div>`}
    </section>`;
}
