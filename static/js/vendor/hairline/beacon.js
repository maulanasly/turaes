/**
 * Beacon: a signal lamp on a tower over a plinth. The value sets how wide the
 * shutter opens: the beam dots light in turn and past half the lamp takes the
 * bright edge. At rest the head leans into the beam; the pointer takes it
 * over on its spring, and near it the lamp is bright whatever the value.
 * The slider is the openness.
 *
 * The pattern: a continuous field of one. A spring, a falloff by distance, a
 * hit test on the ground plane (which never moves), and a rest that is a
 * composition.
 */
// Production module. Skill source: fig-work/beacon.js (validated via
// build.mjs + validate.mjs + look.mjs; bench page at static/preview/).
// Mechanical adaptation only: HL comes from the vendored kernel module and
// the trailing hairline() declare is a meta export instead.
import HL from "./kernel.js";

const {
  Cam, clamp, facing, fit, lerp, prism, proj, rings, unproj, spring, stepS,
  flatDot, mk, place, pointer, put, register, disposer, solid,
} = HL;

const NB = 5, LZ = 40;

function mount({ stage, svg, read }, value) {
  const bag = disposer();
  let open = value;
  const C = Cam(45, 0.5, 2.3);
  fit(C, [[-46, -30, -6], [46, 30, -6], [-8, -8, 0], [8, 8, 0], [78, 0, LZ + 2]], 200, 162);
  const P = proj(C), front = facing(C);
  let over = null, near = false;

  const g = mk("g", {}, svg);
  const [pr, pi] = rings(-40, -24, 40, 24, 9, 2.2);
  const plinth = solid(g);
  put(plinth, prism(P, front, pr, pi, -6, 0));
  const [tr, ti] = rings(-7, -7, 7, 7, 3, 1);
  const tower = solid(g);
  put(tower, prism(P, front, tr, ti, 0, 34));

  const lamp = solid(g);
  const aim = spring(6);
  const dots = [];
  for (let k = 0; k < NB; k++) dots.push(flatDot(g, C, 0.6, "dot off"));
  let drawnH = NaN, drawnO = NaN;

  function draw() {
    const hx = clamp(aim.x, -12, 12);
    if (hx === drawnH && open === drawnO) return;
    drawnH = hx; drawnO = open;
    const [lr, li] = rings(-11 + hx, -8, 11 + hx, 8, 4, 1.2);
    put(lamp, prism(P, front, lr, li, 34, LZ));
    const lit = Math.round(open * NB);
    for (let k = 0; k < NB; k++) {
      place(dots[k], P(18 + hx + k * 11, 0, LZ + 1));
      dots[k].setAttribute("class", k < lit ? "dot" : k === 0 ? "dot m" : "dot off");
    }
    lamp.sil.classList.toggle("hi", near || open > 0.5);
  }

  const B = register(stage, (dt) => {
    const moving = stepS(aim, dt);
    draw();
    return moving;
  });
  bag.add(B.unregister);

  function retarget() {
    if (over) {
      const d = Math.hypot(over[0], over[1]);
      aim.t = clamp(over[0] * 0.3, -12, 12);
      near = d < 16;
      read.textContent = near ? "lamp" : "tower";
    } else {
      aim.t = 6;
      near = false;
      read.textContent = "rest";
    }
    B.wake();
  }

  draw();
  read.textContent = "rest";

  bag.add(pointer(stage, {
    move: (pt) => { over = unproj(C, pt[0], pt[1], 0); retarget(); },
    leave: () => { over = null; retarget(); },
  }));
  bag.add(() => svg.replaceChildren());

  return {
    set: (v) => { open = v; retarget(); },
    destroy: bag.dispose,
  };
}

export const meta = {
  name: "beacon",
  means: "A signal lamp whose beam opens with the value; the pointer aims its head.",
  rules: [1, 3, 4, 5],
  range: [0.15, 0.5, 0.9],
};
export { mount };
