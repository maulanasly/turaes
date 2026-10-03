// turaes dashboard — zero-build Preact + HTM (vendored), no bundler, no CDN.
// Charts are hand-rolled SVG (no chart library).

import { h, render } from "preact";
import { useState, useEffect, useCallback } from "preact/hooks";
import htm from "htm";

const html = htm.bind(h);

async function api(path, options = {}) {
  const res = await fetch(path, {
    headers: { "content-type": "application/json" },
    credentials: "same-origin",
    ...options,
  });
  if (res.status === 204) return null;
  const body = await res.json().catch(() => ({}));
  if (!res.ok) {
    const err = new Error(body.detail || `HTTP ${res.status}`);
    err.status = res.status;
    throw err;
  }
  return body;
}

const fmtBytes = (n) => {
  if (!n) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(i > 0 && v < 10 ? 1 : 0)} ${units[i]}`;
};
const parseTs = (s) => Date.parse(s.replace(" ", "T") + "Z");
const fmtClock = (d) => d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
const shortHash = (h) => (h ? `${h.slice(7, 15)}…${h.slice(-4)}` : "—");

function Chart({ title, points, color, formatY, height = 150 }) {
  const w = 600, pad = { l: 58, r: 8, t: 8, b: 24 };
  if (!points || points.length === 0) {
    return html`<div class="chart"><div class="chart-title">${title}</div><div class="empty">no data yet</div></div>`;
  }
  const xs = points.map((p) => p.x), ys = points.map((p) => p.y);
  let x0 = Math.min(...xs), x1 = Math.max(...xs);
  let y0 = Math.min(...ys, 0), y1 = Math.max(...ys);
  if (x1 === x0) x1 = x0 + 1;
  if (y1 === y0) y1 = y0 + 1;
  const X = (x) => pad.l + ((x - x0) / (x1 - x0)) * (w - pad.l - pad.r);
  const Y = (y) => height - pad.b - ((y - y0) / (y1 - y0)) * (height - pad.t - pad.b);
  const path = points.map((p, i) => `${i ? "L" : "M"}${X(p.x).toFixed(1)},${Y(p.y).toFixed(1)}`).join(" ");
  const area = `${path} L${X(x1).toFixed(1)},${height - pad.b} L${X(x0).toFixed(1)},${height - pad.b} Z`;
  const t0 = new Date(x0), t1 = new Date(x1);
  return html`
    <div class="chart">
      <div class="chart-title">
        <span>${title}</span>
        <span class="muted">${formatY(y1)} peak</span>
      </div>
      <svg viewBox="0 0 ${w} ${height}" preserveAspectRatio="none" class="spark">
        <line class="grid-line" x1=${pad.l} y1=${Y(y0)} x2=${w - pad.r} y2=${Y(y0)} />
        <line class="grid-line" x1=${pad.l} y1=${Y(y1)} x2=${w - pad.r} y2=${Y(y1)} />
        <path d=${area} fill=${color} opacity="0.12" stroke="none" />
        <path d=${path} fill="none" stroke=${color} stroke-width="2" vector-effect="non-scaling-stroke" />
      </svg>
      <div class="axis"><span>${formatY(y0)}</span><span>${fmtClock(t0)}</span><span>${fmtClock(t1)}</span></div>
    </div>`;
}

function AppCard({ app, selected, onSelect, onDeploy }) {
  const [busy, setBusy] = useState(false);
  const deploy = async (e) => {
    e.stopPropagation();
    setBusy(true);
    try { await onDeploy(app.id); } finally { setBusy(false); }
  };
  return html`
    <div class=${"card" + (selected ? " selected" : "")} onClick=${() => onSelect(app.id)}>
      <h3>
        <span class="mono">${app.name}</span>
        <span class=${"badge " + app.status}>${app.status}</span>
      </h3>
      <div class="stat"><span>domain</span><span class="mono">${app.domain || "—"}</span></div>
      <div class="stat"><span>server</span><span class="mono">${app.server_id || "local"}</span></div>
      <div class="stat"><span>port</span><span class="mono">${app.port}</span></div>
      <div class="stat"><span>runtime</span><span>${app.runtime}</span></div>
      <div style="margin-top:8px">
        <button class="btn small" disabled=${busy} onClick=${deploy}>
          ${busy ? "Deploying…" : "Deploy"}
        </button>
      </div>
    </div>`;
}

function Logs({ appId }) {
  const [lines, setLines] = useState([]);
  const [status, setStatus] = useState("connecting…");
  useEffect(() => {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${location.host}/api/v1/apps/${appId}/logs`);
    setLines([]);
    setStatus("connecting…");
    ws.onopen = () => setStatus("live");
    ws.onmessage = (ev) => {
      setLines((prev) => {
        const next = prev.concat(String(ev.data).split("\n"));
        return next.length > 500 ? next.slice(next.length - 500) : next;
      });
    };
    ws.onerror = () => setStatus("error");
    ws.onclose = () => setStatus("closed");
    return () => ws.close();
  }, [appId]);
  return html`
    <div>
      <h2 style="margin-bottom:8px">Logs <span class="muted">· ${status}</span></h2>
      <pre class="log" style="max-height:300px">${lines.join("\n")}</pre>
    </div>`;
}

