import { html } from "../lib/html.js";
import { useEffect, useState, useCallback, useMemo } from "preact/hooks";
import { api } from "../lib/api.js";
import { toast } from "../lib/toast.js";
import { confirmAction } from "../lib/confirm.js";
import { fmtBytes, parseTs, fmtTime, shortHash, serverName, runtimeLabel } from "../lib/format.js";
import { APP_TABS, navigate } from "../lib/router.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { Skeleton } from "../components/Skeleton.js";
import { Chart } from "../components/Chart.js";
import { LogViewer } from "../components/LogViewer.js";

const TAB_LABEL = {
  overview: "Overview",
  deployments: "Deployments",
  environment: "Environment",
  logs: "Logs",
  settings: "Settings",
};

function Kpi({ label, value }) {
  return html`<div class="kpi"><div class="kpi-label">${label}</div><div class="kpi-value">${value}</div></div>`;
}

function Overview({ data }) {
  const metrics = data.metrics || [];
  const visitors = data.visitors || [];
  const cpu = metrics.map((m) => ({ x: parseTs(m.recorded_at), y: m.cpu_pct }));
  const mem = metrics.map((m) => ({ x: parseTs(m.recorded_at), y: m.mem_bytes }));
  const visitSeries = useMemo(() => {
    const map = new Map();
    for (const v of visitors) {
      const t = parseTs(v.recorded_at);
      map.set(t, (map.get(t) || 0) + v.visits);
    }
    return [...map.entries()].sort((a, b) => a[0] - b[0]).map(([x, y]) => ({ x, y }));
  }, [visitors]);
  const regions = useMemo(() => {
    const m = new Map();
    for (const v of visitors) {
      const e = m.get(v.region) || { visits: 0, uniques: 0 };
      e.visits += v.visits;
      e.uniques = Math.max(e.uniques, v.uniques);
      m.set(v.region, e);
    }
    return [...m.entries()].sort((a, b) => b[1].visits - a[1].visits);
  }, [visitors]);

  const last = metrics[metrics.length - 1];
  const totalVisits = regions.reduce((s, [, r]) => s + r.visits, 0);
  const maxUniques = regions.reduce((s, [, r]) => Math.max(s, r.uniques), 0);

  return html`
    <div class="detail">
      <div class="kpis">
        <${Kpi} label="CPU now" value=${last ? `${last.cpu_pct.toFixed(1)}%` : "—"} />
        <${Kpi} label="Memory now" value=${last ? fmtBytes(last.mem_bytes) : "—"} />
        <${Kpi} label="Requests observed" value=${totalVisits} />
        <${Kpi} label="Unique visitors" value=${maxUniques} />
      </div>
      <div class="charts">
        <${Chart} title="CPU %" points=${cpu} color="var(--cpu)" formatY=${(v) => v.toFixed(1)} />
        <${Chart} title="Memory" points=${mem} color="var(--mem)" formatY=${fmtBytes} />
      </div>
      <${Chart} title="Requests observed" points=${visitSeries} color="var(--visits)" formatY=${(v) => Math.round(v)} />
      <div>
        <h2>Visitors by region</h2>
        ${regions.length === 0
          ? html`<p class="muted">No visits recorded yet.</p>`
          : html`<table><thead><tr><th>Region</th><th>Visits</th><th>Unique</th></tr></thead>
              <tbody>${regions.map(([region, r]) => html`
                <tr><td class="mono">${region}</td><td>${r.visits}</td><td>${r.uniques}</td></tr>`)}
              </tbody></table>`}
      </div>
    </div>`;
}

