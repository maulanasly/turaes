// Minimal toast store (module singleton) + hook.
import { useState, useEffect } from "preact/hooks";

let items = [];
let seq = 0;
const listeners = new Set();

function emit() {
  for (const l of listeners) l(items);
}

export function push(kind, text, ttl = 5000) {
  const id = ++seq;
  items = [...items, { id, kind, text }];
  emit();
  if (ttl > 0) setTimeout(() => dismiss(id), ttl);
  return id;
}

export function dismiss(id) {
  items = items.filter((i) => i.id !== id);
  emit();
}

export const toast = {
  success: (t) => push("success", t),
  error: (t) => push("error", t, 8000),
  info: (t) => push("info", t),
};

export function useToasts() {
  const [state, setState] = useState(items);
  useEffect(() => {
    const l = (next) => setState(next);
    listeners.add(l);
    return () => listeners.delete(l);
  }, []);
  return state;
}
