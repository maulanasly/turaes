import { html } from "../lib/html.js";
import { dismiss, useToasts } from "../lib/toast.js";

export function Toasts() {
  const items = useToasts();
  return html`
    <div class="toast-host" role="status" aria-live="polite" aria-atomic="false">
      ${items.map((t) => html`
        <div class=${"toast " + t.kind} role=${t.kind === "error" ? "alert" : null}>
          <span>${t.text}</span>
          <button class="toast-x" aria-label="Dismiss" onClick=${() => dismiss(t.id)}>×</button>
        </div>`)}
    </div>`;
}