function Deployments({ deployments, onRollback }) {
  if (!deployments) return html`<${Skeleton} lines={3} />`;
  if (deployments.length === 0) return html`<p class="muted">No builds yet.</p>`;
  return html`
    <table>
      <thead><tr><th>Status</th><th>Build</th><th>Started</th><th>Finished</th><th></th></tr></thead>
      <tbody>
        ${deployments.map((d) => html`
          <tr>
            <td><${StatusBadge} status=${d.status} /></td>
            <td class="mono" title=${d.artifact_hash || ""}>${shortHash(d.artifact_hash)}
              ${d.artifact_hash && html`<button class="btn small ghost" onClick=${() => {
                navigator.clipboard?.writeText(d.artifact_hash); toast.success("Build id copied");
              }}>Copy</button>`}
            </td>
            <td class="muted">${fmtTime(d.started_at)}</td>
            <td class="muted">${fmtTime(d.finished_at)}</td>
            <td>${d.log && html`<details><summary class="muted">log</summary><pre class="log">${d.log}</pre></details>`}</td>
          </tr>`)}
      </tbody>
    </table>
    <p class="muted">Rollback restores the previous build. <button class="btn small ghost" onClick=${onRollback}>Rollback</button></p>`;
}

function Environment({ env, appId, reload }) {
  const [reveal, setReveal] = useState(false);
  const save = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const key = fd.get("key");
    const value = fd.get("value");
    try {
      await api(`/api/v1/apps/${appId}/env/${encodeURIComponent(key)}`, {
        method: "PUT",
        body: JSON.stringify({ value }),
      });
      toast.success(`Saved ${key}`);
      e.target.reset();
      reload();
    } catch (err) {
      toast.error(err.message);
    }
  };
  const remove = async (key) => {
    if (!(await confirmAction({ title: `Remove ${key}?`, danger: true, confirmLabel: "Remove" }))) return;
    try {
      await api(`/api/v1/apps/${appId}/env/${encodeURIComponent(key)}`, { method: "DELETE" });
      toast.success(`Removed ${key}`);
      reload();
    } catch (err) {
      toast.error(err.message);
    }
  };
  return html`
    <div>
      ${!env || env.length === 0
        ? html`<p class="muted">No variables.</p>`
        : html`<table><thead><tr><th>Key</th><th>Updated</th><th></th></tr></thead>
            <tbody>${env.map((e) => html`
              <tr><td class="mono">${e.key}</td><td class="muted">${fmtTime(e.created_at)}</td>
              <td><button class="btn small ghost" onClick=${() => remove(e.key)}>Remove</button></td></tr>`)}
            </tbody></table>`}
      <form class="form" style="margin-top:12px" onSubmit=${save}>
        <div class="row">
          <label>Key <input name="key" placeholder="API_KEY" required /></label>
          <label>Value
            <span class="pw">
              <input name="value" type=${reveal ? "text" : "password"} placeholder="…" required />
              <button type="button" class="btn small ghost" onClick=${() => setReveal((v) => !v)}>
                ${reveal ? "Hide" : "Show"}
              </button>
            </span>
          </label>
        </div>
        <div><button class="btn" type="submit">Save</button> <span class="muted">redeploy to apply</span></div>
      </form>
    </div>`;
}

function Settings({ app, servers, user }) {
  const del = async () => {
    if (!(await confirmAction({
      title: `Delete ${app.name}?`,
      body: "This removes the app and stops it. This cannot be undone.",
      danger: true,
      confirmLabel: "Delete",
    }))) return;
    try {
      await api(`/api/v1/apps/${app.id}`, { method: "DELETE" });
      toast.success(`Deleted ${app.name}`);
      navigate("#/apps");
    } catch (err) {
      toast.error(err.message);
    }
  };
  return html`
    <table>
      <tbody>
        <tr><th>Name</th><td class="mono">${app.name}</td></tr>
        <tr><th>Domain</th><td class="mono">${app.domain || "—"}</td></tr>
        <tr><th>Server</th><td>${serverName(servers, app.server_id)}</td></tr>
        <tr><th>Port</th><td class="mono">${app.port}</td></tr>
        <tr><th>Managed by</th><td>${runtimeLabel(app.runtime)}</td></tr>
        <tr><th>Binary</th><td class="mono">${app.binary_path}</td></tr>
      </tbody>
    </table>
    ${user && html`<div class="danger-zone">
      <h3>Danger zone</h3>
      <button class="btn danger" onClick=${del}>Delete application</button>
    </div>`}`;
}

