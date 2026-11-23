# Audit brief: home (iOS)

App: fixture. Catalog: `spec/fixture`. iOS root: `apple` (relative to the repository root).
Written by `concordance plan-audit`; it is never overwritten, so edit freely.

## Goal

Describe every user-visible behaviour in `home` on iOS as catalog entries, so the files, identifiers and strings below are all cited by an entry.

## Files (1 of 2 cited)

- [x] `Fixture/Home/HomeView.swift` — cited by home.browse-items
- [ ] `Fixture/Home/Item.swift`

## Identifiers

- `home.list` (Fixture/Home/HomeView.swift) — cited by home.browse-items
- `home.row.*` (Fixture/Home/HomeView.swift) — not cited

## Strings used here

- "%lld episodes" (Fixture/Resources/Localizable.xcstrings) — not cited
- "Nothing here yet" (Fixture/Resources/Localizable.xcstrings) — cited by home.browse-items

## Tests

- [x] `FixtureUITests/HomeUITests.swift` — cited by home.browse-items

## Existing entries

- `home.browse-items` — Browse items (`catalog/home.yaml`; ios implemented)

## Entry format

Each area file `catalog/<area>.yaml` is a nonempty array of entries. Required fields:
`id` (`area.verb-object`), `title`, `summary`, `surfaces`, `interactions` (trigger → result),
`states`, `settings`, `data` (`reads`, `writes`), `copy` (exact strings), `accessibility`
(`label`, `identifier`, `notes`), `design`, `depends_on`, `impact`
(critical|high|medium|low), `platforms` (per platform: `status`
implemented|partial|missing|not-applicable, `source`, `tests`, `gap`) and `acceptance`
(given/when/then). Measurable touches go in `details` (kind motion|haptic|gesture|geometry|
copy|state|accessibility|layout|other, with numbers in `spec`).

```yaml
- id: home.open-item
  title: Open an item
  summary: Tapping a row opens the item at its saved position.
  surfaces: [home]
  interactions:
    - trigger: Tap a row
      result: The item opens at its saved position.
  states: [ready, empty, offline]
  settings: []
  data: {reads: [item, position], writes: []}
  copy: [Open, Nothing here yet]
  accessibility: {label: Open item, identifier: home.row, notes: []}
  design: []
  depends_on: []
  impact: high
  platforms:
    ios: {status: implemented, source: ["App/Home/HomeView.swift#HomeRow"], tests: []}
  acceptance:
    - given: An item with a saved position
      when: The person taps its row
      then: The item opens at that position.
  details:
    - id: open-spring
      kind: motion
      spec: {text: The item expands with a spring., motion: {curve: spring, response: 0.4, damping: 0.8}}
      baseline: ["App/Home/HomeView.swift#openTransition"]
```

Full format: https://github.com/luff-llc/concordance/blob/main/docs/format.md

## Rules

1. Read the source. Do not trust an existing status: `implemented` is a claim, often from an earlier coarse review. Audit at the level a picky designer would notice.
2. For every screen and component in the area compare or record: layout and hierarchy, typography roles, colour and material, motion (named springs, durations, transitions, reduced-motion behaviour), haptics, gestures (swipe actions, long press and context menus, drag, scrub), menus and their items, toolbar items, empty, loading, error and offline states, copy, accessibility labels and identifiers, text scaling, landscape, tablet and wide layouts, system integration (media controls, notifications, share sheet, intents, widgets) and the settings that change behaviour.
3. Give exact values: copy verbatim, spring or duration numbers, haptic kind, gesture thresholds, sizes, identifiers. A brief without numbers produces a flat port; put the numbers in `details`.
4. Keep existing accessibility and test identifiers; tests on every platform find elements by them.
5. For a surface that exists on only one platform, say what the native analogue on the other platform should be (for example a lock-screen live activity and an ongoing notification, a control-centre control and a quick-settings tile, a voice-assistant shortcut and an app action), or record `not-applicable` with the reason.
6. Cite evidence relative to the platform root and anchor it where you can (`path#Symbol` or `path:L10-L20`). Every cited path must exist.
7. Statuses are `implemented`, `partial`, `missing` or `not-applicable`, with a `gap` for partial and missing work. Never write `verification`: verification is recorded later by someone who did not implement the work.
8. Stay inside this area. Something that belongs to another area becomes a `depends_on` link or a note, not a duplicate entry. Never reuse an ID; retire old IDs in `retired-ids.json`.
9. Do not build, run or edit application source during the audit. Only write catalog entries and your findings file.
10. Depth over breadth: an exhaustive account of this area beats a skim of several.

## Output

1. Entries in `spec/fixture/catalog/home.yaml` (or the existing area file that fits), with ios source and tests cited.
2. Check: `concordance validate spec/fixture` passes, and `concordance inventory spec/fixture --platform ios --uncatalogued` no longer lists this area's items.
