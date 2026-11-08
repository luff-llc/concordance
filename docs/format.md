# Native product catalogs

`concordance` describes product behavior once so each platform can implement it
with native navigation, controls, accessibility, and input conventions. A catalog is
a contract for outcomes and interactions, not a shared UI framework or a collection
of screenshots. Apps do not need to parse it at runtime.

The Rust library exposes serde types, `Catalog::load`, `validate_paths`, `entry`,
`brief`, `next`, `report`, `identifiers`, and `journeys`. The binary provides the same workflow to implementing agents and CI.

```sh
cargo install --path .
concordance --help
```

## Files and configuration

Pass the directory containing `catalog.toml`. Entries live in `catalog/*.yaml` or,
when there is no `catalog/` subdirectory, in `*.yaml` beside the configuration.
`.yml` also works. Each file is a nonempty array; its filename defines its area.
Area names use lowercase ASCII letters, numbers, and hyphens. Moving an entry
between area files does not change its stable ID.

```text
spec/product/
  catalog.toml
  catalog/library.yaml
  catalog/reader.yaml
  retired-ids.json       # optional
  parity.md             # generated
```

```toml
# Relative to this catalog directory. Defaults to ".".
repo_root = "../.."

# Each root is relative to repo_root (or validate --repo).
[platforms]
ios = "apps/apple"
android = "apps/android"

[report]
title = "Reader native parity"
baseline = "ios"
target = "android"
area_order = ["library", "reader"]

[report.labels]
ios = "iOS"
android = "Android"
```

Platform names are arbitrary: web, desktop, or specialized device variants can use
the same schema and statuses. A platform in an entry must have a configured root.
`repo_root` can traverse upward to a common checkout parent; individual platform
roots and evidence paths must remain within that parent and platform root,
respectively. Absolute evidence paths, parent traversal, Windows drive/backslash
paths, and symlink escapes are rejected. `validate --repo <root>` overrides
`repo_root` without editing the catalog.

Source paths are relative to their platform root. For a legacy catalog whose paths
carry a checkout prefix, configure the prefix to strip, including its trailing slash:

```toml
[path_prefixes]
ios = "apple-reference/"
```

All iOS source and file-based test references must then start with that prefix.
This mapping does not rewrite the stored catalog. To use several sibling repositories,
set `--repo` to their common parent and map platform roots to the sibling names.

## Entry format

The bundled [JSON Schema](../crates/concordance/schema/catalog.schema.json)
is authoritative for area files. Ordinary YAML and JSON-subset YAML are accepted;
duplicate mapping keys, unknown fields, and invalid types are rejected. There is
no catalog-local schema override. Validation uses a small evaluator limited to the
keywords in the bundled schema, avoiding a general JSON Schema dependency.

```yaml
- id: reader.open-book
  title: Open a book
  summary: Open the selected book at its saved position.
  surfaces: [library, search]
  interactions:
    - trigger: Tap a book
      result: The native reader opens at its saved position.
  states: [ready, empty, unavailable]
  settings: []
  data:
    reads: [book, reading position]
    writes: []
  copy: [Open, Book unavailable]
  accessibility:
    label: Open book
    identifier: reader.open
    notes: [Announce the selected book title.]
  design: []
  depends_on: [library.open-library]
  impact: high
  priority: 10
  platforms:
    ios:
      status: shipped
      source: [Screens/Reader.swift]
      tests: [Tests/ReaderTests.swift#ReaderTests.testOpen]
    android:
      status: partial
      source: [src/main/Reader.kt]
      tests: []
      gap: Restore the saved reading position.
  acceptance:
    - given: A book with a saved reading position
      when: The reader taps that book
      then: Its saved position appears and is announced.
  notes: []
```

Every referenced dependency must exist somewhere in the catalog. Stable IDs match
`area.verb-object` using lowercase ASCII letters, numbers, and hyphens, with one
dot and a letter starting each component. They are unique across files. Never
reuse an ID for a different behavior. Preserve retired identities in an optional
`retired-ids.json` mapping:

```json
{
  "reader.old-open": {
    "replaced_by": ["reader.open-book"],
    "reason": "Merged into the canonical opening journey."
  }
}
```

Retired IDs cannot be active, and replacement IDs must resolve. Historical reuse
can only be detected for IDs actually recorded in this file.

`impact` is `critical`, `high`, `medium`, or `low`. Optional `priority` is an integer
from 1 through 1000, with lower numbers first. The priority of remaining work can
differ from the importance of the entire feature.

Platform status is always one of `shipped`, `partial`, `missing`, or
`not-applicable`. `source`, `tests`, `gap`, and `notes` are optional on a platform
record, allowing a device variant to record status before evidence is collected.
Unrecorded platforms are not inferred to be shipped or missing. Include explicit
statuses for every entry on platforms whose work queue or report you maintain.
A useful catalog links source/tests for shipped or partial implementations and
explains partial/missing work in `gap`; the schema does not enforce this editorial
judgment. Missing gaps are visible as “No gap recorded.” in reports.

Acceptance has at least one nonempty given/when/then journey. Interactions, states,
and surfaces are also nonempty. Empty lists are allowed for settings, data reads
and writes, copy, design, and dependency lists. Accessibility label/identifier may
be empty to record the absence of established source evidence.

