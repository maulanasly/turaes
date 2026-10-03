// Promise-based confirm store. Usage:
//   if (await confirmAction({ title, body, confirmLabel, danger })) { ... }
import { useState, useEffect } from "preact/hooks";

let current = null;
const listeners = new Set();

function emit() {
  for (const l of listeners) l(current);
}

export function confirmAction(opts) {
  return new Promise((resolve) => {
    current = { ...opts, resolve };
    emit();
  });
}

export function settleConfirm(ok) {
  if (current) {
    current.resolve(ok);
    current = null;
    emit();
  }
}

export function useConfirm() {
  const [state, setState] = useState(current);
  useEffect(() => {
    const l = (next) => setState(next);
    listeners.add(l);
    return () => listeners.delete(l);
  }, []);
  return state;
}
