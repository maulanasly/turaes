import { html } from "../lib/html.js";
import { useState, useEffect, useCallback } from "preact/hooks";
import { api, oapi, getOrg, setOrg } from "../lib/api.js";
import { toast } from "../lib/toast.js";
import { confirmAction } from "../lib/confirm.js";
import { fmtTime } from "../lib/format.js";

const ROLES = ["viewer", "developer", "admin", "owner"];

export function OrgView({ user }) {
  const [me, setMe] = useState(null);
  const [members, setMembers] = useState(null);
  const active = getOrg();
  const mine = (me?.orgs || []).find((o) => o.slug === active || o.org_id === active);
  const isOwner = mine?.role === "owner";

  const loadMe = useCallback(async () => {
    try {
      const r = await api("/api/v1/me");
      setMe(r);
    } catch (e) { toast.error(e.message); }
  }, []);

  const loadMembers = useCallback(async () => {
    try {
      const r = await oapi("/members");
      setMembers(r.members || []);
    } catch (e) {
      if (e.status === 403 || e.status === 401 || e.status === 404) setMembers("denied");
      else toast.error(e.message);
    }
  }, []);

  useEffect(() => { loadMe(); loadMembers(); }, [loadMe, loadMembers]);

  const switchOrg = (slug) => {
    try { localStorage.setItem("turaes-org", slug); } catch (e) {}
    setOrg(slug);
    location.reload();
  };

  const createOrg = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const payload = {
      slug: String(fd.get("slug") || ""),
      name: String(fd.get("name") || "") || undefined,
    };
    try {
      const r = await api("/api/v1/orgs", { method: "POST", body: JSON.stringify(payload) });
      toast.success(`Organization ${r.organization.slug} created`);
      switchOrg(r.organization.slug);
    } catch (err) { toast.error(err.message); }
  };

  const invite = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const login = String(fd.get("login") || "").trim();
    const gid = String(fd.get("github_id") || "").trim();
    const payload = { role: String(fd.get("role") || "viewer") };
    if (login) payload.login = login;
    else if (gid) payload.github_id = Number(gid);
    try {
      await oapi("/members", { method: "POST", body: JSON.stringify(payload) });
      toast.success("Member added");
      e.target.reset();
      loadMembers();
    } catch (err) { toast.error(err.message); }
  };

  const changeRole = async (m, role) => {
    try {
      await oapi(`/members/${m.user_id}`, {
        method: "PATCH",
        body: JSON.stringify({ role }),
      });
      toast.success(`${m.login} is now ${role}`);
      loadMembers();
    } catch (e) { toast.error(e.message); }
  };

  const remove = async (m) => {
    if (!(await confirmAction({
      title: `Remove ${m.login}?`,
      body: "They lose access to this organization's applications immediately.",
      confirmLabel: "Remove",
      danger: true,
    }))) return;
    try {
      await oapi(`/members/${m.user_id}`, { method: "DELETE" });
      toast.success(`Removed ${m.login}`);
      loadMembers();
    } catch (e) { toast.error(e.message); }
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
      <h3>Members</h3>
      ${members === null
        ? html`<p class="muted">Loading…</p>`
        : members === "denied"
          ? html`<p class="muted">You are not a member of this organization.</p>`
          : html`<div class="table-wrap"><table>
              <thead><tr><th>Login</th><th>Role</th><th>Since</th><th></th></tr></thead>
              <tbody>
                ${members.map((m) => html`
                  <tr>
                    <td class="mono">${m.login}</td>
                    <td>
                      ${isOwner
                        ? html`<select value=${m.role} onChange=${(e) => changeRole(m, e.target.value)}>
                            ${ROLES.map((r) => html`<option value=${r}>${r}</option>`)}
                          </select>`
                        : html`<span class="pill">${m.role}</span>`}
                    </td>
                    <td class="muted">${m.created_at ? fmtTime(m.created_at) : "—"}</td>
                    <td class="controls">
                      ${isOwner && html`<button class="btn small ghost" onClick=${() => remove(m)}>Remove</button>`}
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
        <div><button class="btn" type="submit">Add member</button></div>
      </form>`}
      ${user && html`<form class="form" style="margin-top:14px" onSubmit=${createOrg}>
        <h3>New organization</h3>
        <div class="row">
          <label>Slug <input name="slug" placeholder="acme" required maxlength="32" /></label>
          <label>Display name <input name="name" placeholder="Acme" /></label>
        </div>
        <div><button class="btn" type="submit">Create organization</button></div>
      </form>`}
    </section>`;
}
