# Native product catalogs

`concordance` describes product behavior once so each platform can implement it
with native navigation, controls, accessibility, and input conventions. A catalog is
a contract for outcomes and interactions, not a shared UI framework or a collection
of screenshots. Apps do not need to parse it at runtime.

Optional `tokens.yaml` and `screens/*.yaml` add design contracts and native capture
comparison. See [UI contracts](ui.md) for their schemas, commands and reviewer workflow.
`tokens.yaml` is reserved and excluded from flat behavior area discovery.

The Rust library exposes serde types, `Catalog::load`, `Catalog::validate`,
`ValidationReport`, `RootOptions`, `validate_evidence`, `drift`, `validate_paths`, `entry`,
`brief`/`brief_with`, `next`/`next_with`, `report`/`report_with`, `verification_gap`,
`split_anchor`, `identifiers`, and `journeys`. The binary provides the same workflow to implementing agents and CI.

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
This mapping does not rewrite the stored catalog. An explicitly supplied platform root
is the checkout directory **after** prefix stripping: `ios-reference/Sources/Player.swift`
with `--root ios=/checkouts/apple` resolves to `/checkouts/apple/Sources/Player.swift`.

### External checkouts

Declare a checkout that is not part of the repository:

```toml
[external_roots]
ios = "Read-only Apple reference checkout"

# Alternatively, include repository and revision hints:
# [external_roots.ios]
# description = "Read-only Apple reference checkout"
# git = "https://example.com/apple.git"
# commit = "0123456789abcdef0123456789abcdef01234567"
```

Supply actual paths with repeatable `--root <platform>=<path>` on `validate`,
`report`, `next`, `entry`, `ui-report`, and `drift`. Relative paths are relative to
the current working directory. Explicit roots may live anywhere; evidence paths
must still be safe relative paths and cannot escape their platform root via symlinks.
An explicit root overrides the platform's configured directory, including for
platforms not declared external. Nothing is cloned, checked out, or modified.

`CONCORDANCE_ROOT_IOS=/checkouts/apple` is equivalent to `--root ios=/checkouts/apple`.
Environment names uppercase the platform and replace `-` with `_`. CLI options take
precedence over the environment; the last repeated option for a platform wins.
Unknown platform overrides and malformed options are errors.

A declared external platform without an override has evidence marked
`unchecked (root not supplied)`, with its description, Git hint, and the exact
option/environment name to supply. This does not fail validation. `--require-roots`
turns missing external roots into errors. Unsafe evidence paths remain errors even
without a checkout. An explicitly supplied nonexistent root is always an error.
Undeclared platform roots retain the repository containment and existence checks.
For several sibling repositories, `--repo` and platform directories under a common
parent remain supported; explicit roots avoid requiring a common parent.

## Entry format

The bundled [JSON Schema](../schema/catalog.schema.json)
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
      status: implemented
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

