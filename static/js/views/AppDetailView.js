import { html } from "../lib/html.js";
import { useEffect, useState, useCallback, useMemo, useRef } from "preact/hooks";
import { oapi } from "../lib/api.js";
import { toast } from "../lib/toast.js";
import { confirmAction } from "../lib/confirm.js";
import { deployConsequences, restartConsequences, stopConsequences, startConsequences, rollbackConsequences, lifecycleSummary, resourceLimitErrors, validateCommandLines, parseCommandArgv, textPatch, argvPatch, hasPendingDeployment } from "../lib/appForm.js";
import { fmtBytes, parseTs, fmtTime, fmtRangeLabel, timeAgo, shortHash, serverName, runtimeLabel } from "../lib/format.js";
import { APP_TABS, navigate } from "../lib/router.js";
import { RANGES, DEFAULT_RANGE, normalizeRange, rangeToHours } from "../lib/route.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { Skeleton } from "../components/Skeleton.js";
import { Chart } from "../components/Chart.js";
import { LogViewer } from "../components/LogViewer.js";

const TAB_LABEL = {
  overview: "Overview",
  deployments: "Deployments",
  activity: "Activity",
  environment: "Environment",
  logs: "Logs",
  settings: "Settings",
};

function FieldError({ errors, name }) {
  return errors[name]
    ? html`<span class="form-error" id=${`field-error-${name}`}>${errors[name]}</span>`
    : null;
}

function Activity({ entries }) {
  if (!entries) return html`<p class="muted">Loading…</p>`;
  if (!entries.length) return html`<p class="muted">No recorded actions yet.</p>`;
  return html`
    <table class="table stacked">
      <thead><tr><th>When</th><th>Actor</th><th>Action</th><th>Detail</th></tr></thead>
      <tbody>
        ${entries.map((e) => html`
          <tr>
            <td data-label="When" class="mono small">${fmtTime(e.created_at)}</td>
            <td data-label="Actor">${e.actor_login || "system"}</td>
            <td data-label="Action" class="mono">${e.action}</td>
            <td data-label="Detail" class="mono small muted">${e.metadata || "—"}</td>
          </tr>`)}
      </tbody>
    </table>`;
}

function Kpi({ label, value }) {
  return html`<div class="kpi"><div class="kpi-label">${label}</div><div class="kpi-value">${value}</div></div>`;
}

const TAB_GROUPS = [
  { label: "Operate", tabs: ["overview", "logs"] },
  { label: "Release", tabs: ["deployments", "activity"] },
  { label: "Configure", tabs: ["environment", "settings"] },
];

function RecentActivity({ entries, appId }) {
  const items = (entries || []).slice(0, 5);
  if (!entries) return html`<p class="muted">Loading recent activity…</p>`;
  if (!items.length) return html`<p class="muted">No recorded actions yet.</p>`;
  return html`
    <ul class="timeline">
      ${items.map((e) => html`<li>
        <span class="mono small muted">${fmtTime(e.created_at)}</span>
        <span class=${"t-dot " + (/fail|unhealthy|error/i.test(e.action || "") ? "bad" : /deploy|restart|start/i.test(e.action || "") ? "warn" : "ok")} aria-hidden="true"></span>
        <span><a href=${`#/apps/${appId}/activity`}>${e.action}</a>
          <span class="muted"> — ${e.actor_login || "system"}</span></span>
      </li>`)}
    </ul>`;
}

