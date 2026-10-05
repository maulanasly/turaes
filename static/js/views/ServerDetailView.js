import { html } from "../lib/html.js";
import { useEffect, useState } from "preact/hooks";
import { api, oapi } from "../lib/api.js";
import { toast } from "../lib/toast.js";
import { fmtTime, fmtBytes, parseTs, fmtRangeLabel } from "../lib/format.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { Chart } from "../components/Chart.js";

const RANGES = [["1h", 1], ["6h", 6], ["24h", 24], ["7d", 168]];
const rangeHours = (r) => (RANGES.find(([k]) => k === r) || [null, 24])[1];

export function ServerDetailView({ id }) {
  const [server, setServer] = useState(null);
  const [apps, setApps] = useState([]);
  const [missing, setMissing] = useState(false);
  const [error, setError] = useState(null);
  const [reloadKey, setReloadKey] = useState(0);
  const [metrics, setMetrics] = useState([]);
  const [range, setRange] = useState("24h");
  const [statsErr, setStatsErr] = useState(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [s, a] = await Promise.all([api(`/api/v1/servers/${id}`), oapi("/apps")]);
        if (cancelled) return;
        setServer(s.server);
        setApps((a.applications || []).filter((x) => x.server_id === id));
        setError(null);
      } catch (e) {
        if (cancelled) return;
        if (e.status === 404) setMissing(true);
        else { setError(e.message); toast.error(e.message); }
      }
    })();
    return () => { cancelled = true; };
  }, [id, reloadKey]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const r = await api(`/api/v1/servers/${id}/stats?hours=${rangeHours(range)}`);
        if (cancelled) return;
        setMetrics(r.metrics || []);
        setStatsErr(null);
      } catch (e) {
        if (cancelled) return;
        if (e.status === 403) setStatsErr("operator access required");
        else if (e.status === 401) setStatsErr(null);
        else setStatsErr(e.message);
      }
    })();
    return () => { cancelled = true; };
  }, [id, range]);

  const retry = () => {
    setError(null);
    setMissing(false);
    setServer(null);
    setReloadKey((k) => k + 1);
  };

  if (missing) {
    return html`<section class="panel"><p class="muted">Server not found.</p>
      <a href="#/servers">← Back to servers</a></section>`;
  }
  if (error) {
    return html`<section class="panel"><p class="muted">Could not load server: ${error}</p>
      <div class="controls"><button class="btn" onClick=${retry}>Retry</button>
      <a href="#/servers">← Back to servers</a></div></section>`;
  }
  if (!server) return html`<section class="panel"><p class="muted">Loading…</p></section>`;

  const cpu = metrics.map((m) => ({ x: parseTs(m.recorded_at), y: m.cpu_pct }));
  const mem = metrics.map((m) => ({ x: parseTs(m.recorded_at), y: m.mem_bytes }));
  const latest = metrics[metrics.length - 1];
  const emptyHint = `No host samples in the ${fmtRangeLabel(range)}${server.is_local ? "" : " — remote nodes report capacity once their agent lands"}.`;

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
        : html`<div class="table-wrap"><table class="stacked"><thead><tr><th>Name</th><th>Status</th></tr></thead>
            <tbody>${apps.map((a) => html`<tr>
              <td data-label="Name"><a class="mono" href=${`#/apps/${a.id}/overview`}>${a.name}</a></td>
              <td data-label="Status"><${StatusBadge} status=${a.status} /></td></tr>`)}</tbody></table></div>`}
      <h2 style="margin-top:16px">Capacity <span class="muted small">· host CPU / memory</span></h2>
      <div class="range-bar">
        <div class="seg" role="radiogroup" aria-label="Capacity range">
          ${RANGES.map(([k]) => html`<button type="button" role="radio" aria-checked=${k === range}
            class=${k === range ? "active" : ""} onClick=${() => setRange(k)}>${k}</button>`)}
        </div>
        <span class="muted small" role="status">
          ${latest ? `latest ${latest.cpu_pct.toFixed(1)}% CPU · ${fmtBytes(latest.mem_bytes)} / ${fmtBytes(latest.mem_total_bytes)}` : "no samples"}
        </span>
      </div>
      ${statsErr
        ? html`<p class="muted">Capacity unavailable: ${statsErr}.</p>`
        : html`<div class="charts">
            <${Chart} title="CPU %" points=${cpu} color="var(--cpu)" formatY=${(v) => v.toFixed(1)}
              subtitle=${fmtRangeLabel(range)} rangeHours=${rangeHours(range)} emptyHint=${emptyHint} />
            <${Chart} title="Memory" points=${mem} color="var(--mem)" formatY=${fmtBytes}
              subtitle=${fmtRangeLabel(range)} rangeHours=${rangeHours(range)} emptyHint=${emptyHint} />
          </div>`}
    </section>`;
}
