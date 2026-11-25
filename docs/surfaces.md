# Surfaces

A catalog has always recorded **features × platforms**: "chapter progress is implemented
on iOS and partial on Android". Many features also reach the user through system
surfaces outside the app: home-screen widgets in several sizes, Control Center, quick
actions, Siri and Shortcuts, a watch app and its complications, the car, the menu bar and
Live Activities. A feature can be done in the app and on the iOS small widget, missing on
the Android 2×2 widget, and absent on the watch. Without a way to say so, and a check
against what the app actually declares, a surface can go missing without anyone noticing.

Surfaces add a third dimension, **features × platforms × surfaces**, and a scanner that
reads what each app declares to the operating system.

1. The catalog declares its surfaces and which platforms can have each one.
2. Entries name the surfaces they reach. Platform records carry a status for each surface.
3. `report`, `ui-report`, `next` and `gate-release` count parity per platform and surface.
4. `concordance surfaces scan` lists what the app declares (widget kinds and sizes,
   controls, shortcuts, watch and car targets...) and compares it with the catalog.
5. `gate` fails a change that declares a new surface without cataloguing it, or that
   removes a declaration the catalog still claims.

Catalogs without a `[surfaces]` section, and entries that name no declared surface,
behave exactly as before.

## Declaring surfaces

```toml
[surfaces."widget.small"]
title = "Small widget"
platforms = ["ios", "android"]

[surfaces."watch-app"]
title = "Watch app"
platforms = ["ios", "android"]

[surfaces.car]
title = "CarPlay and Android Auto"
platforms = ["ios", "android"]
# A gap on this surface fails `gate-release` for the platform.
release = "required"

[surfaces."lock-screen"]
title = "Lock Screen widgets"
platforms = ["ios"]
# Which scanner signals mean "this app declares this surface" (see Signals).
signals = ["apple.widget:accessory*"]
```

Surface IDs use lowercase letters, digits, `.` and `-`. `platforms` must name configured
platforms. `title` is optional (the ID is shown otherwise). `signals` is optional for the
well-known IDs below, which have built-in defaults; any other ID without `signals` is
tracked in reports but never discovered by the scanner (for example `notification`
content that is posted at run time).

## Entries and per-surface records

An entry's existing `surfaces` list names where the feature appears. Values that match a
declared surface ID opt the entry into surface parity; other values (screen names such as
`now-playing` or `library`) keep their existing meaning.

```yaml
- id: widgets.show-playing-episode
  surfaces: [home, widget.small, widget.medium, complication]
  platforms:
    ios:
      status: implemented
      source: [Widgets/NowPlayingWidget.swift#NowPlayingWidget]
      surfaces:
        widget.small:
          status: implemented
          declared: [com.example.now-playing]
        widget.medium: {status: implemented}
        complication: {status: implemented, declared: [com.example.now-playing.watch]}
    android:
      status: partial
      surfaces:
        widget.small: {status: missing, gap: "No 2×2 provider"}
        widget.medium: {status: implemented, declared: [.widget.NowPlayingWidgetReceiver]}
        # complication omitted: counted as unrecorded, which is a gap
```

A per-surface record takes the same fields as a platform record (`status`, `source`,
`tests`, `gap`, `notes`, `implemented_by`, `verification`, `blocked_by`, the stale
fields, `intended_difference`, `reviewed_commit`), plus:

- `declared`: the IDs the scanner reports for this surface on this platform (a widget
  kind, a shortcut type, a receiver class...). With `declared`, the match is exact: the
  gate notices a second widget, and a removed one. Without it, the record covers every
  declaration of that surface on the platform (a *coarse* match, reported as such).

Rules:

- For each surface an entry names, every platform that is both in the surface's
  `platforms` and in the entry's `platforms` needs a record. A missing record is
  **unrecorded** and counts as a gap. An entry whose platform record is `not-applicable`
  has no surface obligations on that platform.
- A record for a platform the surface does not list, for an undeclared surface, or for a
  surface the entry does not name is a validation error, as are nested `surfaces`.
- `verified` is derived exactly as for platform records.
- Flavour platforms (`inherits`) copy the whole `surfaces` map when they have no
  `surfaces` of their own.
- Surface records are addressed as `entry.id@surface`, for example
  `concordance set <dir> widgets.show-playing-episode@widget.small --platform android
  --status implemented`. `entry`, `drift`, `gate` and `sync` use the same label.

## Reports

`report` and `ui-report` append a **Surfaces** section when the catalog declares any:

```text
| Surface | iOS | Android |
| --- | --- | --- |
| Small widget (`widget.small`) | 3/3 (1 verified) | 1/3 · 2 gaps |
| Watch app (`watch-app`) | 4/4 | 0/4 · 4 gaps |
```

Each cell counts entries that name the surface and are applicable on that platform:
implemented over total, verified when any, and gaps (missing, partial, stale or
unrecorded). A dash means the surface does not exist on that platform. A list of surface
gaps follows, by surface and platform.

