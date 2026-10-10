import { html } from "../lib/html.js";
import { useEffect, useState, useCallback, useRef } from "preact/hooks";
import { navigate } from "../lib/router.js";
import { oapi } from "../lib/api.js";
import { toast } from "../lib/toast.js";
import { serverName, fmtTime } from "../lib/format.js";
import { kindInfo } from "../lib/appForm.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { Skeleton } from "../components/Skeleton.js";

function matchesQuery(q, ...fields) {
  const needle = q.trim().toLowerCase();
  if (!needle) return true;
  return fields.some((f) => String(f || "").toLowerCase().includes(needle));
}

export function CatalogView({ user, servers }) {
  const [catalog, setCatalog] = useState(null);
  const [apps, setApps] = useState(null);
  const [error, setError] = useState(null);
  const [q, setQ] = useState("");
  const [section, setSection] = useState("all");

  // Sequence guard: overlapping loads (retry while fetching) resolve in
  // order — a stale response never paints over fresher state.
  const loadSeq = useRef(0);
  const load = useCallback(async () => {
    const seq = ++loadSeq.current;
    try {
      const [c, a] = await Promise.all([oapi("/catalog"), oapi("/apps")]);
      if (document.hidden) return;
      if (seq !== loadSeq.current) return;
      setCatalog(c);
      setApps(a.applications || []);
      setError(null);
    } catch (e) {
      if (seq !== loadSeq.current) return;
      // 401 is handled centrally (Shell re-checks the session): keep prior
      // data instead of faking an empty catalog.
      if (e.status === 401) return;
      else { setError(e.message); toast.error(e.message); }
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  const useTemplate = (slug, name) => {
    navigate(`#/apps?template=${encodeURIComponent(slug)}`);
    toast.info(`Starting “${name}” from a template — review and create it below.`);
  };

  const retry = () => { setError(null); load(); };

  const templates = (catalog && catalog.templates) || [];
  const shownApps = (apps || []).filter((a) => matchesQuery(q, a.name, a.domain));
  const shownTemplates = templates.filter((t) => matchesQuery(q, t.name, t.description, t.kind));
  const showApps = section !== "templates";
  const showTemplates = section !== "applications";

  return html`
    <section class="panel">
      <div class="panel-head">
        <div>
          <h1>Catalog</h1>
          <p class="action-note muted small">Your applications plus one-click starters. Templates open the create wizard prefilled — nothing is created until you confirm.</p>
        </div>
        <div class="controls">
          <div class="seg" role="radiogroup" aria-label="Catalog sections">
            ${[["all", "All"], ["applications", "Applications"], ["templates", "Templates"]].map(([v, label]) => html`
              <button type="button" role="radio" aria-checked=${section === v}
                class=${section === v ? "active" : ""} onClick=${() => setSection(v)}>${label}</button>`)}
          </div>
          <input class="search" placeholder="Search…" aria-label="Search catalog" value=${q}
            onInput=${(e) => setQ(e.target.value)} />
        </div>
      </div>
      ${error
        ? html`<p class="muted">Could not load the catalog: ${error}</p>
          <div><button class="btn" onClick=${retry}>Retry</button></div>`
        : catalog === null || apps === null
        ? html`<${Skeleton} lines={4} height=${64} />`
        : html`
          ${showApps && html`
            <div class="section-band"><h2>Your applications</h2><span class="muted small">${shownApps.length} app${shownApps.length === 1 ? "" : "s"}</span></div>
            ${shownApps.length === 0
              ? html`<p class="muted">${q ? "No applications match this search." : "No applications yet — start one from a template below."}</p>`
              : html`<div class="table-wrap"><table class="stacked">
                  <thead><tr><th>Status</th><th>Name</th><th>Server</th><th>Updated</th><th></th></tr></thead>
                  <tbody>
                    ${shownApps.map((a) => html`
                      <tr>
                        <td data-label="Status"><${StatusBadge} status=${a.status} /></td>
                        <td data-label="Name"><a class="mono" href=${`#/apps/${a.id}/overview`}>${a.name}</a></td>
                        <td data-label="Server">${serverName(servers, a.server_id)}</td>
                        <td data-label="Updated" class="muted">${fmtTime(a.updated_at)}</td>
                        <td class="controls no-label"><a class="btn small ghost" href=${`#/apps/${a.id}/overview`}>Open</a></td>
                      </tr>`)}
                  </tbody>
                </table></div>`}`}
          ${showTemplates && html`
            <div class="section-band"><h2>Templates</h2><span class="muted small">one-click starters</span></div>
            ${shownTemplates.length === 0
              ? html`<p class="muted">No templates match this search.</p>`
              : html`<div class="grid">
                  ${shownTemplates.map((t) => html`
                    <div class="card">
                      <h3><span>${t.name}</span></h3>
                      <div class="stat"><span>Type</span><span>${(kindInfo(t.kind) || {}).label || t.kind}</span></div>
                      <div class="stat"><span>About</span><span>${t.description || "—"}</span></div>
                      <div class="controls" style="margin-top:8px">
                        ${user
                          ? html`<button class="btn small" onClick=${() => useTemplate(t.slug, t.name)}>Use template</button>`
                          : html`<span class="muted small">Sign in to use this template.</span>`}
                      </div>
                    </div>`)}
                </div>`}`}
          <div class="section-band"><h2>Platform</h2></div>
          <div class="callout" role="note">
            <strong>turaes v${(catalog.platform || {}).version || "—"}</strong>
            <ul><li>Control plane and edge versions are reported by the serving binary; versioned platform releases arrive with the registry.</li></ul>
          </div>`}
    </section>`;
}
