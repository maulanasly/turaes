import { html } from "../lib/html.js";
import { useEffect, useRef } from "preact/hooks";
import { settleConfirm, useConfirm } from "../lib/confirm.js";

export function ConfirmHost() {
  const c = useConfirm();
  const modalRef = useRef(null);
  const cancelRef = useRef(null);
  const prevFocus = useRef(null);

  useEffect(() => {
    if (!c) return undefined;
    prevFocus.current = document.activeElement;
    // Focus the safe action: destructive confirms need a deliberate Tab+Enter.
    if (cancelRef.current) cancelRef.current.focus();
    const onKey = (e) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        settleConfirm(false);
        return;
      }
      if (e.key !== "Tab" || !modalRef.current) return;
      const items = [...modalRef.current.querySelectorAll("button")].filter((b) => !b.disabled);
      if (items.length === 0) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("keydown", onKey, true);
      if (prevFocus.current && prevFocus.current.focus) prevFocus.current.focus();
    };
  }, [c]);

  if (!c) return null;
  return html`
    <div class="modal-backdrop" onClick=${() => settleConfirm(false)}>
      <div class="modal" ref=${modalRef} role="alertdialog" aria-modal="true"
        aria-label=${c.title || "Are you sure?"} onClick=${(e) => e.stopPropagation()}>
        <h3>${c.title || "Are you sure?"}</h3>
        ${c.body && html`<p class="muted">${c.body}</p>`}
        <div class="modal-actions">
          <button ref=${cancelRef} class="btn ghost" onClick=${() => settleConfirm(false)}>Cancel</button>
          <button class=${"btn" + (c.danger ? " danger" : "")} onClick=${() => settleConfirm(true)}>
            ${c.confirmLabel || "Confirm"}
          </button>
        </div>
      </div>
    </div>`;
}
