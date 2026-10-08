/**
 * Slots: two engine slots, A and B, on one plinth. The value picks the live
 * slot and its phase: below one A is live, at one and past B is live, and the
 * fraction past the whole is how far the cut has run. The live tray rides
 * high with its flow beads lit; the retained tray rests dim. The pointer picks
 * a slot from its static half and lifts it bright; letting go returns to the
 * value. The slider runs A-quiet, A-busy, B-live.
 *
 * The pattern: discrete items answered from static halves, each riding its own
 * spring, with a rest that names the live slot.
 */
// Production module. Skill source: fig-work/slots.js (validated via
// build.mjs + validate.mjs + look.mjs; bench page at static/preview/).
// Mechanical adaptation only: HL comes from the vendored kernel module and
// the trailing hairline() declare is a meta export instead.
import HL from "./kernel.js";

const {
  Cam, clamp, facing, fit, prism, proj, rings, spring, stepS,
  flatDot, mk, place, pointer, put, register, disposer, solid,
} = HL;

function mount({ stage, svg, read }, value) {
  const bag = disposer();
  let v = value;
  const C = Cam(45, 0.5, 2.1);
  fit(C, [[-62, -36, -6], [62, 36, -6], [-48, -18, 18], [48, 18, 18]], 200, 166);
  const P = proj(C), front = facing(C);
  let pick = null;

  const live = () => (v < 1 ? "A" : "B");
  const phase = () => clamp(v - Math.floor(v), 0, 1);

  const g = mk("g", {}, svg);
  const [pr, pi] = rings(-56, -30, 56, 30, 9, 2.2);
  const plinth = solid(g);
  put(plinth, prism(P, front, pr, pi, -6, 0));

  function tray(x0, x1, id) {
    const [ring, inner] = rings(x0, -18, x1, 18, 5, 1.4);
    const el = solid(g);
    const sp = spring(1.5);
    const dots = [];
    for (let k = 0; k < 3; k++) dots.push(flatDot(g, C, 0.6, "dot off"));
    return { id, ring, inner, el, sp, dots, drawn: NaN, cx: (x0 + x1) / 2 };
  }
  const A = tray(-48, -4, "A"), B = tray(4, 48, "B");

  function targets() {
    const L = live(), ph = phase();
    const T = { A: L === "A" ? 2 + ph * 8 : 1.5, B: L === "B" ? 2 + ph * 8 : 1.5 };
    if (pick) T[pick] = Math.min(13, T[pick] + 5);
    return T;
  }

  function drawSlot(s, lift) {
    if (lift === s.drawn) return false;
    s.drawn = lift;
    put(s.el, prism(P, front, s.ring, s.inner, 0, 2.5 + lift));
    const lit = lift > 6 ? 3 : lift > 3.5 ? 2 : lift > 2 ? 1 : 0;
    for (let k = 0; k < 3; k++) {
      place(s.dots[k], P(s.cx - 7 + k * 7, 0, 2.5 + lift));
      s.dots[k].setAttribute("class", k < lit ? "dot" : "dot off");
    }
    return true;
  }

  function paint() {
    const show = pick || live();
    A.el.sil.classList.toggle("hi", show === "A");
    B.el.sil.classList.toggle("hi", show === "B");
  }

  const T = targets();
  A.sp.t = T.A; B.sp.t = T.B;
  paint();

  const R = register(stage, (dt) => {
    const a = stepS(A.sp, dt), b = stepS(B.sp, dt);
    drawSlot(A, Math.max(0.5, A.sp.x));
    drawSlot(B, Math.max(0.5, B.sp.x));
    return a || b;
  });
  bag.add(R.unregister);

  function retarget() {
    const T = targets();
    A.sp.t = T.A; B.sp.t = T.B;
    paint();
    read.textContent = pick ? "slot " + pick : "rest";
    R.wake();
  }

  drawSlot(A, Math.max(0.5, A.sp.x));
  drawSlot(B, Math.max(0.5, B.sp.x));
  read.textContent = "rest";

  bag.add(pointer(stage, {
    move: (pt) => {
      const p = pt[0] < 200 ? "A" : "B";
      if (p !== pick) { pick = p; retarget(); }
    },
    leave: () => { pick = null; retarget(); },
  }));
  bag.add(() => svg.replaceChildren());

  return {
    set: (nv) => { v = nv; retarget(); },
    destroy: bag.dispose,
  };
}

export const meta = {
  name: "slots",
  means: "Two engine slots with one live; the pointer lifts the slot it names.",
  rules: [1, 3, 4, 5],
  range: [0.2, 0.75, 1.2],
};
export { mount };
