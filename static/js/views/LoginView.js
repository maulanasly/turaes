import { html } from "../lib/html.js";
import { BrandMark } from "../components/Brand.js";

// Standalone sign-in page: no nav, no menus, no data.
export function LoginView({ error, theme, onToggleTheme }) {
  return html`
    <div class="login">
      <div class="login-card">
        ${onToggleTheme && html`<button class="btn small ghost login-theme" onClick=${onToggleTheme}
          title="Toggle theme" aria-label="Toggle theme">${theme === "dark" ? "☾" : "☀"}</button>`}
        <div class="login-brand"><${BrandMark} size=${36} /><span>turaes</span></div>
        <p><strong>Deploy apps as native processes. Operate the whole fleet.</strong></p>
        <p class="muted small">No containers.</p>
        ${error && html`<p class="login-error" role="alert">${error}</p>`}
        <a class="btn login-btn" href="/auth/login">Sign in with GitHub</a>
        <p class="muted small">Access is limited to allowed GitHub accounts.</p>
      </div>
    </div>`;
}
