# Bring Concordance into a repo

Status: being built (Nov 2026); steps 3 and 4 are done. The first adopter is Taupe in Marginalia.

Concordance should be something you add to any repository, not something hand-built for one
app. You run one command, it scans the repo, and from then on agents keep the product state
current by talking to the tool, not by editing files by hand.

The Taupe Android work showed what the scan has to do. People and agents did it by hand there:
they found the platforms, listed every screen and string, grouped the code into areas, audited
each area, wrote catalog entries, and kept a release ledger in prose. This turns those steps
into commands.

## The model: plans are written, facts are recorded, status is derived

| Kind | Who writes it | Where | Examples |
| --- | --- | --- | --- |
| Contract | agents, reviewed by people | `catalog/*.yaml` | what a feature does, its states, copy, identifiers, acceptance |
| Plan | people (or agents on their behalf) | `tracker/releases.yaml` | target window, why now, what a release should carry |
| Fact | tools and agents, append-only | `tracker/log.jsonl` | reported, fixed, verified, uploaded, released, pulled |
| Status | nobody; computed | `concordance track` output | open, fixed, verified, shipped; ready or not |

No command sets a status. A bug is *fixed* because a commit says it fixes it, *verified*
because a check by someone other than the fixer passed, and *shipped* because a released
build contains the fixing commit. If the facts don't support a status, the status isn't shown.

## Commands

### Adoption

```sh
concordance init [--repo .] [--app NAME] [--out spec/NAME]
```

Scans the repository and writes a starting catalog directory:

- **Platforms.** Detected from markers: `Package.swift`, `*.xcodeproj`, `project.yml` (Apple);
  `build.gradle(.kts)` with `AndroidManifest.xml` (Android); `package.json` (web); `Cargo.toml`,
  `go.mod`, `pyproject.toml` (CLI or service). Each one becomes a `[platforms]` root.
- **Source globs.** Defines `all`, `user_visible` and `strings` globs per platform, from the
  layout it finds (for example `**/*.swift` under app targets; `res/values*/strings.xml`).
- **Releases.** From existing tags with a version (`v1.2.0`, `ios/v1.2.0`, `taupe-1.2.0`), init
  saves the most common form as `[tracker] tag_pattern` (for example `ios/v{version}`).
  `concordance log scan-git` then records each matching tag as a `released` fact dated by the tag.
- **Agent kit.** Writes `AGENTS.md` beside the catalog, plus a one-line pointer for the repo's
  own agent guide (printed, never written into it). The kit says how to use `next`, `entry`,
  `inventory`, `log` and commit trailers.

`init` never overwrites. Rerunning it on an adopted repo reports what it would add.

```sh
concordance inventory spec/NAME [--platform P] [--uncatalogued] [--format markdown|json]
```

A deterministic map of what the product exposes, per platform: source files grouped into
candidate areas (by feature folder), the user-visible strings from string catalogs, the
accessibility and test identifiers found in source (`accessibilityIdentifier("…")`,
`testTag("…")`, `data-testid="…"`), and test files. Each item says whether a catalog entry cites
it. `--uncatalogued` lists only what no entry covers. This is the to-do list for adoption, and
afterwards it shows drift whenever the app grows a surface nobody catalogued.

```sh
concordance plan-audit spec/NAME [--platform P] [--out spec/NAME/tasks]
```

