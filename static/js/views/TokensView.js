import { html } from "../lib/html.js";
import { useState, useEffect, useCallback } from "preact/hooks";
import { oapi } from "../lib/api.js";
import { toast } from "../lib/toast.js";
import { confirmAction } from "../lib/confirm.js";
import { fmtTime } from "../lib/format.js";

const SCOPES = [
  ["read", "Read — apps, metrics, history"],
  ["deploy", "Deploy — read plus deploys, lifecycle, env, domains"],
  ["admin", "Admin — deploy plus apps, tokens (never owner)"],
];

export function TokensView({ user }) {
  const [tokens, setTokens] = useState(null);
  const [fresh, setFresh] = useState(null);
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(null);

  const load = useCallback(async () => {
    try {
      const r = await oapi("/tokens");
      setTokens(r.tokens || []);
      setError(null);
    } catch (e) {
      if (e.status === 403 || e.status === 401) { setTokens("denied"); setError(null); }
      else { setError(e.message); toast.error(e.message); }
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  const retry = () => { setError(null); load(); };

  const create = async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    const payload = { name: String(fd.get("name") || ""), scopes: String(fd.get("scopes") || "read") };
    setBusy("create");
    try {
      const r = await oapi("/tokens", { method: "POST", body: JSON.stringify(payload) });
      setFresh({ name: r.token.name, plaintext: r.plaintext });
      toast.success(`Token ${r.token.name} created`);
      e.target.reset();
      load();
    } catch (err) { toast.error(err.message); } finally { setBusy(null); }
  };

  const revoke = async (t) => {
    if (!(await confirmAction({
      title: `Revoke ${t.name}?`,
      body: "Clients using this token stop authenticating immediately.",
      confirmLabel: "Revoke",
      danger: true,
    }))) return;
    setBusy(`revoke:${t.id}`);
    try {
      await oapi(`/tokens/${t.id}`, { method: "DELETE" });
      toast.success(`Revoked ${t.name}`);
      load();
    } catch (e) { toast.error(e.message); } finally { setBusy(null); }
  };

  const copy = async (text) => {
    try {
      await navigator.clipboard.writeText(text);
      toast.success("Copied");
    } catch {
      toast.error("Copy failed — select the token manually");
    }
  };

  return html`
    <section class="panel">
      <div class="panel-head"><h1>API tokens</h1></div>
      <p class="muted small">Programmatic access for CI (use <span class="mono">Authorization: Bearer …</span>).
        Tokens are bound to this organization and can never manage members.</p>
      ${fresh && html`<div class="notice">
        <div><strong>${fresh.name}</strong> — copy it now, it is never shown again.</div>
        <div class="mono">${fresh.plaintext}
          <button class="btn small ghost" onClick=${() => copy(fresh.plaintext)}>Copy</button>
          <button class="btn small ghost" onClick=${() => setFresh(null)}>Dismiss</button>
        </div>
      </div>`}
      ${error
        ? html`<p class="muted">Could not load tokens: ${error}</p>
          <div><button class="btn" onClick=${retry}>Retry</button></div>`
        : tokens === null
        ? html`<p class="muted">Loading…</p>`
        : tokens === "denied"
          ? html`<p class="muted">Token management needs an admin of this organization.</p>`
          : tokens.length === 0
          ? html`<p class="muted">No tokens yet — create one below for CI access.</p>`
          : html`<div class="table-wrap"><table class="stacked">
              <thead><tr><th>Name</th><th>Scope</th><th>Created</th><th>Last used</th><th></th></tr></thead>
              <tbody>
                ${tokens.map((t) => html`
                  <tr>
                    <td data-label="Name" class="mono">${t.name}</td>
                    <td data-label="Scope"><span class="mono small muted">${t.scopes}</span></td>
                    <td data-label="Created" class="muted">${t.created_at ? fmtTime(t.created_at) : "—"}</td>
                    <td data-label="Last used" class="muted">${t.last_used_at ? fmtTime(t.last_used_at) : "never"}</td>
                    <td class="controls no-label">
                      ${user && html`<button class="btn small ghost" disabled=${busy !== null} onClick=${() => revoke(t)}>Revoke</button>`}
                    </td>
                  </tr>`)}
              </tbody>
            </table></div>`}
      ${user && html`<form class="form" style="margin-top:14px" onSubmit=${create}>
        <h3>New token</h3>
        <div class="row">
          <label>Name <input name="name" placeholder="ci-deploy" required maxlength="64" /></label>
          <label>Scope <select name="scopes">
            ${SCOPES.map(([v, label]) => html`<option value=${v}>${label}</option>`)}
          </select></label>
        </div>
        <div><button class="btn" type="submit" disabled=${busy === "create"}>${busy === "create" ? "Creating…" : "Create token"}</button></div>
      </form>`}
    </section>`;
}
