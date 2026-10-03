# turaes dashboard — UX review & IA redesign

A principal-UX review of the turaes operator console, plus the agreed redesign.
Constraints are non-negotiable: **zero-build Preact/HTM, vendored, no npm/CDN,
embedded in the binary**.

## Verdict

Strong bones (tiny, dependency-free, good empty states, secrets never shown),
weak wayfinding and feedback. The console read as one long demo page: selection
wasn't linkable, async outcomes were mostly invisible, destructive/sensitive
actions lacked guardrails, and the multi-node model — the differentiator — was
barely surfaced.

## Locked decisions

- Audience: technical owner + semi-technical operator → plain language.
- Spine: **Applications → App detail (tabs)**; **Servers** a first-class peer.
- Multi-node first-class: server names, placement on create, per-server grouping,
  server detail with agent status, Bootstrap/Validate in-UI.
- Lifecycle in scope: stop/start/restart/delete/edit + domain aliases.
- Light mode now.
- Zero-build preserved.

## Terminology

| Before | After |
|---|---|
| "docker-less paas" | "Deploy your apps — no containers" |
| `runtime: systemd` | "Managed by systemd" |
| "Visits (per scrape)" | "Requests observed" |
| "Unique (latest)" | "Unique visitors (latest)" |
| `server_id: 4284…` | server **name** |
| "Set" | "Updated" |

## Information architecture

| Route | View |
|---|---|
| `#/apps` | Applications |
| `#/apps/:id/:tab` | App detail — `overview` / `deployments` / `environment` / `logs` / `settings` |
| `#/servers` | Servers |
| `#/servers/:id` | Server detail (agent status + its apps) |

Hand-rolled hash router (`lib/router.js`), deep-link/back/refresh safe.

## Interaction principles applied

- **Visibility of status:** toasts for every async outcome, "updated Xs ago",
  loading skeletons, health dot in the header.
- **User control:** deep links, Back, cancelable confirms, pausable polling
  (`document.hidden`).
- **Error prevention:** confirm dialog for rollback (names the target), delete
  app/env/domain; auth-gated management actions.
- **Recognition over recall:** server names not ids; runtime/placement as
  selects; copyable build ids.
- **A11y (WCAG 2.1 AA):** interactive rows as `<a>`/`<button>`, `:focus-visible`,
  `role="tablist"`, charts with `role="img"` + `<title>`/hidden table, status
  text+icon+colour, ≥12px text.

## Phases

| Phase | Scope | Status |
|---|---|---|
| **UX-P0** | router + shell + nav, toasts + confirm, server-name mapping, auth gating, keyboard/focus, loading infra, terminology, **light mode** | 🔄 |
| **UX-P1** | app detail tabs (Overview/Deployments/Environment/Logs/Settings), env editor, logs controls, chart a11y, timestamp consistency | 🔄 |
| **UX-P2** | Servers list + detail (agent version/last-seen), placement select, Validate/Bootstrap UI, lifecycle (stop/start/restart/delete/edit), domain aliases | ⏳ |
| **UX-P3** | search/filter/sort, responsive, favicon/theme-color, build-id detail | ⏳ |

## API additions (additive, no migrations)

- `PATCH /api/v1/apps/{id}` — edit fields + placement.
- `POST /api/v1/apps/{id}/{stop,start,restart}` — lifecycle.
- `POST /api/v1/apps/{id}/rollback` body `{artifact_hash?}` — rollback target.
- `GET/POST/DELETE /api/v1/apps/{id}/domains` — aliases (table + routing exist).
