# Changelog

## 0.3.0-dev

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
  editing the catalog.

## 0.2.0

- Native behavior catalogs, evidence validation, parity reports, and work queues.
- Optional UI contracts, design tokens, capture plans, and capture comparisons.
