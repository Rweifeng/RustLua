use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(serde::Deserialize)]
struct DependencyIndex {
    format: u32,
    edges: BTreeMap<String, Vec<String>>,
    opaque: Vec<String>,
}

pub struct ResourceSelection {
    pub paths: Vec<String>,
    pub full_fallback: bool,
    pub fallback_reason: Option<String>,
}

/// Select the transitive static dependency closure. If a reachable resource is
/// opaque or absent from the package manifest, preserve correctness by
/// returning every manifest path.
pub fn select_resources(
    index_text: &str,
    manifest_paths: &[String],
    roots: &[String],
) -> Result<ResourceSelection, String> {
    let index: DependencyIndex =
        serde_json::from_str(index_text).map_err(|error| format!("parse: {error}"))?;
    if index.format != 1 {
        return Err(format!("unsupported format {}", index.format));
    }

    let manifest: BTreeSet<&str> = manifest_paths.iter().map(String::as_str).collect();
    let opaque: BTreeSet<&str> = index.opaque.iter().map(String::as_str).collect();
    let mut selected = BTreeSet::new();
    let mut pending: VecDeque<String> = roots.iter().cloned().collect();
    while let Some(path) = pending.pop_front() {
        if !selected.insert(path.clone()) {
            continue;
        }
        if !manifest.contains(path.as_str()) || opaque.contains(path.as_str()) {
            let reason = if opaque.contains(path.as_str()) {
                format!("opaque reference at {path}")
            } else {
                format!("resource absent from manifest: {path}")
            };
            return Ok(full_selection(manifest_paths, reason));
        }
        if let Some(dependencies) = index.edges.get(&path) {
            pending.extend(dependencies.iter().cloned());
        }
    }

    Ok(ResourceSelection {
        paths: selected.into_iter().collect(),
        full_fallback: false,
        fallback_reason: None,
    })
}

fn full_selection(manifest_paths: &[String], reason: String) -> ResourceSelection {
    ResourceSelection {
        paths: manifest_paths.to_vec(),
        full_fallback: true,
        fallback_reason: Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn selects_transitive_closure_and_ignores_unrelated_files() {
        let manifest = paths(&[
            "scripts/main.lua",
            "scripts/helper.lua",
            "scenes/main.scene",
            "assets/ui/title.png",
            "assets/audio/unused.ogg",
        ]);
        let index = r#"{"format":1,"edges":{"scripts/main.lua":["scripts/helper.lua","scenes/main.scene"],"scenes/main.scene":["assets/ui/title.png"]},"opaque":[]}"#;
        let selected = select_resources(index, &manifest, &paths(&["scripts/main.lua"])).unwrap();
        assert_eq!(
            selected.paths,
            paths(&[
                "assets/ui/title.png",
                "scenes/main.scene",
                "scripts/helper.lua",
                "scripts/main.lua",
            ])
        );
        assert!(!selected.full_fallback);
    }

    #[test]
    fn opaque_or_missing_reachable_resource_falls_back_to_full_manifest() {
        let manifest = paths(&["scripts/main.lua", "assets/a.png", "assets/b.png"]);
        let index = r#"{"format":1,"edges":{"scripts/main.lua":["assets/a.png"]},"opaque":["scripts/main.lua"]}"#;
        let selected = select_resources(index, &manifest, &paths(&["scripts/main.lua"])).unwrap();
        assert_eq!(selected.paths, manifest);
        assert!(selected.full_fallback);
        assert_eq!(
            selected.fallback_reason.as_deref(),
            Some("opaque reference at scripts/main.lua")
        );

        let index =
            r#"{"format":1,"edges":{"scripts/main.lua":["assets/missing.png"]},"opaque":[]}"#;
        let selected = select_resources(index, &manifest, &paths(&["scripts/main.lua"])).unwrap();
        assert_eq!(selected.paths, manifest);
        assert!(selected.full_fallback);
        assert_eq!(
            selected.fallback_reason.as_deref(),
            Some("resource absent from manifest: assets/missing.png")
        );
    }

    #[test]
    fn rejects_unknown_index_versions() {
        assert!(select_resources(r#"{"format":2,"edges":{},"opaque":[]}"#, &[], &[]).is_err());
    }
}
