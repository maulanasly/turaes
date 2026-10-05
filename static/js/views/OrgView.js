import { html } from "../lib/html.js";
import { useState, useEffect, useCallback } from "preact/hooks";
import { api, oapi, getOrg } from "../lib/api.js";
import { toast } from "../lib/toast.js";
import { confirmAction } from "../lib/confirm.js";
import { fmtTime } from "../lib/format.js";

const ROLES = ["viewer", "developer", "admin", "owner"];

export function OrgView({ user, onOrgChange }) {
  const [me, setMe] = useState(null);
  const [members, setMembers] = useState(null);
  const [quota, setQuota] = useState(null);
  const active = getOrg();
  const mine = (me?.orgs || []).find((o) => o.slug === active || o.org_id === active);
  const isOwner = mine?.role === "owner";

  const loadMe = useCallback(async () => {
    try {
      const r = await api("/api/v1/me");
      setMe(r);
    } catch (e) { toast.error(e.message); }
  }, []);

  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(null);

  const loadMembers = useCallback(async () => {
    try {
      const r = await oapi("/members");
      setMembers(r.members || []);
      setError(null);
    } catch (e) {
      if (e.status === 403 || e.status === 401 || e.status === 404) { setMembers("denied"); setError(null); }
      else { setError(e.message); toast.error(e.message); }
    }
  }, []);

  const loadQuota = useCallback(async () => {
    try {
      const r = await oapi("/quota");
      setQuota(r);
    } catch { setQuota(null); }
  }, []);

  useEffect(() => { loadMe(); loadMembers(); loadQuota(); }, [loadMe, loadMembers, loadQuota]);

  const switchOrg = (slug) => {
    // Shell owns the active org: remounts org-scoped views (key={org}) and
    // refetches alerts, so no full page reload is needed.
    onOrgChange?.(slug);
  };

  const retry = () => { setError(null); loadMembers(); };

  const createOrg = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const payload = {
      slug: String(fd.get("slug") || ""),
      name: String(fd.get("name") || "") || undefined,
    };
    setBusy("createOrg");
    try {
      const r = await api("/api/v1/orgs", { method: "POST", body: JSON.stringify(payload) });
      toast.success(`Organization ${r.organization.slug} created`);
      switchOrg(r.organization.slug);
    } catch (err) { toast.error(err.message); } finally { setBusy(null); }
  };

  const invite = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const login = String(fd.get("login") || "").trim();
    const gid = String(fd.get("github_id") || "").trim();
    const payload = { role: String(fd.get("role") || "viewer") };
    if (login) payload.login = login;
    else if (gid) payload.github_id = Number(gid);
    setBusy("invite");
    try {
      await oapi("/members", { method: "POST", body: JSON.stringify(payload) });
      toast.success("Member added");
      e.target.reset();
      loadMembers();
    } catch (err) { toast.error(err.message); } finally { setBusy(null); }
  };

  const changeRole = async (m, role) => {
    setBusy(`role:${m.user_id}`);
    try {
      await oapi(`/members/${m.user_id}`, {
        method: "PATCH",
        body: JSON.stringify({ role }),
      });
      toast.success(`${m.login} is now ${role}`);
      loadMembers();
    } catch (e) { toast.error(e.message); } finally { setBusy(null); }
  };

  const remove = async (m) => {
    if (!(await confirmAction({
      title: `Remove ${m.login}?`,
      body: "They lose access to this organization's applications immediately.",
      confirmLabel: "Remove",
      danger: true,
    }))) return;
    setBusy(`remove:${m.user_id}`);
    try {
      await oapi(`/members/${m.user_id}`, { method: "DELETE" });
      toast.success(`Removed ${m.login}`);
      loadMembers();
    } catch (e) { toast.error(e.message); } finally { setBusy(null); }
  };

  return html`
    <section class="panel">
      <div class="panel-head">
        <h1>Organization</h1>
        ${mine && html`<span class="pill">${mine.slug} · ${mine.role}</span>`}
      </div>
      <div class="row" style="margin-bottom:14px">
        <label>Active organization
          <select value=${active} onChange=${(e) => switchOrg(e.target.value)}>
            ${(me?.orgs || []).map((o) => html`<option value=${o.slug}>${o.slug} (${o.role})</option>`)}
          </select>
        </label>
      </div>
      <h3>Quota usage</h3>
      ${quota === null
        ? html`<p class="muted">Loading…</p>`
        : html`<div class="kpis">
            <div class="kpi"><div class="kpi-label">Apps</div><div class="kpi-value">${quota.usage.apps}/${quota.quota.max_apps}</div></div>
            <div class="kpi"><div class="kpi-label">Memory</div><div class="kpi-value">${quota.usage.mem_mb}/${quota.quota.max_mem_mb} MB</div></div>
            <div class="kpi"><div class="kpi-label">CPU</div><div class="kpi-value">${quota.usage.cpu_pct}/${quota.quota.max_cpu_pct}%</div></div>
            <div class="kpi"><div class="kpi-label">Domains</div><div class="kpi-value">${quota.usage.domains}/${quota.quota.max_domains}</div></div>
          </div>`}
      <h3>Members</h3>
      ${error
        ? html`<p class="muted">Could not load members: ${error}</p>
          <div><button class="btn" onClick=${retry}>Retry</button></div>`
        : members === null
        ? html`<p class="muted">Loading…</p>`
        : members === "denied"
          ? html`<p class="muted">You are not a member of this organization.</p>`
          : html`<div class="table-wrap"><table class="stacked">
              <thead><tr><th>Login</th><th>Role</th><th>Since</th><th></th></tr></thead>
              <tbody>
                ${members.map((m) => html`
                  <tr>
                    <td data-label="Login" class="mono">${m.login}</td>
                    <td data-label="Role">
                      ${isOwner
                        ? html`<select value=${m.role} disabled=${busy !== null} onChange=${(e) => changeRole(m, e.target.value)}>
                            ${ROLES.map((r) => html`<option value=${r}>${r}</option>`)}
                          </select>`
                        : html`<span class="pill">${m.role}</span>`}
                    </td>
                    <td data-label="Since" class="muted">${m.created_at ? fmtTime(m.created_at) : "—"}</td>
                    <td class="controls no-label">
                      ${isOwner && html`<button class="btn small ghost" disabled=${busy !== null} onClick=${() => remove(m)}>Remove</button>`}
                    </td>
                  </tr>`)}
              </tbody>
            </table></div>`}
      ${isOwner && html`<form class="form" style="margin-top:14px" onSubmit=${invite}>
        <h3>Invite member</h3>
        <div class="row">
          <label>GitHub login <input name="login" placeholder="octocat" /></label>
          <label>…or numeric GitHub id <input name="github_id" placeholder="583231" inputmode="numeric" /></label>
        </div>
        <div class="row">
          <label>Role <select name="role">
            ${ROLES.map((r) => html`<option value=${r}>${r}</option>`)}
          </select></label>
        </div>
        <div><button class="btn" type="submit" disabled=${busy === "invite"}>${busy === "invite" ? "Adding…" : "Add member"}</button></div>
      </form>`}
      ${user && html`<form class="form" style="margin-top:14px" onSubmit=${createOrg}>
        <h3>New organization</h3>
        <div class="row">
          <label>Slug <input name="slug" placeholder="acme" required maxlength="32" /></label>
          <label>Display name <input name="name" placeholder="Acme" /></label>
        </div>
        <div><button class="btn" type="submit" disabled=${busy === "createOrg"}>${busy === "createOrg" ? "Creating…" : "Create organization"}</button></div>
      </form>`}
    </section>`;
}
