//! 特效预设（`.prefab`，`kind=effect`）：把一张 `.sgraph` 连同它的参数取值做成
//! **可在逻辑里按需加载的特效资产**，Lua 侧用 `effect.play("炎刃斩击")` 触发。
//!
//! ## 为什么复用 `.prefab` 而不是新造后缀
//!
//! 工程里已经有一套预制体导航：资源扫描认 `.prefab`、编辑器有预制体面板、`prefab.spawn`
//! 按路径实例化。特效预设是**同一类东西的另一种呈现**——"一个可以反复生成的东西，参数
//! 随实例不同"。因此这里沿用同一份文本格式（`key=value` 头 + 制表符分行的段），只是把
//! "实体"换成"着色图"，并且用 `kind=effect` 明确区分，避免被当成场景预制体去实例化。
//!
//! ```text
//! prefab_version=1
//! kind=effect
//! name=炎刃斩击
//! graph=assets/fx/flame_blade_slash.sgraph
//! overlay=true
//! param   spin     2.4
//! param   color_r  1.0
//! ```
//!
//! ## 语义
//!
//! - `graph` 必须是**工程内相对路径**（拒绝绝对路径与 `..`），与 `effect_overlay` 同一条
//!   校验口径，运行时不会读到工程外的东西；
//! - `overlay=true` 表示播放时挂到"场景级叠加层"（与 `project.toml [render] effect_overlay`
//!   同一个位置，一次一层）；缺省 `true`——单层是最常见的用法；
//! - `param` 行的键必须能在这张图编译出的**命名参数表**里找到，否则加载时就报错，而不是
//!   运行时静默丢参数（那是最难查的一类"特效不生效"）。
//!
//! 参数覆盖**在加载期**就合并进槽位默认值：播放时不需要再逐个 `set_effect_param`，
//! 逻辑里只写它想改的那几个，其余跟随预设。

use std::collections::BTreeMap;
use std::path::Path;

use crate::shader_graph::{CompiledShader, ShaderGraph};

/// 特效预设当前的格式版本。
pub const EFFECT_PREFAB_VERSION: u32 = 1;

/// `.prefab` 头里用来把特效预设与场景预制体区分开的取值。
pub const EFFECT_PREFAB_KIND: &str = "effect";

/// 一份加载完成的特效预设：已编译的 WGSL + 合并了预设覆盖的参数槽。
#[derive(Clone, Debug)]
pub struct EffectPrefab {
    /// 预设名（`name=` 行；缺省时用文件名）。
    pub name: String,
    /// 工程内相对的 `.sgraph` 路径（用于日志与诊断）。
    pub graph_path: String,
    /// 播放时是否挂到场景级叠加层。
    pub overlay: bool,
    /// 已按预设覆盖合并过的运行时叠加层 WGSL。
    pub source: String,
    /// 编译结果（槽位默认值已含预设覆盖，命名参数表用于脚本解析）。
    pub compiled: CompiledShader,
    /// 预设显式覆盖过的参数名 → 值（诊断用；槽位里已经是最终值）。
    pub overrides: BTreeMap<String, f32>,
}

