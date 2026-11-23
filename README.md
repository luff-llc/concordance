# Concordance

Concordance describes what a product does, once, so every platform can build it natively
and prove it.

Optional UI contracts add shared design tokens and screen anatomy. Each platform
uses native components; capture comparison checks visual structure and token use.
See [the UI workflow](docs/ui.md).

A concordance is the index of every word in a book; the word also means agreement.
This tool is both: a catalog of every feature and interaction in an app, and a check that
each platform's native implementation agrees with it.

It is not a cross-platform UI framework. Each entry is a behaviour contract: how a person
triggers the feature, what happens, its states, settings, copy, accessibility identifier,
the data it reads and writes, and acceptance journeys in given/when/then form, plus each
platform's status and the source and tests that prove it. Implementing agents read
entries; UI tests on every platform share the same identifiers and journeys; reports show
exactly what is missing where.

```sh
cargo install --path .
concordance validate path/to/catalog
concordance validate path/to/catalog --root ios=../apple-reference --require-roots
concordance report path/to/catalog --out parity.md
concordance drift path/to/catalog --root ios=../apple-reference --platform ios
concordance next path/to/catalog --platform android --limit 10 --details
concordance entry path/to/catalog player.skip-forward --format brief --details
concordance ids path/to/catalog --platform kotlin --out Ids.kt --package org.example
concordance journeys path/to/catalog --platform swift --out Tests/Journeys
concordance tokens path/to/catalog --platform swift --out DesignTokens.swift
concordance tokens path/to/catalog --platform css --out tokens.css
concordance capture-plan path/to/catalog --platform android
concordance capture-plan path/to/catalog --platform ios,android --coverage --captures captures
concordance compare path/to/catalog --captures captures --out ui-report --check
concordance set path/to/catalog player.skip-forward --platform android --status implemented --by lane-1 --clear gap
concordance rebase-anchors path/to/catalog --root ios=../apple-reference --to <sha>
concordance gate path/to/catalog --platform ios --base <sha> --head <sha>
concordance sync path/to/catalog --root ios=../apple-reference --platform-from ios --platform-to android --tasks-dir tasks --apply
concordance gate-release path/to/catalog --platform android --max-age-days 7 --max-age-days-for copy=0
concordance strings export path/to/catalog --from Localizable.xcstrings --keys-from-catalog --platform android --out app/src/main/res
concordance review set ui-report detail.dark --verdict differs --note "Title wraps"
concordance verify path/to/catalog --review ui-report/review.json --out proposal.yaml
concordance ui-report path/to/catalog --captures captures
concordance track path/to/catalog --check
concordance track path/to/catalog --format json --today 2026-11-22 --out tracker.json
concordance suite suite.toml --out suite.md
```

To adopt Concordance in a repository, run `concordance init` (it detects platforms and
writes `spec/<app>/` with `catalog.toml`, an empty `catalog/`, `tracker/` and an agent kit),
then `concordance inventory spec/<app> --uncatalogued` and `concordance plan-audit
spec/<app>` for per-area audit briefs; `concordance guide spec/<app>` prints the agent kit.
See [Adopting](docs/adopt.md).

Validation collects schema, identity, dependency, retirement, and evidence problems in
one sorted report. External checkouts can be supplied with repeatable
`--root platform=path` options or `CONCORDANCE_ROOT_<PLATFORM>` environment variables.
Declared external roots are optional unless `--require-roots` is set. Git baseline
pins let `drift` identify entries whose evidence changed and uncatalogued source
changes; parity reports also flag baseline drift when the checkout is available.

Status records what is `implemented` (`shipped` is a deprecated alias); an optional `verification` record (reviewer,
date, commits, captures, journeys) records independent proof, and `verified` is
derived from it. Reports show implemented and verified counts separately and list
claimed-but-unverified work. Entries can carry `details` (motion, haptic, gesture,
geometry, copy...) with numbers and per-platform status, so briefs carry the numbers.
Blocked work names a registered blocker so `next` skips it and reports rank blockers;
flavour platforms can inherit another platform's records.
Evidence may be anchored (`path#Symbol`, preferred, or `path:L10-L20`) so `drift` reports precise
hits rather than whole files. Capture coverage counts produced, skipped and missing
captures per area and screen, and `compare` writes side-by-side pair images and a
review page whose notes `verify` turns into a proposed patch, never an edit.

Optional `tracker/releases.yaml` and `tracker/issues.yaml` add releases, bug reports
and feature requests that point at catalog entries; `track` writes them into one
deterministic `tracker.md` (or JSON for boards), and `suite` joins several apps' catalogs
with shared features and dated beats. See [docs/tracker.md](docs/tracker.md).

The format is in [docs/format.md](docs/format.md) and the JSON Schema in
[schema/catalog.schema.json](schema/catalog.schema.json).

Concordance is a sibling of [Colophon](https://github.com/luff-llc/colophon), LUFF's
open standard for user-owned, end-to-end encrypted app data. Colophon keeps every
platform's data in agreement; Concordance keeps every platform's behaviour in agreement.

Apache-2.0.
