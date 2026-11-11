# One product, native screens

Behaviour parity does not ensure visual parity. A screen can expose the same actions
and still have a different hierarchy, emphasis, artwork treatment or typography.
Concordance's optional UI contracts describe those decisions once. SwiftUI and
Compose implement them with their own controls, sheets, accessibility trees and
materials. Apps do not load these files at runtime; there is no shared renderer.

Keep behaviour in `catalog/*.yaml`. Add `tokens.yaml` next to `catalog.toml` and
one screen object per `screens/*.yaml` (also `.yml`) file. Screen IDs survive file
renames; `entries` connects each screen to existing behaviour IDs. Old catalogs
need no UI files. `validate` validates the UI contract when UI files are present;
it does not require captures to exist. `report` remains the behaviour report;
`ui-report` generates that report with a screen summary appended.

```text
spec/product/
  catalog.toml
  catalog/*.yaml
  tokens.yaml
  screens/detail.yaml
  captures/detail.populated.ios.png
  captures/detail.populated.ios.json
  captures/detail.populated.android.png
  captures/detail.populated.android.json
```

The bundled [token schema](../schema/tokens.schema.json) and
[screen schema](../schema/screens.schema.json) reject unknown fields and duplicate
YAML keys. Loading also checks token, entry, region, layout and platform references.
UI names (token keys, screen/state/region IDs, platform names) start with a lowercase
ASCII letter and contain lowercase letters, digits, dots and hyphens. The full `screen.state.platform` capture stem must be unique; ambiguous
combinations of dotted IDs are rejected.

## Tokens

```yaml
colors:
  surface: {light: '#FAF8F5', dark: '#171513'}
  accent:
    light: '#775544'
    dark: '#DDAA88'
    rule: Dominant artwork hue, adjusted to 4.5 contrast against surface.
typography:
  title: {size: 24, line_height: 29, weight: 600, dynamic_type: title2}
  body: {size: 17, line_height: 22, weight: 400, dynamic_type: body}
spacing: {small: 8, regular: 16, generous: 24}
radii: {artwork: 12, control: 24}
materials:
  floating-glass: {swift: ultraThinMaterial, elevation: 8, blur: 16, opacity: 0.85}
  paper: {swift: regularMaterial, elevation: 0, blur: 0, opacity: 1}
motions:
  settle: {response: 0.4, damping: 0.85}
  fade: {duration: 0.2}
```

All six categories are required (maps other than colors/typography may be empty).
Colors are opaque sRGB `#RRGGBB`. Every role has light and dark values. A `rule`
declares an artwork-derived role; its values are deterministic fallbacks. Agents
implement the palette rule in the app. The tool neither downloads artwork nor runs
palette extraction. Snapshots for a rule-based region must record `resolved_color`,
which the sampled PNG is checked against. Resolved artwork colors must also agree
across the pair within the screen’s ΔE tolerance. This verifies the rendered result against
the supplied resolution, not the correctness of the palette algorithm itself.

Sizes use logical points/dp; type sizes and line heights use points/sp. Weight is
1–1000, mapped to the nearest UIKit weight on Swift and `FontWeight` on Compose.
`dynamic_type` is one of `largeTitle`, `title1`, `title2`, `title3`, `headline`,
`subheadline`, `body`, `callout`, `footnote`, `caption1`, `caption2`. Swift uses
`UIFontMetrics` for the named style and exposes computed SwiftUI `Font` values,
base `CGFloat` size/line-height values, dynamic `Color` roles and explicit light/dark
colors. Compose uses `TextStyle` with `sp` (Android font scale), theme-aware color
getters, explicit colors, and `Dp` geometry. The Swift template targets iOS/UIKit;
it is not a macOS AppKit generator.

Materials generate a native SwiftUI `Material` plus elevation/blur/opacity metadata,
and Compose elevation/blur/opacity values. The native app applies the appropriate
surface and shadow; equal blur numbers do not promise identical compositing.
Motions specify **either** duration in seconds **or** spring response in seconds
and damping ratio. Swift emits `Animation`; Kotlin emits `AnimationSpec<Float>`,
converting response to stiffness as `(2π / response)²` with unit mass. These are
native interpretations of the same intent; their trajectories can differ.

```sh
concordance tokens spec/product --platform swift --out apple/DesignTokens.swift
concordance tokens spec/product --platform kotlin --out android/DesignTokens.kt
concordance tokens spec/product --platform css --out web/tokens.css
```

The object is `DesignTokens`. Members use category prefixes such as `color_surface`,
`type_body`, `spacing_regular`, `radius_artwork`, `material_floating_h_glass`, and
`motion_settle`. Hyphens encode as `_h_`, dots as `_d_`, keeping names unambiguous.
Swift line-height metadata is for the app to apply; a `Font` alone has no line-height
setting. Kotlin outputs have no package declaration; place a generated copy in the
appropriate package or add the declaration when adopting it.

### CSS output

