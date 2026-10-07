import { html } from "../lib/html.js";
import { BrandMark } from "../components/Brand.js";

// Standalone sign-in page: no nav, no menus, no data.
export function LoginView({ error, theme, onToggleTheme }) {
  return html`
    <div class="login">
      <div class="login-card">
        ${onToggleTheme && html`<button class="btn small ghost login-theme" onClick=${onToggleTheme}
          title="Toggle theme" aria-label="Toggle theme" aria-pressed=${theme === "light"}>
          ${theme === "dark"
            ? html`<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M12 11A5.5 5.5 0 0 1 5 4a5.5 5.5 0 1 0 7 7z" fill="currentColor" /></svg>`
            : html`<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><circle cx="8" cy="8" r="3.5" fill="none" stroke="currentColor" stroke-width="1.5" /><path d="M8 1v2M8 13v2M1 8h2M13 8h2M3 3l1.4 1.4M11.6 11.6L13 13M13 3l-1.4 1.4M4.4 11.6L3 13" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>`}</button>`}
        <div class="login-brand"><${BrandMark} size=${36} theme=${theme} /><span>turaes</span></div>
        <p><strong>Deploy apps as native processes. Operate the whole fleet.</strong></p>
        <p class="muted small">No containers.</p>
        ${error && html`<p class="login-error" role="alert">${error}</p>`}
        <a class="btn login-btn" href="/auth/login">Sign in with GitHub</a>
        <p class="muted small">Access is limited to allowed GitHub accounts.</p>
      </div>
    </div>`;
}