function Detail({ app, hours, data, deployments, env, onSetEnv, onDelEnv }) {
  const metrics = data?.metrics || [];
  const visitors = data?.visitors || [];
  const cpu = metrics.map((m) => ({ x: parseTs(m.recorded_at), y: m.cpu_pct }));
  const mem = metrics.map((m) => ({ x: parseTs(m.recorded_at), y: m.mem_bytes }));
  const visitSeries = (() => {
    const map = new Map();
    for (const v of visitors) {
      const t = parseTs(v.recorded_at);
      map.set(t, (map.get(t) || 0) + v.visits);
    }
    return [...map.entries()].sort((a, b) => a[0] - b[0]).map(([x, y]) => ({ x, y }));
  })();
  const regions = (() => {
    const m = new Map();
    for (const v of visitors) {
      const e = m.get(v.region) || { visits: 0, uniques: 0 };
      e.visits += v.visits;
      e.uniques = Math.max(e.uniques, v.uniques);
      m.set(v.region, e);
    }
    return [...m.entries()].sort((a, b) => b[1].visits - a[1].visits);
  })();

  return html`
    <div class="detail">
      <div class="charts">
        <${Chart} title="CPU %" points=${cpu} color="var(--cpu)" formatY=${(v) => v.toFixed(1)} />
        <${Chart} title="Memory" points=${mem} color="var(--mem)" formatY=${fmtBytes} />
      </div>
      <${Chart} title="Visits (per scrape)" points=${visitSeries} color="var(--visits)" formatY=${(v) => Math.round(v)} />
      <div>
        <h2 style="margin-bottom:8px">Visitors by region <span class="muted">· last ${hours}h</span></h2>
        ${regions.length === 0
          ? html`<p class="muted">No visits recorded yet.</p>`
          : html`<table>
              <thead><tr><th>Region</th><th>Visits</th><th>Unique (latest)</th></tr></thead>
              <tbody>
                ${regions.map(([region, r]) => html`
                  <tr><td class="mono">${region}</td><td>${r.visits}</td><td>${r.uniques}</td></tr>`)}
              </tbody>
            </table>`}
      </div>
      <div>
        <h2 style="margin-bottom:8px">Deployments</h2>
        ${!deployments || deployments.length === 0
          ? html`<p class="muted">No deployments yet.</p>`
          : html`<table>
              <thead><tr><th>Status</th><th>Artifact</th><th>Started</th><th>Finished</th></tr></thead>
              <tbody>
                ${deployments.map((d) => html`
                  <tr>
                    <td>${d.status}</td>
                    <td class="mono">${shortHash(d.artifact_hash)}</td>
                    <td class="muted">${d.started_at || ""}</td>
                    <td class="muted">${d.finished_at || "—"}</td>
                  </tr>
                  ${d.log
                    ? html`<tr><td colspan="4">
                        <details><summary class="muted">log</summary>
                          <pre class="log">${d.log}</pre>
                        </details></td></tr>`
                    : null}`)}
              </tbody>
            </table>`}
      </div>
      <div>
        <h2 style="margin-bottom:8px">Environment</h2>
        ${!env || env.length === 0
          ? html`<p class="muted">No variables.</p>`
          : html`<table>
              <thead><tr><th>Key</th><th>Set</th><th></th></tr></thead>
              <tbody>
                ${env.map((e) => html`
                  <tr>
                    <td class="mono">${e.key}</td>
                    <td class="muted">${e.created_at}</td>
                    <td><button class="btn small ghost" onClick=${() => onDelEnv(e.key)}>Remove</button></td>
                  </tr>`)}
              </tbody>
            </table>`}
        <form class="form" style="margin-top:10px" onSubmit=${(ev) => {
          ev.preventDefault();
          const fd = new FormData(ev.target);
          onSetEnv(fd.get("key"), fd.get("value"));
          ev.target.reset();
        }}>
          <div class="row">
            <label>Key <input name="key" placeholder="API_KEY" required /></label>
            <label>Value <input name="value" placeholder="…" required /></label>
          </div>
          <div><button class="btn" type="submit">Save variable</button> <span class="muted">redeploy to apply</span></div>
        </form>
      </div>
      <${Logs} appId=${app.id} />
    </div>`;
}

