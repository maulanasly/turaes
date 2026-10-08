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
- Palette: graphite dark surfaces, warm-paper light surfaces (`#fef9ef`);
  lagoon primary (`#227c9d`, `#1f7594` on light), teal links/CPU/visits
  (`#17c3b2`, darkened `#0e6b7a`/`#0b6b52` on light), sand warnings
  (`#ffcb77` dark / `#8a4b08` light), coral danger (`#fe6d73` dark / `#c62828`
  light), green ok kept, purple mem kept for CPU/MEM separation.
- Brand lockup: icon-only theme-specific SVG mark from `turaes-logo-assets`
  (teal `#17c3b2` + cream `#fef9ef` on dark; lagoon `#227c9d` on light) +
  lowercase "turaes" wordmark; square PNG favicon/touch sizes per variant
  (`16/32/48` verbatim, `192` downscaled from `favicon-256`, `512` from the
  themed mark render) plus `favicon.ico`. Repeatable via
  `scripts/sync-brand-assets.sh`. Hero copy is "Deploy apps as native
  processes. Operate the whole fleet." with "No containers." as proof.
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
| destructive verbs | **Delete** removes a whole app; **Remove** takes a member out of a collection (env var, domain, server, org member); **Revoke** invalidates a token; **Roll back** redeploys a previous build |
| "Primary domain" vs "Domain aliases" | primary is set in Settings; aliases route in addition (hint text in both places) |
| "Requests observed" | "Visits observed" (the series counts `visitors_total`, not requests) |
| env "Updated" column | "Created" (only the creation timestamp exists) |

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
| **UX-P2** | Servers list + detail (agent version/last-seen), placement select, Validate/Bootstrap UI, lifecycle (stop/start/restart/delete/edit), domain aliases | ✅ |
| **UX-P3** | search/filter/sort, responsive, favicon/theme-color, build-id detail | ✅ |
| **UX-P3 guided forms** | stepped create wizard (workload → process → placement → review), review-before-create summary, plain-language native-process constraints, field-linked API errors, workload-specific lifecycle consequences, manifest-managed launch settings labeled explicitly | ✅ |
| **UX-P5 workflow preflight** | server preflight on Review + recheck before submit, multi-error inline mapping, advanced launch (argv/workdir) in wizard and Settings with tri-state PATCH parity, queued-deployment polling, dashboard/manifest launch parity | ✅ |
| **UX-P4 mobile & a11y** | nav disclosure focus/Escape, scrollable key-value tables, route/tab focus restore, toast live-region severity, skeleton `role=status`, 44px tap targets, AA contrast tokens enforced by `tests/frontend/contrast.test.mjs` | ✅ |
| **UX-1 control room** | indigo primary actions (cyan reserved for links/telemetry), amber warnings, identity typography + status strips/resource rows/timelines, global org switcher + fleet summary + labeled health, grouped nav (Operate/Access/System), app attention filter + server grouping, app header with last deploy + grouped tabs + activity preview, server fleet strip, guided create form | ✅ |
| **UX-2 polish** | mobile nav disclosure, 44px coarse-pointer targets, skip link, focus-into-view on route change | ✅ |
| **UX-3 responsive tables** | stacked resource rows (`data-label`) for all data tables under 640px, headers kept for AT | ✅ |
| **UX-6 nav clarity** | grouped nav clusters with labeled sections + dividers, permission-aware Servers/Tokens links, fleet summary as Servers link with unavailable state, mobile menu ordered below brand row | ✅ |
| **UX-7 nav flattening** | flat primary nav (Applications + Servers, alert badge) with Organization/Tokens/About in an avatar account menu; single fleet status pill (API-down wins); org switcher with labeled `slug — role`; SVG theme icon with `aria-pressed`; explicit nav hover/active; controls stay on the top row on mobile | 🔄 |

## Parked backlog (deferred deliberately, not forgotten)

| Item | Why parked | Unblocks |
|---|---|---|
| ~~Server-capacity telemetry (per-server CPU/mem in fleet view)~~ | **done** — `server_metrics` + fleet/detail views (PR #83) | — |
| Cursor pagination (`?limit`/`?cursor` on all lists) | backend API change; current caps suffice at this scale | API milestone |
| ISO-8601 `Z` timestamp storage migration | UTC-naive TEXT across tables with string-compared rollups; string-munging handlers would fake a standard | storage migration PR |
| ~~Tap-target enlargement pass~~ | **done** — coarse-pointer ≥44px targets incl. toast dismiss + wizard steps | — |
| Mobile bottom navigation | drawer (UX-2) covers wayfinding; bottom nav is a bigger IA change | design review |

## Sign-in page

When unauthenticated the SPA renders a standalone **LoginView** (brand, "Sign
in with GitHub", optional error from `?login_error=`) with **no nav/menu**; OAuth
failures redirect back to it with a friendly message. The intended deep link is
restored after sign-in (sessionStorage).

## API additions (additive, no migrations)

- `PATCH /api/v1/apps/{id}` — edit fields + placement.
- `POST /api/v1/apps/{id}/{stop,start,restart}` — lifecycle.
- `POST /api/v1/apps/{id}/rollback` body `{artifact_hash?}` — rollback target.
- `GET/POST/DELETE /api/v1/apps/{id}/domains` — aliases (table + routing exist).
