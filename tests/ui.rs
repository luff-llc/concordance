use concordance::{
    ui::{Tokens, UiCatalog},
    Catalog, Language,
};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "concordance-ui-{}-{}",
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
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ui-fixtures"),
            &path,
        );
        Self(path)
    }
    fn ui(&self) -> UiCatalog {
        UiCatalog::load(&self.0).expect("UI fixture")
    }
    fn edit(&self, path: &str, edit: impl FnOnce(&mut Value)) {
        let file = self.0.join(path);
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(&file).expect("read")).expect("YAML");
        let mut value = serde_json::to_value(yaml).expect("JSON");
        edit(&mut value);
        fs::write(
            file,
            serde_json::to_string_pretty(&value).expect("serialize"),
        )
        .expect("write");
    }
    fn cli(&self, command: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_concordance"))
            .arg(command)
            .arg(&self.0)
            .args(args)
            .output()
            .expect("CLI")
    }
    fn matching_only(&self) {
        self.edit("screens/demo.yaml", |s| {
            s["states"].as_object_mut().expect("states").remove("dark");
        });
    }
    fn compare(&self) -> concordance::ui::Comparison {
        self.ui()
            .compare(&self.0.join("captures"), "ios", "android", None)
            .expect("compare")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF8")
}

#[test]
fn fixture_matching_and_drifting_pairs_report_and_check() {
    let f = Fixture::new();
    success(f.cli("validate", &[]));
    let ui = f.ui();
    let out = f.0.join("report");
    let c = ui
        .compare(&f.0.join("captures"), "ios", "android", Some(&out))
        .expect("compare");
    let matching = c
        .pairs
        .iter()
        .find(|p| p.state == "populated")
        .expect("matching");
    assert!(matching.baseline.failures.is_empty());
    assert!(matching.target.failures.is_empty());
    assert_eq!(matching.similarity, Some(1.0));
    let drift = c.pairs.iter().find(|p| p.state == "dark").expect("drift");
    assert!(drift.baseline.failures.is_empty());
    let failures = drift.target.failures.join("\n");
    for expected in [
        "order",
        "width",
        "below",
        "color",
        "text_size",
        "differs from ios",
    ] {
        assert!(failures.contains(expected), "{failures}");
    }
    assert!(c.failed());
    let html = fs::read_to_string(out.join("index.html")).expect("HTML");
    for expected in [
        "<svg",
        "<rect",
        "For reviewers",
        "native modal bottom sheet",
        "captures/demo.populated.ios.png",
    ] {
        assert!(html.contains(expected), "{expected}");
    }
    assert!(out.join("captures/demo.dark.android.png").exists());
    assert!(out.join("comparison.json").exists());
    let summary = c.summary();
    assert!(summary.contains("| ios | 2 | 2 | 0 | 0 |"));
    assert!(summary.contains("| android | 2 | 1 | 1 | 0 |"));
    let captures = f.0.join("captures");
    let args = [
        "--captures",
        captures.to_str().expect("path"),
        "--out",
        out.to_str().expect("path"),
        "--check",
    ];
    assert!(!f.cli("compare", &args).status.success());
    assert!(f.cli("compare", &args[..4]).status.success());
    f.matching_only();
    assert!(f.cli("compare", &args).status.success());
    success(f.cli("ui-report", &[]));
    success(f.cli("ui-report", &["--check"]));
    let report = fs::read_to_string(f.0.join("parity.md")).expect("report");
    assert!(report.contains("Native parity"));
    assert!(report.contains("## Screen parity"));
    assert!(report.contains("| android | 1 | 1 | 0 | 0 |"));
}

