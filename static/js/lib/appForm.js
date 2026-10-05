// Pure helpers for the guided app-creation form and lifecycle consequences.
// No Preact/DOM imports so this stays unit-testable under `node --test`.

// Workload types, each with plain-language constraints (native processes, no
// build, no containers).
export const KINDS = [
  {
    id: "service",
    label: "Web service",
    blurb: "A long-running process that listens on a port. turaes routes a domain to it through the proxy.",
    constraints: [
      "Runs a prebuilt binary already on the server — turaes never builds or containers it.",
      "The port must be free on the chosen server.",
      "Arguments are passed literally: there is no shell, so no pipes, globs or quoting.",
    ],
  },
  {
    id: "static",
    label: "Static site",
    blurb: "A directory of files that turaes serves itself. No process is started.",
    constraints: [
      "turaes serves the directory directly — there is no build step.",
      "Needs a free port for its internal file server.",
      "Files are synced to the server on the next deploy.",
    ],
  },
  {
    id: "worker",
    label: "Background worker",
    blurb: "A long-running process with no port and no route — queues, jobs, watch loops.",
    constraints: [
      "Runs a prebuilt binary already on the server.",
      "No port and no domain: nothing can reach it over HTTP.",
      "It is supervised and restarted on failure; watch it in the logs tab.",
    ],
  },
];

export function kindInfo(id) {
  return KINDS.find((k) => k.id === id) || KINDS[0];
}

export const STEPS = ["Workload", "Process", "Placement", "Review"];

export function emptyDraft(serverId = "local") {
  return {
    kind: "service",
    name: "",
    description: "",
    binary_path: "",
    args: "",
    publish_dir: "",
    port: "",
    health_path: "",
    domain: "",
    server_id: serverId,
    runtime: "systemd",
    auto_restart: true,
    mem_limit_mb: "",
    cpu_quota_pct: "",
  };
}

// First blocking error for a step, or null. Mirrors the server's validation so
// the form never submits something the API will reject.
export function stepError(draft, step) {
  if (step === 0) {
    return draft.kind ? null : "Choose a workload type.";
  }
  if (step === 1) {
    const name = draft.name.trim();
    if (!name) return "Name is required.";
    if (name.length > 64 || !/^[a-z0-9-]+$/.test(name) || name.startsWith("-") || name.endsWith("-")) {
      return "Name must be a lowercase slug of a-z, 0-9 and '-'.";
    }
    if (draft.kind === "static") {
      if (!draft.publish_dir.trim()) return "A source directory is required for a static site.";
    } else if (!draft.binary_path.trim()) {
      return "A binary path is required.";
    }
    if (draft.kind !== "worker") {
      const port = Number(draft.port);
      if (!Number.isInteger(port) || port < 1 || port > 65535) return "Enter a port between 1 and 65535.";
    }
    return null;
  }
  return null;
}

// API payload for POST /apps. Only sends fields that apply to the workload.
export function buildPayload(draft) {
  const kind = draft.kind;
  const payload = { kind, name: draft.name.trim() };
  if (draft.description.trim()) payload.description = draft.description.trim();
  if (kind === "static") {
    payload.publish_dir = draft.publish_dir.trim();
  } else {
    payload.binary_path = draft.binary_path.trim();
  }
  if (draft.args.trim()) payload.args = draft.args.trim();
  if (kind !== "worker") {
    payload.port = Number(draft.port);
    if (draft.health_path.trim()) payload.health_path = draft.health_path.trim();
    if (draft.domain.trim()) payload.domain = draft.domain.trim();
  }
  if (draft.server_id) payload.server_id = draft.server_id;
  if (draft.runtime) payload.runtime = draft.runtime;
  payload.auto_restart = !!draft.auto_restart;
  const mem = Number(draft.mem_limit_mb);
  if (draft.mem_limit_mb !== "" && Number.isFinite(mem)) payload.mem_limit_mb = mem;
  const cpu = Number(draft.cpu_quota_pct);
  if (draft.cpu_quota_pct !== "" && Number.isFinite(cpu)) payload.cpu_quota_pct = cpu;
  return payload;
}

// Grouped rows for the review-before-create summary.
export function reviewGroups(draft, serverLabel) {
  const kind = kindInfo(draft.kind);
  const identity = [["Name", draft.name.trim() || "—"]];
  if (draft.description.trim()) identity.push(["Description", draft.description.trim()]);

  const process = [];
  if (draft.kind === "static") process.push(["Source directory", draft.publish_dir.trim() || "—"]);
  else process.push(["Binary", draft.binary_path.trim() || "—"]);
  if (draft.args.trim()) process.push(["Arguments", draft.args.trim()]);
  if (draft.kind !== "worker") {
    process.push(["Port", draft.port || "—"]);
    process.push(["Health path", draft.health_path.trim() || "/health (default)"]);
  }

  const placement = [["Server", serverLabel || draft.server_id || "—"]];
  if (draft.kind !== "worker") placement.push(["Domain", draft.domain.trim() || "— (no route)"]);
  placement.push(["Managed by", draft.runtime === "proc" ? "turaes (proc)" : "systemd"]);
  placement.push(["Restart when unhealthy", draft.auto_restart ? "yes" : "no"]);
  if (draft.mem_limit_mb !== "") placement.push(["Memory limit", `${draft.mem_limit_mb} MiB`]);
  if (draft.cpu_quota_pct !== "") placement.push(["CPU limit", `${draft.cpu_quota_pct}%`]);

  return [
    { title: "Workload", rows: [["Type", kind.label]] },
    { title: "Identity", rows: identity },
    { title: "Process", rows: process },
    { title: "Placement", rows: placement },
  ];
}

// What creating the app does (it does not start anything yet).
export function createConsequences(kind) {
  const notes = ["Creating only records the app — it starts as stopped and is not reachable until you deploy."];
  if (kind === "static") {
    notes.push("On deploy, turaes serves the files through the proxy — no build runs.");
  } else if (kind === "worker") {
    notes.push("On deploy, turaes installs the binary and starts the process; there is no port or route to swap.");
  } else {
    notes.push("On deploy, turaes installs the binary, starts the process, then routes the domain once a health check passes.");
  }
  return notes;
}

// Consequences shown when confirming lifecycle actions.
export function deployConsequences(app) {
  const notes = ["Installs the app's current binary and starts a new version on a spare port."];
  if (app && app.domain) {
    notes.push("Re-points the proxy route to the new process only after it passes its health check — no downtime.");
  } else {
    notes.push("Keeps the previous process serving until the new one passes its health check.");
  }
  notes.push("The previous build is retained, so you can roll back.");
  return notes;
}

export function restartConsequences() {
  return [
    "Restarts the active process on the same port.",
    "Expect a brief interruption while it comes back up.",
  ];
}

export function stopConsequences() {
  return [
    "Stops the process and removes its route.",
    "Traffic drops immediately until you start or redeploy it.",
  ];
}

export function startConsequences() {
  return [
    "Starts the process and re-adds its route.",
    "Traffic returns only after a successful health check.",
  ];
}

export function rollbackConsequences(hash) {
  return [
    hash
      ? `Redeploys build ${hash} on a spare port, then swaps the route when it is healthy.`
      : "Redeploys the previous build on a spare port, then swaps the route when it is healthy.",
  ];
}
