# Inventory: fixture

| Platform | Root | Files cited | Identifiers cited | Strings cited | Tests cited |
| --- | --- | --- | --- | --- | --- |
| android | `android` | 1/3 | 1/3 | 1/3 | 0/1 |
| ios | `apple` | 1/4 | 1/3 | 1/5 | 1/1 |

Files are user-visible sources (`[source_globs] user_visible`, else `all`); areas are candidate areas named after feature folders.

## android

| Area | Files cited | Identifiers cited | Strings cited | Tests | Entries |
| --- | --- | --- | --- | --- | --- |
| app | 0/1 | 0/0 | 0/0 | 0 | 0 |
| home | 1/1 | 1/2 | 1/1 | 1 | 1 |
| player | 0/1 | 0/1 | 0/1 | 0 | 0 |

### Files (3)

- `app/src/main/kotlin/com/example/fixture/MainActivity.kt` (app) — not cited
- `app/src/main/kotlin/com/example/fixture/home/HomeScreen.kt` (home) — cited by home.browse-items
- `app/src/main/kotlin/com/example/fixture/player/PlayerScreen.kt` (player) — not cited

### Identifiers (3)

- `home.list` (home; app/src/main/kotlin/com/example/fixture/home/HomeScreen.kt) — cited by home.browse-items
- `home.row.*` (home; app/src/main/kotlin/com/example/fixture/home/HomeScreen.kt) — not cited
- `player.play` (player; app/src/main/kotlin/com/example/fixture/player/PlayerScreen.kt) — not cited

### Strings (3)

- `app_name` = `Fixture` (app/src/main/res/values/strings.xml; unused in scanned source) — not cited
- `home_empty` = `Nothing here yet` (app/src/main/res/values/strings.xml; home) — cited by home.browse-items
- `player_play` = `Play` (app/src/main/res/values/strings.xml; player) — not cited

### Tests (1)

- `app/src/test/kotlin/com/example/fixture/HomeTest.kt` (home) — not cited

## ios

| Area | Files cited | Identifiers cited | Strings cited | Tests | Entries |
| --- | --- | --- | --- | --- | --- |
| app | 0/1 | 0/0 | 0/0 | 0 | 0 |
| home | 1/2 | 1/2 | 1/2 | 1 | 1 |
| player | 0/1 | 0/1 | 0/2 | 0 | 0 |

### Files (4)

- `Fixture/FixtureApp.swift` (app) — not cited
- `Fixture/Home/HomeView.swift` (home) — cited by home.browse-items
- `Fixture/Home/Item.swift` (home) — not cited
- `Fixture/Player/PlayerView.swift` (player) — not cited

### Identifiers (3)

- `home.list` (home; Fixture/Home/HomeView.swift) — cited by home.browse-items
- `home.row.*` (home; Fixture/Home/HomeView.swift) — not cited
- `player.play` (player; Fixture/Player/PlayerView.swift) — not cited

### Strings (5)

- `%lld episodes` (Fixture/Resources/Localizable.xcstrings; home) — not cited
- `Nothing here yet` (Fixture/Resources/Localizable.xcstrings; home) — cited by home.browse-items
- `Pause` (Fixture/Resources/Localizable.xcstrings; player) — not cited
- `Play` (Fixture/Resources/Localizable.xcstrings; player) — not cited
- `Unused string` (Fixture/Resources/Localizable.xcstrings; unused in scanned source) — not cited

### Tests (1)

- `FixtureUITests/HomeUITests.swift` (home) — cited by home.browse-items
