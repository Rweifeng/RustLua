//! Component schema audit for real projects (docs/55 §1 item 3 / stage 2).
//!
//! The editor writes components as `key=value;…` strings, so "Inspector / 校验 / 迁移 /
//! 运行时 schema 漂移" is a standing risk. The schema registry plus the drift gate
//! (`component_schema::tests::builtin_schemas_match_engine_encoders`) keeps the *engine* side
//! honest; this module applies the same registry to *actual project files*, so drift in
//! authored content is reported instead of silently absorbed.

use engine::core::component_schema::{SchemaError, builtin_registry, validate_component};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// One field-level mismatch: the component is registered but its payload is rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaFinding {
    pub file: PathBuf,
    pub entity: u64,
    pub component: String,
    pub field: String,
    pub reason: String,
}

/// Aggregated audit result.
#[derive(Clone, Debug, Default)]
pub struct SchemaAudit {
    /// Number of documents scanned.
    pub documents: usize,
    /// Number of entity components validated.
    pub components: usize,
    /// How many times each component name appeared.
    pub per_component: BTreeMap<String, usize>,
    /// Components with no registered schema (reported, not an error).
    pub unknown: BTreeMap<String, usize>,
    /// Field-level mismatches for registered components.
    pub findings: Vec<SchemaFinding>,
}

impl SchemaAudit {
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// Report lines in the same `key=value` style as `editor --check`.
    pub fn report_lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "schema_audit documents={} components={} schema_errors={} unknown_components={}",
            self.documents,
            self.components,
            self.findings.len(),
            self.unknown.len()
        )];
        for (component, count) in &self.unknown {
            lines.push(format!(
                "component_no_schema component={component} count={count}"
            ));
        }
        for finding in self.findings.iter().take(50) {
            lines.push(format!(
                "schema_error file={} entity={} component={} field={} reason={}",
                finding.file.display(),
                finding.entity,
                finding.component,
                finding.field,
                finding.reason
            ));
        }
        if self.findings.len() > 50 {
            lines.push(format!(
                "schema_error_truncated remaining={}",
                self.findings.len() - 50
            ));
        }
        lines
    }
}

/// Validate every component of every scene/prefab/widget document under `root`.
pub fn audit_project(root: &Path) -> Result<SchemaAudit, std::io::Error> {
    let registry = builtin_registry();
    let mut audit = SchemaAudit::default();
    for document in project_documents(root) {
        let Ok(scene) = load_document(&document) else {
            // 解析失败由既有的场景加载路径报告；审计只关心能读到的组件。
            continue;
        };
        audit.documents += 1;
        for entity in scene.entities.values() {
            for (name, payload) in &entity.components {
                audit.components += 1;
                *audit.per_component.entry(name.clone()).or_insert(0) += 1;
                if !registry.contains_key(name.as_str()) {
                    *audit.unknown.entry(name.clone()).or_insert(0) += 1;
                    continue;
                }
                if let Err(error) = validate_component(&registry, name, payload) {
                    let SchemaError {
                        component,
                        field,
                        reason,
                    } = error;
                    audit.findings.push(SchemaFinding {
                        file: relative(root, &document),
                        entity: entity.id.0,
                        component,
                        field,
                        reason,
                    });
                }
            }
        }
    }
    Ok(audit)
}

fn relative(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root)
        .map_or_else(|_| path.to_path_buf(), PathBuf::from)
}