#[test]
fn captures_generate_skipped_stubs_only_for_missing_and_preserve_edits() {
    let f = Fixture::new();
    let ui = f.ui();
    let captures = f.0.join("captures");
    let out = f.0.join("stubs");
    assert_eq!(
        ui.capture_plan("ios").expect("plan"),
        "demo\tdark\ndemo\tpopulated\n"
    );
    assert!(ui
        .capture_stubs("ios", Language::Swift, &captures, &out)
        .expect("complete")
        .created
        .is_empty());
    fs::remove_file(captures.join("demo.populated.ios.json")).expect("missing JSON");
    let r = ui
        .capture_stubs("ios", Language::Swift, &captures, &out)
        .expect("stub");
    assert_eq!(r.created.len(), 1);
    let file = &r.created[0];
    let text = fs::read_to_string(file).expect("Swift");
    assert!(text.contains("XCTSkip"));
    assert!(text.contains("demo.populated.ios.json"));
    assert_eq!(
        ui.capture_stubs("ios", Language::Swift, &captures, &out)
            .expect("rerun")
            .unchanged
            .len(),
        1
    );
    fs::write(file, "// implemented").expect("edit");
    assert_eq!(
        ui.capture_stubs("ios", Language::Swift, &captures, &out)
            .expect("preserved")
            .preserved
            .len(),
        1
    );
    assert_eq!(fs::read_to_string(file).expect("read"), "// implemented");
    fs::remove_file(captures.join("demo.dark.android.png")).expect("missing PNG");
    let r = ui
        .capture_stubs("android", Language::Kotlin, &captures, &out)
        .expect("Kotlin");
    let text = fs::read_to_string(&r.created[0]).expect("Kotlin");
    assert!(text.contains("@Ignore"));
    assert!(text.contains("createComposeRule"));
    let plan = success(f.cli("capture-plan", &["--platform", "ios"]));
    assert_eq!(plan, "demo\tdark\ndemo\tpopulated\n");
    let c = f.compare();
    assert!(c.summary().contains("| ios | 1 | 1 | 0 | 1 |"));
    assert!(c.summary().contains("| android | 1 | 0 | 1 | 1 |"));
    assert!(ui.capture_plan("../../oops").is_err());
}

#[test]
fn native_tokens_generate_light_dark_scaling_materials_motions_and_preserve() {
    let f = Fixture::new();
    let tokens = Tokens::load(&f.0).expect("tokens");
    for (lang, ext) in [(Language::Swift, "swift"), (Language::Kotlin, "kt")] {
        let native = tokens.native(lang).expect("native");
        for expected in [
            "color_accent_light",
            "color_accent_dark",
            "type_body",
            "spacing_regular",
            "radius_artwork",
            "material_floating_h_glass",
            "motion_settle",
            "motion_fade",
            "Artwork rule",
        ] {
            assert!(native.contains(expected), "{expected}");
        }
        if lang == Language::Swift {
            assert!(native.contains("UIFontMetrics"));
            assert!(native.contains("public static var type_body: Font"));
            assert!(native.contains("Animation.spring"));
        } else {
            assert!(native.contains("16.sp"));
            assert!(native.contains("Color(0xFF336699)"));
            assert!(native.contains("dampingRatio"));
        }
        let out = f.0.join(format!("Tokens.{ext}"));
        assert_eq!(
            tokens
                .write_native(lang, &out)
                .expect("generate")
                .created
                .len(),
            1
        );
        assert_eq!(
            tokens
                .write_native(lang, &out)
                .expect("repeat")
                .unchanged
                .len(),
            1
        );
        fs::write(&out, "// edited").expect("edit");
        assert_eq!(
            tokens
                .write_native(lang, &out)
                .expect("preserve")
                .preserved
                .len(),
            1
        );
        assert_eq!(fs::read_to_string(&out).expect("read"), "// edited");
    }
    let out = f.0.join("Native.swift");
    success(f.cli(
        "tokens",
        &["--platform", "swift", "--out", out.to_str().expect("path")],
    ));
    // Flat behavior catalogs must reserve tokens.yaml rather than treat it as an area.
    fs::rename(f.0.join("catalog/demo.yaml"), f.0.join("demo.yaml")).expect("flat");
    fs::remove_dir(f.0.join("catalog")).expect("flat");
    assert_eq!(Catalog::load(&f.0).expect("flat tokens").entries.len(), 1);
}

