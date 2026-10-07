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
      "For a custom executable or working directory, use Advanced launch below — or define command/workdir in turaes.yaml.",
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
      "For a custom executable or working directory, use Advanced launch below — or define command/workdir in turaes.yaml.",
    ],
  },
];

export function kindInfo(id) {
  return KINDS.find((k) => k.id === id) || KINDS[0];
}

export const STEPS = ["Workload", "Process", "Placement", "Review"];

// Which wizard step owns each payload field, so server-reported problems
// can jump straight to the input that caused them.
export const FIELD_STEPS = {
  name: 1, binary_path: 1, publish_dir: 1, args: 1, command: 1, workdir: 1,
  port: 1, health_path: 1,
  server_id: 2, domain: 2, runtime: 2, mem_limit_mb: 2, cpu_quota_pct: 2,
};

// Fold a list of `{field, detail}` issues (preflight `errors`, or a submit
// error carrying `errors`) into inline field errors plus the earliest step
// and field to focus.
export function mapIssuesToFields(issues) {
  const fieldErrors = {};
  let firstField = null;
  let step = 3;
  for (const issue of issues || []) {
    if (!issue || !issue.field) continue;
    if (!(issue.field in fieldErrors)) {
      fieldErrors[issue.field] = issue.detail || "Invalid value.";
    }
    if (!firstField) firstField = issue.field;
    const s = FIELD_STEPS[issue.field];
    if (s !== undefined && s < step) step = s;
  }
  return { fieldErrors, firstField, step: firstField ? step : 3 };
}

