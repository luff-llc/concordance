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
concordance report path/to/catalog --out parity.md
concordance next path/to/catalog --platform android --limit 10
concordance entry path/to/catalog player.skip-forward --format brief
concordance ids path/to/catalog --platform kotlin --out Ids.kt --package org.example
concordance journeys path/to/catalog --platform swift --out Tests/Journeys
concordance tokens path/to/catalog --platform swift --out DesignTokens.swift
concordance tokens path/to/catalog --platform css --out tokens.css
concordance capture-plan path/to/catalog --platform android
concordance compare path/to/catalog --captures captures --out ui-report --check
concordance ui-report path/to/catalog --captures captures
```

The format is in [docs/format.md](docs/format.md) and the JSON Schema in
[schema/catalog.schema.json](schema/catalog.schema.json).

Concordance is a sibling of [Colophon](https://github.com/luff-llc/colophon), LUFF's
open standard for user-owned, end-to-end encrypted app data. Colophon keeps every
platform's data in agreement; Concordance keeps every platform's behaviour in agreement.

Apache-2.0.
