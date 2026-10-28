//! Web / 微信的 UI 证据 harness（wasm32）。
//!
//! 输入是一份 `.ui.json` 的文本与工程 `project.toml` 的文本（JS 侧读盘后传进来），输出是
//! [`rustlua_ui_host_evidence`] 算出的读数：版面矩形、提交顶点块序、revision 与红项。
//! 判定逻辑与 pc 例子共用同一个 crate，所以这里只负责"把输入递进去、把结论递出来"。
//!
//! 为什么要 wasm：宿主证据必须由**宿主的编译目标**自己跑。原生 pc 的读数不能替 Web/微信
//! 背书（`f32`、资源可达性、wasm 上没有文件系统）；反过来，这台 harness 也不碰浏览器
//! canvas/WebGL——它测的是"文档 → 场景 → 宿主解算"这一段，与 `pc` 同一口径。
//!
//! 工程配置：wasm 上 `std::fs` 恒失败，`RuntimeScene` 读 `project.toml`（字体/文本渲染模式
//! 等）会走 `vfs`。真实宿主把工程作为资源表挂进 `vfs`（`game_web::bundle_mount`），这里
//! 同样装一个只含 `project.toml` 的 provider——否则读数会落在与 pc 不同的缺省配置上，
//! "三宿主一致"就成了拿两套配置比数字。

use std::path::Path;

use rustlua_ui_host_evidence::{HostRequest, run_evidence};
use serde_json::json;

/// 跑一次证据，返回 `{ "ok", "revision", "problems", "report" }`（JSON 文本）。
///
/// 失败（解析/导出/实例化）返回 `{ "ok": false, "error": "..." }`；`report` 只在成功时出现。
/// JS 侧必须看 `ok`：`problems` 非空同样是失败——证据不允许"带着红项落盘"。
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn ui_host_evidence(
    document: &str,
    host: &str,
    command: &str,
    spec_json: &str,
    project_toml: &str,
) -> String {
    if !matches!(host, "pc" | "web" | "wechat") {
        return json!({ "ok": false, "error": format!("未知宿主键 {host:?}，只认 pc/web/wechat") })
            .to_string();
    }
    if let Err(error) = install_project_config(project_toml) {
        return json!({ "ok": false, "error": format!("工程配置挂载失败：{error}") }).to_string();
    }
    let request = HostRequest {
        document,
        host,
        command,
        scene_name: "ui_matrix",
        // 与 `game_web` 同一口径：wasm 上没有真实文件系统，工程根是虚拟 `/project`。
        project_root: Some(Path::new("/project")),
    };
    match run_evidence(spec_json.as_bytes(), &request) {
        Ok(report) => json!({
            "ok": report.problems.is_empty(),
            "revision": report.revision,
            "problems": report.problems,
            "report": report.report,
        })
        .to_string(),
        Err(error) => json!({ "ok": false, "error": error }).to_string(),
    }
}

/// 把 `project.toml` 挂进 vfs（wasm 单线程的 thread_local provider）。
///
/// 原生构建不动 provider：桌面宿主本来就能直接读文件，装上它反而会截断其它 `fs` 读取。
#[cfg(target_arch = "wasm32")]
fn install_project_config(project_toml: &str) -> Result<(), String> {
    let text = project_toml.as_bytes().to_vec();
    rustlua_infra::vfs::set_provider(move |path: &Path| {
        (path.file_name().is_some_and(|name| name == "project.toml")).then(|| text.clone())
    })
    .map_err(str::to_owned)
}

#[cfg(not(target_arch = "wasm32"))]
fn install_project_config(_project_toml: &str) -> Result<(), String> {
    Ok(())
}
