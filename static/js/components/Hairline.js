import { html } from "../lib/html.js";
import { useEffect, useRef } from "preact/hooks";
import HL from "../vendor/hairline/kernel.js";
import * as beacon from "../vendor/hairline/beacon.js";
import * as slots from "../vendor/hairline/slots.js";

const FIGURES = {
  beacon: { mount: beacon.mount, meta: beacon.meta },
  slots: { mount: slots.mount, meta: slots.meta },
};

let styled = false;
function ensureStyle() {
  if (!styled) {
    HL.inject(document);
    styled = true;
  }
}

// A hairline figure as a labeled image. `figure` selects the engine,
// `value` is the figure-native number (see lib/figureState.js), `label`
// names it for assistive tech. Mirrors the skill bench: an svg stage plus a
// screen-reader caption fed by the figure's read-out. The figure augments
// surrounding text (badges, hints) and never replaces it.
export function HairlineFigure({ figure, value, label }) {
  const ref = useRef(null);
  const handle = useRef(null);
  const kind = FIGURES[figure] ? figure : "beacon";

  useEffect(() => {
    const el = ref.current;
    if (!el) return undefined;
    ensureStyle();
    el.setAttribute("data-hairline", FIGURES[kind].meta.name);
    const svg = HL.mk("svg", { viewBox: "0 0 400 320", "aria-hidden": "true" }, el);
    const cap = document.createElement("div");
    cap.className = "sr-only";
    el.appendChild(cap);
    const read = {
      get textContent() { return cap.textContent; },
      set textContent(v) { cap.textContent = v == null ? "" : String(v); },
    };
    const h = FIGURES[kind].mount({ stage: el, svg, read }, value);
    handle.current = h;
    return () => {
      try { h.destroy(); } catch (e) {}
      handle.current = null;
    };
  }, [kind]);

  useEffect(() => {
    if (handle.current) {
      try { handle.current.set(value); } catch (e) {}
    }
  }, [value]);

  return html`<div class="fig" role="img" aria-label=${label} ref=${ref}></div>`;
}
