# Changelog

## 0.4.0-dev

## 0.3.0 (2026-11-25)

- Surfaces: a features × platforms × surfaces dimension. `[surfaces.<id>]` declares
  system surfaces (widget sizes, Lock Screen, controls, quick actions, app shortcuts,
  watch apps, complications, Wear OS tiles, the car, the menu bar, Live Activities,
  notifications) and the platforms that can have each. Entries opt in by naming a
  declared ID in `surfaces`; platform records carry `surfaces.<id>` records with the
  usual status, evidence, verification and blockers, plus `declared` IDs. An unrecorded
  surface counts as a gap. Catalogs and entries without surfaces are unchanged.
- `report` and `ui-report` add a Surfaces table and gap list; `next` queues surface gaps
  (`--surface <id>`, `--no-surfaces`); `gate-release` fails on stale surface records past
  their limit and on gaps on `release = "required"` surfaces; `set` and `sync` address
  surface records as `entry@surface`.
- `concordance surfaces scan` reads what each app declares, without building it:
  WidgetKit widgets and families, controls, Live Activities, `AppShortcutsProvider`,
  `MenuBarExtra`, CarPlay scene delegates, Info.plist quick actions, watch apps, CarPlay
  scenes and notification extensions; Android widget receivers with their
  `appwidget-provider` cell sizes, `shortcuts.xml` shortcuts and App Actions, Android Auto
  descriptors, media library services, Quick Settings tiles, Wear OS apps, tiles and
  complications. It lists uncatalogued, unmapped and no-longer-declared surfaces.
  Parsers are configured per catalog in `[surface_scan.<platform>]`.
- `gate` fails a change that adds a declared surface the catalog does not cover, or
  removes one a record still claims.

- Collect recoverable validation errors across documents, identities, dependency
  graphs, retirement records, and evidence paths, with structured library reports.
- Resolve evidence in independent checkouts using repeatable platform roots or
  environment overrides, with optional external-root hints and strict root checks.
- Track Git baseline drift by entry and uncatalogued source globs; append baseline
  drift to behavior and UI parity reports when the pinned checkout is available.
- Preserve catalog path safety and existing Result-based library entry points.
- Separate implemented from verified: optional `implemented_by` and `verification`
  records on platform statuses; `verified` is derived (implemented, a capture or
  journey, and a reviewer other than the implementer). `implemented` is a synonym
  for `shipped`. Reports show implemented/verified/partial/missing per area and a
  claimed-shipped-but-unverified list. Report tables gain a Verified column.
- Detail items on entries (`motion`, `haptic`, `gesture`, `geometry`, `copy`, `state`,
  `accessibility`, `layout`, `other`) with structured numbers, baseline evidence and
  per-platform status; detail counts and detail gaps in reports; `--details` on
  `next` and `entry --format brief`.
- Evidence anchors `path#Symbol`, `path:Lstart-Lend` and `path:line`; `drift` matches
  them against `git diff -U0` hunks (symbol blocks by a documented brace/indent
  heuristic), labels file-level references as coarse, and counts precise versus
  coarse hits.
- Capture coverage (required, produced, skipped via `.skip` markers, missing) per
  platform, area and screen in `capture-plan --coverage` and `ui-report`; skipped or
  missing captures withdraw verification. Captures may live in `<platform>/`
  subdirectories.
- `compare` writes side-by-side `pairs/<screen>.<state>.png` images (no new
  dependencies), a review form, and a `review.json` template; `concordance verify
  --review` turns reviewer verdicts into a proposed verification patch without
  editing the catalog; `concordance review set` edits `review.json` from a terminal.
- The tool writes `implemented`; `shipped` remains a deprecated read alias.
  `verification` requires `implemented_by`. Entries may declare `verify_requires`;
  by default state-changing entries (non-empty `data.writes`) need a capture and a
  journey, visual ones a capture. `verify` proposes `incomplete` when unmet.
- Capture-run manifests (`manifest.json`: produced/skipped with reasons, device,
  commit, fixture pack). `compare`, coverage and `ui-report` warn when fixture packs
  differ.
- Report area and README links are relative to the output file's directory.
- Blocker registry (`[blockers.<id>]`) and `blocked_by` on platform and detail records;
  `next` skips blocked work (`--include-blocked`), reports split actionable and blocked
  gaps and rank blockers by what they hold back.
- `concordance set` edits statuses and bookkeeping in place, preserving key order and
  JSON-style formatting, and clears stale fields; it never writes verification.
- Validation warnings (`--strict` to fail): implemented records with a gap or open
  details, resolved blockers still referenced, unknown motion tokens.
- Detail `geometry` lists, `motion.token`, detail `verify_requires` (journey by default
  for motion, haptic and gesture). The schema evaluator supports `anyOf`.
- `concordance rebase-anchors --to <sha>` moves line anchors through the diff and
  flags edited ranges and lost symbols; `--write` applies the moves.
- Equivalence tolerances: `[ui.tolerance] position_dp`, `size_ratio`, `radius_dp` (per-screen
  overrides) judge cross-platform geometry in logical units, recording passing deltas as
  measurements; snapshots may report `corner_radius`.
- Review verdicts `platform` (accepted difference with a category from `[ui]
  platform_categories` and a note) and `fixture` (capture-tooling difference with a note).
  `verify` treats same and platform as equivalent and reports fixture pairs as tooling
  debt; `review set --category`; `ui-report --review` lists tooling debt.
- Continuous parity: a `stale` status with `previous_status`, `since_commit` and
  `since_date`; `gate` (commit range or patch plus message file; user-visible and
  string-catalog globs in `[source_globs.<platform>]`; `Catalog: none — <reason>`
  trailer; exit 0/1/2); `sync` (stale marking, task files, optional `gh` issues, anchor
  rebase and pin advance; dry run unless `--apply`; idempotent); `gate-release` with
  per-kind age limits; `set --intended`.
- `sync` marks stale only on precise hits (anchors, or changed copy text/keys in code or
  string catalogs) and queues file-level hits as review tasks (`--coarse-as stale|review`,
  default review); `set --stale` and `set --reviewed-unchanged` (`reviewed_commit`) triage
  them. `anchors suggest` proposes `#Symbol` anchors for file-level evidence.
- Setting a non-stale status on a stale record clears `previous_status`, `since_commit`
  and `since_date`, and with `--tasks-dir` notes the closure (date, `--closing-commit` or
  the platform root's HEAD) in the task file; `--clear` accepts those fields.
- Shared marketing version: `[version] source` plus per-platform `file`/`pattern`,
  `command` (with timeout) or `reads_source` checks; `version-check` (exit 0/1/2) and the
  same check inside `gate-release` (`--skip-version`). Adds the `regex` dependency.
- `strings export` writes Android `strings_<app>_shared.xml` resources (with locales)
  for copy-detail keys from an `.xcstrings` file, with documented deterministic resource
  names; `copy.key` on copy details.
- Platform inheritance: `horizon = { root = "...", inherits = "android" }`; an
  inheriting record unsets a parent key with an explicit `null`.

## 0.2.0

- Native behavior catalogs, evidence validation, parity reports, and work queues.
- Optional UI contracts, design tokens, capture plans, and capture comparisons.