/// 特效预设的解析/加载错误。
#[derive(Debug, thiserror::Error)]
pub enum EffectPrefabError {
    #[error("第 {line} 行格式错误：{reason}")]
    Parse { line: usize, reason: String },
    #[error("这不是特效预设（缺少 kind={expected}）")]
    NotAnEffect { expected: &'static str },
    #[error("graph 必须是工程内相对路径：{0}")]
    BadGraphPath(String),
    #[error("读取 {path} 失败：{reason}")]
    Io { path: String, reason: String },
    #[error("{path}：{reason}")]
    Graph { path: String, reason: String },
    #[error("预设引用了图里没有的参数：{0}")]
    UnknownParam(String),
}

impl EffectPrefab {
    /// 解析一段 `.prefab` 文本（不读盘）。
    ///
    /// 只做格式与字段校验；`graph` 的存在性与参数名有效性由 [`Self::load`] 负责。
    pub fn from_text(text: &str) -> Result<Self, EffectPrefabError> {
        let mut version = EFFECT_PREFAB_VERSION;
        let mut kind: Option<String> = None;
        let mut name = String::new();
        let mut graph: Option<String> = None;
        let mut overlay = true;
        let mut overrides: BTreeMap<String, f32> = BTreeMap::new();

        for (index, line) in text.lines().enumerate() {
            let line_no = index + 1;
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            if let Some(value) = line.strip_prefix("kind=") {
                kind = Some(value.trim().to_owned());
                continue;
            }
            // 场景预制体（`kind` 缺省）里的 `entity` / `component` 行在这里必然"无法识别"；
            // 先把它认出来再报"不是特效预设"，否则用户看到的是"第 5 行格式错误"这种
            // 完全指错方向的诊断。
            if kind
                .as_deref()
                .is_none_or(|kind| kind != EFFECT_PREFAB_KIND)
                && [
                    "asset_id=",
                    "root=",
                    "entity\t",
                    "component\t",
                    "resource\t",
                ]
                .iter()
                .any(|prefix| line.starts_with(prefix))
            {
                return Err(EffectPrefabError::NotAnEffect {
                    expected: EFFECT_PREFAB_KIND,
                });
            }
            if let Some(value) = line.strip_prefix("prefab_version=") {
                version = value.trim().parse().map_err(|_| EffectPrefabError::Parse {
                    line: line_no,
                    reason: "prefab_version 不是整数".to_owned(),
                })?;
                continue;
            }
            if let Some(value) = line.strip_prefix("kind=") {
                kind = Some(value.trim().to_owned());
                continue;
            }
            if let Some(value) = line.strip_prefix("name=") {
                name = value.trim().to_owned();
                continue;
            }
            if let Some(value) = line.strip_prefix("graph=") {
                graph = Some(value.trim().to_owned());
                continue;
            }
            if let Some(value) = line.strip_prefix("overlay=") {
                overlay = match value.trim() {
                    "true" | "1" | "yes" => true,
                    "false" | "0" | "no" => false,
                    other => {
                        return Err(EffectPrefabError::Parse {
                            line: line_no,
                            reason: format!("overlay 只能是 true/false，实际是 {other}"),
                        });
                    }
                };
                continue;
            }
            if line.starts_with("param\t") {
                let fields: Vec<&str> = line.split('\t').collect();
                if fields.len() != 3 {
                    return Err(EffectPrefabError::Parse {
                        line: line_no,
                        reason: format!(
                            "param 行需要 3 段（param/名字/数值），实际 {} 段",
                            fields.len()
                        ),
                    });
                }
                let key = fields[1].trim();
                if key.is_empty() {
                    return Err(EffectPrefabError::Parse {
                        line: line_no,
                        reason: "param 名字为空".to_owned(),
                    });
                }
                let value: f32 =
                    fields[2]
                        .trim()
                        .parse()
                        .map_err(|_| EffectPrefabError::Parse {
                            line: line_no,
                            reason: format!("param {key} 的数值无法解析：{}", fields[2].trim()),
                        })?;
                if !value.is_finite() {
                    return Err(EffectPrefabError::Parse {
                        line: line_no,
                        reason: format!("param {key} 的数值必须有限"),
                    });
                }
                overrides.insert(key.to_owned(), value);
                continue;
            }
            return Err(EffectPrefabError::Parse {
                line: line_no,
                reason: format!("无法识别的行：{line}"),
            });
        }

        if kind.as_deref() != Some(EFFECT_PREFAB_KIND) {
            return Err(EffectPrefabError::NotAnEffect {
                expected: EFFECT_PREFAB_KIND,
            });
        }
        if version != EFFECT_PREFAB_VERSION {
            return Err(EffectPrefabError::Parse {
                line: 1,
                reason: format!(
                    "不支持的 prefab_version={version}（当前只认 {EFFECT_PREFAB_VERSION}）"
                ),
            });
        }
        let graph = graph.ok_or_else(|| EffectPrefabError::Parse {
            line: 1,
            reason: "缺少 graph= 行".to_owned(),
        })?;
        if !is_project_relative(&graph) {
            return Err(EffectPrefabError::BadGraphPath(graph));
        }
        // `source` / `compiled` 由 `load` 填充；解析阶段留空，保证 `from_text` 纯文本可用。
        Ok(Self {
            name,
            graph_path: graph,
            overlay,
            source: String::new(),
            compiled: CompiledShader {
                source: String::new(),
                params: Vec::new(),
                slots: Vec::new(),
            },
            overrides,
        })
    }