function Overview({ data, range, onRange, updatedAt, loading, onRefresh, activity, appId }) {
  const hours = rangeToHours(range);
  const rangeLabel = fmtRangeLabel(range);
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
  const cpuAvg = metrics.length
    ? metrics.reduce((s, m) => s + m.cpu_pct, 0) / metrics.length
    : null;
  const memPeak = metrics.length ? Math.max(...metrics.map((m) => m.mem_bytes)) : null;
  const freshness = updatedAt ? `updated ${timeAgo(Date.now() - updatedAt)}` : "loading…";
  const emptyHint = `No samples in the ${rangeLabel} — the app may have been down, or samples aged out of retention.`;

  return html`
    <div class="detail">
      <div class="range-bar">
        <div class="seg" role="radiogroup" aria-label="Time range">
          ${RANGES.map((r) => html`
            <button type="button" role="radio" aria-checked=${r === range}
              class=${r === range ? "active" : ""} disabled=${loading}
              onClick=${() => onRange(r)} title=${fmtRangeLabel(r)}>${r}</button>`)}
        </div>
        <div class="controls">
          <span class="muted small" role="status">${freshness} · ${rangeLabel}</span>
          <button type="button" class="btn small ghost" disabled=${loading} onClick=${onRefresh}>
            ${loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </div>
      <div class="kpis">
        <${Kpi} label=${`CPU avg (${rangeLabel})`} value=${cpuAvg === null ? "—" : `${cpuAvg.toFixed(1)}%`} />
        <${Kpi} label=${`Memory peak (${rangeLabel})`} value=${memPeak === null ? "—" : fmtBytes(memPeak)} />
        <${Kpi} label=${`Visits (${rangeLabel})`} value=${totalVisits} />
        <${Kpi} label="Unique visitors (latest per region)" value=${maxUniques} />
      </div>
      <div class="charts">
        <${Chart} title="CPU %" points=${cpu} color="var(--cpu)" formatY=${(v) => v.toFixed(1)}
          subtitle=${rangeLabel} rangeHours=${hours} emptyHint=${emptyHint} />
        <${Chart} title="Memory" points=${mem} color="var(--mem)" formatY=${fmtBytes}
          subtitle=${rangeLabel} rangeHours=${hours} emptyHint=${emptyHint} />
      </div>
      <${Chart} title="Visits observed" points=${visitSeries} color="var(--visits)" formatY=${(v) => Math.round(v)}
        subtitle=${rangeLabel} rangeHours=${hours} emptyHint=${emptyHint} />
      <div>
        <div class="section-band"><h2>Recent activity</h2><span class="muted small">latest first</span></div>
        <${RecentActivity} entries=${activity} appId=${appId} />
      </div>
      <div>
        <h2>Visitors by region <span class="muted small">· ${rangeLabel}, unique is latest per region (never summed)</span></h2>
        ${regions.length === 0
          ? html`<p class="muted">No visits recorded in the ${rangeLabel}.</p>`
          : html`<table class="stacked"><thead><tr><th>Region</th><th>Visits</th><th>Unique (latest)</th></tr></thead>
              <tbody>${regions.map(([region, r]) => html`
                <tr><td data-label="Region" class="mono">${region}</td><td data-label="Visits">${r.visits}</td><td data-label="Unique (latest)">${r.uniques}</td></tr>`)}
              </tbody></table>`}
      </div>
    </div>`;
}

function Deployments({ deployments, onRollbackTo, rollbackNote }) {
  if (!deployments) return html`<${Skeleton} lines={3} />`;
  if (deployments.length === 0) return html`<p class="muted">No deployments yet.</p>`;
  const pending = hasPendingDeployment(deployments);
  return html`
    <div class="table-wrap">
    ${rollbackNote && html`<p class="muted small">${rollbackNote}</p>`}
    ${pending && html`<p class="muted small" role="status">A deployment is still in progress — this list refreshes automatically.</p>`}
    <table class="stacked">
      <thead><tr><th>Status</th><th>Version</th><th>Started</th><th>Finished</th><th></th></tr></thead>
      <tbody>
        ${deployments.map((d) => html`
          <tr>
            <td data-label="Status"><${StatusBadge} status=${d.status} /></td>
            <td data-label="Version">
              <span class="mono">${shortHash(d.artifact_hash)}</span>
              ${d.artifact_hash && html`<button class="btn small ghost" onClick=${async () => {
                try {
                  await navigator.clipboard.writeText(d.artifact_hash);
                  toast.success("Version id copied");
                } catch {
                  toast.error("Copy failed — select the id manually");
                }
              }}>Copy</button>`}
              ${d.artifact_hash && html`<details><summary class="muted small">full id</summary>
                <div class="mono small break">${d.artifact_hash}</div></details>`}
            </td>
            <td data-label="Started" class="muted">${fmtTime(d.started_at)}</td>
            <td data-label="Finished" class="muted">${fmtTime(d.finished_at)}</td>
            <td class="controls no-label">
              ${d.artifact_hash && onRollbackTo && html`<button class="btn small" onClick=${() => onRollbackTo(d.artifact_hash)}>
                Roll back to this
              </button>`}
              ${d.log && html`<details><summary class="muted">log</summary><pre class="log">${d.log}</pre></details>`}
            </td>
          </tr>`)}
      </tbody>
    </table>
    </div>`;
}