Writes one audit brief per candidate area, with the files, identifiers and strings in that area,
the entry format, and the rules from the Taupe audits ("don't trust shipped; give exact copy,
motion, haptics and identifiers"). Agents take one brief each, write entries, and run
`validate`. With a target platform it becomes a parity audit, like Taupe Android's.

```sh
concordance guide spec/NAME
```

Prints the agent kit for this catalog's version and configuration, so instructions never go
stale in a checked-in copy.

### Facts

```sh
concordance log report bug|request "Title" [--entry ID]... [--platform P]... [--impact I] [--source S]
concordance log fixed ISSUE --commit SHA [--by WHO]
concordance log verified ISSUE --evidence TEXT... --by WHO
concordance log reopened ISSUE --reason TEXT
concordance log uploaded RELEASE --platform P --build B [--commit SHA]
concordance log released RELEASE [--platform P] [--date D] [--commit SHA]
concordance log pulled RELEASE --reason TEXT
concordance log scan-git [--repo .] [--since REF]
```

Each command appends one event to `tracker/log.jsonl`: a line with the event kind, its subject,
time, who recorded it, its evidence, and a stable id derived from those. Appending the same fact
twice is a no-op, so tools can call these freely. `report` creates the issue (its id comes from
the title, `bug.`/`request.` plus a slug, made unique), so `tracker/issues.yaml` is optional.

`scan-git` reads commit trailers and tags and appends what they state:

```text
Fixes: bug.home-blank-after-recovery
Verifies: bug.home-blank-after-recovery (test-taupe-colophon-05-notes.py 13/13)
Reports: request.reuse-luff-identity
```

and released tags matching the saved pattern. It is idempotent and safe in CI or a hook.

### Status

`track` and `suite` replay the log over the plan:

- issue: **open** once reported; **fixed** when a fix commit is recorded; **verified** when a
  verification is recorded by someone other than the fixer; **shipped** when a released release's
  commit contains the fix commit (`git merge-base --is-ancestor`); **reopened** sends it back to open.
- release: **planned** (only in the plan), **uploaded** per platform with builds, **released**
  with its date, **pulled**.
- Declared `state` fields in `issues.yaml` still work for catalogs without a log. When both
  exist, the derived state wins and a disagreement is shown as a warning.

## How agents use it

The repo's agent guide carries one line: *product state is kept by Concordance; run
`concordance guide spec/NAME`*. From then on:

- **Before work:** `concordance next` and `concordance entry ID --format brief`.
- **When a person reports something:** `concordance log report …`.
- **When committing a fix:** a `Fixes:` trailer. A test runner or reviewer adds `Verifies:`
  or runs `log verified`.
- **When releasing:** release tooling runs `log uploaded` and `log released`.
- **When the app grows:** `concordance inventory --uncatalogued` shows the new surface.
- **For a person:** `concordance track` for one app, `concordance suite` for all of them; Galley
  reads the same data as JSON.

## Build order

1. Event log, `log` commands, fold into `track` (derived states, warnings). Done; see
   docs/tracker.md ("Log").
2. `scan-git`: trailers and tags. Done.
3. `init` and `inventory` (detection, globs, strings, identifiers, coverage). Done.
4. `plan-audit` and `guide`. Done.
5. Adopt on Taupe: log events from today's facts, trailers in the agent guide, release tooling
   calls `log`. Then try `init` on a repo with no catalog.

## As built: `init`, `inventory`, `plan-audit`, `guide`

Choices where the proposal left room:

- **`init` paths.** `--out` is relative to `--repo` and must be inside it; `repo_root` is
  derived from its depth. Platform roots: Apple is the common ancestor of every
  `Package.swift`, `project.yml` and `*.xcodeproj`; it is named `ios` or `macos` only when
  the project files mention exactly one of them, else `apple`. Android is the deepest
  `settings.gradle(.kts)` directory containing every module that has a `build.gradle(.kts)`
  above an `AndroidManifest.xml`. Web is the `package.json` directory with the most sources
  under `src/`, `app/`, `pages/`, `components/` or `lib/`. `Cargo.toml`, `go.mod` and
  `pyproject.toml` each give a CLI platform (`cli`, or `cli-rust`/`cli-go`/`cli-python` when
  there are several). A platform with no source files is dropped. Hidden directories,
  `node_modules`, `build`, `.build`, `DerivedData`, `target`, `Pods`, `.git` and `dist` are
  skipped.
- **Globs.** One `all` glob per top-level source directory (per module source set on
  Android). `user_visible` keeps the directories whose files import a UI framework
  (SwiftUI, UIKit, AppKit, WidgetKit; Compose or `android.view`/`android.widget`; `.tsx`,
  `.jsx`, `.vue`, `.svelte` on the web). A new `tests` key in `[source_globs.<platform>]`
  names test files for `inventory`; files matching `all` that look like tests by path
  convention count as tests too.
- **Releases.** Tags are read only when `--repo` is the top level of its Git repository. The
  pattern is the most common text before the version followed by `{version}` (for example
  `ios/v{version}`), the glob form `log scan-git` reads. Without version tags, `[tracker]` is a
  commented example. `init` does not append `released` facts itself; `log scan-git` does that
  from the saved pattern.
- **Monorepos and `--scope`.** `init --scope PATH` (repeatable, relative to `--repo`, a
  directory inside it) searches only those subtrees for sources, plus the marker files and
  `*.xcodeproj` bundles in each scope's parent directories, so a shared project or Gradle
  settings file above the scope still names the platform and its root; a scope inside an
  Android module's source set (`android/app/src/main/kotlin`) also picks up
  `<module>/src/main/AndroidManifest.xml` for the nearest module above it. Globs are never
  broader than the scopes: a scope inside a glob's fixed prefix narrows it
  (`--scope apple/Fixture/Home` gives `Fixture/Home/**/*.swift`), and globs that match nothing
  in the scopes are dropped. String-catalog globs are limited to the top-level folders that
  hold them. The user-visible count `init` prints is computed exactly as `inventory` counts
  files (matching `user_visible`, else `all`, and neither a string catalog nor a test), so the
  two agree on the written catalog. Without `--scope`, `init` lists the apps
  it finds when there is more than one: Apple application targets (from `project.pbxproj`,
  quoted or not, and XcodeGen `project.yml`) and Android modules applying
  `com.android.application` (a declaration with `apply false`, as in a root build file, does
  not count). Apps are grouped by name without platform suffixes (`TaupeWatch`
  joins `Taupe`; an Android module called `app` takes the Gradle `rootProject.name`). Each
  target's scope is the source folder named after it, plus extension targets whose names start
  with it (`TaupeWidgets`); a project holding one app falls back to its own folder. `init` then
  prints one `concordance init --app <name> --scope …` line per app and still writes the
  whole-repository catalog.
- **Symbolic links.** `init` refuses (before writing, and again while writing) when any
  existing component of the catalog path or of a file it writes is a symbolic link, or when
  the nearest existing ancestor resolves outside the repository. Directories are created one
  component at a time and files with exclusive create, so nothing is written through a link.
  Scans follow a symbolic link only when it is a file inside the root; directory links,
  dangling links, links leaving the root, and fixed glob prefixes that resolve outside it are
  skipped, and `inventory` lists them as skipped symbolic links. A fixed glob prefix that
  passes through any symbolic link, even one pointing inside the root, is never walked and is
  listed the same way. Out of scope: a concurrent local process that replaces a parent
  directory with a symbolic link between these checks and the writes.
- **Rerunning.** When `catalog.toml` exists, `init` writes nothing, not even missing kit files;
  it lists what it would add (missing files, newly detected platforms, missing
  `[source_globs]`, a tag pattern). In a fresh catalog directory, files that already exist are
  kept and the rest are created. `tracker/` is created empty.
- **Empty catalogs.** A catalog with a `catalog/` directory and no area files is valid
  (`validate`, `track`); a flat catalog still needs at least one area file.
- **Inventory.** Files are the user-visible sources (`user_visible`, else `all`; with no
  `[source_globs]`, common source extensions). A candidate area is the first non-generic
  folder below the matched glob's fixed prefix (`src`, `main`, `kotlin`, `java`, `sources`,
  `views`, `screens`, `features` and similar are skipped); in Kotlin and Java the package
  directories every file shares are stripped first. Files at that root are `app` for the
  largest group and named after the glob's folder otherwise (`TaupeWidgets` →
  `taupe-widgets`). Tests named after an area (`HomeUITests.swift`) join it. Strings come
  from the source language only (`.xcstrings` minus stale keys, `en`/`Base` `.strings`,
  `res/values/strings.xml`, `en` or unlocalized i18n JSON); a string's areas are the areas
  whose source uses its key, matching `%@`/`%lld` placeholders to interpolations, or
  `R.string.<name>`. Strings are keyed by file and key, so two modules' `title` strings stay
  separate; a string's areas come from uses in its own module (the path before `src/`), or
  from anywhere when its module has none. Identifiers with interpolation or `+` concatenation are shown with `*`
  and match entry identifiers as a glob. Citations: files by entry evidence on that platform
  (after `[path_prefixes]`), identifiers by `accessibility.identifier`, strings by `copy` or
  a copy detail's text or key. External platforms without `--root` are listed as unchecked.
- **`plan-audit`.** Briefs go to `DIR/tasks` (or `--out`) as `audit-<platform>-<area>.md`, or
  `parity-<baseline>-<target>-<area>.md` with `--target`, created exclusively; edited briefs
  are preserved. `--platform` defaults to `[report].baseline`. The parity findings file is
  named `<brief>-findings.md`.
- **`guide`.** Paths are printed as passed to `guide`. Release tag patterns come from
  `[tracker] tag_pattern` (a `{version}` glob, or a list of them), the same setting `scan-git` reads. `log` commands are shown with
  `--catalog DIR`.