    /// 从工程根读取 `.prefab`，加载并编译它引用的 `.sgraph`。
    ///
    /// 预设里的 `param` 覆盖会**合并进槽位默认值**：调用方拿到的 `compiled.slots`
    /// 已经是"预设生效后"的值，播放时不需要再补一遍。
    pub fn load(root: &Path, relative_prefab: &str) -> Result<Self, EffectPrefabError> {
        let prefab_path = root.join(relative_prefab);
        let text =
            std::fs::read_to_string(&prefab_path).map_err(|error| EffectPrefabError::Io {
                path: prefab_path.display().to_string(),
                reason: error.to_string(),
            })?;
        let mut prefab = Self::from_text(&text)?;
        if prefab.name.is_empty() {
            prefab.name = std::path::Path::new(relative_prefab)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| relative_prefab.to_owned());
        }

        let (source, mut compiled) = ShaderGraph::load_runtime_overlay(root, &prefab.graph_path)
            .map_err(|reason| EffectPrefabError::Graph {
                path: prefab.graph_path.clone(),
                reason,
            })?;

        // 参数名 → (槽号, 分量)；与 `render.set_effect_param` 用的是同一张表，
        // 所以预设里写的名字和脚本里写的名字**必然一致**，不存在两套命名。
        let names: Vec<(String, usize, usize)> = compiled
            .named_params()
            .into_iter()
            .map(|param| (param.name, param.slot, param.component))
            .collect();
        for (key, value) in &prefab.overrides {
            let Some((_, slot, component)) = names.iter().find(|(name, _, _)| name == key) else {
                let known: Vec<&str> = names.iter().map(|(name, _, _)| name.as_str()).collect();
                return Err(EffectPrefabError::UnknownParam(format!(
                    "{key}（可用参数：{}）",
                    known.join(", ")
                )));
            };
            let slots = &mut compiled.slots;
            if *slot >= slots.len() {
                slots.resize(*slot + 1, [0.0; 4]);
            }
            slots[*slot][*component] = *value;
        }

        prefab.source = source;
        prefab.compiled = compiled;
        Ok(prefab)
    }

    /// 命名参数表（名字 → 槽号 + 分量），推给脚本层用。
    #[must_use]
    pub fn param_names(&self) -> Vec<(String, usize, usize)> {
        self.compiled
            .named_params()
            .into_iter()
            .map(|param| (param.name, param.slot, param.component))
            .collect()
    }

    /// 由面板参数直接拼出一份**未加载**的预设（只做格式生成，不读图）。
    ///
    /// 参数按名字升序写出（`BTreeMap` 的顺序）：同一组取值每次生成的文本逐字节相同，
    /// `git diff` 不会因为"滑块动过又拖回来"产生噪声。
    #[must_use]
    pub fn to_text(
        name: &str,
        graph_path: &str,
        overlay: bool,
        overrides: &BTreeMap<String, f32>,
    ) -> String {
        let mut out = String::new();
        out.push_str(&format!("prefab_version={EFFECT_PREFAB_VERSION}\n"));
        out.push_str(&format!("kind={EFFECT_PREFAB_KIND}\n"));
        out.push_str(&format!("name={name}\n"));
        out.push_str(&format!("graph={graph_path}\n"));
        out.push_str(&format!("overlay={overlay}\n"));
        for (key, value) in overrides {
            out.push_str(&format!("param\t{key}\t{value}\n"));
        }
        out
    }
}

/// 从一张图生成预设文本：`overrides` 里给的取值优先，缺的参数用图自己的默认值补齐。
///
/// 为什么要补齐：只写"动过的参数"会让预设依赖图的当前默认值——明天改了模板默认，
/// 这些没写的参数就跟着变了，预设不再是"那一刻的样子"。全量写出才能让预设真正可复现。
/// `skip` 里的名字不写（面板用它剔除无意义参数，例如颜色分量的重复项）。
///
/// 没有 `#[must_use]`：返回类型 `Result` 本身就是 `must_use`，再加一个是冗余，
/// clippy 会报 `must_use_candidate` 的反面（`double_must_use`）。
pub fn preset_text_from_graph(
    graph: &ShaderGraph,
    name: &str,
    graph_path: &str,
    overlay: bool,
    overrides: &BTreeMap<String, f32>,
    skip: &[String],
) -> Result<String, EffectPrefabError> {
    let compiled = graph
        .compile_wgsl_uniform(1, 3)
        .map_err(|error| EffectPrefabError::Graph {
            path: graph_path.to_owned(),
            reason: error.to_string(),
        })?;
    let mut values = BTreeMap::new();
    for param in compiled.named_params() {
        if skip.iter().any(|skip| skip == &param.name) {
            continue;
        }
        let value = overrides.get(&param.name).copied().unwrap_or(param.default);
        values.insert(param.name, value);
    }
    Ok(EffectPrefab::to_text(name, graph_path, overlay, &values))
}

