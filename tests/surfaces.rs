//! Surfaces: per-surface records, reports, the work queue, `surfaces scan` and the gate.
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "concordance-surfaces-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fn copy(from: &Path, to: &Path) {
            fs::create_dir_all(to).expect("mkdir");
            for e in fs::read_dir(from).expect("fixture") {
                let e = e.expect("entry");
                let dest = to.join(e.file_name());
                if e.path().is_dir() {
                    copy(&e.path(), &dest);
                } else {
                    fs::copy(e.path(), dest).expect("copy");
                }
            }
        }
        let _ = fs::remove_dir_all(&path);
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/surface-fixtures"),
            &path,
        );
        Self(path)
    }
    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
    fn write(&self, relative: &str, text: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }
    fn mutate(&self, area: &str, edit: impl FnOnce(&mut Value)) {
        let path = self.path(&format!("catalog/{area}.yaml"));
        let mut value: Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("JSON");
        edit(&mut value);
        fs::write(
            path,
            serde_json::to_string_pretty(&value).expect("serialize") + "\n",
        )
        .expect("write");
    }
    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(args)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("UTF8")
            .trim()
            .to_owned()
    }
    fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            message,
        ]);
        self.git(&["rev-parse", "HEAD"])
    }
    fn cli(&self, args: &[&str]) -> Output {
        let (command, rest) = args.split_first().expect("command");
        let mut c = Command::new(env!("CARGO_BIN_EXE_concordance"));
        if *command == "surfaces" {
            c.arg("surfaces").arg(rest[0]).arg(&self.0).args(&rest[1..]);
        } else {
            c.arg(command).arg(&self.0).args(rest);
        }
        c.output().expect("CLI")
    }
    fn scan(&self, platform: &str) -> (Option<i32>, Value) {
        let output = self.cli(&[
            "surfaces",
            "scan",
            "--platform",
            platform,
            "--format",
            "json",
        ]);
        let value: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|_| panic!("JSON: {}", text(&output)));
        (output.status.code(), value[0].clone())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn findings(report: &Value, kind: &str) -> BTreeSet<String> {
    report["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter(|f| f["kind"] == kind)
        .map(|f| {
            format!(
                "{}{}",
                f["subject"].as_str().unwrap_or_default(),
                f["surface"]
                    .as_str()
                    .map(|s| format!(" ({s})"))
                    .unwrap_or_default()
            )
        })
        .collect()
}
fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

const QUEUE_WIDGET: &str = r#"import WidgetKit
struct QueueWidget: Widget {
    var body: some WidgetConfiguration {
        AppIntentConfiguration(kind: "com.example.queue", intent: QueueIntent.self, provider: QueueProvider()) { e in QueueView(e) }
            .supportedFamilies([.systemSmall])
    }
}
"#;

#[test]
fn scan_lists_declarations_and_compares_them_with_the_catalog() {
    let f = Fixture::new();
    let (code, ios) = f.scan("ios");
    assert_eq!(code, Some(1));
    let declared: BTreeSet<String> = ios["declarations"]
        .as_array()
        .expect("declarations")
        .iter()
        .map(|d| {
            let surfaces: Vec<&str> = d["surfaces"]
                .as_array()
                .expect("surfaces")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            format!(
                "{}{}{}#{} -> {}",
                d["signal"].as_str().unwrap_or_default(),
                d["detail"]
                    .as_str()
                    .map(|x| format!(":{x}"))
                    .unwrap_or_default(),
                d["context"]
                    .as_str()
                    .map(|x| format!("@{x}"))
                    .unwrap_or_default(),
                d["id"].as_str().unwrap_or_default(),
                surfaces.join(",")
            )
        })
        .collect();
    // DebugWidget is ignored; the commented-out widget is not a declaration; CarPlay from
    // Swift and from Info.plist is one declaration.
    assert_eq!(
        declared,
        set(&[
            "apple.app-shortcut#ResumeIntent -> app-shortcut",
            "apple.carplay#CarSceneDelegate -> car",
            "apple.control#com.example.play-pause -> ",
            "apple.quick-action#com.example.resume -> quick-action",
            "apple.quick-action#com.example.search -> quick-action",
            "apple.watch-app@watch#com.example.player.watchkitapp -> watch-app",
            "apple.widget:accessoryCircular#com.example.now-playing -> lock-screen",
            "apple.widget:accessoryCircular@watch#com.example.watch-now-playing -> complication",
            "apple.widget:accessoryCorner@watch#com.example.watch-now-playing -> complication",
            "apple.widget:systemMedium#com.example.now-playing -> widget.medium",
            "apple.widget:systemSmall#com.example.now-playing -> widget.small",
        ])
    );
    assert_eq!(
        findings(&ios, "uncatalogued"),
        set(&[
            "apple.quick-action#com.example.search (quick-action)",
            "apple.watch-app@watch#com.example.player.watchkitapp (watch-app)",
            "apple.widget:accessoryCircular#com.example.now-playing (lock-screen)",
        ])
    );
    assert_eq!(
        findings(&ios, "unmapped"),
        set(&["apple.control#com.example.play-pause"])
    );
    assert_eq!(
        findings(&ios, "no-longer-declared"),
        set(&["widgets.now-playing@lock-screen (lock-screen)"])
    );

    let (code, android) = f.scan("android");
    assert_eq!(code, Some(1));
    let widget = android["declarations"]
        .as_array()
        .expect("declarations")
        .iter()
        .find(|d| d["signal"] == "android.widget")
        .expect("widget")
        .clone();
    // 250 dp × 110 dp is four cells by two: a medium widget, so the 2×2 is missing.
    assert_eq!(widget["detail"], "4x2");
    assert_eq!(widget["surfaces"], json!(["widget.medium"]));
    assert_eq!(
        findings(&android, "uncatalogued"),
        set(&[
            "android.complication@wear#.complication.ProgressComplication (complication)",
            "android.wear-app@wear#com.example.player.wear (watch-app)",
            "android.wear-tile@wear#.tile.NowPlayingTile (wear-tile)",
        ])
    );
    assert_eq!(
        findings(&android, "unmapped"),
        set(&["android.qs-tile#.tile.PlayPauseTile"])
    );
    assert_eq!(
        findings(&android, "status-disagrees"),
        set(&[
            "car.browse@car (car)",
            "player.resume@app-shortcut (app-shortcut)"
        ])
    );
    assert!(findings(&android, "no-longer-declared").is_empty());

    // Markdown names each finding; lines in Info.plist are real.
    let output = f.cli(&["surfaces", "scan", "--platform", "ios"]);
    let out = text(&output);
    assert!(out.contains("## Declared surfaces: iOS"), "{out}");
    assert!(
        out.contains("`apple.quick-action#com.example.search` | `App/Info.plist:17` |"),
        "{out}"
    );
    assert!(out.contains("`Watch/Info.plist:7` |"), "{out}");
    assert!(out.contains("### No longer declared"), "{out}");
}

#[test]
fn catalogued_surfaces_pass_the_scan() {
    let f = Fixture::new();
    // Map the control, catalogue the watch, search and Lock Screen; drop the stale ID.
    let config = fs::read_to_string(f.path("catalog.toml")).expect("config");
    fs::write(
        f.path("catalog.toml"),
        config + "\n[surfaces.control]\nplatforms = [\"ios\", \"android\"]\n",
    )
    .expect("config");
    f.mutate("widgets", |v| {
        let ios = &mut v[0]["platforms"]["ios"]["surfaces"];
        ios["lock-screen"]["declared"] = json!(["com.example.now-playing"]);
    });
    f.mutate("player", |v| {
        v[0]["surfaces"] = json!([
            "now-playing",
            "quick-action",
            "app-shortcut",
            "control",
            "watch-app"
        ]);
        let ios = &mut v[0]["platforms"]["ios"]["surfaces"];
        ios["quick-action"]["declared"] = json!(["com.example.resume", "com.example.search"]);
        ios["control"] = json!({"status": "implemented", "declared": ["com.example.play-pause"]});
        ios["watch-app"] = json!({"status": "implemented"});
    });
    let (code, ios) = f.scan("ios");
    assert_eq!(code, Some(0), "{ios}");
    assert_eq!(
        findings(&ios, "coarse"),
        set(&[
            "apple.carplay#CarSceneDelegate (car)",
            "apple.watch-app@watch#com.example.player.watchkitapp (watch-app)",
        ])
    );
    let strict = f.cli(&["surfaces", "scan", "--platform", "ios", "--strict"]);
    assert_eq!(strict.status.code(), Some(1));
    // The report counts the new records; android now owes the control and the watch.
    let report = f.cli(&[
        "report",
        "--out",
        f.path("parity.md").to_str().expect("path"),
    ]);
    assert_eq!(report.status.code(), Some(0), "{}", text(&report));
    let parity = fs::read_to_string(f.path("parity.md")).expect("report");
    assert!(
        parity.contains("| Watch app (`watch-app`) | 1/1 | 0/1 · 1 gap |"),
        "{parity}"
    );
    assert!(
        parity.contains("| `control` | Android | `player.resume` | unrecorded |  |"),
        "{parity}"
    );
}

#[test]
fn reports_queue_and_release_gate_understand_surfaces() {
    let f = Fixture::new();
    let out_path = f.path("parity.md");
    let output = f.cli(&["report", "--out", out_path.to_str().expect("path")]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let parity = fs::read_to_string(&out_path).expect("report");
    for row in [
        "| Surface | iOS | Android |",
        "| Small widget (`widget.small`) | 1/1 | 0/1 · 1 gap |",
        "| Medium widget (`widget.medium`) | 1/1 | 1/1 |",
        "| Lock Screen widget (`lock-screen`) | 1/1 | — |",
        "| Watch app (`watch-app`) | none catalogued | none catalogued |",
        "| `widget.small` | Android | `widgets.now-playing` | missing | No 2×2 provider. |",
        "| `complication` | Android | `widgets.now-playing` | unrecorded |  |",
    ] {
        assert!(parity.contains(row), "missing {row:?} in\n{parity}");
    }

    let next = text(&f.cli(&["next", "--platform", "android"]));
    let lines: Vec<&str> = next.lines().collect();
    assert_eq!(
        lines,
        [
            "widgets.now-playing\thigh\tpartial\tShow the playing episode in widgets",
            "widgets.now-playing@widget.small\tsurface\tmissing\tSmall widget",
            "widgets.now-playing@complication\tsurface\tunrecorded\tComplication",
            "player.resume\tmedium\timplemented\tResume playback",
            "player.resume@app-shortcut\tsurface\tmissing\tApp shortcut",
            "car.browse\tmedium\timplemented\tBrowse the library in the car",
            "car.browse@car\tsurface\tmissing\tCarPlay and Android Auto",
        ]
    );
    let only = text(&f.cli(&["next", "--platform", "android", "--surface", "car"]));
    assert_eq!(
        only.lines().collect::<Vec<_>>(),
        [
            "car.browse\tmedium\timplemented\tBrowse the library in the car",
            "car.browse@car\tsurface\tmissing\tCarPlay and Android Auto",
        ]
    );
    let old = text(&f.cli(&["next", "--platform", "android", "--no-surfaces"]));
    assert_eq!(
        old.lines().collect::<Vec<_>>(),
        ["widgets.now-playing\thigh\tpartial\tShow the playing episode in widgets"]
    );
    assert_eq!(
        f.cli(&["next", "--platform", "android", "--surface", "nope"])
            .status
            .code(),
        Some(1)
    );

    // car is release = "required": Android's missing record fails the release gate.
    let release = f.cli(&[
        "gate-release",
        "--platform",
        "android",
        "--max-age-days",
        "30",
    ]);
    assert_eq!(release.status.code(), Some(1), "{}", text(&release));
    assert!(text(&release).contains("| `car.browse@car` | `car` | missing |"));
    let release = f.cli(&["gate-release", "--platform", "ios", "--max-age-days", "30"]);
    assert_eq!(release.status.code(), Some(0), "{}", text(&release));
    // A stale surface record beyond the limit fails like any stale record.
    f.mutate("widgets", |v| {
        v[0]["platforms"]["ios"]["surfaces"]["widget.small"] = json!({
            "status": "stale", "previous_status": "implemented",
            "since_commit": "0123456789abcdef0123456789abcdef01234567", "since_date": "2026-01-01"
        });
    });
    let release = f.cli(&[
        "gate-release",
        "--platform",
        "ios",
        "--max-age-days",
        "30",
        "--today",
        "2026-03-01",
    ]);
    assert_eq!(release.status.code(), Some(1), "{}", text(&release));
    assert!(text(&release).contains("`widgets.now-playing@widget.small`"));
}

#[test]
fn set_edits_surface_records() {
    let f = Fixture::new();
    let output = f.cli(&[
        "set",
        "widgets.now-playing@complication",
        "--platform",
        "android",
        "--status",
        "missing",
        "--gap",
        "No Wear OS complication.",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let value: Value =
        serde_json::from_str(&fs::read_to_string(f.path("catalog/widgets.yaml")).expect("read"))
            .expect("JSON");
    assert_eq!(
        value[0]["platforms"]["android"]["surfaces"]["complication"],
        json!({"status": "missing", "gap": "No Wear OS complication."})
    );
    // A surface record never invents the entry's own platform record.
    f.mutate("car", |v| {
        v[0]["platforms"]
            .as_object_mut()
            .expect("platforms")
            .remove("android");
    });
    let output = f.cli(&[
        "set",
        "car.browse@car",
        "--platform",
        "android",
        "--status",
        "implemented",
    ]);
    assert_ne!(output.status.code(), Some(0), "{}", text(&output));
    assert!(
        text(&output).contains("has no android record"),
        "{}",
        text(&output)
    );
    // A surface the entry does not name is refused and the edit reverted.
    let output = f.cli(&[
        "set",
        "widgets.now-playing@car",
        "--platform",
        "android",
        "--status",
        "missing",
    ]);
    assert_ne!(output.status.code(), Some(0), "{}", text(&output));
    let after = fs::read_to_string(f.path("catalog/widgets.yaml")).expect("read");
    assert!(!after.contains("\"car\""));
}

#[test]
fn validation_rejects_misplaced_surface_records() {
    let f = Fixture::new();
    f.mutate("widgets", |v| {
        let android = &mut v[0]["platforms"]["android"];
        android["surfaces"]["lock-screen"] = json!({"status": "missing"});
        android["surfaces"]["menu-bar"] = json!({"status": "missing"});
        android["surfaces"]["widget.medium"]["surfaces"] =
            json!({"widget.small": {"status": "missing"}});
        android["declared"] = json!(["x"]);
        v[0]["platforms"]["ios"]["surfaces"]["car"] = json!({"status": "missing"});
        v[0]["details"] = json!([{
            "id": "artwork", "kind": "layout", "spec": {"text": "Artwork fills the widget"},
            "platforms": {"ios": {"status": "implemented", "surfaces": {"widget.small": {"status": "implemented"}}}}
        }]);
    });
    let out = text(&f.cli(&["validate"]));
    for message in [
        "the surface does not list this platform",
        "not a [surfaces] ID in catalog.toml",
        "surface records cannot nest",
        "declared belongs on surface records",
        "the entry's surfaces list does not name this surface",
        "surface records belong to entry platform records, not details",
    ] {
        assert!(out.contains(message), "missing {message:?} in\n{out}");
    }
    let f = Fixture::new();
    let config = fs::read_to_string(f.path("catalog.toml")).expect("config");
    fs::write(
        f.path("catalog.toml"),
        config
            + "\n[surfaces.Bad]\nplatforms = [\"tv\"]\nsignals = [\"widget\"]\n[surface_scan.tv]\nswift = [\"../x\"]\n",
    )
    .expect("config");
    let out = text(&f.cli(&["validate"]));
    for message in [
        "invalid surface ID",
        "unknown platform",
        "signal must be",
        "unsafe surface_scan glob",
    ] {
        assert!(out.contains(message), "missing {message:?} in\n{out}");
    }
}

#[test]
fn gate_fails_new_uncatalogued_and_removed_declarations() {
    let f = Fixture::new();
    f.git(&["init", "-q"]);
    let base = f.commit("base");
    let gate = |platform: &str, base: &str, head: &str| {
        f.cli(&[
            "gate",
            "--platform",
            platform,
            "--base",
            base,
            "--head",
            head,
        ])
    };
    // Existing uncatalogued declarations are the scan's backlog, not the gate's.
    f.write("README.md", "notes\n");
    let unrelated = f.commit("Unrelated");
    let output = gate("ios", &base, &unrelated);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));

    // A new widget kind fails until it is catalogued (a non-ASCII file name, read from Git).
    f.write("apple/Widgets/QueueWidgét.swift", QUEUE_WIDGET);
    let added = f.commit("Add a queue widget");
    let output = gate("ios", &unrelated, &added);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    let out = text(&output);
    assert!(out.contains("### Declared surfaces"), "{out}");
    assert!(
        out.contains(
            "new, uncatalogued: `apple.widget:systemSmall#com.example.queue` (`widget.small`)"
        ),
        "{out}"
    );
    f.mutate("widgets", |v| {
        v[0]["platforms"]["ios"]["surfaces"]["widget.small"]["declared"] =
            json!(["com.example.now-playing", "com.example.queue"]);
    });
    let catalogued = f.commit("Catalogue the queue widget");
    let output = gate("ios", &unrelated, &catalogued);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));

    // Removing a declaration the catalog still claims fails.
    let plist = fs::read_to_string(f.path("apple/App/Info.plist")).expect("plist");
    let start = plist.find("\t\t<dict>\n\t\t\t<key>UIApplicationShortcutItemType</key>\n\t\t\t<string>com.example.resume").expect("resume");
    let end = start + plist[start..].find("</dict>\n").expect("end") + "</dict>\n".len();
    fs::write(
        f.path("apple/App/Info.plist"),
        format!("{}{}", &plist[..start], &plist[end..]),
    )
    .expect("plist");
    let removed = f.commit("Drop the resume quick action");
    let output = gate("ios", &catalogued, &removed);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("removed: `player.resume@quick-action` (`quick-action`)"),
        "{}",
        text(&output)
    );
    // The waiver trailer lets it through; the report still lists it.
    let waived = f.commit("Explain\n\nCatalog: none — quick action retired in a later change");
    let output = gate("ios", &catalogued, &waived);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(text(&output).contains("removed: `player.resume@quick-action`"));

    // A new Android 2×2 widget (target cells) fails until catalogued.
    f.write(
        "android/app/src/main/res/xml/small_widget.xml",
        "<appwidget-provider xmlns:android=\"http://schemas.android.com/apk/res/android\" android:targetCellWidth=\"2\" android:targetCellHeight=\"2\" android:minWidth=\"@dimen/w\" android:minHeight=\"40dp\"/>\n",
    );
    let manifest =
        fs::read_to_string(f.path("android/app/src/main/AndroidManifest.xml")).expect("manifest");
    f.write(
        "android/app/src/main/AndroidManifest.xml",
        &manifest.replace(
            "    </application>",
            "        <receiver android:name=\".widget.SmallWidget\">\n            <meta-data android:name=\"android.appwidget.provider\" android:resource=\"@xml/small_widget\" />\n        </receiver>\n    </application>",
        ),
    );
    // Patch mode: the working tree against HEAD.
    let patch = f.git(&["diff"]);
    f.write("change.patch", &(patch + "\n"));
    let output = f.cli(&[
        "gate",
        "--platform",
        "android",
        "--patch",
        f.path("change.patch").to_str().expect("path"),
    ]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    // widget.small on Android is recorded missing, so the new declaration contradicts it.
    let out = text(&output);
    assert!(
        out.contains(
            "new, but recorded as not there: `widgets.now-playing@widget.small` (`widget.small`)"
        ) && out.contains("`android.widget:2x2#.widget.SmallWidget` is declared"),
        "{out}"
    );
}