function ServersPanel({ servers, onChanged }) {
  const [msg, setMsg] = useState("");
  const submit = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const payload = Object.fromEntries([...fd.entries()].filter(([, v]) => v !== ""));
    if (payload.ssh_port) payload.ssh_port = Number(payload.ssh_port);
    try {
      await api("/api/v1/servers", { method: "POST", body: JSON.stringify(payload) });
      e.target.reset();
      setMsg(`added ${payload.name}`);
      onChanged();
    } catch (err) {
      setMsg(err.message);
    }
  };
  return html`
    <section class="panel">
      <div class="panel-head"><h2>Servers</h2><span class="pill">${servers.length}</span></div>
      <table>
        <thead><tr><th>Name</th><th>Address</th><th>Status</th><th>Local</th></tr></thead>
        <tbody>
          ${servers.map((s) => html`
            <tr>
              <td class="mono">${s.name}</td>
              <td class="mono">${s.address}</td>
              <td>${s.status}</td>
              <td>${s.is_local ? "yes" : "no"}</td>
            </tr>`)}
        </tbody>
      </table>
      <form class="form" onSubmit=${submit} style="margin-top:12px">
        <div class="row">
          <label>Name <input name="name" placeholder="worker-1" required /></label>
          <label>Address <input name="address" placeholder="10.0.0.12" required /></label>
        </div>
        <label>SSH host <input name="ssh_host" placeholder="10.0.0.12" /></label>
        <div><button class="btn" type="submit">Add server</button> <span class="muted">${msg}</span></div>
      </form>
    </section>`;
}

function AddForm({ onCreated }) {
  const [msg, setMsg] = useState("");
  const submit = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const payload = Object.fromEntries([...fd.entries()].filter(([, v]) => v !== ""));
    if (payload.port) payload.port = Number(payload.port);
    try {
      await api("/api/v1/apps", { method: "POST", body: JSON.stringify(payload) });
      e.target.reset();
      setMsg(`created ${payload.name}`);
      onCreated();
    } catch (err) {
      setMsg(err.message);
    }
  };
  return html`
    <form class="form" onSubmit=${submit}>
      <div class="row">
        <label>Name <input name="name" placeholder="beruang" required /></label>
        <label>Port <input name="port" type="number" placeholder="8000" required /></label>
      </div>
      <label>Binary path <input name="binary_path" placeholder="/srv/beruang/target/release/beruang-gateway" required /></label>
      <div class="row">
        <label>Domain <input name="domain" placeholder="beruang.example.com" /></label>
        <label>Runtime <input name="runtime" placeholder="systemd" /></label>
      </div>
      <div><button class="btn" type="submit">Create</button> <span class="muted">${msg}</span></div>
    </form>`;
}