/// 工程内相对路径校验：非空、无前后空白、每段都是普通段（挡住绝对路径与 `..`）。
///
/// 与 `ShaderGraph::load_runtime_overlay` 同一条口径，各自校验一次是刻意的：
/// 预设文件可以引用任何路径字符串，而真正读盘的是图加载器。
fn is_project_relative(relative: &str) -> bool {
    !relative.is_empty()
        && relative.trim() == relative
        && Path::new(relative)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_text() -> String {
        "\
prefab_version=1
kind=effect
name=炎刃斩击
graph=assets/fx/flame_blade_slash.sgraph
overlay=true
param\tspin\t2.4
param\tcolor_r\t1.0
"
        .to_owned()
    }

    #[test]
    fn parses_header_and_param_overrides() {
        let prefab = EffectPrefab::from_text(&sample_text()).expect("parse");
        assert_eq!(prefab.name, "炎刃斩击");
        assert_eq!(prefab.graph_path, "assets/fx/flame_blade_slash.sgraph");
        assert!(prefab.overlay);
        assert_eq!(prefab.overrides.get("spin"), Some(&2.4));
        assert_eq!(prefab.overrides.get("color_r"), Some(&1.0));
    }

    #[test]
    fn overlay_defaults_to_true_and_can_be_turned_off() {
        let text = "kind=effect\ngraph=assets/fx/a.sgraph\n";
        assert!(EffectPrefab::from_text(text).expect("parse").overlay);

        let text = "kind=effect\ngraph=assets/fx/a.sgraph\noverlay=false\n";
        assert!(!EffectPrefab::from_text(text).expect("parse").overlay);
    }

    /// 场景预制体（`kind` 缺省）被误当特效预设时必须在**加载前**就拒绝，
    /// 否则会把一份实体快照当成着色器去编译，报出完全无关的错误。
    #[test]
    fn scene_prefabs_are_rejected() {
        let text =
            "prefab_version=1\nasset_id=11111111-1111-4111-8111-111111111111\nname=Slime\nroot=1\n";
        let error = EffectPrefab::from_text(text).expect_err("不是特效预设");
        assert!(
            matches!(error, EffectPrefabError::NotAnEffect { .. }),
            "场景预制体应当判为 NotAnEffect，实际 {error}"
        );
    }

    #[test]
    fn absolute_and_parent_paths_are_rejected() {
        for bad in ["/etc/passwd", "C:/x.sgraph", "../outside.sgraph"] {
            let text = format!("kind=effect\ngraph={bad}\n");
            let error = EffectPrefab::from_text(&text).expect_err("应当拒绝");
            assert!(
                matches!(error, EffectPrefabError::BadGraphPath(_)),
                "{bad} 应当被判为非法路径，实际 {error}"
            );
        }
    }

    #[test]
    fn param_lines_need_three_fields_and_finite_values() {
        let two = "kind=effect\ngraph=a.sgraph\nparam\tspin\n";
        assert!(EffectPrefab::from_text(two).is_err());

        let nan = "kind=effect\ngraph=a.sgraph\nparam\tspin\tnan\n";
        assert!(EffectPrefab::from_text(nan).is_err());

        let not_number = "kind=effect\ngraph=a.sgraph\nparam\tspin\tfast\n";
        assert!(EffectPrefab::from_text(not_number).is_err());
    }

    #[test]
    fn unknown_lines_are_reported_with_line_number() {
        let text = "kind=effect\ngraph=a.sgraph\nbogus=1\n";
        match EffectPrefab::from_text(text).expect_err("应当报错") {
            EffectPrefabError::Parse { line, .. } => assert_eq!(line, 3),
            other => panic!("期望 Parse 错误，实际 {other}"),
        }
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let text = "# 注释\n\nkind=effect\n# 又一条\ngraph=assets/fx/a.sgraph\n";
        assert!(EffectPrefab::from_text(text).is_ok());
    }

    /// 端到端：示例工程的预设 → 编译真实 `.sgraph` → 参数覆盖落进槽位。
    ///
    /// 抓的假绿：`from_text` 全绿但 `load` 根本没把 `param` 行合进 `slots`——那样脚本
    /// `effect.play` 出来的画面全是模板默认值，预设文件形同虚设。
    #[test]
    fn loads_the_sample_preset_and_merges_overrides_into_slots() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../examples/game_api")
            .canonicalize()
            .expect("示例工程路径");
        let prefab =
            EffectPrefab::load(&root, "assets/fx/flame_blade_slash.prefab").expect("加载示例预设");
        assert_eq!(prefab.name, "炎刃斩击");
        assert!(!prefab.source.is_empty(), "应当编译出叠加层 WGSL");

        let names = prefab.param_names();
        let slot_of = |key: &str| {
            names
                .iter()
                .find(|(name, _, _)| name == key)
                .map(|(_, slot, component)| (*slot, *component))
                .unwrap_or_else(|| panic!("预设里写了 {key}，但图里没有这个参数"))
        };
        let (spin_slot, spin_component) = slot_of("spin");
        assert!((prefab.compiled.slots[spin_slot][spin_component] - 2.4).abs() < 1e-6);
        let (green_slot, green_component) = slot_of("color_g");
        assert!((prefab.compiled.slots[green_slot][green_component] - 0.42).abs() < 1e-6);

        // 预设没写的参数保持**模板默认**。示例预设现在写了 radius（舞台需要把弧推到 3D
        // 视口里），所以改用 `arc_width` 检验"没写就跟随图"：它在示例图里是 0.24。
        let (arc_slot, arc_component) = slot_of("arc_width");
        assert!(
            !prefab.overrides.contains_key("arc_width"),
            "这条断言的前提是预设没写 arc_width"
        );
        let expected = {
            let (_, compiled) = crate::shader_graph::ShaderGraph::load_runtime_overlay(
                &root,
                "assets/fx/flame_blade_slash.sgraph",
            )
            .expect("示例图应当能编译");
            compiled.slots[arc_slot][arc_component]
        };
        assert!(
            (prefab.compiled.slots[arc_slot][arc_component] - expected).abs() < 1e-6,
            "没写的参数应当保持图的默认值（{} vs {expected}）",
            prefab.compiled.slots[arc_slot][arc_component]
        );
    }

    /// 同一张图、两份预设 → 槽位不同。这是"预设而不是复制图"的核心断言。
    #[test]
    fn two_presets_on_one_graph_differ_only_in_values() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../examples/game_api")
            .canonicalize()
            .expect("示例工程路径");
        let warm = EffectPrefab::load(&root, "assets/fx/flame_blade_slash.prefab").expect("暖色");
        let frost = EffectPrefab::load(&root, "assets/fx/frost_blade_slash.prefab").expect("冷色");

        assert_eq!(warm.graph_path, frost.graph_path, "两份预设指向同一张图");
        assert_ne!(warm.name, frost.name);
        assert_ne!(warm.compiled.slots, frost.compiled.slots, "参数必须不同");
        assert_eq!(
            warm.compiled.params.len(),
            frost.compiled.params.len(),
            "命名参数表由图决定，预设不改结构"
        );
    }

    /// 预设引用了图里没有的参数名时必须**加载期**报错，并列出可用参数。
    #[test]
    fn unknown_param_is_rejected_at_load_time_with_the_known_list() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../examples/game_api")
            .canonicalize()
            .expect("示例工程路径");
        let path = root.join("assets/fx/__tmp_unknown_param.prefab");
        std::fs::write(
            &path,
            "kind=effect\ngraph=assets/fx/flame_blade_slash.sgraph\nparam\tspin_speeed\t1.0\n",
        )
        .expect("写临时预设");
        let error = EffectPrefab::load(&root, "assets/fx/__tmp_unknown_param.prefab")
            .expect_err("拼错的参数名必须报错");
        let _ = std::fs::remove_file(&path);
        let text = error.to_string();
        assert!(
            text.contains("spin_speeed"),
            "错误里要指出写错的名字：{text}"
        );
        assert!(text.contains("spin"), "错误里要列出可用参数：{text}");
    }
}
