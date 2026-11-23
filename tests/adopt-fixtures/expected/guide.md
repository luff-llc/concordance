# Concordance agent kit: fixture

Product state for fixture is kept by Concordance in `spec/fixture`. Paths here are relative to the
directory `concordance guide` ran in (for the copy `init` writes, the repository root). Plans are written, facts are recorded, status is derived: no command sets a
status, and nobody edits a status that a recorded fact can establish. This kit is printed by
`concordance guide spec/fixture`; when this copy and that output differ, the output wins.

## Layout

- `spec/fixture/catalog.toml`: platforms, source globs, report and tracker settings.
- `spec/fixture/catalog/<area>.yaml`: contract entries (what a feature does, its states, copy,
  identifiers, acceptance, and each platform's status and evidence).
- `spec/fixture/tracker/releases.yaml`: the release plan (written by people).
- `spec/fixture/tracker/log.jsonl`: facts, append-only, written by `concordance log`. Never edit it.

Platforms (baseline `ios`, target `android`):

- `android` (Android): root `android`
- `ios` (iOS): root `apple`

## Before work

    concordance next spec/fixture --platform android --limit 10 --details
    concordance entry spec/fixture <ENTRY-ID> --format brief --details

Read the brief before touching code. Keep the entry's accessibility identifiers and copy
exactly; tests on every platform find elements by those identifiers.

## Catalog entries

- Entry format: https://github.com/luff-llc/concordance/blob/main/docs/format.md. IDs are
  `area.verb-object`, stable, never reused (retire them in `retired-ids.json`).
- Cite evidence relative to the platform root, anchored where possible (`path#Symbol`).
- After any catalog edit: `concordance validate spec/fixture`.
- What nobody has catalogued yet: `concordance inventory spec/fixture --uncatalogued`.
- One audit brief per candidate area: `concordance plan-audit spec/fixture` (add
  `--target android` for a parity audit against `ios`).
- Change a platform record with `concordance set spec/fixture <ENTRY-ID> --platform <p> --status ...`;
  never write `verification` for your own work.

## Facts

Each command appends one event to `tracker/log.jsonl`; repeating a fact is a no-op. `log`
finds the catalog from `--catalog spec/fixture`, else `$CONCORDANCE_CATALOG`, else `./catalog.toml`,
else the only `spec/*/catalog.toml`; the commands below pass it explicitly.

    concordance log --catalog spec/fixture report bug|request "Title" [--entry ID]... [--platform P]... [--impact I] [--source S]
    concordance log --catalog spec/fixture fixed ISSUE --commit SHA [--by WHO]
    concordance log --catalog spec/fixture verified ISSUE --evidence TEXT... --by WHO
    concordance log --catalog spec/fixture reopened ISSUE --reason TEXT
    concordance log --catalog spec/fixture uploaded RELEASE --platform P --build B [--commit SHA]
    concordance log --catalog spec/fixture released RELEASE [--platform P] [--date D] [--commit SHA]
    concordance log --catalog spec/fixture pulled RELEASE --reason TEXT
    concordance log --catalog spec/fixture scan-git [--since REF]

- When a person reports something: `concordance log --catalog spec/fixture report ...`.
- When committing a fix, add a trailer; a test runner or reviewer adds `Verifies:` (or runs
  `log verified`). Verification must come from someone other than the fixer.

      Fixes: bug.<slug>
      Verifies: bug.<slug> (<evidence, such as a test name and result>)
      Reports: request.<slug>

- When releasing, release tooling runs `log uploaded` and `log released`.
- No release tag pattern is configured; set `[tracker] tag_pattern` in catalog.toml so `concordance log scan-git` can record tagged releases.

## Status

- `concordance track spec/fixture`: releases, open bugs and requests, unverified fixes and gaps.
- An issue is open once reported, fixed when a fix commit is recorded, verified when someone
  other than the fixer records a verification, and shipped when a released build contains
  the fix commit. A catalog record is `implemented` by claim and verified only through an
  independent verification record.