`--platform css` writes one file of custom properties named `--<category>-<name>`, with
dots in token names written as hyphens (`--color-accent`, `--spacing-regular`). Two
tokens that map to the same property are an error. Light colors and every non-color
token go under `:root`; dark colors go under
`@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { … } }` and again
under `:root[data-theme="dark"]`; light colors repeat under `:root[data-theme="light"]`,
so an explicit `data-theme` wins over the system setting either way. Colors are lowercase
`#rrggbb`, spacing, radius, type size, line height, blur and elevation are `px`, weight
and opacity are unitless, and durations are `ms`. Typography emits `-size`,
`-line-height` and `-weight`; materials emit `-elevation`, `-blur` and `-opacity`. Spring
motions, `dynamic_type` and the Swift material name have no CSS form and are skipped
with a comment line naming them. The file starts with a header comment naming the
catalog path as given, and uses the same never-overwrite rule below. The token schema is
unchanged: it already carries light and dark values.

Both Swift/Kotlin commands use exclusive file creation, just like `journeys`. Existing files
are **never overwritten**, including stale generated files. Reruns report created,
unchanged and preserved paths. Generate into a fresh path to review token changes.
Parent directories for token files must already exist.

## Screens and anatomy

```yaml
id: detail
entries: [library.open-item]
tolerances: {geometry: 0.05, delta_e: 5, text_size: 1}
notes:
  ios: Use a native sheet with medium and large detents.
  android: Use a native modal bottom sheet.
anatomy:
  - id: detail.content
    role: surface
    priority: 1
    children:
      - id: detail.artwork
        role: artwork-surface
        priority: 1
        tokens: {color: accent, radius: artwork, spacing: regular, sample: [0.1, 0.1]}
        layout: {min_width: 0.4}
      - id: detail.title
        role: title-block
        priority: 1
        tokens: {typography: title}
        layout: {below: detail.artwork}
      - id: detail.transport
        role: floating-control
        priority: 2
        tokens: {material: floating-glass, motion: settle}
        layout: {below: detail.title}
states:
  populated:
    theme: light
    captures:
      ios: captures/detail.populated.ios.png
      android: captures/detail.populated.android.png
  large-text:
    theme: light
    font_scale: 1.5
    captures:
      ios: references/detail.large-text.ios.png
      android: references/detail.large-text.android.png
```

Anatomy is an ordered tree. Every region has a stable accessibility identifier,
a component `role`, and content `priority` (1 is highest, up to 1000). Roles are
extensible strings: `artwork-surface`, `primary-action`, `floating-control`,
`list-row`, `shelf`, `scrubber`, `sheet`, and so on. The snapshot exporter maps native
semantics onto these component roles. It must retain region containers even when
the native accessibility system normally merges them. Extra native nodes are allowed.

Each region can reference `color`, `typography`, `spacing`, `radius`, `material`, and
`motion` tokens. `sample` is an optional `[x,y]` point relative to the region,
0–1 on each axis; it defaults to the center. Select a stable flat patch rather than
a glyph, image detail or antialiased edge. One point checks one declared color role;
use child regions for distinct colored surfaces.

`layout` supports `min_width`, `max_width`, `min_height`, `max_height` as fractions
of the whole viewport, plus `below` and `right_of` region IDs. Minimum and maximum
bounds are hard requirements. `geometry` is an absolute normalized tolerance for
cross-platform x/y/width/height differences, child containment and relative placement.
For example, 0.05 permits a 5% viewport difference. `below` compares the region's top
to the referenced bottom; `right_of` compares its left to the referenced right.
Anatomy order is checked against snapshot preorder; explicit layout relationships
check spatial order. Overlapping layers can share a parent without a below constraint.

States are named fixtures, not a hard-coded enum. Declare the relevant empty,
loading, populated, error, large-text, dark, compact and wide combinations for
each screen, with `theme`, an optional `font_scale` (default 1), and reference
capture paths for every configured platform. `absent: [region-id]` explicitly
omits individual regions in a state; omit descendants explicitly too. Update layout
relationships if their referenced region must remain present. Per-platform `notes`
on screens and regions explain legitimate native differences; notes do not silently
waive mechanical checks. Choose tolerances that accommodate those adaptations.

Reference paths are relative to the catalog directory, confined to it, and must use
the capture filename convention. References may be missing during implementation.
They identify approved/reference fixtures; `compare` evaluates the current run's
files in `--captures`, with the baseline platform as the geometric comparator.
It does not perform an additional historical-reference image comparison.

## Capturing native implementations

```sh
concordance capture-plan spec/product --platform ios
concordance capture-plan spec/product --platform android \
  --captures artifacts/captures --out android/CaptureTests
```

The plan prints sorted, tab-separated `screen<TAB>state` rows to stdout. Generation
counts and preserved paths go to stderr. Without overrides, captures are inspected
in `<dir>/captures` and stubs go to `<dir>/capture-tests/<platform>`. `ios`/`swift`
select XCTest, `android`/`kotlin` select Compose/JUnit; for another configured
platform, pass `--language swift|kotlin`. Platform names always refer to
`catalog.toml` keys, while `tokens --platform` selects a language.

