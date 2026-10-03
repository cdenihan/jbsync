//! Test-only reference for differential validation of the Zig rewrite.
use jbsync::{
    sync::merge::{ConflictPolicy, merge_file},
    xml::dom,
};
use serde_json::{Value, json};
fn main() {
    let input = std::fs::read_to_string(std::env::args().nth(1).expect("input file")).unwrap();
    let cases: Vec<Value> = serde_json::from_str(&input).unwrap();
    let results: Vec<Value> = cases.iter().map(|c| {
        let text = |k: &str| c[k].as_str();
        match text("op").unwrap() {
            "xml" => match dom::parse(text("local").unwrap()) {
                Ok(n) => json!({"content": dom::serialize(&n)}),
                Err(_) => json!({"error": true}),
            },
            "plugin" => {
                let plugin: jbsync::plugins::Plugin = serde_json::from_value(c["plugin"].clone()).unwrap();
                let ide = jbsync::ide::Ide {
                    product: text("product").unwrap().into(), path: "fixture".into(), pattern_index: 0,
                    metadata: text("build").filter(|v| !v.is_empty()).map(|build| jbsync::ide::ProductMetadata { build_number: build.into(), ..Default::default() }),
                };
                let caps = serde_json::from_value(c["capabilities"].clone()).unwrap();
                json!({"compatible": jbsync::plugins::compatibility(&plugin, &ide, &caps, &std::collections::BTreeSet::default(), &jbsync::config::PluginsConfig::default()).compatible})
            },
            "glob" => json!({"matches": globset::Glob::new(text("pattern").unwrap()).unwrap().compile_matcher().is_match(text("local").unwrap())}),
            "merge" => {
                let policy = match text("policy").unwrap() {"remote" => ConflictPolicy::PreferRemote, "neither" => ConflictPolicy::Fail, _ => ConflictPolicy::PreferLocal};
                let r = merge_file(text("base").map(str::as_bytes), text("local").map(str::as_bytes), text("remote").map(str::as_bytes), policy);
                json!({"content": r.content.map(|v| String::from_utf8(v).unwrap()), "conflicts": r.conflicts.len()})
            },
            _ => panic!("invalid operation"),
        }
    }).collect();
    println!("{}", serde_json::to_string(&results).unwrap());
}