#[test]
fn schemas_and_cross_references_reject_invalid_ui_contracts() {
    let cases = [
        ("tokens.yaml", "/colors/accent/light", json!("#oops")),
        ("tokens.yaml", "/colors/accent/extra", json!(1)),
        ("tokens.yaml", "/typography/body/size", json!(-1)),
        (
            "tokens.yaml",
            "/typography/body/dynamic_type",
            json!("custom"),
        ),
        ("tokens.yaml", "/motions/settle/duration", json!(0.5)),
        ("tokens.yaml", "/materials/paper/opacity", json!(2)),
        ("tokens.yaml", "/spacing/regular", json!(-1)),
        ("screens/demo.yaml", "/id", json!("../escape")),
        ("screens/demo.yaml", "/entries", json!(["unknown.entry"])),
        (
            "screens/demo.yaml",
            "/anatomy/0/children/0/tokens/color",
            json!("missing"),
        ),
        (
            "screens/demo.yaml",
            "/anatomy/0/children/0/id",
            json!("title"),
        ),
        (
            "screens/demo.yaml",
            "/anatomy/0/children/0/tokens/sample",
            json!([0.5]),
        ),
        (
            "screens/demo.yaml",
            "/anatomy/0/children/0/layout/below",
            json!("unknown"),
        ),
        (
            "screens/demo.yaml",
            "/anatomy/0/children/0/layout/max_width",
            json!(0.1),
        ),
        (
            "screens/demo.yaml",
            "/states/dark/captures/ios",
            json!("../demo.dark.ios.png"),
        ),
        (
            "screens/demo.yaml",
            "/states/dark/captures/ios",
            json!("captures/wrong.png"),
        ),
        ("screens/demo.yaml", "/states/dark/font_scale", json!(0)),
        (
            "screens/demo.yaml",
            "/states/dark/absent",
            json!(["unknown"]),
        ),
        ("screens/demo.yaml", "/notes/unknown", json!("native")),
    ];
    for (path, pointer, value) in cases {
        let f = Fixture::new();
        f.edit(path, |v| {
            // serde_json pointer_mut does not insert missing object fields.
            let (parent, key) = pointer.rsplit_once('/').expect("pointer");
            v.pointer_mut(parent)
                .expect("parent")
                .as_object_mut()
                .expect("object")
                .insert(key.into(), value);
        });
        assert!(UiCatalog::load(&f.0).is_err(), "accepted {path} {pointer}");
    }
    let f = Fixture::new();
    let text = fs::read_to_string(f.0.join("tokens.yaml")).expect("tokens");
    fs::write(f.0.join("tokens.yaml"), format!("{text}\ncolors: {{}}\n")).expect("duplicate");
    assert!(Tokens::load(&f.0).is_err());
    let f = Fixture::new();
    fs::copy(
        f.0.join("screens/demo.yaml"),
        f.0.join("screens/duplicate.yaml"),
    )
    .expect("duplicate screen");
    assert!(UiCatalog::load(&f.0).is_err());
}

#[test]
fn structural_snapshot_failures_are_actionable_and_do_not_abort_report() {
    let cases = [
        ("/nodes/0/identifier", json!("artwork"), "duplicate"),
        ("/nodes/1/frame/x", json!(-1), "frame"),
        ("/width", json!(64), "dimensions"),
        ("/scale", json!(0), "scale"),
        ("/nodes/1/role", json!("wrong"), "role"),
        (
            "/nodes/1/identifier",
            json!("missing"),
            "missing region artwork",
        ),
    ];
    for (pointer, value, expected) in cases {
        let f = Fixture::new();
        f.matching_only();
        f.edit("captures/demo.populated.android.json", |v| {
            *v.pointer_mut(pointer).expect("pointer") = value;
        });
        let c = f.compare();
        assert!(c.failed());
        assert!(
            c.pairs[0].target.failures.join(" ").contains(expected),
            "{c:?}"
        );
    }
    let f = Fixture::new();
    f.matching_only();
    fs::write(
        f.0.join("captures/demo.populated.android.png"),
        "broken PNG",
    )
    .expect("invalid");
    assert!(f.compare().pairs[0].target.failures[0].contains("invalid PNG"));
}