`next --platform P` also selects entries that are implemented in the app but have surface
gaps on P, and prints one row per gap after the entry row:

```text
widgets.show-playing-episode@widget.small	surface	missing	Small widget
```

`--surface <id>` limits the queue to one surface; `--no-surfaces` restores the old queue.

`gate-release --platform P` fails on stale surface records older than the limit (like
details), and on any gap on a surface with `release = "required"`.

## Declared-surface discovery

`concordance surfaces scan <dir>` reads platform sources and reports the surfaces each
app declares. Nothing is compiled or executed: the parsers read Info.plist, Swift source,
`AndroidManifest.xml` and Android XML resources as text. Configure them per platform:

```toml
[surface_scan.ios]
# Root-relative globs, as in source_globs.
swift = ["App/**/*.swift", "Widgets/**/*.swift", "Watch/**/*.swift"]
plist = ["App/Info.plist", "Watch/Info.plist"]
# Declarations from files under these globs carry a context (used by signal patterns):
# accessory widgets in the watch target are complications, not Lock Screen widgets.
contexts = { watch = ["Watch/**"] }
# Families assumed for a widget without .supportedFamilies, per context (default: system
# small, medium and large).
default_families = { watch = ["accessoryCircular", "accessoryRectangular", "accessoryInline", "accessoryCorner"] }
# Files to leave out, for example sources built only for another platform (globs).
exclude = ["Widgets/Spatial/**"]
# Declaration IDs to leave out (globs), for example debug-only widgets.
ignore = ["DebugWidget"]

[surface_scan.android]
manifests = ["app/src/main/AndroidManifest.xml", "wear/src/main/AndroidManifest.xml"]
# Extra res directories, for Gradle source sets that share resources (optional).
res = ["shared/src/main/res"]
contexts = { wear = ["wear/**"] }
```

`res/xml` resources referenced from a manifest (`@xml/name`), and `@dimen/` values in
`values/dimens.xml`, are read from the `res` directory beside it, then from each `res`
directory listed. Paths are relative to the platform root; the scan refuses `..`.

### What the parsers recognise

| Signal | Source | Declaration ID | Detail |
| --- | --- | --- | --- |
| `apple.widget` | Swift `struct X: Widget` with `StaticConfiguration`/`AppIntentConfiguration`/`IntentConfiguration(kind:)` | kind string (or the struct name) | each family from `.supportedFamilies([...])`; system small/medium/large when absent |
| `apple.control` | Swift `struct X: ControlWidget` (`StaticControlConfiguration`/`AppIntentControlConfiguration(kind:)`) | kind | |
| `apple.live-activity` | `ActivityConfiguration(for: X.self)` | attributes type | |
| `apple.app-shortcut` | `struct X: AppShortcutsProvider`, each `AppShortcut(intent: Y(...))` | intent type | |
| `apple.menu-bar` | `MenuBarExtra(` | title literal, else `MenuBarExtra` | |
| `apple.carplay` | `CPTemplateApplicationSceneDelegate` conformance; Info.plist scene role `CPTemplateApplicationSceneSessionRoleApplication` | delegate class | |
| `apple.quick-action` | Info.plist `UIApplicationShortcutItems` | `UIApplicationShortcutItemType` | |
| `apple.watch-app` | Info.plist `WKApplication`, `WKWatchKitApp` or `WKCompanionAppBundleIdentifier` | `CFBundleIdentifier`, else the plist path | |
| `apple.complication` | Swift `CLKComplicationDataSource` conformance | class | |
| `apple.notification` | Info.plist `NSExtensionPointIdentifier` `com.apple.usernotifications.content-extension` | plist path | |
| `android.widget` | manifest `<receiver>` with `android.appwidget.provider` metadata and its `appwidget-provider` XML | receiver name | `COLSxROWS` from `targetCellWidth`/`Height`, else from `minWidth`/`minHeight` (`(dp + 30) / 70`) |
| `android.shortcut` | `shortcuts.xml` referenced by `android.app.shortcuts` | `shortcutId` | |
| `android.app-action` | `<capability>` in `shortcuts.xml` | capability name | |
| `android.car` | `com.google.android.gms.car.application` metadata → `automotive_app_desc` | `uses name` | the use (`media`, `template`...) |
| `android.media-library` | `<service>` handling `androidx.media3.session.MediaLibraryService` or `android.media.browse.MediaBrowserService` | service name | |
| `android.qs-tile` | `<service>` handling `android.service.quicksettings.action.QS_TILE` | service name | |
| `android.wear-app` | `<uses-feature android:name="android.hardware.type.watch">` | manifest `package`, else the path | |
| `android.wear-tile` | `<service>` handling `androidx.wear.tiles.action.BIND_TILE_PROVIDER` | service name | |
| `android.complication` | `<service>` handling `android.support.wearable.complications.ACTION_COMPLICATION_UPDATE_REQUEST` | service name | |

