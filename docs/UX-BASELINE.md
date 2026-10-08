# UX intuitiveness baseline (pre-Hairline)

Why this exists: before adding illustrative figures to the dashboard, record
how intuitive the current text-only UI is, with numbers we can re-take after
each figure lands. A figure ships/keeps its place iff its number moves.
Companion to [UX-REVIEW.md](./UX-REVIEW.md) (design history) — this doc is
measurement, not redesign.

Base commit: `docs/ux-baseline` worktree off `origin/main`.
Control screenshots: `docs/ux-baseline-shots/` (Chrome headless, 1280 + 390px).

## Reproduce the fixture

```bash
cargo build
rm -f /tmp/turaes-ux/db/turaes.db
env AUTH_DISABLED=1 \
  TURAES_BACKUP_DIR=/tmp/turaes-ux/backups \
  TURAES_STATE_DIR=/tmp/turaes-ux/state \
  TURAES_ARTIFACT_DIR=/tmp/turaes-ux/artifacts \
  TURAES_DATABASE_URL="sqlite:///tmp/turaes-ux/db/turaes.db?mode=rwc" \
  TURAES_RUNTIME_DRIVER=proc \
  TURAES_MONITOR_INTERVAL_SECS=86400 \
  ./target/debug/turaes serve
sqlite3 /tmp/turaes-ux/db/turaes.db < scripts/ux-baseline-seed.sql
# open http://localhost:8787 (dev user, no login)
```

`TURAES_MONITOR_INTERVAL_SECS=86400` freezes the health loop so seeded
`failed`/`deploying` rows survive the session (default 15s flips them to
`unhealthy` — observed while capturing the control shots).

Seed (`scripts/ux-baseline-seed.sql`): `beruang` running, `monthly-logs`
failed, `pdf-gen` deploying, `uangku` stopped worker; beruang has one
finished + one queued deployment. A second clean profile (empty DB, no seed)
covers the first-run tasks.

## Static audit (no users)

### Text-only state inventory (the control set)

| # | State | Location | Screenshot |
|---|-------|----------|------------|
| 1 | No applications yet | `static/js/views/AppsView.js:491` | `01-*, 08-*` |
| 2 | No applications match filter | `AppsView.js:493` | — (filter live) |
| 3 | Apps list, mixed statuses | `AppsView.js:15-30` | `02-*, 03-*` |
| 4 | Overview, zero-sample charts + `—` KPIs | `AppDetailView.js:99-138`, `Chart.js:27-32` | `04-*` |
| 5 | Deployments, queued + finished rows | `AppDetailView.js:155-194` | `05-*` |
| 6 | Wizard step 0, kind cards + callout | `AppsView.js:207-218` | `06-*` |
| 7 | Servers list, local online | `ServersView.js` | `07-*` |
| 8 | No servers / no tokens / no activity rows | `ServersView.js:173`, `TokensView.js:93`, `AppDetailView.js:32` | — (same pattern as 1) |
| 9 | 404 view | `static/js/app.js:329-336` | — |
| 10 | Loading skeletons | `Skeleton.js`, `AppsView.js:489` | — (transient) |

### Badge audit (status legibility input)

Six statuses, text + glyph + color (`StatusBadge.js:3-28`):
`Running ●`, `Stopped ■`, `Unhealthy ▲`, `Failed ✕`, `Deploying ◐`, `Unknown ?`.
Glyph shapes differ (no color-only encoding). Text contrast AA-enforced by
`tests/frontend/contrast.test.mjs` (4/4 green at baseline: ok/warn/bad/muted
on panel ≥ 4.5:1, both themes).
Known semantic wrinkle: deployment rows reuse app badges, so a *finished*
deploy reads `Running` and a queued one reads `?` (`05-*`). Recorded as
evidence for the Slots figure, not fixed here.

### Wizard audit (onboarding input)

4 steps (`Workload → Process → Placement → Review`, `appForm.js:45`).
Inputs per step: step 0 = 1 choice (3 kind cards + constraint callout, 3–4
bullets each); step 1 ≈ 5–7 fields (launch-mode dependent); step 2 = 6
(server, domain, runtime, auto-restart, 2 limits); step 3 = review + create.
Error paths: per-field inline + preflight warnings on Review with step jump.

## Task test (2–3 first-time users, same people return after)

Setup: seeded profile for T2–T4, clean profile for T1/T5. Unaided, observe +
time, then confidence 1–5. Plus a keyboard-only wizard pass (tab order, focus)
as regression guard for later.

| # | Track | Script | Record |
|---|-------|--------|--------|
| T1 | Onboarding | "Put this binary online with its own domain." | success / time / wrong kind? / questions asked |
| T2 | Status | "Which app needs attention right now, and what is it doing?" | correct app? correct status word? / time |
| T3 | Deployments | "A deploy just broke things. Get back to the previous version." | mentions previous slot? finds Roll back unaided? / time |
| T4 | Metrics | Show `04-*` state: "Is this broken?" | "broken" misread? finds hint text? |
| T5 | Empty fleet | Clean profile: "What do you do first?" | finds `+ New app` unaided? / time |

## Score tables (fill per round: before / after-PR1 / after-PRs)

### Round: ______ date: ______ participants: ______

| Task | P1 ok/time/conf | P2 ok/time/conf | P3 ok/time/conf | Misreads & quotes |
|------|-----------------|-----------------|-----------------|-------------------|
| T1 onboard | | | | |
| T2 status | | | | |
| T3 rollback | | | | |
| T4 metrics | | | | |
| T5 first action | | | | |

Keyboard pass: ______ (pass/issues). Expert-efficiency check (no added clicks
on existing flows): ______.

## Decision rules (agreed)

- Keep a figure iff its task improves with no regressions: error/misread rate
  drops (aim ≥ 50% relative, minimum strictly better, same participants) or
  task time drops, confidence not down, keyboard pass clean, `make verify` green.
- Abort rules: T2 ≈ 100% at baseline → cut Beacon's data-wiring (decorative
  only or drop). T1 kinds already correct → Kinds becomes polish, deprioritize.
- Regression guard: figures add zero clicks to existing flows (augment beside
  badges/buttons, never gates); text carriers (badges, hints) stay.

## Baseline quirks (do not fix in this branch)

- Monitor rewrites seeded statuses within ~15s on default config; sessions use
  `TURAES_MONITOR_INTERVAL_SECS=86400`.
- Finished deployments badge as `Running`; queued as `?` (`05-*`).
- Zero-data KPIs render `—` (ambiguous with real zero); charts explain, KPIs don't.