#[test]
fn scales_are_normalized_and_missing_text_measurement_fails() {
    let f = Fixture::new();
    f.matching_only();
    f.edit("captures/demo.populated.android.json", |v| {
        v["scale"] = json!(2);
        for node in v["nodes"].as_array_mut().expect("nodes") {
            for key in ["x", "y", "width", "height"] {
                node["frame"][key] = json!(node["frame"][key].as_f64().expect("frame") / 2.0);
            }
        }
    });
    assert!(!f.compare().failed());
    f.edit("captures/demo.populated.android.json", |v| {
        v["nodes"][2]
            .as_object_mut()
            .expect("node")
            .remove("text_size");
    });
    assert!(f.compare().pairs[0]
        .target
        .failures
        .iter()
        .any(|f| f.contains("text_size")));
}

#[test]
fn artwork_rules_require_resolved_color_and_verify_the_sample() {
    let f = Fixture::new();
    f.matching_only();
    f.edit("screens/demo.yaml", |v| {
        v["anatomy"][0]["children"][0]["tokens"]["color"] = json!("artwork-tint")
    });
    assert!(f.compare().pairs[0]
        .target
        .failures
        .iter()
        .any(|f| f.contains("resolved_color")));
    for p in ["ios", "android"] {
        f.edit(&format!("captures/demo.populated.{p}.json"), |v| {
            v["nodes"][1]["resolved_color"] = json!("#336699")
        });
    }
    assert!(!f.compare().failed());
    f.edit("captures/demo.populated.android.json", |v| {
        v["nodes"][1]["resolved_color"] = json!("#FFFFFF")
    });
    assert!(f.compare().failed());
}

#[test]
fn absent_regions_and_parent_containment() {
    let f = Fixture::new();
    f.matching_only();
    f.edit("screens/demo.yaml", |v| {
        v["states"]["populated"]["absent"] = json!(["transport"]);
    });
    assert!(f.compare().pairs[0]
        .target
        .failures
        .iter()
        .any(|f| f.contains("should be absent")));
    for p in ["ios", "android"] {
        f.edit(&format!("captures/demo.populated.{p}.json"), |v| {
            v["nodes"].as_array_mut().expect("nodes").pop();
        });
    }
    assert!(!f.compare().failed());
    f.edit("captures/demo.populated.android.json", |v| {
        v["nodes"][0]["frame"]["width"] = json!(16)
    });
    assert!(f.compare().pairs[0]
        .target
        .failures
        .iter()
        .any(|f| f.contains("escapes parent")));
}

#[test]
fn html_escapes_notes_and_ui_report_counts_all_configured_platforms() {
    let f = Fixture::new();
    f.edit("screens/demo.yaml", |v| {
        v["notes"]["ios"] = json!("<script>alert('x')</script>")
    });
    let ui = f.ui();
    let out = f.0.join("report");
    ui.compare(&f.0.join("captures"), "ios", "android", Some(&out))
        .expect("compare");
    let html = fs::read_to_string(out.join("index.html")).expect("html");
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
    let report = ui.ui_report(&f.0.join("captures"), "ios").expect("report");
    assert!(report.contains("| ios | 2 | 2 | 0 | 0 |"));
    assert!(report.contains("| android | 2 | 1 | 1 | 0 |"));
}

#[cfg(unix)]
#[test]
fn symlink_capture_escape_is_rejected() {
    let f = Fixture::new();
    f.matching_only();
    fs::remove_file(f.0.join("captures/demo.populated.android.png")).expect("remove");
    std::os::unix::fs::symlink(
        f.0.join("tokens.yaml"),
        f.0.join("captures/demo.populated.android.png"),
    )
    .expect("symlink");
    assert!(f
        .ui()
        .compare(&f.0.join("captures"), "ios", "android", None)
        .is_err());
}

