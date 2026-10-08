// Pure mappings from dashboard state to hairline figure values (see
// components/Hairline.js for the mounting side). No Preact/DOM imports so
// this stays unit-testable under `node --test`.
import { hasPendingDeployment } from "./appForm.js";

// Beacon openness by app status: a quiet beam for healthy/idle states, a
// wide one for attention states. Unknown statuses read mid-low, never alarming.
const STATUS_INTENSITY = {
  running: 0.15,
  stopped: 0.2,
  unknown: 0.3,
  deploying: 0.55,
  unhealthy: 0.7,
  failed: 0.9,
};

export function beaconValue(status) {
  return STATUS_INTENSITY[status] ?? STATUS_INTENSITY.unknown;
}

// Slots value: the whole part picks the live slot (0 = A, 1 = B), the
// fraction is the cut phase. Mirrors the figure's range semantics.
const SLOT_PHASE = { empty: 0.05, idle: 0.3, progress: 0.8 };

export function slotsValue(active, phase) {
  return (active === "B" ? 1 : 0) + (SLOT_PHASE[phase] ?? SLOT_PHASE.idle);
}

// Derive a slots figure's inputs from an app row and its deployments.
// Only two slots exist (A = base port, B = base + offset), so any set
// active_port off the base port is B; null means unslotted/A.
export function slotsInput(app, deployments) {
  const list = deployments || [];
  const phase = list.length === 0 ? "empty" : hasPendingDeployment(list) ? "progress" : "idle";
  const active = app && app.active_port != null && app.active_port !== app.port ? "B" : "A";
  const retained = list.length >= 2 || list.some((d) => d.previous_artifact);
  return { active, phase, retained };
}
