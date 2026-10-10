import { html } from "../lib/html.js";
import { Component } from "preact";

// Chair the shell against a single view throwing during render. Without an
// error boundary, Preact aborts the whole patch: the overview blanked and
// navigation froze when one identifier was missing. A crashed view is
// replaced with an inline, recoverable panel; the topbar and router stay
// responsive.
export class ErrorBoundary extends Component {
  constructor(props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error) {
    return { error };
  }

  componentDidCatch(error) {
    try { console.error("View render failed:", error); } catch {}
  }

  render({ children }, { error }) {
    if (!error) return children;
    return html`
      <section class="panel">
        <div class="panel-head"><h1>Something went wrong</h1></div>
        <p class="muted">This view could not be rendered.</p>
        <p class="muted small mono">${error && error.message ? error.message : ""}</p>
        <div class="controls">
          <button class="btn" onClick=${() => this.setState({ error: null })}>Retry</button>
          <a class="btn ghost" href="#/apps">Applications</a>
        </div>
      </section>`;
  }
}