//! M84 P3：Spine Runtimes License 的边界必须由**可执行的检查**守着，而不是靠注释。
//!
//! 该许可证允许"加载并渲染 Spine 数据"，但**禁止用运行时构建骨骼编辑/创作工具**。
//! 这条约束落到代码上是两条：
//!
//! 1. `rustlua-spine` 只**读**骨架，不提供把骨架写回 `.skel`/`.spineatlas` 的导出 API；
//! 2. 编辑器对 Spine 的唯一操作是"造组件载荷字符串"（`to_component_str`），
//!    那是场景文件里的字段行，不是骨架数据。
//!
//! 下面的测试把这两条变成断言。它们看的是**源码文本**——听起来笨，但这类"不该存在的
//! 功能"只能用"不该出现的符号"来表达；等真出现了再靠人工 review 拦住，代价太大。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` = engine/crates/rustlua-spine
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("engine 根")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("读 {} 失败: {error}", path.display()))
}

/// 运行时 crate 不得提供骨架导出/序列化能力。
///
/// 允许 `to_component_str`（组件字段行），禁止出现把 `SkeletonData` 写成 Spine 数据文件的
/// 函数——那正是许可证要禁的"创作工具"能力。
#[test]
fn runtime_never_exports_skeleton_data() {
    let src = repo_root().join("crates/rustlua-spine/src");
    let mut offenders = Vec::new();
    for entry in walk(&src) {
        let text = read(&entry);
        for (line_no, line) in text.lines().enumerate() {
            let lowered = line.to_ascii_lowercase();
            // 函数签名里出现"写回骨架"的动词，就是越界。
            let writes_skeleton = (lowered.contains("fn write_skel")
                || lowered.contains("fn export_skel")
                || lowered.contains("fn save_skel")
                || lowered.contains("fn write_atlas")
                || lowered.contains("fn export_atlas"))
                && line.contains("fn ");
            if writes_skeleton {
                offenders.push(format!(
                    "{}:{}: {}",
                    entry.display(),
                    line_no + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "Spine Runtimes License 禁止用运行时构建创作工具；发现疑似骨架导出 API：{offenders:#?}"
    );
}

/// 编辑器的 Spine 操作只允许"造组件载荷字符串"，不允许直接读写骨架数据文件。
#[test]
fn editor_only_builds_component_payloads() {
    let editor = repo_root().join("crates/editor/src");
    let mut offenders = Vec::new();
    for entry in walk(&editor) {
        let text = read(&entry);
        for (line_no, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            // 直接碰 Spine 数据类型（而不是 `SpineSkeletonComponent` 的字符串方法）就越界。
            let touches_skeleton_data = trimmed.contains("SkeletonData")
                || trimmed.contains("SpineData::from_bytes")
                || trimmed.contains("spine2d::");
            if touches_skeleton_data {
                offenders.push(format!("{}:{}: {trimmed}", entry.display(), line_no + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "编辑器只应构造 `SpineSkeletonComponent` 载荷，不应直接操作骨架数据：{offenders:#?}"
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    out
}
