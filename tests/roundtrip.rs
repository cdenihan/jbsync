//! The safety property the whole storage design rests on: canonicalizing a
//! settings file must not change what it means. Fixtures under `tests/corpus`
//! are verbatim copies of real JetBrains files covering the awkward shapes
//! (deep nesting, element text, maps, registry entries, ordered lists).
//!
//! Point `JBSYNC_CORPUS` at a JetBrains config root to run the same checks
//! against a live installation:
//!   JBSYNC_CORPUS="$HOME/Library/Application Support/JetBrains" \
//!     cargo test --test roundtrip -- --include-ignored

use std::path::{Path, PathBuf};

use jbsync::xml::{dom, project};

fn assert_canonicalization_is_lossless(label: &str, contents: &str) {
    let Ok(original) = dom::parse(contents) else {
        return; // Not XML (JetBrains writes `DELETED` tombstones); handled elsewhere.
    };

    let serialized = dom::serialize(&original);
    let reparsed = dom::parse(&serialized)
        .unwrap_or_else(|error| panic!("{label}: canonical output does not parse: {error}"));
    assert_eq!(
        original, reparsed,
        "{label}: canonicalization changed the document"
    );
    assert_eq!(
        project::project(&original),
        project::project(&reparsed),
        "{label}: canonicalization changed the projection"
    );

    // Idempotence: re-serializing must be byte-identical, otherwise the store
    // would churn on every sync.
    assert_eq!(
        serialized,
        dom::serialize(&reparsed),
        "{label}: serialization is not idempotent"
    );
}

fn xml_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "xml") {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn corpus_fixtures_canonicalize_losslessly() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let files = xml_files(&corpus);
    assert!(!files.is_empty(), "corpus fixtures are missing");
    for path in files {
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_canonicalization_is_lossless(&path.display().to_string(), &contents);
    }
}

#[test]
#[ignore = "requires a local JetBrains installation"]
fn live_installation_canonicalizes_losslessly() {
    let Some(root) = std::env::var_os("JBSYNC_CORPUS") else {
        panic!("set JBSYNC_CORPUS to a JetBrains config root");
    };
    let files = xml_files(Path::new(&root));
    assert!(!files.is_empty(), "no XML found under JBSYNC_CORPUS");
    for path in files {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        assert_canonicalization_is_lossless(&path.display().to_string(), &contents);
    }
}