function Environment({ env, appId, reload }) {
  const [reveal, setReveal] = useState(false);
  const [busy, setBusy] = useState(false);
  const save = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const key = fd.get("key");
    const value = fd.get("value");
    setBusy(true);
    try {
      await oapi(`/apps/${appId}/env/${encodeURIComponent(key)}`, {
        method: "PUT",
        body: JSON.stringify({ value }),
      });
      toast.success(`Saved ${key}`);
      e.target.reset();
      reload();
    } catch (err) {
      toast.error(err.message);
    } finally {
      setBusy(false);
    }
  };
  const remove = async (key) => {
    if (!(await confirmAction({ title: `Remove ${key}?`, danger: true, confirmLabel: "Remove" }))) return;
    setBusy(true);
    try {
      await oapi(`/apps/${appId}/env/${encodeURIComponent(key)}`, { method: "DELETE" });
      toast.success(`Removed ${key}`);
      reload();
    } catch (err) {
      toast.error(err.message);
    } finally {
      setBusy(false);
    }
  };
  return html`
    <div>
      ${!env || env.length === 0
        ? html`<p class="muted">No variables.</p>`
        : html`<table class="stacked"><thead><tr><th>Key</th><th>Created</th><th></th></tr></thead>
            <tbody>${env.map((e) => html`
              <tr><td data-label="Key" class="mono">${e.key}</td><td data-label="Created" class="muted">${fmtTime(e.created_at)}</td>
              <td class="no-label"><button class="btn small ghost" disabled=${busy} onClick=${() => remove(e.key)}>Remove</button></td></tr>`)}
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
        <div><button class="btn" type="submit" disabled=${busy}>${busy ? "Saving…" : "Add"}</button> <span class="muted">redeploy to apply</span></div>
      </form>
    </div>`;
}

function Domains({ appId }) {
  const [domains, setDomains] = useState(null);
  const [busy, setBusy] = useState(false);
  const [domainError, setDomainError] = useState(null);
  const domainInput = useRef(null);
  const load = useCallback(async () => {
    try {
      const r = await oapi(`/apps/${appId}/domains`);
      setDomains(r.domains || []);
    } catch { setDomains([]); }
  }, [appId]);
  useEffect(() => { load(); }, [load]);

  const add = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    setDomainError(null);
    setBusy(true);
    try {
      await oapi(`/apps/${appId}/domains`, {
        method: "POST",
        body: JSON.stringify({ domain: fd.get("domain") }),
      });
      toast.success("Domain added");
      e.target.reset();
      load();
    } catch (err) {
      if (err.field === "domain") {
        setDomainError(err.message);
        domainInput.current?.focus();
      }
      else toast.error(err.message);
    } finally { setBusy(false); }
  };
  const remove = async (d) => {
    if (!(await confirmAction({ title: `Remove ${d}?`, danger: true, confirmLabel: "Remove" }))) return;
    setBusy(true);
    try {
      await oapi(`/apps/${appId}/domains/${encodeURIComponent(d)}`, { method: "DELETE" });
      toast.success("Removed");
      load();
    } catch (err) { toast.error(err.message); } finally { setBusy(false); }
  };

  return html`
    <div>
      <h2>Domain aliases</h2>
      <p class="muted small">The primary domain lives in Settings; aliases route here in addition.</p>
      ${!domains || domains.length === 0
        ? html`<p class="muted">No aliases.</p>`
        : html`<table class="stacked"><tbody>${domains.map((d) => html`
            <tr><td data-label="Domain" class="mono">${d.domain}</td>
            <td class="no-label"><button class="btn small ghost" disabled=${busy} onClick=${() => remove(d.domain)}>Remove</button></td></tr>`)}
          </tbody></table>`}
      <form class="form" style="margin-top:10px" onSubmit=${add}>
        <div class="row">
          <label>Alias domain <input ref=${domainInput} name="domain" placeholder="www.example.com" required
            aria-invalid=${domainError ? "true" : null}
            aria-describedby=${domainError ? "alias-domain-error" : null}
            onInput=${() => setDomainError(null)} />
            ${domainError && html`<span class="form-error" id="alias-domain-error" role="alert">${domainError}</span>`}
          </label>
        </div>
        <div><button class="btn" type="submit" disabled=${busy}>${busy ? "Saving…" : "Add alias"}</button></div>
      </form>
    </div>`;
}

function EditForm({ app, servers, onSaved }) {
  const [busy, setBusy] = useState(false);
  const [fieldErrors, setFieldErrors] = useState({});
  const [formError, setFormError] = useState(null);
  const [focusField, setFocusField] = useState(null);
  const [launchMode, setLaunchMode] = useState(app.command ? "command" : "binary");
  const formRef = useRef(null);
  const initialArgv = parseCommandArgv(app.command);

  useEffect(() => {
    if (!focusField || !formRef.current) return;
    formRef.current.elements.namedItem(focusField)?.focus();
    setFocusField(null);
  }, [focusField]);

  const submit = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const mem = String(fd.get("mem_limit_mb") || "").trim();
    const cpu = String(fd.get("cpu_quota_pct") || "").trim();
    const payload = {
      port: Number(fd.get("port")),
      domain: fd.get("domain"),
      runtime: fd.get("runtime"),
      server_id: fd.get("server_id"),
      health_path: fd.get("health_path"),
      metrics_path: fd.get("metrics_path"),
      auto_restart: fd.get("auto_restart") === "on",
      mem_limit_mb: mem === "" ? null : Number(mem),
      cpu_quota_pct: cpu === "" ? null : Number(cpu),
    };
    const errors = resourceLimitErrors(payload.runtime, mem, cpu);
    if (app.kind !== "worker") {
      const port = Number(payload.port);
      if (!Number.isInteger(port) || port < 1 || port > 65535) {
        errors.port = "Enter a whole-number port from 1 to 65535.";
      }
    }
    if (!payload.server_id) errors.server_id = "Choose a server.";
    // Advanced launch: tri-state patches — unchanged keys are omitted so the
    // server keeps them, emptied values clear, new values replace.
    if (launchMode === "command" && app.kind !== "static") {
      const { lines, error } = validateCommandLines(fd.get("command"));
      if (error) {
        errors.command = error;
      } else {
        const patch = argvPatch(initialArgv, lines);
        if (patch.present) payload.command = patch.value;
      }
    } else if (app.command) {
      // Back to the stored binary: clearing argv re-points at it.
      payload.command = null;
    }
    if (fd.get("workdir") !== null && fd.get("workdir") !== undefined) {
      const patch = textPatch(app.workdir, String(fd.get("workdir")).trim());
      if (patch.present) payload.workdir = patch.value;
    }
    if (app.kind === "static" && fd.get("publish_dir") !== null && fd.get("publish_dir") !== undefined) {
      const patch = textPatch(app.publish_dir, String(fd.get("publish_dir")).trim());
      if (patch.present) payload.publish_dir = patch.value;
    }
    if (Object.keys(errors).length > 0) {
      const first = Object.keys(errors)[0];
      setFieldErrors(errors);
      setFormError("Correct the highlighted fields before saving.");
      setFocusField(first);
      return;
    }
    setFieldErrors({});
    setFormError(null);
    setBusy(true);
    try {
      await oapi(`/apps/${app.id}`, { method: "PATCH", body: JSON.stringify(payload) });
      toast.success("Saved");
      onSaved();
    } catch (err) {
      if (err.field) {
        setFieldErrors({ [err.field]: err.message });
        setFormError("Correct the highlighted field and try again.");
        setFocusField(err.field);
      } else {
        toast.error(err.message);
        setFormError(err.message);
      }
    } finally { setBusy(false); }
  };
  return html`
    <form ref=${formRef} class="form" noValidate onSubmit=${submit}>
      <h2>Settings</h2>
      <div class="row">
        ${app.kind !== "worker" ? html`
          <label>Port <input name="port" type="number" min="1" max="65535" value=${app.port} required
            aria-invalid=${fieldErrors.port ? "true" : null}
            aria-describedby=${fieldErrors.port ? "field-error-port" : null} />
            <${FieldError} errors=${fieldErrors} name="port" />
          </label>` : null}
        <label>Server
          <select name="server_id" aria-invalid=${fieldErrors.server_id ? "true" : null}
            aria-describedby=${fieldErrors.server_id ? "field-error-server_id" : null}>
            ${servers.map((s) => html`<option value=${s.id} selected=${s.id === app.server_id}>${s.name}</option>`)}
          </select>
          <${FieldError} errors=${fieldErrors} name="server_id" />
        </label>
      </div>
      ${app.kind !== "worker" ? html`
        <label>Primary domain <input name="domain" value=${app.domain || ""}
          aria-invalid=${fieldErrors.domain ? "true" : null}
          aria-describedby=${fieldErrors.domain ? "field-error-domain" : null} />
          <${FieldError} errors=${fieldErrors} name="domain" />
        </label>` : null}
      <div class="row">
        <label>Managed by
          <select name="runtime" aria-invalid=${fieldErrors.runtime ? "true" : null}
            aria-describedby=${fieldErrors.runtime ? "field-error-runtime" : null}>
            <option value="systemd" selected=${app.runtime === "systemd"}>systemd</option>
            <option value="proc" selected=${app.runtime === "proc"}>turaes (proc)</option>
          </select>
          <${FieldError} errors=${fieldErrors} name="runtime" />
        </label>
        ${app.kind !== "worker" ? html`
          <label>Health path <input name="health_path" value=${app.health_path || "/health"}
            aria-invalid=${fieldErrors.health_path ? "true" : null}
            aria-describedby=${fieldErrors.health_path ? "field-error-health_path" : null} />
            <${FieldError} errors=${fieldErrors} name="health_path" />
          </label>` : null}
      </div>
      <div class="row">
        ${app.kind !== "worker" ? html`
          <label>Metrics path <input name="metrics_path" value=${app.metrics_path || ""}
            aria-invalid=${fieldErrors.metrics_path ? "true" : null}
            aria-describedby=${fieldErrors.metrics_path ? "field-error-metrics_path" : null} />
            <${FieldError} errors=${fieldErrors} name="metrics_path" />
          </label>` : null}
        <label class="inline"><input type="checkbox" name="auto_restart" checked=${app.auto_restart} /> Restart when unhealthy</label>
      </div>
      <div class="row">
        <label>Memory limit (MiB) <input name="mem_limit_mb" type="number" min="16" max="65536" value=${app.mem_limit_mb ?? ""} placeholder="unlimited"
          aria-invalid=${fieldErrors.mem_limit_mb ? "true" : null}
          aria-describedby=${fieldErrors.mem_limit_mb ? "field-error-mem_limit_mb" : null} />
          <${FieldError} errors=${fieldErrors} name="mem_limit_mb" />
        </label>
        <label>CPU limit (% of one core) <input name="cpu_quota_pct" type="number" min="1" max="6400" value=${app.cpu_quota_pct ?? ""} placeholder="unlimited"
          aria-invalid=${fieldErrors.cpu_quota_pct ? "true" : null}
          aria-describedby=${fieldErrors.cpu_quota_pct ? "field-error-cpu_quota_pct" : null} />
          <${FieldError} errors=${fieldErrors} name="cpu_quota_pct" />
        </label>
      </div>
      <div class="section-band"><h2>Advanced launch</h2><span class="muted small">argv, working directory, source</span></div>
      ${app.kind !== "static" ? html`
        <div class="seg" role="radiogroup" aria-label="Launch mode">
          ${[["binary", "Stored binary"], ["command", "Explicit command"]].map(([m, label]) => html`
            <button type="button" role="radio" aria-checked=${launchMode === m}
              class=${launchMode === m ? "active" : ""}
              onClick=${() => setLaunchMode(m)}>${label}</button>`)}
        </div>` : null}
      ${launchMode === "command" && app.kind !== "static" ? html`
        <label>Command (one argument per line)
          <textarea name="command" rows="3" class="mono"
            defaultValue=${(initialArgv || []).join("\n")}
            aria-invalid=${fieldErrors.command ? "true" : null}
            aria-describedby=${fieldErrors.command ? "field-error-command" : null}></textarea>
          <span class="muted small">The first line is the executable — no shell. ${app.command ? "To go back to the stored binary, choose Stored binary above." : "Setting a command supersedes the stored binary and flat arguments."}</span>
          <${FieldError} errors=${fieldErrors} name="command" />
        </label>` : html`
        <p class="muted small">Runs <span class="mono">${app.command ? "the configured argv" : app.binary_path}</span> — no shell is involved.</p>`}
      <label>Working directory <input name="workdir" defaultValue=${app.workdir ?? ""} placeholder="app state directory" />
        <span class="muted small">Where the process runs. Empty the field to revert to the default.</span>
      </label>
      ${app.kind === "static" ? html`
        <label>Source directory <input name="publish_dir" defaultValue=${app.publish_dir ?? ""}
          aria-invalid=${fieldErrors.publish_dir ? "true" : null}
          aria-describedby=${fieldErrors.publish_dir ? "field-error-publish_dir" : null} />
          <span class="muted small">The files turaes serves. Takes effect on the next deploy.</span>
          <${FieldError} errors=${fieldErrors} name="publish_dir" />
        </label>` : null}
      <p class="muted small">Limits need the systemd runtime and take effect on the next deploy or restart. Launch changes take effect on the next deploy.</p>
      ${formError ? html`<p class="form-error" role="alert">${formError}</p>` : null}
      <div><button class="btn" type="submit" disabled=${busy}>${busy ? "Saving…" : "Save settings"}</button></div>
    </form>`;
}

function Settings({ app, servers, user, onSaved }) {
  const del = async () => {
    if (!(await confirmAction({
      title: `Delete ${app.name}?`,
      body: "This removes the app and stops it. This cannot be undone.",
      danger: true,
      confirmLabel: "Delete",
    }))) return;
    try {
      await oapi(`/apps/${app.id}`, { method: "DELETE" });
      toast.success(`Deleted ${app.name}`);
      navigate("#/apps");
    } catch (err) {
      toast.error(err.message);
    }
  };
  return html`
    ${user ? html`<${EditForm} app=${app} servers=${servers} onSaved=${onSaved} />` : null}
    <div class="table-wrap"><table>
      <tbody>
        <tr><th>Name</th><td class="mono">${app.name}</td></tr>
        <tr><th>Binary</th><td class="mono break">${app.binary_path}</td></tr>
      </tbody>
    </table></div>
    ${user && app.kind !== "worker" ? html`<${Domains} appId=${app.id} />` : null}
    ${user && html`<div class="danger-zone">
      <h3>Danger zone</h3>
      <button class="btn danger" onClick=${del}>Delete application</button>
    </div>`}`;
}

export function AppDetailView({ id, tab, range: routeRange, user, servers }) {
  const [app, setApp] = useState(null);
  const [notFound, setNotFound] = useState(false);
  const [error, setError] = useState(null);
  const [data, setData] = useState({});
  const [metricsAt, setMetricsAt] = useState(null);
  const [metricsLoading, setMetricsLoading] = useState(false);
  const [deployments, setDeployments] = useState(null);
  const [activity, setActivity] = useState(null);
  const [env, setEnv] = useState(null);
  const [busy, setBusy] = useState(false);
  const [latest, setLatest] = useState(null);

  // Single range drives CPU, memory and visitors together. URL wins, then
  // the stored preference, then the default — all normalized.
  const [range, setRange] = useState(() => {
    try {
      const stored = localStorage.getItem("turaes-range");
      return normalizeRange(routeRange || stored || DEFAULT_RANGE);
    } catch {
      return normalizeRange(routeRange || DEFAULT_RANGE);
    }
  });

  // Follow back/forward navigation that changes `?range=`.
  useEffect(() => {
    if (routeRange && normalizeRange(routeRange) !== range) {
      setRange(normalizeRange(routeRange));
    }
  }, [routeRange]);

  const loadApp = useCallback(async () => {
    try {
      const r = await oapi(`/apps/${id}`);
      setApp(r.application);
      setError(null);
    } catch (e) {
      if (e.status === 404) setNotFound(true);
      else { setError(e.message); toast.error(e.message); }
    }
  }, [id]);

  const retry = () => { setError(null); loadApp(); };

  const loadMetrics = useCallback(async (hours) => {
    setMetricsLoading(true);
    try {
      const h = hours ?? rangeToHours(range);
      const [m, v] = await Promise.all([
        oapi(`/apps/${id}/stats?hours=${h}`),
        oapi(`/apps/${id}/visitors?hours=${h}`),
      ]);
      if (document.hidden) return;
      setData({ metrics: m.metrics || [], visitors: v.visitors || [] });
      setMetricsAt(Date.now());
    } catch { /* ignore */ } finally {
      setMetricsLoading(false);
    }
  }, [id, range]);

  const loadDeployments = useCallback(async () => {
    try {
      const r = await oapi(`/apps/${id}/deployments?limit=15`);
      setDeployments(r.deployments || []);
    } catch { setDeployments([]); }
  }, [id]);

  const loadEnv = useCallback(async () => {
    try {
      const r = await oapi(`/apps/${id}/env`);
      setEnv(r.env || []);
    } catch { setEnv([]); }
  }, [id]);

  const loadActivity = useCallback(async () => {
    try {
      const r = await oapi(`/audit?app=${encodeURIComponent(id)}&limit=50`);
      setActivity(r.audit || []);
    } catch { setActivity([]); }
  }, [id]);

  useEffect(() => {
    loadApp();
    const t = setInterval(() => { if (!document.hidden) loadApp(); }, 15000);
    return () => clearInterval(t);
  }, [loadApp]);

  const changeRange = useCallback((next) => {
    const r = normalizeRange(next);
    setRange(r);
    try { localStorage.setItem("turaes-range", r); } catch {}
    // Deep-linkable + back-button safe; the route effect picks it up.
    navigate(`#/apps/${id}/overview${r === DEFAULT_RANGE ? "" : `?range=${r}`}`);
    loadMetrics(rangeToHours(r));
  }, [id, loadMetrics]);

  const refreshMetrics = useCallback(() => { loadMetrics(); }, [loadMetrics]);

  const loadLatest = useCallback(async () => {
    try {
      const r = await oapi(`/apps/${id}/deployments?limit=1`);
      setLatest((r.deployments || [])[0] || null);
    } catch { setLatest(null); }
  }, [id]);

  useEffect(() => {
    if (tab === "overview") { loadMetrics(); loadActivity(); }
    if (tab === "deployments") loadDeployments();
    if (tab === "activity") loadActivity();
    if (tab === "environment") loadEnv();
  }, [tab, id, range, loadMetrics, loadDeployments, loadActivity, loadEnv]);

  useEffect(() => { loadLatest(); }, [loadLatest]);

  // While a deployment is still queued or installing (typically a remote
  // node reconciling), poll the list so the tab settles into its terminal
  // state without a manual refresh.
  const deploymentPending = hasPendingDeployment(deployments);
  useEffect(() => {
    if (tab !== "deployments" || !deploymentPending) return undefined;
    const t = setInterval(() => {
      if (!document.hidden) {
        loadDeployments();
        loadLatest();
      }
    }, 5000);
    return () => clearInterval(t);
  }, [tab, deploymentPending, loadDeployments, loadLatest]);

  const deploy = async () => {
    if (!(await confirmAction({
      title: `Deploy ${app.name}?`,
      body: "Applies the current app version using its workload-specific deployment process.",
      consequences: deployConsequences(app),
      confirmLabel: "Deploy",
    }))) return;
    setBusy(true);
    try {
      const result = await oapi(`/apps/${id}/deploy`, { method: "POST" });
      if (result.state === "unknown") {
        toast.info(`Deploy queued for ${serverName(servers, app.server_id)} — watch it land on the Deployments tab.`);
        navigate(`#/apps/${id}/deployments`);
      } else toast.success(`Deployed ${app.name}`);
      loadApp();
      loadDeployments();
      loadLatest();
    } catch (e) {
      toast.error(e.message);
      loadApp();
      loadDeployments();
      loadLatest();
    } finally {
      setBusy(false);
    }
  };

  const rollback = async () => {
    if (!(await confirmAction({
      title: `Roll back ${app.name}?`,
      body: "Reverts this app to a previous deployed version.",
      consequences: rollbackConsequences(app),
      confirmLabel: "Roll back",
    }))) return;
    setBusy(true);
    try {
      await oapi(`/apps/${id}/rollback`, { method: "POST" });
      toast.success("Rolled back");
      loadApp();
      loadDeployments();
      loadLatest();
    } catch (e) {
      toast.error(e.message);
      loadApp();
      loadDeployments();
      loadLatest();
    } finally {
      setBusy(false);
    }
  };

  const rollbackTo = async (hash) => {
    if (!(await confirmAction({
      title: `Roll back ${app.name}?`,
      body: `Reverts this app to build ${shortHash(hash)}.`,
      consequences: rollbackConsequences(app, shortHash(hash)),
      confirmLabel: "Roll back",
    }))) return;
    setBusy(true);
    try {
      await oapi(`/apps/${id}/rollback`, {
        method: "POST",
        body: JSON.stringify({ artifact_hash: hash }),
      });
      toast.success("Rolled back");
      loadApp();
      loadDeployments();
      loadLatest();
    } catch (e) {
      toast.error(e.message);
      loadApp();
      loadDeployments();
      loadLatest();
    } finally {
      setBusy(false);
    }
  };

  const action = async (a) => {
    const consequences = a === "stop" ? stopConsequences(app)
      : a === "restart" ? restartConsequences(app)
      : startConsequences(app);
    if (!(await confirmAction({
      title: `${a[0].toUpperCase()}${a.slice(1)} ${app.name}?`,
      body: a === "stop"
        ? "Traffic to this app will drop until it starts again."
        : a === "restart"
        ? "The active process restarts; expect a brief interruption."
        : "Brings the app back online.",
      consequences,
      confirmLabel: a[0].toUpperCase() + a.slice(1),
      danger: a === "stop",
    }))) return;
    setBusy(true);
    try {
      await oapi(`/apps/${id}/${a}`, { method: "POST" });
      toast.success(`${a[0].toUpperCase()}${a.slice(1)} ${app.name}`);
      loadApp();
    } catch (e) {
      toast.error(e.message);
    } finally {
      setBusy(false);
    }
  };

  if (notFound) {
    return html`<section class="panel"><p class="muted">Application not found.</p>
      <a href="#/apps">← Back to applications</a></section>`;
  }
  if (error) {
    return html`<section class="panel"><p class="muted">Could not load application: ${error}</p>
      <div class="controls"><button class="btn" onClick=${retry}>Retry</button>
      <a href="#/apps">← Back to applications</a></div></section>`;
  }
  if (!APP_TABS.includes(tab)) {
    return html`<section class="panel"><p class="muted">Unknown tab “${tab}”.</p>
      <div><a class="btn ghost" href=${`#/apps/${id}/overview`}>Go to overview</a></div></section>`;
  }
  if (!app) return html`<section class="panel"><${Skeleton} lines={4} height=${28} /></section>`;

  return html`
    <section class="panel">
      <div class="panel-head">
        <div>
          <a href="#/apps" class="muted">← Applications</a>
          <h1 class="identity"><span class="mono">${app.name}</span> <${StatusBadge} status=${app.status} /></h1>
          <div class="status-strip" aria-label="Application status">
            <div class="stat"><span>Server</span><span class="mono">${serverName(servers, app.server_id)}</span></div>
            ${app.kind === "worker"
              ? html`<div class="stat"><span>Workload</span><span>Background worker</span></div>`
              : html`<div class="stat"><span>Route</span><span class="mono">${app.domain || "—"}</span></div>
                <div class="stat"><span>Port</span><span class="mono">:${app.port}</span></div>`}
            <div class="stat"><span>Managed by</span><span>${runtimeLabel(app.runtime)}</span></div>
            <div class="stat"><span>Last deploy</span><span>${latest
              ? html`<span class="mono" title=${latest.artifact_hash || ""}>${shortHash(latest.artifact_hash)} · ${timeAgo(Date.now() - parseTs(latest.started_at))}</span>`
              : html`<span class="muted">never</span>`}</span></div>
          </div>
        </div>
        <div class="controls">
          ${user && html`
            <button class="btn" disabled=${busy} onClick=${deploy}>${busy ? "Deploying…" : "Deploy"}</button>
            <button class="btn ghost" disabled=${busy} onClick=${() => action("restart")}>Restart</button>
            <button class="btn ghost" disabled=${busy} onClick=${() => action("stop")}>Stop</button>
            <details class="menu">
              <summary class="btn ghost" aria-label="More actions">More</summary>
              <div class="menu-list">
                ${!app.command && html`<button class="btn small ghost" disabled=${busy} onClick=${rollback}>Rollback</button>`}
                <button class="btn small ghost" disabled=${busy} onClick=${() => action("start")}>Start</button>
              </div>
            </details>`}
        </div>
      </div>
      <p class="action-note muted small">${lifecycleSummary(app)}</p>
      <nav class="tabs" role="tablist" aria-label="Application sections">
        ${TAB_GROUPS.map((g) => html`<span class="tab-group"><span class="tab-label" aria-hidden="true">${g.label}</span>${
          g.tabs.map((t) => {
            const href = t === "overview" && range !== DEFAULT_RANGE
              ? `#/apps/${id}/${t}?range=${range}`
              : `#/apps/${id}/${t}`;
            return html`
              <a role="tab" aria-selected=${t === tab} class=${t === tab ? "active" : ""}
                href=${href}>${TAB_LABEL[t]}</a>`;
          })
        }</span>`)}
      </nav>
      <div class="tab-body" role="tabpanel">
        ${tab === "overview" && html`<${Overview} data=${data} range=${range} onRange=${changeRange}
          updatedAt=${metricsAt} loading=${metricsLoading} onRefresh=${refreshMetrics}
          activity=${activity} appId=${id} />`}
        ${tab === "deployments" && html`<${Deployments} deployments=${deployments}
          onRollbackTo=${app.command || app.kind === "static" ? null : rollbackTo}
          rollbackNote=${app.command
            ? "Command apps do not retain prior argv configurations. Restore the desired command in turaes.yaml, run turaes apply, then deploy."
            : app.kind === "static"
            ? "Static rollback switches to the previous retained slot; selecting an arbitrary older deployment is not supported."
            : null} />`}
        ${tab === "activity" && html`<${Activity} entries=${activity} />`}
        ${tab === "environment" && html`<${Environment} env=${env} appId=${id} reload=${loadEnv} />`}
        ${tab === "logs" && html`<${LogViewer} appId=${id} />`}
        ${tab === "settings" && html`<${Settings} app=${app} servers=${servers} user=${user} onSaved=${loadApp} />`}
      </div>
    </section>`;
}