#[test]
fn watch_widgets_default_to_configured_families() {
    let f = Fixture::new();
    let config = fs::read_to_string(f.path("catalog.toml")).expect("config");
    fs::write(
        f.path("catalog.toml"),
        config.replace(
            "contexts = { watch = [\"Watch/**\"] }",
            "contexts = { watch = [\"Watch/**\"] }\ndefault_families = { watch = [\"accessoryCircular\", \"accessoryRectangular\"] }",
        ),
    )
    .expect("config");
    f.write(
        "apple/Watch/Queue.swift",
        "struct WatchQueue: Widget {\n    var body: some WidgetConfiguration {\n        StaticConfiguration(kind: \"com.example.watch-queue\", provider: P()) { _ in V() }\n    }\n}\n",
    );
    let (_, ios) = f.scan("ios");
    let keys: BTreeSet<String> = ios["declarations"]
        .as_array()
        .expect("declarations")
        .iter()
        .filter(|d| d["id"] == "com.example.watch-queue")
        .map(|d| {
            format!(
                "{} {}",
                d["detail"].as_str().unwrap_or_default(),
                d["surfaces"]
            )
        })
        .collect();
    assert_eq!(
        keys,
        set(&[
            "accessoryCircular [\"complication\"]",
            "accessoryRectangular [\"complication\"]",
        ])
    );
}

#[test]
fn catalogs_without_surfaces_are_unchanged() {
    // The original fixture has no [surfaces]; its report has no Surfaces section.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let out = std::env::temp_dir().join(format!("concordance-nosurf-{}.md", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_concordance"))
        .args(["report"])
        .arg(&dir)
        .arg("--out")
        .arg(&out)
        .output()
        .expect("CLI");
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let report = fs::read_to_string(&out).expect("report");
    let _ = fs::remove_file(&out);
    assert!(!report.contains("## Surfaces"));
    let scan = Command::new(env!("CARGO_BIN_EXE_concordance"))
        .args(["surfaces", "scan"])
        .arg(&dir)
        .output()
        .expect("CLI");
    assert_eq!(scan.status.code(), Some(2));
}