Platform status is always one of `implemented`, `partial`, `missing`, or
`not-applicable`. `shipped` is a **deprecated** read alias for `implemented`; the tool
writes `implemented` in all output. Status records implementation only; whether anyone
independently checked it is recorded separately (see
[Implemented versus verified](#implemented-versus-verified)). `source`, `tests`, `gap`, and `notes` are optional on a platform
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

## Blockers

Work that waits on a decision or an outside dependency names it instead of hiding it in
`gap` prose:

```toml
[blockers.ask-engine]
title = "On-device LLM for Ask"
kind = "dependency"            # decision | dependency | sdk | platform | hardware
owner = "Mike"
link = "docs/decisions-needed.md#1"
status = "open"                # open (default) | decided | resolved
```

```yaml
platforms:
  android:
    status: partial
    blocked_by: [ask-engine]
    gap: The unavailable state is shown; generation needs the engine.
```

Detail platform records take `blocked_by` too. Only `open` blockers hold work back.
`validate` rejects unknown keys and `blocked_by` on `implemented` or `not-applicable`
records, and warns when a `resolved` blocker is still referenced. `next` skips blocked
entries and, transitively, entries that depend on unfinished blocked ones;
`--include-blocked` lists them with `[blocked: key]` after the title (blocked detail
rows likewise). The report splits the gap list into actionable and blocked gaps, adds a
Blocked column to the area table and a **Blockers** table (key, title, kind, owner,
status, entries and details held back, highest impact, link) ranked by how much each
unblocks. Briefs print the blocker title, link and status.

## Flavour platforms

A platform that shares another's code can inherit its records:

```toml
[platforms]
android = "apps/android"
horizon = { root = "apps/android", inherits = "android" }
```

An entry (or detail) without a `horizon` record gets a copy of its `android` record;
a `horizon` record that is present carries only its differences and is merged over the
parent (its keys win; when it sets a different `status`, the parent's `verification`,
`gap` and `blocked_by` are not inherited). Whole copies are not validated separately,
so a broken parent path is reported once. Unknown parents and cycles are errors.

## Editing records

```sh
concordance set spec/product reader.open-book --platform android \
  --status implemented --by parity/player --clear gap
concordance set spec/product reader.open-book/open-spring --platform android --status partial
```

`set` changes `status`, `implemented_by` (`--by`), `gap`, `notes` and `blocked_by`
(comma-separated), and `--clear`s `gap`, `notes`, `implemented_by`, `blocked_by`,
`verification`, `source` or `tests`. A detail is `entry/detail` or `entry#detail`; a
missing record needs `--status`. It never writes `verification` (that is `verify`'s
proposal, applied by a person). Keys keep their order and pretty-printed JSON-style
area files are rewritten byte-for-byte apart from the edit; files that would be
reformatted (comments, other layouts) are refused unless `--allow-reformat`. If the
result has new validation errors for the record (for example verification left on a
`missing` record) the file is restored and the errors printed; warnings are printed
after a successful edit.

## Warnings

`validate` prints warnings without failing: an `implemented` record with a non-empty
`gap`, an implemented entry with partial or missing details on that platform, a resolved
blocker still referenced, and (with UI contracts) a detail `motion.token` that is not a
motion in `tokens.yaml`. `validate --strict` turns warnings into errors. The report
lists implemented entries with open work.

## Implemented versus verified

A status of `shipped` means *implemented*. It is a claim, usually from source review
or from the implementing agent. Verification is a separate, optional record:

```yaml
platforms:
  android:
    status: implemented          # `shipped` is a deprecated read alias
    implemented_by: sol          # agent or person ID; required with verification
    source: [app/src/main/kotlin/Player.kt]
    verification:
      by: agy                    # reviewer ID; must differ from implemented_by
      at: 2026-11-20             # YYYY-MM-DD
      commit: 0123abc            # target commit reviewed (hex, optional)
      baseline_commit: 6c29f3e   # baseline commit compared against (hex, optional)
      captures: [now-playing.playing, build/reviews/2026-11-20/player.png]
      journeys: [app/src/androidTest/PlayerJourney.kt#PlayerJourney.skipForward]
      notes: Side-by-side review; no differences noted.
```

`verified` is derived, never stored. A record is verified only when its status is
implemented, `implemented_by` is recorded, `verification` is present, `by` differs
from `implemented_by`, and the record has the evidence the entry requires.

An entry's optional `verify_requires: [capture, journey]` names that evidence. The
default is inferred: an entry whose interactions change state (non-empty
`data.writes`) needs at least one capture **and** one journey; a purely visual entry
(no writes) verifies on a capture. Detail items need at least one capture or journey. A capture is either a
capture key `screen.state` (for the record's platform, resolved through UI contracts)
or a file path relative to the repository root. In UI reports, a capture key whose
capture is skipped or missing withdraws verification (see [UI contracts](ui.md)).

`validate` rejects verification without `implemented_by`, a verifier equal to it, verification on a `missing`
or `not-applicable` record, invalid dates, non-hexadecimal commits, missing capture
files, missing journey files, and capture keys that are not declared screen states.
Existing catalogs need no change: `shipped` without `verification` reads as
implemented and unverified.

Reports never print a single "shipped" total. The summary and area table show
implemented, verified, partial and missing counts, and a **Claimed shipped but
unverified** table lists every implemented target entry without independent
verification, with the reason (no record, no captures or journeys, verifier is the
implementer, or a skipped/missing capture).

## Detail items

Behavior contracts in prose lose the numbers: spring response, haptic kind, swipe
threshold, corner radius, exact copy. An entry can carry `details`, each tracked
per platform with the same implemented/verified model:

```yaml
details:
  - id: open-spring              # entry-scoped slug; referenced as reader.open-book#open-spring
    kind: motion                 # motion | haptic | gesture | geometry | copy | state |
                                 # accessibility | layout | other
    spec:
      text: The reader settles into place with a soft spring.
      motion: {curve: spring, response: 0.42, damping: 0.78}
    baseline: [Sources/Reader.swift:L120-L134, Sources/Reader.swift#ReaderTransition]
    platforms:
      android:
        status: missing
        gap: Uses a 300 ms tween.
  - id: swipe-dismiss
    kind: gesture
    spec:
      text: Swipe down dismisses after a third of the height.
      gesture: {kind: vertical-drag, threshold: 0.33, unit: fraction}
  - id: artwork-radius
    kind: geometry
    spec: {text: Artwork corner radius, geometry: {property: corner-radius, value: 12, unit: pt}}
  - id: tap-haptic
    kind: haptic
    spec: {text: Light impact on open, haptic: {kind: light-impact}}
  - id: empty-copy
    kind: copy
    spec: {text: Empty state title, copy: {text: No books yet, locale: en}}
```

`spec.text` is required; the structured fields are optional. `geometry` is one
`{property, value, unit}` or a list of them (`[{property: width, value: 38, unit: pt},
{property: corner-radius, value: 8, unit: pt}]`). `motion.token` names a design-token
motion (for example `Motion.fold`) so the token and its numbers travel together; a
motion needs a `curve` or a `token`, and numbers need a `curve`. `motion.curve` is
`spring`, `ease` or `linear`. A spring takes `response` (with `damping`) **or**
`duration` (with `bounce`, −1 to 1); ease and linear take only `duration`. Numbers
are non-negative seconds or ratios. Detail IDs must be unique within an entry.
`baseline` cites evidence on the report baseline platform and is validated and
drift-checked like source evidence. Detail platforms must be configured platforms;
an unrecorded detail status counts as open. A detail's optional `verify_requires`
works like the entry field; by default `motion`, `haptic` and `gesture` details need a
journey, because a still capture cannot prove them, and other details need a capture or
a journey.

The parity report adds a *Details implemented / verified / total* column to the
area table, open-detail counts to the gap list and gap tables, and a **Detail gaps in
implemented entries** table: entries already claimed shipped whose details are not.
`entry ID --format brief --details` adds a Details section with each spec's numbers,
baseline evidence and per-platform status. `next --details` also selects implemented
entries with open details and prints one extra row per open detail in the same four
tab-separated columns: `entry#detail`, `detail:<kind>`, status, and the spec text
with its numbers in brackets.

## Evidence anchors

Source, test, journey and detail-baseline references may carry an anchor:

| Form | Meaning |
| --- | --- |
| `path` | The whole file (file-level, coarse). |
| `path#Symbol` or `path#Suite.method` | A declaration; the last dot-separated component is the symbol. |
| `path:L10-L20`, `path:10-20` | Inclusive, 1-based baseline line range. |
| `path:L10`, `path:10` | A single line. |

Validation checks the file. Anchors make `drift` precise (below).

## Agent workflow

```sh
concordance validate spec/product --repo .
concordance next spec/product --platform android --area reader --limit 5
concordance entry spec/product reader.open-book --format brief
concordance entry spec/product reader.open-book --format yaml
concordance entry spec/product reader.open-book --format json
```

Add `--details` to `next` or `entry --format brief` to include detail items.
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

`validate` collects every recoverable schema, ID, dependency graph, retirement, and
evidence error instead of stopping at the first missing file. Errors are grouped
by category and sorted by entry ID, platform, reference, and message, with a final
error and unchecked-reference count. Every entry citing a bad file is reported.
Malformed YAML affects its document; other documents are still validated. An entry
that cannot be deserialized cannot undergo further semantic or evidence checks;
an unreadable or unparseable configuration prevents catalog loading.
Validation exits nonzero if any errors are present.

`Catalog::load_collect` returns a partial catalog and a `ValidationReport`;
`Catalog::validate(dir, &RootOptions)` combines structural and evidence issues in a
structured report. `ValidationReport.errors` and `.unchecked` contain serializable
`Issue` records (`group`, `entry`, `platform`, `reference`, `message`).
`report.into_result()` provides a convenient failure wrapper, and `Catalog::load`
and `validate_paths` retain their `Result` interfaces. `validate_evidence` returns
a report for an already loaded catalog. Library callers can opt into environment
resolution with `RootOptions::with_env(&catalog)`; explicit roots still win.
The CLI also checks optional UI contracts and includes UI loading errors.
Parity reports include an Evidence roots section for declared external checkouts
that were not supplied.

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
current working directory. Area and README links are written relative to the output
file's directory, so they resolve wherever the report is placed. `--check` compares exact contents, exits nonzero for a
missing/stale file, and never writes it. 

Reports count entries shipped on the configured baseline, tally target statuses
(implemented, verified, partial, missing) by area, list implemented-but-unverified
entries, list the ten highest-impact gaps, list missing and partial capabilities, and
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

## Git baselines and drift

```toml
# Shared pin for the configured report baseline platform.
[baseline]
commit = "0123456789abcdef0123456789abcdef01234567"

# A per-platform pin takes precedence over [baseline].
[baselines.ios]
commit = "abcdef0123456789abcdef0123456789abcdef0123"

# Root-relative source globs used to identify changes with no evidence citation.
[source_globs]
ios = ["Sources/**/*.swift", "Tests/**/*.swift"]
android = ["src/**/*.kt", "src/**/*.xml"]
```

The `commit` hint in an external-root record is also a fallback baseline pin.
`[baseline]` applies only to `[report].baseline`; `[baselines.<platform>]` supplies
pins for other platforms. Prefer full commit SHAs for stable historical baselines.

```sh
concordance drift spec/product --root ios=../apple --platform ios
concordance drift spec/product --root ios=../apple --since <sha> --to HEAD
concordance drift spec/product --root ios=../apple --platform ios --format json
```

`drift` compares the baseline commit to `HEAD` by default. `--since` overrides the
catalog pin and `--to` selects the end revision. With no `--platform`, it considers
all configured platforms and marks those without pins or external roots unchecked.
Revision and Git failures return nonzero; drift itself is informational. Both
revisions are resolved as commits. The comparison uses `git diff --name-only`,
includes deletions and both sides of renames, and excludes uncommitted changes.
Git is read-only and never fetches revisions.

Every entry citing a changed source, file-based test, verification journey or
detail baseline reference is listed, regardless of its status; detail evidence is
listed as `entry#detail`. Runner labels are ignored.

For each changed file cited with an anchor, `drift` reads `git diff -U0` between the
two revisions and the file's content at the baseline revision:

- `path:Lstart-Lend` is a **precise** hit when an old-side hunk intersects the range.
  Pure insertions count only when they land strictly inside it.
- `path#Symbol` resolves the symbol at the baseline revision. A declaration line
  contains the symbol as a whole word after a declaration keyword (`func`, `fun`,
  `class`, `struct`, `enum`, `protocol`, `extension`, `interface`, `object`, `var`,
  `let`, `val`, `const`, `def`, `fn`, `type`, `typealias`, `case`, `init`, `actor`,
  `trait`, `impl`, `mod`, `function`, `macro`, `record`). Its block is brace-delimited
  when a `{` opens on that line or on a continuation of its signature (unclosed
  parentheses, a trailing `,` `(` `->` `:` `=`, or a next line beginning with `{`,
  `where`, `throws` or `->`); braces after `//` and inside double-quoted strings are
  ignored. Without a brace, the block is the following lines indented deeper than the
  declaration (Python, YAML). Otherwise it is the declaration line alone. All
  declarations of the name (overloads) are considered. It is a heuristic: block
  comments, raw strings, and macros can mislead it.
- A deleted file is a precise hit for every anchored reference, noted "file deleted".
- An anchored reference whose lines did not change is not listed; it is counted as
  *anchored, unaffected*.
- Unanchored references, and anchors that cannot be resolved ("symbol not found at
  baseline"), keep file-level behavior and are labelled **file-level (coarse)**.

Each platform heading reports entries with precise hits versus coarse-only entries,
and reference counts: precise, coarse, and anchored-unaffected.

Prefer `path#Symbol` anchors for entry evidence: they survive edits elsewhere in the
file and need no rewriting. When the baseline pin moves, re-locate line anchors:

```sh
concordance rebase-anchors spec/product --root ios=../apple --to <new-sha>
concordance rebase-anchors spec/product --root ios=../apple --to <new-sha> --write
```

From the platform's pin (or `--from`) to `--to`, each cited reference on the platform
(default: the report baseline) is **unchanged**, **moved** (a line range shifted by
hunks above it, or its file renamed), **changed** (a hunk touches the range; left as is
for review) or **lost** (file deleted, or `#Symbol` no longer declared). `--write`
rewrites moved references in the area files with the same formatting rules as `set`;
then update the baseline pin. Evidence paths and
globs are relative to the resolved platform root, even when that root is a Git
subdirectory. Uncited changed files matching `source_globs` appear as
**Uncatalogued changes**. Without source globs, only cited evidence is examined.
Globs support `*` within a path component, `?` for one character, and `**` across
components; `**/` also matches zero directories. JSON contains `platforms` with
`platform`, `since`, `to`, `entries` (`id`, `files`, `hits` with `reference`, `file`,
`precision`, optional `lines` and `note`), `uncatalogued`, `precise_hits`,
`coarse_hits` and `anchored_unaffected`, plus `unchecked` messages. Output is sorted by platform, ID, and file.

`report` and `ui-report` append a Baseline drift section when the configured baseline
has both a pin and an available root. Changed baseline evidence signals that the
target status may be stale; the tool does not change any recorded status. A missing
undeclared source checkout leaves ordinary report generation available. A supplied
root with an invalid pin or unavailable Git history produces an error. `--check`
includes the drift section when checking report freshness.

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
