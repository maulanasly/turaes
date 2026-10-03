import { html } from "../lib/html.js";
import { settleConfirm, useConfirm } from "../lib/confirm.js";

export function ConfirmHost() {
  const c = useConfirm();
  if (!c) return null;
  return html`
    <div class="modal-backdrop" onClick=${() => settleConfirm(false)}>
      <div class="modal" role="alertdialog" aria-modal="true" onClick=${(e) => e.stopPropagation()}>
        <h3>${c.title || "Are you sure?"}</h3>
        ${c.body && html`<p class="muted">${c.body}</p>`}
        <div class="modal-actions">
          <button class="btn ghost" onClick=${() => settleConfirm(false)}>Cancel</button>
          <button class=${"btn" + (c.danger ? " danger" : "")} onClick=${() => settleConfirm(true)}>
            ${c.confirmLabel || "Confirm"}
          </button>
        </div>
      </div>
    </div>`;
}