export function emptyDraft(serverId = "local") {
  return {
    kind: "service",
    name: "",
    description: "",
    launchMode: "binary",
    binary_path: "",
    args: "",
    command: "",
    workdir: "",
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

// Split an argv textarea (one argument per line) into lines, mirroring the
// API rule: non-empty argv, executable without spaces (no shell involved).
export function validateCommandLines(text) {
  const lines = String(text || "")
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l !== "");
  if (lines.length === 0) {
    return { lines, error: "Enter the executable path, one argument per line." };
  }
  if (lines[0].includes(" ")) {
    return { lines, error: "The executable path cannot contain spaces; no shell is used." };
  }
  return { lines, error: null };
}

// Parse the stored argv JSON (`Application.command`) back into lines.
export function parseCommandArgv(stored) {
  if (!stored) return null;
  try {
    const v = JSON.parse(stored);
    return Array.isArray(v) ? v.map(String) : null;
  } catch {
    return null;
  }
}

// True while any deployment still needs reconciling (a remote agent
// catching up, a slow install): the Deployments tab polls instead of
// going stale.
export function hasPendingDeployment(deployments) {
  return (deployments || []).some((d) => d.status === "queued" || d.status === "installing");
}

// Tri-state PATCH helpers for Settings: distinguish "unchanged" (omit the
// key so the server keeps the value) from "cleared" (send null) and "set".
export function textPatch(initial, current) {
  const before = initial ?? "";
  if (current === before) return { present: false };
  if (current === "") return { present: true, value: null };
  return { present: true, value: current };
}

export function argvPatch(initial, current) {
  const before = initial || null;
  const same = before === null
    ? current.length === 0
    : before.length === current.length && before.every((v, i) => v === current[i]);
  if (same) return { present: false };
  if (current.length === 0) return { present: true, value: null };
  return { present: true, value: current };
}

export function resourceLimitErrors(runtime, memLimit, cpuLimit) {
  const errors = {};
  const hasMemLimit = memLimit !== "" && memLimit !== null && memLimit !== undefined;
  const hasCpuLimit = cpuLimit !== "" && cpuLimit !== null && cpuLimit !== undefined;
  if (runtime !== "systemd" && runtime !== "proc") errors.runtime = "Choose systemd or turaes (proc).";
  if (hasMemLimit) {
    const mem = Number(memLimit);
    if (!Number.isInteger(mem) || mem < 16 || mem > 65536) {
      errors.mem_limit_mb = "Enter a whole number from 16 to 65536 MiB.";
    }
  }
  if (hasCpuLimit) {
    const cpu = Number(cpuLimit);
    if (!Number.isInteger(cpu) || cpu < 1 || cpu > 6400) {
      errors.cpu_quota_pct = "Enter a whole number from 1 to 6400 percent.";
    }
  }
  if (runtime === "proc" && (hasMemLimit || hasCpuLimit)) {
    errors.runtime = "Resource limits require systemd; choose systemd or clear both limits.";
  }
  return errors;
}

// Blocking errors keyed by payload field. Ranges and kind/runtime constraints
// mirror the API so review never knowingly submits an invalid draft.
export function stepErrors(draft, step) {
  const errors = {};
  if (step === 0) {
    if (!KINDS.some((k) => k.id === draft.kind)) errors.kind = "Choose a workload type.";
    return errors;
  }
  if (step === 1) {
    const name = draft.name.trim();
    if (!name) errors.name = "Name is required.";
    else if (name.length > 64 || !/^[a-z0-9-]+$/.test(name) || name.startsWith("-") || name.endsWith("-")) {
      errors.name = "Use up to 64 lowercase letters, digits or dashes; do not start or end with a dash.";
    }
    if (draft.kind === "static") {
      if (!draft.publish_dir.trim()) errors.publish_dir = "A source directory is required for a static site.";
    } else if (draft.launchMode === "command") {
      const { error } = validateCommandLines(draft.command);
      if (error) errors.command = error;
      else if (draft.args.trim()) {
        errors.args = "Arguments cannot be combined with an explicit command; put them on their own lines above.";
      }
    } else if (!draft.binary_path.trim()) {
      errors.binary_path = "A prebuilt binary path is required.";
    }
    if (draft.kind !== "worker") {
      const port = Number(draft.port);
      if (!Number.isInteger(port) || port < 1 || port > 65535) {
        errors.port = "Enter a whole-number port from 1 to 65535.";
      }
    }
    return errors;
  }
  if (step === 2) {
    if (!draft.server_id) errors.server_id = "Choose a server.";
    Object.assign(errors, resourceLimitErrors(draft.runtime, draft.mem_limit_mb, draft.cpu_quota_pct));
  }
  return errors;
}

// First blocking error for callers that only need a single summary message.
export function stepError(draft, step) {
  return Object.values(stepErrors(draft, step))[0] || null;
}

// API payload for POST /apps. Only sends fields that apply to the workload.
export function buildPayload(draft) {
  const kind = draft.kind;
  const payload = { kind, name: draft.name.trim() };
  if (draft.description.trim()) payload.description = draft.description.trim();
  const useCommand = kind !== "static" && draft.launchMode === "command";
  if (kind === "static") {
    payload.publish_dir = draft.publish_dir.trim();
  } else if (useCommand) {
    const { lines } = validateCommandLines(draft.command);
    payload.command = lines;
  } else {
    payload.binary_path = draft.binary_path.trim();
  }
  if (!useCommand && draft.args.trim()) payload.args = draft.args.trim();
  if (draft.workdir.trim()) payload.workdir = draft.workdir.trim();
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
  const useCommand = draft.kind !== "static" && draft.launchMode === "command";
  process.push(["Launch", useCommand ? "Explicit argv command" : draft.kind === "static" ? "Built-in file server" : "Prebuilt binary"]);
  if (draft.kind === "static") process.push(["Source directory", draft.publish_dir.trim() || "—"]);
  else if (useCommand) {
    const { lines } = validateCommandLines(draft.command);
    process.push(["Command", lines.length > 0 ? lines.join(" ") : "—"]);
  } else process.push(["Binary", draft.binary_path.trim() || "—"]);
  if (!useCommand && draft.args.trim()) process.push(["Arguments", draft.args.trim()]);
  if (draft.workdir.trim()) process.push(["Working directory", draft.workdir.trim()]);
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
export function createConsequences(kind, domain = "") {
  const notes = ["Creating only records the app — it starts as stopped and is not reachable until you deploy."];
  if (kind === "static") {
    notes.push(domain
      ? "On deploy, turaes serves the files and routes this domain — no build runs."
      : "On deploy, turaes serves the files; no public domain route is configured and no build runs.");
  } else if (kind === "worker") {
    notes.push("On deploy, turaes installs the binary and starts the process; there is no port or route to swap.");
  } else {
    notes.push(domain
      ? "On deploy, turaes installs the binary, starts the process, then routes this domain once a health check passes."
      : "On deploy, turaes installs the binary and health-checks the process; no public domain route is configured.");
  }
  return notes;
}

// Consequences shown when confirming lifecycle actions.
export function deployConsequences(app) {
  if (app && app.kind === "static") {
    const route = app.domain
      ? "Checks the new file server, then moves the configured domain route."
      : "Starts the new file server; no public domain route is configured.";
    return ["Copies the source directory into the inactive slot and starts the built-in file server.", route,
      "The previous slot remains available for rollback."];
  }
  if (app && app.kind === "worker") {
    return [
      "Starts the new worker in the inactive slot; workers have no HTTP port or health check.",
      "After the new worker starts, the previous slot is stopped and retained for rollback.",
    ];
  }
  const launch = app && app.command
    ? "Starts the configured argv in a new slot; the executable runs in place and is not copied."
    : "Copies the current binary into the inactive slot and starts it on the paired port.";
  const route = app && app.domain
    ? "Moves the domain route only after its health check passes; the previous process keeps serving meanwhile."
    : "Health-checks the new process before replacing the previous slot.";
  return [launch, route, "The previous slot remains available for rollback."];
}

export function restartConsequences(app) {
  if (app && app.kind === "worker") {
    return ["Stops and starts the active worker in place.", "Background processing pauses briefly during the restart."];
  }
  return [
    "Restarts the active process on the same port.",
    "Expect a brief interruption while it comes back up.",
  ];
}

export function stopConsequences(app) {
  if (app && app.kind === "worker") return ["Stops the background process; there is no HTTP route."];
  return [
    "Stops the process. Its configured route remains, but requests fail until the app starts again.",
  ];
}

export function startConsequences(app) {
  if (app && app.kind === "worker") return ["Starts the background process; there is no HTTP route or health check."];
  return [
    "Starts the process on its active port. The configured route remains in place.",
    "Requests can succeed once the process is listening and ready.",
  ];
}

export function lifecycleSummary(app) {
  if (app.kind === "worker") return "Deploy starts a replacement worker; restart pauses background work briefly. Workers have no HTTP route.";
  if (app.kind === "static") {
    const route = app.domain
      ? "switches the configured route after the file server is ready"
      : "starts a new slot without a public route";
    return `Deploy syncs files into a new slot and ${route}. Restart briefly interrupts the active file server.`;
  }
  const route = app.domain ? "switching the configured route" : "replacing the active slot";
  return `Deploy health-checks a new process before ${route}. Restart briefly interrupts the active process; Stop leaves the route configured but requests fail until Start.`;
}

export function rollbackConsequences(app, hash) {
  if (app && app.kind === "static") {
    return [app.domain
      ? "Switches the configured domain route back to the previous static slot and its files."
      : "Switches the active static version back to the previous retained slot; no public route is configured."];
  }
  if (app && app.kind === "worker") {
    return [
      `Starts the previous worker version${hash ? ` (${hash})` : ""} in the inactive slot; no HTTP health check is used.`,
      "Stops the current worker after the replacement starts.",
    ];
  }
  return [
    hash
      ? `Deploys build ${hash} to the inactive slot, health-checks it, then switches traffic.`
      : "Deploys the previous build to the inactive slot, health-checks it, then switches traffic.",
    "Keeps the currently active slot serving if the replacement fails its health check.",
  ];
}
