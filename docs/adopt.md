# Bring Concordance into a repo

Status: proposal, being built (Nov 2026). The first adopter is Taupe in Marginalia.

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
- **Releases.** Git tags matching a version (`v1.2.0`, `ios/v1.2.0`, `taupe-1.2.0`) are recorded
  as `released` facts dated by the tag. The tag pattern is saved in `catalog.toml` for later scans.
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
3. `init` and `inventory` (detection, globs, strings, identifiers, coverage).
4. `plan-audit` and `guide`.
5. Adopt on Taupe: log events from today's facts, trailers in the agent guide, release tooling
   calls `log`. Then try `init` on a repo with no catalog.