Each missing PNG **or** JSON generates one test file for that screen/state/platform.
Existing tests are preserved even if edited. Complete filename pairs need no stub;
`compare` detects invalid contents. XCTest skeletons throw `XCTSkip`; Compose
skeletons use `@Ignore` and `TODO`. They cannot silently count as capture coverage.
Add native launch/navigation, state fixtures and screenshot/snapshot export, then
remove the skip. The tool does not run a simulator or invent a generic app harness.

Every run writes these two files into one flat captures directory:

```text
<screen-id>.<state>.<platform>.png
<screen-id>.<state>.<platform>.json
```

For example, this is a complete snapshot for a tiny standalone region:

```json
{
  "width": 200,
  "height": 400,
  "scale": 2,
  "nodes": [
    {
      "identifier": "detail.artwork",
      "role": "artwork-surface",
      "frame": {"x": 10, "y": 20, "width": 80, "height": 80},
      "resolved_color": "#775544"
    }
  ]
}
```

`width`/`height` are PNG pixels; `scale` is pixels per logical point/dp. Frames are
logical units relative to the captured viewport, **not** physical screen coordinates.
Crop system chrome consistently, translate frames to the crop origin, and include
only visible, positive-area nodes. PNG dimensions must agree with the snapshot.
Export anatomy in preorder (parent, then ordered descendants); native nodes may be
interspersed. Identifiers must be unique. A typography region also needs numeric
`text_size`, the rendered logical font size after accessibility scaling. Neither
XCTest nor Compose's raw accessibility dump universally exposes text size or these
component roles; add app-specific test instrumentation for that metadata.

Choose matching viewport aspect ratios, content, locale, font scale, theme, artwork
and UI state. Freeze clocks, networking, animation and fixtures. `font_scale` is the
expected effective multiplier for the captured typography; Apple's style-dependent
Dynamic Type and modern Android nonlinear scaling require deliberate fixtures.
This initial format uses a single effective multiplier per state, so use a tolerance
appropriate for the selected styles or separate fixtures when necessary.

## Comparing and reviewing

```sh
concordance compare spec/product --captures artifacts/captures \
  --baseline ios --target android --out artifacts/ui-report --check
concordance ui-report spec/product --captures artifacts/captures
concordance ui-report spec/product --captures artifacts/captures --check
```

Baseline/target default to `[report]` configuration (normally ios/android).
`compare` writes `index.html`, `comparison.json` and copies of the PNGs into its
report directory. Keep the output directory separate from input captures. The HTML
opens directly as a static file: side-by-side images, toggleable anatomy overlays,
failures by platform, and a short **For reviewers** prompt containing roles, token
usage, priorities and native adaptation notes. Give that report to a vision-capable
agent or human reviewer; judge whether it expresses the same product design.

Mechanical pass/fail checks are:

- Every required region exists exactly once, has its declared role, appears in
  anatomy preorder, and fits its parent. Declared size and relative-position
  constraints hold. Each region's normalized frame agrees across platforms within
  the screen's `geometry` tolerance.
- Declared color samples match the state's light/dark token (or the recorded
  resolved artwork color) within CIE76 ΔE in D65 CIELAB. Samples must be opaque.
  PNG color-management profiles are not applied; export sRGB captures.
- Declared type sizes match `size * font_scale` within `text_size` logical units.
  A missing font measurement fails rather than being inferred from glyph pixels.
- Missing files, invalid PNG/JSON, duplicate IDs and invalid/offscreen frames fail.

The trend score averages SSIM over sixteen 8×8 windows of a 32×32 area-downsampled
luminance image. It reports 0–1; transparent pixels are composited over white for
this score only. It never gates parity, regardless of how high or low it is.
Only a PNG decoder and small Rust calculations are used; no browser engine or ML.
Decoded PNG buffers are limited to 64 MiB.

`compare` normally exits successfully even with drift so reviewers can open the
report; `--check` exits nonzero for structural/token/missing/invalid evidence.
The report is written before that failure. Invalid contracts or I/O failures always
return nonzero. `ui-report` freshly compares every configured platform against the
baseline and writes `<dir>/parity.md` (or `--out`). Its summary counts screen/state
pairs: captured (both files exist), matching (all declared checks pass), drifting
(captured with failures), and missing (either file absent). Invalid pairs count as
drifting. `ui-report --check` checks report freshness without writing, matching
`report --check`; use `compare --check` to gate design parity.

Spacing, radius, material and motion references drive implementation and reviewer
intent. This version does not measure those effects directly, OCR text, detect
font families, inspect line heights or judge artwork composition. Static captures
cannot establish spring behavior. Tight declared geometry and meaningful color
samples make the gate useful; passing it is evidence of those checks, not proof of
identical design. Keep the human/vision review for the remaining design intent.

The Rust API is under `concordance::ui`: `Tokens::load/native/write_native`,
`UiCatalog::load/capture_plan/capture_stubs/compare/ui_report`, and serializable
`Comparison` results. A generic fixture is in `tests/ui-fixtures`; it contains a
matching light pair and a deliberately drifting dark pair.