function Dashboard() {
  const [user, setUser] = useState(undefined);
  const [apps, setApps] = useState([]);
  const [servers, setServers] = useState([]);
  const [deployments, setDeployments] = useState([]);
  const [env, setEnv] = useState([]);
  const [selected, setSelected] = useState(null);
  const [hours, setHours] = useState(1);
  const [data, setData] = useState({});
  const [error, setError] = useState("");
  const [showAdd, setShowAdd] = useState(false);

  const loadApps = useCallback(async () => {
    try {
      const r = await api("/api/v1/apps");
      setApps(r.applications || []);
      setError("");
      return r.applications || [];
    } catch (e) {
      setError(e.message);
      return [];
    }
  }, []);

  const loadUser = useCallback(async () => {
    try { setUser(await api("/auth/me")); } catch { setUser(null); }
  }, []);

  const loadServers = useCallback(async () => {
    try {
      const r = await api("/api/v1/servers");
      setServers(r.servers || []);
    } catch { /* unauthenticated */ }
  }, []);

  const loadDetail = useCallback(async (id, hsel) => {
    if (!id) return;
    try {
      const [m, v] = await Promise.all([
        api(`/api/v1/apps/${id}/stats?hours=${hsel}`),
        api(`/api/v1/apps/${id}/visitors?hours=${hsel}`),
      ]);
      setData({ metrics: m.metrics, visitors: v.visitors });
    } catch (e) {
      setError(e.message);
    }
  }, []);

  const loadDeployments = useCallback(async (id) => {
    if (!id) { setDeployments([]); return; }
    try {
      const r = await api(`/api/v1/apps/${id}/deployments?limit=15`);
      setDeployments(r.deployments || []);
    } catch { setDeployments([]); }
  }, []);

  const loadEnv = useCallback(async (id) => {
    if (!id) { setEnv([]); return; }
    try {
      const r = await api(`/api/v1/apps/${id}/env`);
      setEnv(r.env || []);
    } catch { setEnv([]); }
  }, []);

  const setEnvVar = useCallback(async (id, key, value) => {
    try {
      await api(`/api/v1/apps/${id}/env/${encodeURIComponent(key)}`, {
        method: "PUT",
        body: JSON.stringify({ value }),
      });
    } catch (e) { setError(e.message); }
    loadEnv(id);
  }, [loadEnv]);

  const delEnvVar = useCallback(async (id, key) => {
    try {
      await api(`/api/v1/apps/${id}/env/${encodeURIComponent(key)}`, { method: "DELETE" });
    } catch (e) { setError(e.message); }
    loadEnv(id);
  }, [loadEnv]);

  useEffect(() => { loadUser(); loadApps(); loadServers(); }, [loadUser, loadApps, loadServers]);
  useEffect(() => {
    loadDetail(selected, hours);
    loadDeployments(selected);
    loadEnv(selected);
  }, [selected, hours, loadDetail, loadDeployments, loadEnv]);
  useEffect(() => {
    const t = setInterval(() => {
      loadApps(); loadServers();
      if (selected) { loadDetail(selected, hours); loadDeployments(selected); loadEnv(selected); }
    }, 15000);
    return () => clearInterval(t);
  }, [loadApps, loadServers, loadDetail, loadDeployments, loadEnv, selected, hours]);

  const deploy = async (id) => {
    try { await api(`/api/v1/apps/${id}/deploy`, { method: "POST" }); } catch (e) { setError(e.message); }
    await loadApps();
    loadDeployments(id);
  };

  const rollback = async (id) => {
    try { await api(`/api/v1/apps/${id}/rollback`, { method: "POST" }); } catch (e) { setError(e.message); }
    await loadApps();
    loadDeployments(id);
  };

  const selectedApp = apps.find((a) => a.id === selected);

  return html`
    <header class="topbar">
      <div class="brand">turaes <span class="muted">· docker-less paas</span></div>
      <div class="controls">
        <select value=${hours} onChange=${(e) => setHours(Number(e.target.value))}
          style="background:#0d0f13;color:var(--text);border:1px solid var(--border);border-radius:6px;padding:6px 8px">
          <option value="1">1h</option>
          <option value="6">6h</option>
          <option value="24">24h</option>
        </select>
        <button class="btn ghost" onClick=${() => { loadApps(); if (selected) loadDetail(selected, hours); }}>Refresh</button>
        ${user
          ? html`<span class="muted">${user.login}</span> <button class="btn ghost" onClick=${async () => { await api("/auth/logout", { method: "POST" }); setUser(null); }}>Sign out</button>`
          : html`<a class="btn" href="/auth/login">Sign in with GitHub</a>`}
      </div>
    </header>

    <main>
      ${error && html`<div class="panel" style="border-color:var(--bad)"><span class="muted">${error}</span></div>`}

      <section class="panel">
        <div class="panel-head">
          <h2>Applications</h2>
          <button class="btn ghost" onClick=${() => setShowAdd((v) => !v)}>
            ${showAdd ? "Close" : "+ Add"}
          </button>
        </div>
        ${showAdd && html`<div style="margin-bottom:14px"><${AddForm} onCreated=${loadApps} /></div>`}
        ${apps.length === 0
          ? html`<p class="muted">No applications yet.${user ? "" : " Sign in to manage apps."}</p>`
          : html`<div class="grid">
              ${apps.map((a) => html`
                <${AppCard} app=${a} selected=${a.id === selected} onSelect=${setSelected} onDeploy=${deploy} />`)}
            </div>`}
      </section>

      <${ServersPanel} servers=${servers} onChanged=${loadServers} />

      ${selectedApp && html`
        <section class="panel">
          <div class="panel-head">
            <h2 class="mono">${selectedApp.name}</h2>
            <div class="controls">
              <span class="pill">${selectedApp.runtime} · :${selectedApp.port}</span>
              <button class="btn small ghost" onClick=${() => rollback(selectedApp.id)}>Rollback</button>
            </div>
          </div>
          <${Detail}
            app=${selectedApp}
            hours=${hours}
            data=${data}
            deployments=${deployments}
            env=${env}
            onSetEnv=${(k, v) => setEnvVar(selectedApp.id, k, v)}
            onDelEnv=${(k) => delEnvVar(selectedApp.id, k)}
          />
        </section>`}
    </main>`;
}

render(html`<${Dashboard} />`, document.getElementById("app"));
