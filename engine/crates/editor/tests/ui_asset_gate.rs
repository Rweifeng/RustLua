//! M76-05/06 发布门：扫描仓库内每个工程（含 `project.toml` 的目录）的全部
//! `.ui.json` 与资产边车。来源边车按工程根解析，所以不能拿仓库根当唯一 store。
//!
//! 这是 `verify.ps1` 的独立门：坏 JSON、缺来源、缺资产文件、未审阅被引用、
//! 单件/总包超体积都会让这里红；三宿主未测只作为读数打印，绝不写成“通过”。

use editor::review::{HOST_KEYS, review_project};
use editor::ui_spec_host::UiSpecStore;
use rustlua_ui_spec::AssetLimits;
use std::path::{Path, PathBuf};

const SKIP_DIRS: [&str; 3] = ["target", "node_modules", "builds"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("仓库根")
        .to_path_buf()
}

fn discover_projects(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    if dir.join("project.toml").is_file() {
        found.push(dir.to_path_buf());
    }
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            discover_projects(&entry.path(), depth + 1, found);
        }
    }
}

#[test]
fn repository_ui_documents_pass_the_release_gate() {
    let repo = repo_root();
    let mut projects = Vec::new();
    discover_projects(&repo, 0, &mut projects);
    projects.sort();
    assert!(!projects.is_empty(), "仓库里必须至少有一个工程根");

    let limits = AssetLimits::default();
    let mut scanned = 0;
    let mut errors = 0;
    let mut warnings = 0;
    let mut untested_hosts = 0;
    let mut total_bytes = 0_u64;
    let mut host_entries = 0_usize;

    for project in &projects {
        let store = UiSpecStore::new(project);
        let report = review_project(&store, &limits).expect("扫描工程");
        let relative = project
            .strip_prefix(&repo)
            .unwrap_or(project)
            .to_string_lossy()
            .replace('\\', "/");
        println!(
            "UI 发布门 · {relative}：{} 份 .ui.json · error {} · warning {} · 资产 {} B · 未测宿主 {}",
            report.scanned,
            report.errors(),
            report.warnings(),
            report.total_bytes,
            report.untested_hosts(),
        );
        for document in &report.documents {
            host_entries += document.hosts.len();
            println!(
                "  {} · revision {} · error {} · warning {} · 未审阅 {} · 生成 {}",
                document.document,
                document.revision,
                document.errors,
                document.warnings,
                document.unreviewed.len(),
                document.generated.len(),
            );
        }
        scanned += report.scanned;
        errors += report.errors();
        warnings += report.warnings();
        untested_hosts += report.untested_hosts();
        total_bytes += report.total_bytes;
    }
    println!(
        "汇总：{} 个工程 · {} 份 .ui.json · error {errors} · warning {warnings} · 资产 {total_bytes} B · 未测宿主 {untested_hosts}",
        projects.len(),
        scanned,
    );
    assert_eq!(errors, 0, "UI 发布门红：见上方逐工程诊断");
    assert!(scanned > 0, "工程里至少要有一份 .ui.json，否则这不是门");
    assert_eq!(
        host_entries,
        scanned * HOST_KEYS.len(),
        "每份文档都必须带齐三宿主证据项（未测就写 untested）"
    );
    assert!(
        untested_hosts <= host_entries,
        "未测宿主数不能超过证据项总数"
    );

    let repo_store = UiSpecStore::new(&repo);
    for document in repo_store.document_paths().expect("扫描仓库文档") {
        let path = repo.join(&document);
        assert!(
            projects.iter().any(|project| path.starts_with(project)),
            "孤儿 .ui.json 不在任何工程根下，发布门无从解析它的资产来源：{document}"
        );
    }
}