/// Scene/prefab/widget documents under `root` (bounded recursion, symlink-free).
fn project_documents(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut visited = 0_usize;
    while let Some(directory) = stack.pop() {
        visited += 1;
        if visited > 10_000 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                // 不看隐藏/缓存/构建产物目录，避免把 `.cache`、`target`、`builds` 里
                // 的**已发布副本**当成作者内容重复报错。
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.')
                    || name == "target"
                    || name == "node_modules"
                    || name == "builds"
                {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let extension = path.extension().and_then(|value| value.to_str());
            if matches!(extension, Some("scene") | Some("prefab") | Some("widget")) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn load_document(path: &Path) -> Result<engine::core::scene::Scene, String> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("prefab") => engine::core::prefab::Prefab::load(path)
            .map(|prefab| crate::document::scene_from_prefab(&prefab))
            .map_err(|error| error.to_string()),
        _ => engine::core::scene::Scene::load(path).map_err(|error| error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::core::scene::{Scene, SceneEntityId};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "p2-schema-audit-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("scenes")).expect("scenes dir");
        root
    }

    fn write_scene(root: &Path, name: &str, components: &[(&str, &str)]) -> PathBuf {
        let mut scene = Scene::new("audit");
        let id = SceneEntityId(1);
        scene.add_entity(id, "Root", None).expect("entity");
        let entity = scene.entities.get_mut(&id).expect("entity exists");
        for (component, payload) in components {
            entity
                .components
                .insert((*component).to_owned(), (*payload).to_owned());
        }
        let path = root.join("scenes").join(format!("{name}.scene"));
        scene.save(&path).expect("scene saves");
        path
    }

    /// **未迁移数据的负控门**（第 57 轮统一之后）：审计必须把"旧组件名"与"旧 2D 载荷形态"
    /// 分别点出来——这正是外部老工程在删掉自动迁移之后**唯一**的发现通道，也是仓库内示例
    /// 数据的常驻门（`slime_*.prefab` 那次漏迁就是靠它现形的）。
    ///
    /// 两类信号的形状不同，必须都断：旧名 ⇒ `component_no_schema`（unknown，不报 schema_error）；
    /// 旧形态 ⇒ `schema_error`（统一字段表里没有 `x` 这个键）。
    #[test]
    fn un_migrated_legacy_names_and_shapes_are_reported() {
        let root = temp_root("legacy");
        write_scene(
            &root,
            "legacy",
            &[
                // 旧名：引擎不再认识它，因此进 unknown 明细。
                (
                    "Transform3D",
                    "position=0,0,0;rotation=0,0,0,1;scale=1,1,1",
                ),
                // 旧形态：名字对、载荷是 2D 那套 —— 必须报 schema_error。
                ("Transform", "x=1;y=2;rotation=0;scale_x=1;scale_y=1"),
                ("Camera2D", "center=0,0;zoom=1"),
            ],
        );
        let audit = audit_project(&root).expect("audit runs");
        assert_eq!(audit.unknown.get("Transform3D"), Some(&1), "{audit:?}");
        assert_eq!(audit.unknown.get("Camera2D"), Some(&1), "{audit:?}");
        assert_eq!(audit.findings.len(), 1, "{:?}", audit.findings);
        let finding = &audit.findings[0];
        assert_eq!(finding.component, "Transform");
        // 报的是**旧形态里的那个键**（`x` 不在统一字段表里），因此用户一眼能看出是旧写法。
        assert_eq!(finding.field, "x", "{finding:?}");
        assert!(
            finding.reason.contains("unknown field"),
            "原因要指出这是不认识的字段：{finding:?}"
        );
        assert!(!audit.is_clean());
        let lines = audit.report_lines();
        assert!(lines[0].contains("schema_errors=1"), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("component_no_schema component=Transform3D")),
            "{lines:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn clean_project_reports_zero_errors() {
        let root = temp_root("clean");
        write_scene(
            &root,
            "main",
            &[
                (
                    "Transform",
                    "position=0,0,0;rotation=0,0,0,1;scale=1,1,1",
                ),
                (
                    "Sprite",
                    "texture=a.png;size=16x16;pivot=0.5,0.5;tint=1,1,1,1;sorting_layer=default;order=0",
                ),
            ],
        );
        let audit = audit_project(&root).expect("audit runs");
        assert_eq!(audit.documents, 1);
        assert_eq!(audit.components, 2);
        assert!(audit.is_clean(), "{:?}", audit.findings);
        assert!(audit.report_lines()[0].contains("schema_errors=0"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn field_level_drift_is_reported_with_file_and_entity() {
        let root = temp_root("drift");
        // size 写成逗号形式：这是 Sprite 的真实漂移（编码约定是 `宽x高`）。
        write_scene(
            &root,
            "broken",
            &[(
                "Sprite",
                "texture=a.png;size=16,16;pivot=0.5,0.5;tint=1,1,1,1;sorting_layer=default;order=0",
            )],
        );
        let audit = audit_project(&root).expect("audit runs");
        assert!(!audit.is_clean());
        let finding = &audit.findings[0];
        assert_eq!(finding.component, "Sprite");
        assert_eq!(finding.field, "size");
        assert_eq!(finding.entity, 1);
        assert!(
            finding.file.ends_with("scenes/broken.scene"),
            "{:?}",
            finding.file
        );
        let lines = audit.report_lines();
        assert!(lines[0].contains("schema_errors=1"), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("component=Sprite field=size"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unregistered_components_are_counted_not_failed() {
        let root = temp_root("unknown");
        write_scene(
            &root,
            "custom",
            &[
                ("MyCustomThing", "whatever=1"),
                (
                    "Transform",
                    "position=0,0,0;rotation=0,0,0,1;scale=1,1,1",
                ),
            ],
        );
        let audit = audit_project(&root).expect("audit runs");
        assert!(audit.is_clean(), "未注册组件不是错误");
        assert_eq!(audit.unknown.get("MyCustomThing"), Some(&1));
        let lines = audit.report_lines();
        assert!(
            lines
                .iter()
                .any(|line| line.contains("component_no_schema component=MyCustomThing"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_directory_is_not_an_error() {
        let root =
            std::env::temp_dir().join(format!("p2-schema-audit-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let audit = audit_project(&root).expect("audit tolerates a missing root");
        assert_eq!(audit.documents, 0);
        assert!(audit.is_clean());
        let _ = std::fs::remove_dir_all(root);
    }

    /// 仓库自带示例工程必须**零字段级漂移**：这是"真实/旧内容不会被 schema 误判"的回归门
    /// （也就是 `editor --check` 在 verify.ps1 里断言的那条）。checkout 里没有示例工程时跳过。
    #[test]
    fn workspace_example_project_has_no_schema_drift() {
        let example = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../examples/game_jhyt")
            .canonicalize();
        let Ok(example) = example else {
            eprintln!("跳过：checkout 内没有 examples/game_jhyt");
            return;
        };
        let audit = audit_project(&example).expect("audit runs");
        assert!(
            audit.documents > 0,
            "示例工程应至少包含一个场景文档：{}",
            example.display()
        );
        assert!(
            audit.is_clean(),
            "示例工程出现字段级 schema 漂移：{:?}",
            audit.findings.iter().take(5).collect::<Vec<_>>()
        );
        assert!(
            audit.components > 100,
            "示例工程组件数异常偏少：{}",
            audit.components
        );
    }
}