#[test]
fn ambiguous_capture_stems_and_absent_parent_contracts_are_rejected() {
    let f = Fixture::new();
    f.edit("screens/demo.yaml", |v| {
        v["states"]["populated"]["absent"] = json!(["content"]);
    });
    assert!(UiCatalog::load(&f.0)
        .expect_err("absent descendants")
        .to_string()
        .contains("absent children"));
    let f = Fixture::new();
    f.matching_only();
    f.edit("screens/demo.yaml", |v| {
        let mut state = v["states"]["populated"].clone();
        state["captures"] =
            json!({"ios":"captures/demo.a.b.ios.png","android":"captures/demo.a.b.android.png"});
        v["states"] = json!({"a.b":state});
    });
    let mut other: Value =
        serde_json::from_str(&fs::read_to_string(f.0.join("screens/demo.yaml")).expect("read"))
            .expect("json");
    let state = other["states"]["a.b"].clone();
    other["id"] = json!("demo.a");
    other["states"] = json!({"b":state});
    fs::write(f.0.join("screens/other.yaml"), other.to_string()).expect("screen");
    assert!(UiCatalog::load(&f.0)
        .expect_err("ambiguous capture")
        .to_string()
        .contains("ambiguous capture"));
}

fn write_pixels(path: &Path, pixels: &[u8]) {
    let mut encoder = png::Encoder::new(fs::File::create(path).expect("png"), 32, 32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("header")
        .write_image_data(pixels)
        .expect("pixels");
}

#[test]
fn similarity_does_not_gate_and_large_text_uses_the_declared_scale() {
    let f = Fixture::new();
    f.matching_only();
    let mut pixels = vec![0; 32 * 32 * 3];
    for y in 0..32 {
        for x in 0..32 {
            let color = if y < 16 {
                [51, 102, 153]
            } else if y >= 24 {
                [255, 255, 255]
            } else {
                [0, 0, 0]
            };
            pixels[(y * 32 + x) * 3..(y * 32 + x) * 3 + 3].copy_from_slice(&color);
        }
    }
    write_pixels(&f.0.join("captures/demo.populated.android.png"), &pixels);
    let comparison = f.compare();
    assert!(comparison.pairs[0].similarity.expect("score") < 0.9);
    assert!(!comparison.failed(), "{comparison:?}");
    f.edit("screens/demo.yaml", |v| {
        v["states"]["populated"]["font_scale"] = json!(1.5)
    });
    for p in ["ios", "android"] {
        f.edit(&format!("captures/demo.populated.{p}.json"), |v| {
            v["nodes"][2]["text_size"] = json!(24)
        });
    }
    assert!(!f.compare().failed());
}

#[test]
fn artwork_colors_must_agree_across_platforms_even_with_matching_metadata() {
    let f = Fixture::new();
    f.matching_only();
    f.edit("screens/demo.yaml", |v| {
        v["anatomy"][0]["children"][0]["tokens"]["color"] = json!("artwork-tint")
    });
    f.edit("captures/demo.populated.ios.json", |v| {
        v["nodes"][1]["resolved_color"] = json!("#336699")
    });
    f.edit("captures/demo.populated.android.json", |v| {
        v["nodes"][1]["resolved_color"] = json!("#FF0000")
    });
    let mut pixels = vec![255; 32 * 32 * 3];
    for p in pixels[..32 * 16 * 3].chunks_exact_mut(3) {
        p.copy_from_slice(&[255, 0, 0]);
    }
    write_pixels(&f.0.join("captures/demo.populated.android.png"), &pixels);
    let c = f.compare();
    assert!(c.pairs[0]
        .target
        .failures
        .iter()
        .any(|f| f.contains("artwork color artwork-tint differs")));
}

#[test]
fn report_does_not_truncate_input_when_output_assets_share_the_capture_directory() {
    let f = Fixture::new();
    let path = f.0.join("captures/demo.populated.ios.png");
    let before = fs::read(&path).expect("PNG");
    f.ui()
        .compare(&f.0.join("captures"), "ios", "android", Some(&f.0))
        .expect("report alongside captures");
    assert_eq!(fs::read(&path).expect("preserved PNG"), before);
}