export function AppDetailView({ id, tab, user, servers }) {
  const [app, setApp] = useState(null);
  const [notFound, setNotFound] = useState(false);
  const [data, setData] = useState({});
  const [deployments, setDeployments] = useState(null);
  const [env, setEnv] = useState(null);
  const [busy, setBusy] = useState(false);

  const loadApp = useCallback(async () => {
    try {
      const r = await api(`/api/v1/apps/${id}`);
      setApp(r.application);
    } catch (e) {
      if (e.status === 404) setNotFound(true);
      else toast.error(e.message);
    }
  }, [id]);

  const loadMetrics = useCallback(async () => {
    try {
      const [m, v] = await Promise.all([
        api(`/api/v1/apps/${id}/stats?hours=1`),
        api(`/api/v1/apps/${id}/visitors?hours=24`),
      ]);
      setData({ metrics: m.metrics || [], visitors: v.visitors || [] });
    } catch { /* ignore */ }
  }, [id]);

  const loadDeployments = useCallback(async () => {
    try {
      const r = await api(`/api/v1/apps/${id}/deployments?limit=15`);
      setDeployments(r.deployments || []);
    } catch { setDeployments([]); }
  }, [id]);

  const loadEnv = useCallback(async () => {
    try {
      const r = await api(`/api/v1/apps/${id}/env`);
      setEnv(r.env || []);
    } catch { setEnv([]); }
  }, [id]);

  useEffect(() => {
    loadApp();
    const t = setInterval(loadApp, 15000);
    return () => clearInterval(t);
  }, [loadApp]);

  useEffect(() => {
    if (tab === "overview") loadMetrics();
    if (tab === "deployments") loadDeployments();
    if (tab === "environment") loadEnv();
  }, [tab, loadMetrics, loadDeployments, loadEnv]);

  const deploy = async () => {
    setBusy(true);
    try {
      await api(`/api/v1/apps/${id}/deploy`, { method: "POST" });
      toast.success("Deploy started");
      loadApp();
      loadDeployments();
    } catch (e) {
      toast.error(e.message);
    } finally {
      setBusy(false);
    }
  };

  const rollback = async () => {
    if (!(await confirmAction({
      title: `Roll back ${app.name}?`,
      body: "Redeploys the previous build.",
      confirmLabel: "Roll back",
    }))) return;
    try {
      await api(`/api/v1/apps/${id}/rollback`, { method: "POST" });
      toast.success("Rolled back");
      loadApp();
      loadDeployments();
    } catch (e) {
      toast.error(e.message);
    }
  };

  if (notFound) {
    return html`<section class="panel"><p class="muted">Application not found.</p>
      <a href="#/apps">← Back to applications</a></section>`;
  }
  if (!app) return html`<section class="panel"><${Skeleton} lines={4} height=${28} /></section>`;

  return html`
    <section class="panel">
      <div class="panel-head">
        <div>
          <a href="#/apps" class="muted">← Applications</a>
          <h1 class="mono">${app.name} <${StatusBadge} status=${app.status} /></h1>
          <div class="muted small">
            ${app.domain ? html`<span class="mono">${app.domain}</span> · ` : null}
            ${serverName(servers, app.server_id)} · :${app.port} · ${runtimeLabel(app.runtime)}
          </div>
        </div>
        <div class="controls">
          ${user && html`
            <button class="btn" disabled=${busy} onClick=${deploy}>${busy ? "Deploying…" : "Deploy"}</button>
            <button class="btn ghost" onClick=${rollback}>Rollback</button>`}
        </div>
      </div>
      <nav class="tabs" role="tablist">
        ${APP_TABS.map((t) => html`
          <a role="tab" aria-selected=${t === tab} class=${t === tab ? "active" : ""}
            href=${`#/apps/${id}/${t}`}>${TAB_LABEL[t]}</a>`)}
      </nav>
      <div class="tab-body" role="tabpanel">
        ${tab === "overview" && html`<${Overview} data=${data} />`}
        ${tab === "deployments" && html`<${Deployments} deployments=${deployments} onRollback=${rollback} />`}
        ${tab === "environment" && html`<${Environment} env=${env} appId=${id} reload=${loadEnv} />`}
        ${tab === "logs" && html`<${LogViewer} appId=${id} />`}
        ${tab === "settings" && html`<${Settings} app=${app} servers=${servers} user=${user} />`}
      </div>
    </section>`;
}