The Swift parser is a lexical scan: it skips comments and string contents when matching
braces. A `kind:` or `.supportedFamilies(...)` given as an identifier is resolved from
`let`/`static let` constants in the scanned files: `Type.name` and `Self.name` from that
type's body or extensions, a bare name from the widget's own body, its type, or a name
defined only once across the files. A kind that does not resolve falls back to the struct
name; families that do not resolve give one `unknown` declaration, which no default
pattern maps, so it is reported as unmapped rather than dropped.

Contexts match the file that declared a surface: for shortcuts, App Actions and Android
Auto descriptors, the manifest that references the resource, not the resource itself.

### Signals

A surface's `signals` are patterns `signal[:detail][@context]`. `*` matches any run of
characters in the detail or context. A pattern without `@` matches declarations that have
no context; `@watch` matches only the `watch` context; `@*` matches any.

Built-in defaults:

| Surface | Default signals |
| --- | --- |
| `widget.small` | `apple.widget:systemSmall`, `android.widget:1x1`, `1x2`, `2x1`, `2x2` (each `@*`) |
| `widget.medium` | `apple.widget:systemMedium`, `android.widget:3x1`, `3x2`, `4x1`, `4x2`, `5x1`, `5x2` (`@*`) |
| `widget.large` | `apple.widget:systemLarge`, `android.widget:*x3`, `*x4`, `*x5` (`@*`) |
| `widget.extra-large` | `apple.widget:systemExtraLarge*` |
| `lock-screen` | `apple.widget:accessory*` |
| `complication` | `apple.widget:accessory*@watch`, `apple.complication@*`, `android.complication@*` |
| `control` | `apple.control@*`, `android.qs-tile@*` |
| `quick-action` | `apple.quick-action@*`, `android.shortcut@*` |
| `app-shortcut` | `apple.app-shortcut@*`, `android.app-action@*` |
| `watch-app` | `apple.watch-app@*`, `android.wear-app@*` |
| `wear-tile` | `android.wear-tile@*` |
| `car` | `apple.carplay@*`, `android.car@*`, `android.media-library@*` |
| `menu-bar` | `apple.menu-bar@*` |
| `live-activity` | `apple.live-activity@*` |
| `notification` | `apple.notification@*` |

A declaration may match several surfaces (a widget kind with three families yields three
declarations). Only surfaces whose `platforms` include the scanned platform match.

### Findings

`surfaces scan` prints, per platform:

- **Uncatalogued**: a declaration that no entry's per-surface record covers.
- **Unmapped**: a declaration that matches no declared surface. The catalog has no place
  to record it; add a surface (or a `signals` pattern), or `ignore` it.
- **No longer declared**: a record that claims the surface (implemented, partial or
  stale) on a scanned platform, whose `declared` IDs are not found, or, for a coarse
  record, with no declaration of that surface on the platform. Surfaces with no signals
  for a platform are never reported here.
- **Status disagrees**: a covering record whose status is `missing` or `not-applicable`.
- **Coarse**: covered only by a record without `declared` (informational).

`--format json` prints the declarations and findings for other tools. Exit status: 0
when there are no uncatalogued, unmapped or no-longer-declared findings, 1 otherwise
(`--strict` also fails on status disagreements and coarse matches), 2 on error.

### `gate`

When the platform has a `[surface_scan]` section, `gate` scans the declared surfaces on
both sides of the range (files are read from Git at `--base` and `--head`) and fails when:

- a declaration present at head but not at base is uncatalogued or unmapped, or is
  covered only by a record that says `missing` or `not-applicable`; or
- a declaration present at base is gone at head while a record still claims it (exact
  `declared` match, or a coarse record whose surface has no declaration left).

With `--patch`, the working tree is scanned as head and `HEAD` as base, and only
declarations from files the patch touches count as new, so other uncommitted work in a
shared checkout is not blamed on the patch. Declarations that
already existed are reported by `surfaces scan`, not the gate, so a catalog can adopt
surfaces gradually. The `Catalog: none — <reason>` trailer waives surface findings too;
the report still lists them.

## Design notes

- **Why reuse `surfaces`.** Entries already list where a feature appears. Making the
  declared IDs special keeps one list, keeps old catalogs valid, and avoids a second field
  with nearly the same meaning.
- **Why per-surface records inside platform records.** Status, evidence, verification and
  blockers already have one shape with one set of rules. A surface record is that shape,
  so `verified`, `blocked_by`, staleness and `set` work without new concepts.
- **Why unrecorded counts as a gap.** The failure this prevents is silence. An entry that
  says it reaches the small widget must say what happens there on every platform that has
  one, even if the answer is `not-applicable`.
- **Why text parsers, not build tools.** The gate runs on every merge, often on Linux
  runners without Xcode or the Android SDK. Declarations of these surfaces are static and
  greppable by design (the OS has to find them), so a lexical scan is reliable enough and
  fast. Anything it cannot resolve falls back to a stable ID (a struct name or a path)
  rather than being dropped.
- **Why new-versus-base in the gate.** A gate that failed on every pre-existing
  uncatalogued surface would block adoption. The scan lists the backlog; the gate stops it
  from growing.
