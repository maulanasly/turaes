import { html } from "../lib/html.js";
import { useEffect, useState, useCallback, useRef } from "preact/hooks";
import { navigate } from "../lib/router.js";
import { oapi } from "../lib/api.js";
import { toast } from "../lib/toast.js";
import { serverName, runtimeLabel, fmtTime } from "../lib/format.js";
import { sortApps, filterApps } from "../lib/sort.js";
import {
  KINDS, STEPS, emptyDraft, applyTemplate, kindInfo, stepErrors, buildPayload, reviewGroups, createConsequences,
  mapIssuesToFields, validateCommandLines,
} from "../lib/appForm.js";
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
            <td data-label="Route" class="mono small break">${a.kind === "worker" ? "background worker (no route)" : a.domain || "n/a"}</td>
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
      ${app.kind === "worker"
        ? html`<div class="stat"><span>Workload</span><span>Background worker</span></div>`
        : html`<div class="stat"><span>Domain</span><span class="mono">${app.domain || "n/a"}</span></div>`}
      <div class="stat"><span>Server</span><span class="mono">${serverName(servers, app.server_id)}</span></div>
      ${app.kind !== "worker" && html`<div class="stat"><span>Port</span><span class="mono">${app.port}</span></div>`}
      <div class="stat"><span>Managed by</span><span>${runtimeLabel(app.runtime)}</span></div>
    </a>`;
}

// Static constraints/consequences explainer reused across steps.
function Callout({ title, notes }) {
  return html`
    <div class="callout" role="note">
      ${title ? html`<strong>${title}</strong>` : null}
      <ul>${notes.map((n) => html`<li>${n}</li>`)}</ul>
    </div>`;
}

function FieldError({ errors, name }) {
  return errors[name]
    ? html`<span class="form-error" id=${`field-error-${name}`}>${errors[name]}</span>`
    : null;
}

function NewAppForm({ servers, onCreated, initial }) {
  const [draft, setDraft] = useState(() => {
    const base = emptyDraft(servers[0] ? servers[0].id : "local");
    if (initial) applyTemplate(base, initial);
    return base;
  });
  const [step, setStep] = useState(0);
  const [busy, setBusy] = useState(false);
  const [fieldErrors, setFieldErrors] = useState({});
  const [formError, setFormError] = useState(null);
  const [focusField, setFocusField] = useState(null);
  const [preflightWarnings, setPreflightWarnings] = useState([]);
  const [preflightChecking, setPreflightChecking] = useState(false);
  const formRef = useRef(null);

  const set = (patch) => {
    setDraft((d) => ({ ...d, ...patch }));
    setFieldErrors((current) => {
      const next = { ...current };
      Object.keys(patch).forEach((field) => delete next[field]);
      return next;
    });
    setFormError(null);
  };
  const kind = kindInfo(draft.kind);
  const lastStep = STEPS.length - 1;

  useEffect(() => {
    if (!focusField || !formRef.current) return;
    formRef.current.elements.namedItem(focusField)?.focus();
    setFocusField(null);
  }, [focusField, step]);

  const next = () => {
    const errors = stepErrors(draft, step);
    const firstField = Object.keys(errors)[0];
    setFieldErrors(errors);
    if (firstField) {
      setFormError("Correct the highlighted fields before continuing.");
      setFocusField(firstField);
      return;
    }
    setFormError(null);
    setStep((s) => Math.min(s + 1, lastStep));
  };
  const back = () => { setFormError(null); setStep((s) => Math.max(s - 1, 0)); };

  // Render every reported problem inline (a submit error carries at most
  // one `field`, while preflight reports carry the full `errors` array).
  const showIssues = (issues, summary) => {
    const { fieldErrors, firstField, step: target } = mapIssuesToFields(issues);
    if (!firstField) {
      toast.error(summary);
      setFormError(summary);
      return;
    }
    setFieldErrors(fieldErrors);
    setFormError(`Correct the highlighted field${Object.keys(fieldErrors).length > 1 ? "s" : ""} and try again.`);
    setStep(target);
    setFocusField(firstField);
  };

  const showApiError = (err) => {
    if (Array.isArray(err.errors) && err.errors.length > 0) {
      showIssues(err.errors);
      return;
    }
    if (err.field) {
      showIssues([{ field: err.field, detail: err.message }]);
      return;
    }
    toast.error(err.message);
    setFormError(err.message);
  };

  // Run the server preflight for the current draft. Returns true when the
  // draft would pass every blocking check.
  const runPreflight = async () => {
    let report;
    try {
      report = await oapi("/apps/preflight", {
        method: "POST",
        body: JSON.stringify(buildPayload(draft)),
      });
    } catch (e) {
      // The preflight itself failed (auth, network): surface it globally and
      // let the user retry instead of blocking on a stale assumption.
      toast.error(e.message);
      setFormError(e.message);
      return false;
    }
    const issues = report.errors || [];
    if (issues.length > 0) {
      showIssues(issues);
      return false;
    }
    setFieldErrors({});
    setFormError(null);
    setPreflightWarnings(report.warnings || []);
    return true;
  };

  const confirm = async () => {
    setBusy(true);
    try {
      // Recheck immediately before writing: ports, quotas and domains may
      // have been claimed since the Review step ran its own check.
      if (!(await runPreflight())) return;
      const r = await oapi("/apps", { method: "POST", body: JSON.stringify(buildPayload(draft)) });
      toast.success(`Created ${r.application.name}`);
      onCreated(r.application);
    } catch (err) {
      showApiError(err);
    } finally {
      setBusy(false);
    }
  };

  // Validate the full draft against the server whenever Review is shown,
  // so blockers surface before Create instead of after a failed submit.
  const preflightSeq = useRef(0);
  useEffect(() => {
    if (step !== lastStep) {
      setPreflightWarnings([]);
      return;
    }
    const seq = ++preflightSeq.current;
    setPreflightChecking(true);
    runPreflight().finally(() => {
      if (preflightSeq.current === seq) setPreflightChecking(false);
    });
    // runPreflight reads the draft captured when Review was entered.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [step]);

  return html`
    <form ref=${formRef} class="wizard" noValidate onSubmit=${(e) => { e.preventDefault(); step < lastStep ? next() : confirm(); }}>
      <ol class="wizard-steps" aria-label="Create app steps">
        ${STEPS.map((label, i) => html`
            <li class=${i === step ? "active" : i < step ? "done" : ""}>
              <button type="button" disabled=${i > step} aria-current=${i === step ? "step" : null}
              onClick=${() => { setFormError(null); setStep(i); }}>
              <span class="wizard-num" aria-hidden="true">${i < step ? "✓" : i + 1}</span>
              <span>${label}</span>
            </button>
          </li>`)}
      </ol>

      ${step === 0 ? html`
        <div class="section-band"><h2>What kind of workload?</h2><span class="muted small">this decides which fields you need</span></div>
        <div class="kind-grid" role="radiogroup" aria-label="Workload type">
          ${KINDS.map((k) => html`
            <button type="button" role="radio" aria-checked=${draft.kind === k.id}
              class=${"kind-card" + (draft.kind === k.id ? " active" : "")}
              onClick=${() => set({ kind: k.id })}>
              <strong>${k.label}</strong>
              <span class="muted small">${k.blurb}</span>
            </button>`)}
        </div>
        <${Callout} title=${`How ${kind.label.toLowerCase()}s run`} notes=${kind.constraints} />` : null}

      ${step === 1 ? html`
        <div class="section-band"><h2>Process</h2><span class="muted small">${kind.label}</span></div>
        <div class="row">
          <label>Name <input name="name" maxlength="64" placeholder="beruang" value=${draft.name}
            aria-invalid=${fieldErrors.name ? "true" : null}
            aria-describedby=${fieldErrors.name ? "field-error-name" : null}
            onInput=${(e) => set({ name: e.target.value })} />
            <span class="muted small">Lowercase slug: becomes the service name on the server.</span>
            <${FieldError} errors=${fieldErrors} name="name" />
          </label>
          <label>Description (optional) <input name="description" placeholder="What this app does" value=${draft.description}
            onInput=${(e) => set({ description: e.target.value })} /></label>
        </div>
        ${draft.kind === "static" ? html`
          <label>Source directory on the server
            <input name="publish_dir" placeholder="/srv/beruang/dist" value=${draft.publish_dir}
              aria-invalid=${fieldErrors.publish_dir ? "true" : null}
              aria-describedby=${fieldErrors.publish_dir ? "field-error-publish_dir" : null}
              onInput=${(e) => set({ publish_dir: e.target.value })} />
            <span class="muted small">The files turaes serves. No build runs. Sync them yourself before deploying.</span>
            <${FieldError} errors=${fieldErrors} name="publish_dir" />
          </label>
          <label>Working directory (optional)
            <input name="workdir" placeholder="/srv/beruang" value=${draft.workdir}
              onInput=${(e) => set({ workdir: e.target.value })} />
            <span class="muted small">Where the file server runs. Defaults to the app state directory.</span>
          </label>` : html`
          <div class="section-band"><h2>Launch</h2><span class="muted small">binary or explicit command</span></div>
          <div class="seg" role="radiogroup" aria-label="Launch mode">
            ${[["binary", "Prebuilt binary"], ["command", "Explicit command"]].map(([m, label]) => html`
              <button type="button" role="radio" aria-checked=${draft.launchMode === m}
                class=${draft.launchMode === m ? "active" : ""}
                onClick=${() => set({ launchMode: m })}>${label}</button>`)}
          </div>
          ${draft.launchMode === "command" ? html`
            <label>Command (one argument per line)
              <textarea name="command" rows="3" class="mono" placeholder=${"/opt/venv/bin/python\nworker.py --queue default"}
                value=${draft.command}
                aria-invalid=${fieldErrors.command ? "true" : null}
                aria-describedby=${fieldErrors.command ? "field-error-command" : null}
                onInput=${(e) => set({ command: e.target.value })}></textarea>
              <span class="muted small">The first line is the executable (no shell), so write each argument on its own line.</span>
              <${FieldError} errors=${fieldErrors} name="command" />
            </label>` : html`
            <label>Binary path on the server
              <input name="binary_path" placeholder="/srv/beruang/target/release/beruang-gateway" value=${draft.binary_path}
                aria-invalid=${fieldErrors.binary_path ? "true" : null}
                aria-describedby=${fieldErrors.binary_path ? "field-error-binary_path" : null}
                onInput=${(e) => set({ binary_path: e.target.value })} />
              <span class="muted small">A prebuilt binary already on the server. turaes never builds or containers it.</span>
              <${FieldError} errors=${fieldErrors} name="binary_path" />
            </label>
            <label>Arguments (optional)
              <input name="args" placeholder="--listen :8000 --config /etc/beruang.toml" value=${draft.args}
                aria-invalid=${fieldErrors.args ? "true" : null}
                aria-describedby=${fieldErrors.args ? "field-error-args" : null}
                onInput=${(e) => set({ args: e.target.value })} />
              <span class="muted small">Passed as separate words, literally (no shell), so no pipes, globs or quoting.</span>
              <${FieldError} errors=${fieldErrors} name="args" />
            </label>`}
          <label>Working directory (optional)
            <input name="workdir" placeholder="/srv/beruang" value=${draft.workdir}
              onInput=${(e) => set({ workdir: e.target.value })} />
            <span class="muted small">Where the process runs. Defaults to the app state directory.</span>
          </label>`}
        ${draft.kind !== "worker" ? html`
          <div class="row">
            <label>Port <input name="port" type="number" min="1" max="65535" placeholder="8000" value=${draft.port}
              aria-invalid=${fieldErrors.port ? "true" : null}
              aria-describedby=${fieldErrors.port ? "field-error-port" : null}
              onInput=${(e) => set({ port: e.target.value })} />
              <span class="muted small">Must be free on the chosen server; turaes checks before deploying.</span>
              <${FieldError} errors=${fieldErrors} name="port" />
            </label>
            <label>Health path <input name="health_path" placeholder="/health" value=${draft.health_path}
              onInput=${(e) => set({ health_path: e.target.value })} />
              <span class="muted small">Polled after start; traffic waits for a healthy response.</span>
            </label>
          </div>` : null}
        <${Callout} title="Native-process constraints" notes=${kind.constraints} />` : null}

      ${step === 2 ? html`
        <div class="section-band"><h2>Placement</h2><span class="muted small">where it runs and how it is reached</span></div>
        <div class="row">
          <label>Server
            <select name="server_id" value=${draft.server_id}
              aria-invalid=${fieldErrors.server_id ? "true" : null}
              aria-describedby=${fieldErrors.server_id ? "field-error-server_id" : null}
              onChange=${(e) => set({ server_id: e.target.value })}>
              ${servers.map((s) => html`<option value=${s.id}>${s.name}</option>`)}
            </select>
            <${FieldError} errors=${fieldErrors} name="server_id" />
          </label>
          ${draft.kind !== "worker" ? html`
            <label>Domain (optional) <input name="domain" placeholder="app.rayakala.ink" value=${draft.domain}
              aria-invalid=${fieldErrors.domain ? "true" : null}
              aria-describedby=${fieldErrors.domain ? "field-error-domain" : null}
              onInput=${(e) => set({ domain: e.target.value })} />
              <span class="muted small">Routes public traffic here. Leave blank to run with no public route.</span>
              <${FieldError} errors=${fieldErrors} name="domain" />
            </label>` : null}
        </div>
        <div class="row">
          <label>Managed by
            <select name="runtime" value=${draft.runtime}
              aria-invalid=${fieldErrors.runtime ? "true" : null}
              aria-describedby=${fieldErrors.runtime ? "field-error-runtime" : null}
              onChange=${(e) => set({ runtime: e.target.value })}>
              <option value="systemd">systemd (default)</option>
              <option value="proc">turaes (proc)</option>
            </select>
            <${FieldError} errors=${fieldErrors} name="runtime" />
          </label>
          <label class="inline" style="align-self:end">
            <input name="auto_restart" type="checkbox" checked=${draft.auto_restart}
              onChange=${(e) => set({ auto_restart: e.target.checked })} /> Restart when unhealthy
          </label>
        </div>
        <div class="section-band"><h2>Resource limits <span class="muted small">optional</span></h2><span class="muted small">systemd only</span></div>
        <div class="row">
          <label>Memory limit (MiB) <input name="mem_limit_mb" type="number" min="16" max="65536" placeholder="512" value=${draft.mem_limit_mb}
            aria-invalid=${fieldErrors.mem_limit_mb ? "true" : null}
            aria-describedby=${fieldErrors.mem_limit_mb ? "field-error-mem_limit_mb" : null}
            onInput=${(e) => set({ mem_limit_mb: e.target.value })} />
            <${FieldError} errors=${fieldErrors} name="mem_limit_mb" />
          </label>
          <label>CPU limit (% of one core) <input name="cpu_quota_pct" type="number" min="1" max="6400" placeholder="100" value=${draft.cpu_quota_pct}
            aria-invalid=${fieldErrors.cpu_quota_pct ? "true" : null}
            aria-describedby=${fieldErrors.cpu_quota_pct ? "field-error-cpu_quota_pct" : null}
            onInput=${(e) => set({ cpu_quota_pct: e.target.value })} />
            <${FieldError} errors=${fieldErrors} name="cpu_quota_pct" />
          </label>
        </div>
        <span class="muted small">Enforced by systemd. The proc runtime cannot confine, so leave these blank there.</span>` : null}

      ${step === 3 ? html`
        <div class="section-band"><h2>Review before you create</h2><span class="muted small">nothing has been created yet</span></div>
        <div class="review">
          ${reviewGroups(draft, serverName(servers, draft.server_id)).map((g) => html`
            <div class="review-group">
              <h3>${g.title}</h3>
              <table><tbody>
                ${g.rows.map(([label, value]) => html`<tr><th>${label}</th><td class="mono break">${value}</td></tr>`)}
              </tbody></table>
            </div>`)}
        </div>
        <${Callout} title="What happens when you create" notes=${createConsequences(draft.kind, draft.domain)} />
        ${preflightChecking
          ? html`<p class="muted small" role="status">Checking the draft against the server…</p>`
          : preflightWarnings.length > 0
          ? html`<div class="callout" role="note"><strong>Heads up</strong><ul>
              ${preflightWarnings.map((w) => html`<li>${w.detail}</li>`)}
            </ul></div>`
          : null}` : null}

      ${formError ? html`<p class="form-error" role="alert">${formError}</p>` : null}
      <div class="controls wizard-nav">
        ${step > 0 ? html`<button type="button" class="btn ghost" disabled=${busy} onClick=${back}>Back</button>` : null}
        ${step < lastStep
          ? html`<button type="submit" class="btn">Next</button>`
          : html`<button type="submit" class="btn" disabled=${busy || preflightChecking}>${busy ? "Creating…" : preflightChecking ? "Checking…" : "Create app"}</button>`}
      </div>
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
  const [templateSlug, setTemplateSlug] = useState(() => {
    try {
      const hash = location.hash || "";
      const qi = hash.indexOf("?");
      if (qi < 0) return "";
      return new URLSearchParams(hash.slice(qi + 1)).get("template") || "";
    } catch { return ""; }
  });
  const [templateDefaults, setTemplateDefaults] = useState(null);
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
      // 401 is handled centrally (Shell re-checks the session): keep prior
      // data instead of faking an empty fleet.
      if (e.status === 401) return;
      else { setError(e.message); toast.error(e.message); }
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  // Deep link from the Catalog (`#/apps?template=<slug>`): prefill the
  // wizard from the template defaults and open it. Consumed once so a
  // refresh does not re-apply it over the user's own edits.
  useEffect(() => {
    if (!templateSlug || !user) return undefined;
    let live = true;
    oapi("/catalog").then((c) => {
      if (!live) return;
      const t = (c.templates || []).find((x) => x.slug === templateSlug);
      if (t) {
        setTemplateDefaults(t.defaults || {});
        setShowAdd(true);
        toast.info(`Prefilled from the “${t.name}” template, give it a name and create.`);
      } else {
        toast.error(`Unknown template “${templateSlug}”.`);
      }
      try { history.replaceState(null, "", "#/apps"); } catch {}
      setTemplateSlug("");
    }).catch((e) => { if (live) toast.error(e.message); });
    return () => { live = false; };
  }, [templateSlug, user]);

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
      ${showAdd && user && html`<div class="form-wrap"><${NewAppForm} servers=${servers} initial=${templateDefaults} onCreated=${created} /></div>`}
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
