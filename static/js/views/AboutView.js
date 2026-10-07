import { html } from "../lib/html.js";
import { BrandMark } from "../components/Brand.js";

// Static product + build info. `health` is the already-polled /health state,
// so this view makes no extra requests and stays current.
export function AboutView({ health, user }) {
  const runtime = health && health.runtime ? health.runtime : "—";
  const version = health && health.version ? health.version : "—";
  const proxy = !health ? "—" : health.proxy ? "enabled (Pingora)" : "disabled";
  const ok = health && health.status === "ok" ? "healthy" : "unreachable";

  return html`
    <section class="panel">
      <div class="panel-head"><h1>About</h1></div>
      <div class="brand" style="font-size:20px"><${BrandMark} size=${26} /><span>turaes</span></div>
      <p><strong>Deploy apps as native processes. Operate the whole fleet.</strong></p>
      <p class="muted small">No containers.</p>
      <div class="table-wrap"><table>
        <tbody>
          <tr><th>Version</th><td class="mono">${version}</td></tr>
          <tr><th>Runtime</th><td class="mono">${runtime}</td></tr>
          <tr><th>Proxy</th><td>${proxy}</td></tr>
          <tr><th>Status</th><td>${ok}</td></tr>
          <tr><th>Signed in as</th><td class="mono break">${user && user.login ? user.login : "—"}</td></tr>
        </tbody>
      </table></div>
    </section>`;
}