## Agent workflow

```sh
concordance validate spec/product --repo .
concordance next spec/product --platform android --area reader --limit 5
concordance entry spec/product reader.open-book --format brief
concordance entry spec/product reader.open-book --format yaml
concordance entry spec/product reader.open-book --format json
```

Start with `next`, then read the implementation brief and the linked source. A brief
includes behavior, surfaces, interactions, states, settings, data, exact copy,
accessibility, acceptance, dependency statuses, implementation gaps, iOS source
paths, test references, design, and notes. Implement the actual native interactions
and failure/recovery behavior; add tests before advancing the catalog status.

`next` returns tab-separated ID, impact, status, and title. It selects missing and
partial entries. Dependencies always precede their dependents; among currently
ready work it orders by impact, priority (omitted means 1000), configured area
order, then ID. A low-impact prerequisite can therefore precede a critical feature.
An area filter includes unfinished prerequisites from other areas, and `--limit`
counts those prerequisites too. Shipped/not-applicable dependencies are treated as
complete. An unrecorded dependency status is an error. The default limit is 10;
zero emits no rows. Unknown platforms and areas are errors. All commands reject
unresolved dependencies and cycles before producing output.

`validate` additionally checks evidence files. `path#Suite.method` verifies the file,
not the declaration or assertions. Plain file paths, even without `#`, are checked.
Bare test-runner labels without a slash, dot, or `#` remain permitted and are not
file-checked. Source evidence is always a file path. Validation is not a build,
a test run, a reachability audit, or proof of complete feature coverage.

Other commands perform structural/identity/dependency validation but do not require
native source checkouts. In CI, run `validate` before `report --check` when evidence
checkouts are available.

## Parity reports

```sh
concordance report spec/product
concordance report spec/product --check
concordance report spec/product --out spec/product/parity.md
```

The default output is `<dir>/parity.md`; explicit output paths are relative to the
current working directory. `--check` compares exact contents, exits nonzero for a
missing/stale file, and never writes it. Report links are relative to the catalog
directory, so keep the output beside `catalog.toml` for working area links.

Reports count entries shipped on the configured baseline, tally target statuses by
area, list the ten highest-impact gaps, list missing and partial capabilities, and
expose test-evidence gaps. Areas are alphabetic; gap ordering uses the same impact,
priority, area, and ID ranking as the work queue, without dependency reordering.
Nonbaseline adaptations are excluded from parity totals. Partial/missing baseline
entries appear separately as outside the shipped baseline. A target
`not-applicable` status gets an explicit count column when present.

Projects migrating an existing report can also configure these `[report]` fields:

| Field | Meaning |
| --- | --- |
| `intro` | Introductory Markdown, including baseline scope and audit limits. |
| `ordering_note` | Prose describing the project's impact and area ranking. |
| `evidence_note` | One evidence-limit bullet about native implementations. |
| `adaptation_platform` | Optional secondary status shown on adaptation rows. |
| `labels` | Map of platform keys to display labels. |

No project prose or area names are hard-coded into the crate. The generator credit
always names `concordance report`.

## Native identifiers and journeys

```sh
concordance ids spec/product --platform kotlin --package org.example.reader --out CatalogIds.kt
concordance ids spec/product --platform swift --out CatalogIds.swift
concordance journeys spec/product --platform kotlin --out android/catalog-tests
concordance journeys spec/product --platform swift --out apple/CatalogTests
```

`ids` writes `CatalogIds` with one constant per stable entry ID. Values preserve
`accessibility.identifier` exactly when nonempty, including duplicates shared by
several capabilities. Otherwise the stable entry ID becomes a proposed identifier,
with a comment noting that it has not been established in source. This keeps all
entries available to UI and tests without pretending the app already uses them.
For example `CatalogIds.entry_reader_d_open_h_book` holds `"reader.open"`.
Use it with Compose `Modifier.testTag(...)` or SwiftUI
`.accessibilityIdentifier(...)` and the corresponding native test query.

Symbols start with `entry_`; dots encode as `_d_` and hyphens as `_h_`. Since valid
stable IDs contain no underscores, this is deterministic and collision-free, even
for IDs such as `a.b-c` and `a-b.c`. Both languages use the same strings; escaping
preserves literal quotes, backslashes, interpolation markers, and control characters.
`--package` is Kotlin-only. ID outputs are generated files and are overwritten on
explicit regeneration. Parent directories for ID/report files must already exist.

`journeys` creates one native test class/file per entry, one test per acceptance
journey, and given/when/then comments. Kotlin uses a Compose test rule, JUnit `@Test`
and `@Ignore`, and a `TODO` body. Swift uses XCTest and throws `XCTSkip` with a TODO
message. These skeletons cannot silently count as successful acceptance tests.
Add them to the app's test target and supply app startup, fixtures, native actions,
and assertions; then remove the ignore/skip and TODO.

Journey writes use exclusive creation. **Existing files are never overwritten**,
even if they still look generated or the acceptance text has changed. Reruns report
created, unchanged, and preserved counts; differing existing paths go to stderr.
Review changed acceptance manually or generate into a fresh directory and compare.
Removed entries do not delete existing tests. Generated native tests require the
app's normal Compose/JUnit or XCTest dependencies and test runner configuration.
