//! The generated JSON Schema.
//!
//! The schema exists to give per-layer autocomplete and per-layer required fields in the
//! editor. It is generated from the same Rust types the validator uses, so it cannot drift
//! away from them and start lying — these tests are what enforce that.

use scans::model;

fn schema() -> serde_json::Value {
    model::json_schema()
}

/// The keystone. One `layer` discriminator drives three variants, each pinning `layer` to a
/// literal, so an editor can narrow completions the moment `layer` is typed.
#[test]
fn the_layer_discriminator_produces_three_pinned_branches() {
    let s = schema();
    let branches = s["oneOf"].as_array().expect("oneOf");
    assert_eq!(branches.len(), 3);

    let layers: Vec<&str> = branches
        .iter()
        .map(|b| b["properties"]["layer"]["const"].as_str().expect("const"))
        .collect();
    assert_eq!(layers, vec!["source", "copy", "document"]);
}

/// `layer = "page"` is not a variant. Pages are inline only.
#[test]
fn page_is_not_a_variant() {
    let s = schema();
    let layers: Vec<String> = s["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["properties"]["layer"]["const"].to_string())
        .collect();
    assert!(!layers.iter().any(|l| l.contains("page")));
}

/// `deny_unknown_fields` must reach the schema, or the editor will happily complete a typo
/// that the loader then rejects.
#[test]
fn every_branch_forbids_unknown_properties() {
    let s = schema();
    for branch in s["oneOf"].as_array().unwrap() {
        assert_eq!(
            branch["additionalProperties"], false,
            "branch {} allows unknown properties",
            branch["properties"]["layer"]["const"]
        );
    }
    for (name, def) in s["$defs"].as_object().unwrap() {
        // The identifier table is deliberately open; everything else is closed.
        if def["type"] == "object" && def.get("additionalProperties") != Some(&false.into()) {
            assert!(
                def.get("properties").is_none(),
                "$defs/{name} has fixed properties but allows unknown ones"
            );
        }
    }
}

/// Required fields differ per layer: a source and a copy must have a title, a terse issue
/// need not.
#[test]
fn required_fields_are_per_layer() {
    let s = schema();
    let required = |layer: &str| -> Vec<String> {
        s["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["properties"]["layer"]["const"] == layer)
            .unwrap()["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    };

    assert_eq!(required("source"), vec!["layer", "title"]);
    assert_eq!(required("copy"), vec!["layer", "title"]);
    assert_eq!(required("document"), vec!["layer"]);
}

/// TOML has no null, so the schema must never mention one. An optional key is absent, not
/// null.
#[test]
fn the_schema_never_mentions_null() {
    let text = model::json_schema_text();
    assert!(
        !text.contains("null"),
        "the schema still offers null somewhere:\n{}",
        text.lines()
            .filter(|l| l.contains("null"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Fields that inherit and fields that do not both have to be reachable, or a hand-editor
/// cannot write them.
#[test]
fn the_inline_structures_are_all_defined() {
    let s = schema();
    let defs = s["$defs"].as_object().expect("$defs");
    for name in [
        "Page",
        "Graphic",
        "Resp",
        "Roles",
        "Rights",
        "Holding",
        "Scan",
        "Text",
        "Link",
        "Zone",
        "PageRange",
    ] {
        assert!(defs.contains_key(name), "$defs is missing {name}");
    }
}

/// `scans schema --check` is only a real guard if the committed file actually matches. This
/// is the same comparison, run in CI by `cargo test`.
#[test]
fn the_committed_schema_is_up_to_date() {
    let path = repo_root().join("schemas/source.json");
    let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}\nrun `cargo run -- schema` to generate it",
            path.display()
        )
    });
    assert_eq!(
        on_disk.replace("\r\n", "\n"),
        model::json_schema_text().replace("\r\n", "\n"),
        "schemas/source.json is out of date with tools/src/model.rs; run `cargo run -- schema`"
    );
}

fn repo_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is <repo>/tools.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tools has a parent")
        .to_path_buf()
}
