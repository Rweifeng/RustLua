//! Command-line argument parsing, project discovery and resource-integrity
//! verification, used by both interactive and `--check` modes.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Default)]
pub struct Arguments {
    pub project: Option<PathBuf>,
    pub log_filter: Option<String>,
    pub title: Option<String>,
    /// 发布包模式：目录为 assetc package 生成的发布资源目录，启动期严格校验资源。
    pub pack: bool,
    /// 仅校验资源完整性后退出（不弹窗口），供 CI/打包自测使用。
    pub check: bool,
    /// 关闭 CPU 视锥剔除（M8-08 基线回退）。
    pub no_cull: bool,
    /// 关闭实例化（M8-08 基线回退）。
    pub no_instance: bool,
    /// 加载集观测：把本次运行的资源读取按场景上下文写成 JSON（`docs/67` §5）。
    pub trace_loads: Option<PathBuf>,
}

pub fn parse_arguments() -> Result<Arguments, String> {
    let mut args = Arguments::default();
    let mut values = env::args().skip(1);
    while let Some(value) = values.next() {
        match value.as_str() {
            "--project" => {
                args.project = Some(PathBuf::from(
                    values.next().ok_or("--project requires a path")?,
                ))
            }
            "--pack" => {
                args.project = Some(PathBuf::from(
                    values.next().ok_or("--pack requires a path")?,
                ));
                args.pack = true;
            }
            "--check" => args.check = true,
            "--no-cull" => args.no_cull = true,
            "--no-instance" => args.no_instance = true,
            "--log-filter" => {
                args.log_filter = Some(
                    values
                        .next()
                        .ok_or("--log-filter requires a tracing filter")?,
                )
            }
            "--title" => args.title = Some(values.next().ok_or("--title requires a title")?),
            "--trace-loads" => {
                args.trace_loads = Some(PathBuf::from(
                    values.next().ok_or("--trace-loads requires a path")?,
                ));
            }
            "--help" | "-h" => {
                println!(
                    "Usage: game [--project <game-project-root>] [--pack <release-pack>] [--title <window-title>] [--check] [--log-filter <tracing-filter>]\n\
                     \n\
                     --project <root>  加载开发游戏工程（含 .cache 等本地工作目录）\n\
                     --pack <dir>      加载 assetc package 生成的发布资源目录（严格校验资源完整性）\n\
                     --title <title>   设置窗口标题；未指定时优先读取发布包 release.toml\n\
                     --check           仅校验资源完整性后退出（exit 0/1，不弹窗口）\n\
                     --trace-loads <file>  采集资源加载集（按场景上下文）并写 JSON"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

/// When no `--project` is given, walks up from the executable to find a game
/// project: either a directory with its own `project.toml` (packaged layout) or
/// a sibling `game/project.toml` (workspace layout). Lets double-clicking the
/// exe launch the game instead of the diagnostic scene.
///
/// 顺序即优先级：显式 `--project` > `pack/`（发布包）> 检出目录里的示例工程。
/// 示例工程按 `examples/game_api`（API 演示工程）优先，其次 `examples`/`game`
/// （历史布局与外部游戏工程）。
pub fn find_project_root() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)?;
    let packaged = exe_dir.join("pack").join("project.toml");
    if packaged.is_file() {
        return packaged.parent().map(Path::to_path_buf);
    }
    for dir in exe_dir.ancestors() {
        let direct = dir.join("project.toml");
        if direct.is_file() {
            return Some(dir.to_path_buf());
        }
        for project_name in ["examples/game_api", "examples", "game"] {
            let project = dir.join(project_name).join("project.toml");
            if project.is_file() {
                return Some(dir.join(project_name));
            }
        }
    }
    None
}

pub fn resolve_window_title(project: Option<&Path>, explicit: Option<String>) -> String {
    if let Some(title) = explicit.filter(|title| !title.trim().is_empty()) {
        return title;
    }
    if let Some(title) = release_title() {
        return title;
    }
    if let Some(root) = project
        && let Ok(text) = fs::read_to_string(root.join("project.toml"))
        && let Some(name) = extract_key(&text, "name")
    {
        return name;
    }
    "RustLua Engine".to_owned()
}

fn release_title() -> Option<String> {
    let exe_dir = env::current_exe().ok()?.parent().map(Path::to_path_buf)?;
    let text = fs::read_to_string(exe_dir.join("release.toml")).ok()?;
    extract_key(&text, "publish_name")
}

/// 校验项目/发布包资源完整性：项目清单、主脚本、启动场景及场景引用的资源。
/// 返回缺失项列表（空表示完整）。
pub fn verify_project(root: &Path) -> Vec<String> {
    let mut missing = verify_project_inner(root);
    if let Err(error) = crate::splash::load(root) {
        missing.push(error);
    }
    if let Err(error) = crate::splash::load_icon(root) {
        missing.push(error);
    }
    missing
}

fn verify_project_inner(root: &Path) -> Vec<String> {
    let mut missing = Vec::new();
    let project_toml = root.join("project.toml");
    // 全走 `vfs`：发布包形态下 `project.toml` 是**骨架**（磁盘上有），但 `scripts/`、`scenes/`、
    // `assets/` 只在 RLB 容器里——用 `path.is_file()` / `fs::read` 会把它们全判成"缺失"
    // （批 A 落地时实测过：`game --pack <单包> --check` 当场红）。见 `docs/67` §8 第 2 项。
    if !rustlua_infra::vfs::is_file(&project_toml) {
        missing.push(format!("缺少项目清单 {}", project_toml.display()));
        return missing;
    }
    let Some(text) = read_text(&project_toml) else {
        missing.push(format!("无法读取项目清单 {}", project_toml.display()));
        return missing;
    };
    if let Some(script) = extract_key(&text, "main_script") {
        let path = root.join(script);
        if !rustlua_infra::vfs::is_file(&path) {
            missing.push(format!("缺少主脚本 {}", path.display()));
        }
    }
    if let Some(scene) = extract_key(&text, "startup_scene") {
        let path = root.join(scene);
        if !rustlua_infra::vfs::is_file(&path) {
            missing.push(format!("缺少启动场景 {}", path.display()));
        } else if let Some(scene_text) = read_text(&path) {
            for asset in extract_asset_refs(&scene_text) {
                let asset_path = root.join(&asset);
                if !rustlua_infra::vfs::is_file(&asset_path) {
                    let message = format!("场景引用的资源缺失 {}", asset);
                    if !missing.contains(&message) {
                        missing.push(message);
                    }
                }
            }
        }
    }
    // M20-01：脚本语法校验（导入期同一校验器）+ 领域数据校验（复用工程
    // 自身 validate_data.lua），让发布前检查覆盖启动期会失败的问题。
    missing.extend(verify_script_syntax(root));
    missing.extend(verify_domain_data(root));
    missing
}

// An optional image must not prevent startup; --check still reports it.
pub fn verify_startup(root: &Path, packaged: bool) -> Result<(), String> {
    let issues = verify_project_inner(root);
    for item in &issues {
        tracing::error!(target: "game", "资源校验失败: {item}");
    }
    if packaged && !issues.is_empty() {
        return Err(format!(
            "发布资源包校验失败（{} 项缺失）：\n  {}",
            issues.len(),
            issues.join("\n  ")
        ));
    }
    Ok(())
}

/// M20-01：校验 `scripts/**/*.lua` 全部脚本语法（与导入期共用 Importer 校验器，
/// 错误含文件路径与 Lua 行列定位）。
fn verify_script_syntax(root: &Path) -> Vec<String> {
    use rustlua_assets_import::builtin;
    let mut issues = Vec::new();
    let scripts_root = root.join("scripts");
    // 目录形态下这里是真实目录；发布包形态下 `scripts/` 只在容器里，磁盘上不存在。
    // 因此**不能**用 `scripts_root.is_dir()` 提前返回，枚举走 `vfs::list_files_recursive`
    // （容器条目 ∪ 磁盘目录）。
    let mut files = rustlua_infra::vfs::list_files_recursive(&scripts_root);
    files.sort();
    let registry = builtin();
    for file in files {
        let is_lua = file
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("lua"));
        if !is_lua {
            continue;
        }
        let Some(importer) = registry.from_path(&file) else {
            continue;
        };
        let Ok(bytes) = rustlua_infra::vfs::read(&file) else {
            issues.push(format!("无法读取脚本 {}", file.display()));
            continue;
        };
        if let Err(error) = importer.validate(&file, &bytes) {
            issues.push(error);
        }
    }
    issues
}

/// M20-01：领域数据校验。约定入口 `scripts/validate_data.lua`（若存在）返回
/// `check()` 字符串：`""` 通过，否则换行分隔的问题列表。规则全部来自工程
/// 自身，不在此硬编码任何 NPC/任务语义；未提供该模块的工程跳过此项。
fn verify_domain_data(root: &Path) -> Vec<String> {
    let entry = root.join("scripts").join("validate_data.lua");
    if !rustlua_infra::vfs::is_file(&entry) {
        return Vec::new();
    }
    let runtime = match rustlua_scripting::script::LuaRuntime::with_installers(
        rustlua_gameplay::input_lua::registry(),
    ) {
        Ok(runtime) => runtime,
        Err(error) => return vec![format!("Lua 运行时初始化失败: {error}")],
    };
    let mut runtime = runtime;
    runtime.set_project_root(root.to_path_buf());
    // 以校验入口自身作为工程脚本执行：既建立 require 搜索路径，又不启动游戏。
    if let Err(error) = runtime.execute_project_script(&entry) {
        return vec![format!(
            "领域数据校验入口加载失败 {}: {error}",
            entry.display()
        )];
    }
    match runtime.execute_console("return require('validate_data').check()") {
        Ok(report) => report
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
        Err(error) => vec![format!("领域数据校验执行失败: {error}")],
    }
}

/// 读一份工程文本：走 `vfs`（provider 优先，桌面无 provider 时回落文件系统）。
///
/// 发布包形态下 `project.toml` 在骨架里、`scenes/*.scene` 在容器里，两条通道必须同源——
/// 这也是把 `fs::read_to_string` 换成它的原因。
fn read_text(path: &Path) -> Option<String> {
    let bytes = rustlua_infra::vfs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// 递归收集目录下的全部文件（诊断扫描用；符号链接不跟随）。
///
/// 已由 [`rustlua_infra::vfs::list_files_recursive`] 取代（容器条目 ∪ 磁盘目录）。
/// 保留这个名字只为让调用点读起来仍然是"收集文件"的语义。
#[allow(dead_code)]
fn collect_files(root: &Path, out: &mut Vec<PathBuf>) {
    out.extend(rustlua_infra::vfs::list_files_recursive(root));
}

/// 从 `key = "value"` 样式的项目清单文本中提取字段值。
fn extract_key(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim().trim_start_matches('\u{feff}');
        line.strip_prefix(&format!("{key} ="))
            .map(|rest| rest.trim().trim_matches('"').to_owned())
    })
}

/// 提取场景文本中以 `assets/` 开头、到 `;`/`:`/换行为止的资源引用。
/// 共享 `rustlua_assets::dependency` 提取器，保证运行时校验与依赖图同源。
fn extract_asset_refs(text: &str) -> Vec<String> {
    use rustlua_assets::dependency::DependencyRef;
    use rustlua_assets::resource_kind::ResourceKind;
    rustlua_assets::dependency::extract_refs(text, ResourceKind::Scene)
        .into_iter()
        .filter_map(|reference| match reference {
            DependencyRef::Asset(path) => Some(path.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{extract_asset_refs, extract_key, resolve_window_title, verify_project};

    #[test]
    fn splash_check_missing_corrupt_plain_and_packed_images() {
        let root =
            std::env::temp_dir().join(format!("rustlua-splash-check-{}", std::process::id()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let config = root.join("project.toml");
        std::fs::write(&config, "name = \"SplashTest\"\n").unwrap();
        assert!(
            verify_project(&root).is_empty(),
            "no configuration preserves old check"
        );
        std::fs::write(&config, "splash_image = \"assets/start.png\"\n").unwrap();
        let issues = verify_project(&root);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("splash_image 读取失败"));
        assert!(
            super::verify_startup(&root, true).is_ok(),
            "optional image must not prevent startup"
        );
        std::fs::write(root.join("assets/start.png"), b"broken").unwrap();
        assert!(verify_project(&root)[0].contains("splash_image 解码失败"));
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let image = image::RgbImage::from_pixel(4, 2, image::Rgb([20, 40, 80]));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image.write_to(&mut bytes, format).unwrap();
            for packed in [false, true] {
                let data = rustlua_infra::pack::encode(bytes.get_ref(), packed, packed);
                std::fs::write(root.join("assets/start.png"), data).unwrap();
                assert!(verify_project(&root).is_empty());
                assert_eq!(
                    crate::splash::load(&root).unwrap().unwrap().dimensions(),
                    (4, 2)
                );
            }
        }
        std::fs::write(&config, "splash_image = \"../escape.png\"\n").unwrap();
        assert!(verify_project(&root)[0].contains("相对路径"));
        std::fs::write(&config, "main_script = \"missing.lua\"\n").unwrap();
        assert!(super::verify_startup(&root, false).is_ok());
        assert!(super::verify_startup(&root, true).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn icon_check_and_startup_fallback_plain_and_packed_images() {
        let root = std::env::temp_dir().join(format!("rustlua-icon-check-{}", std::process::id()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let config = root.join("project.toml");
        std::fs::write(&config, "name = \"IconTest\"\n").unwrap();
        let default_splash = crate::splash::startup_image(None).unwrap();
        let default_icon = crate::splash::icon_image(None);
        assert_eq!(
            crate::splash::startup_image(Some(&root)).unwrap(),
            default_splash
        );
        assert_eq!(crate::splash::icon_image(Some(&root)), default_icon);
        std::fs::write(
            &config,
            "icon_image = \"assets/icon.png\"\nsplash_image = \"assets/start.png\"\n",
        )
        .unwrap();
        let issues = verify_project(&root);
        assert_eq!(issues.len(), 2);
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("icon_image 读取失败"))
        );
        assert!(super::verify_startup(&root, true).is_ok());
        std::fs::write(root.join("assets/icon.png"), b"broken").unwrap();
        std::fs::write(root.join("assets/start.png"), b"broken").unwrap();
        assert!(
            verify_project(&root)
                .iter()
                .any(|issue| issue.contains("icon_image 解码失败"))
        );
        assert_eq!(
            crate::splash::startup_image(Some(&root)).unwrap(),
            default_splash
        );
        assert_eq!(crate::splash::icon_image(Some(&root)), default_icon);
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let image = image::RgbImage::from_pixel(4, 2, image::Rgb([20, 40, 80]));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image.write_to(&mut bytes, format).unwrap();
            let expected = image::load_from_memory(bytes.get_ref()).unwrap().to_rgba8();
            for packed in [false, true] {
                let data = rustlua_infra::pack::encode(bytes.get_ref(), packed, packed);
                std::fs::write(root.join("assets/icon.png"), &data).unwrap();
                std::fs::write(root.join("assets/start.png"), &data).unwrap();
                assert!(verify_project(&root).is_empty());
                assert_eq!(crate::splash::icon_image(Some(&root)), expected);
                assert_eq!(crate::splash::startup_image(Some(&root)).unwrap(), expected);
            }
        }
        for path in ["../escape.png", "/absolute.png", "C:\\absolute.png"] {
            std::fs::write(&config, format!("icon_image = \"{path}\"\n")).unwrap();
            assert!(verify_project(&root)[0].contains("icon_image 路径必须"));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extract_key_parses_quoted_project_fields() {
        let text = "[project]\nname = \"game\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n";
        assert_eq!(
            extract_key(text, "main_script"),
            Some("scripts/main.lua".to_owned())
        );
        assert_eq!(
            extract_key(text, "startup_scene"),
            Some("scenes/main.scene".to_owned())
        );
        assert_eq!(extract_key(text, "missing"), None);
        assert_eq!(
            extract_key("\u{feff}publish_name = \"Demo\"", "publish_name"),
            Some("Demo".to_owned())
        );
    }

    #[test]
    fn resolve_window_title_prefers_explicit_title_then_project_name() {
        assert_eq!(
            resolve_window_title(None, Some("Published".to_owned())),
            "Published"
        );
        let root = std::env::temp_dir().join(format!("rustlua-title-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nname = \"DemoGame\"\n",
        )
        .unwrap();
        assert_eq!(resolve_window_title(Some(&root), None), "DemoGame");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn extract_asset_refs_scans_component_strings() {
        let scene = "component 1 Sprite texture=assets/textures/player.png;size=32\n\
                     component 1 Animation default=idle;clip:idle:assets/textures/hero-idle.png:4:4:true\n\
                     component 2 Tilemap texture=assets/textures/tileset.png;width=2\n";
        let refs = extract_asset_refs(scene);
        assert!(refs.contains(&"assets/textures/player.png".to_owned()));
        assert!(refs.contains(&"assets/textures/hero-idle.png".to_owned()));
        assert!(refs.contains(&"assets/textures/tileset.png".to_owned()));
        assert!(!refs.contains(&"assets/missing.png".to_owned()));
    }

    #[test]
    fn verify_project_reports_missing_resources_once() {
        let root = std::env::temp_dir().join(format!("rustlua-verify-{}", std::process::id()));
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scenes/main.scene"),
            "component 1 Sprite texture=assets/textures/player.png\ncomponent 2 Sprite texture=assets/textures/player.png\n",
        )
        .unwrap();
        // 主脚本缺失 + player.png 缺失（引用两次，应去重为一条）
        let missing = verify_project(&root);
        assert_eq!(
            missing.len(),
            2,
            "missing script + missing asset: {missing:?}"
        );
        assert!(missing.iter().any(|m| m.contains("main.lua")));
        assert_eq!(
            missing.iter().filter(|m| m.contains("player.png")).count(),
            1,
            "duplicate asset reference deduplicated"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_project_passes_for_complete_project() {
        let root = std::env::temp_dir().join(format!("rustlua-verify-ok-{}", std::process::id()));
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/main.lua"),
            "function update() return 'ok' end\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\nname=main\n",
        )
        .unwrap();
        assert!(verify_project(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M20-01：发布前检查同时覆盖脚本语法与工程自定义领域数据校验，
    /// 诊断含文件路径（以及 Lua/数据的行或字段定位）。
    #[test]
    fn verify_project_reports_script_syntax_and_domain_data_issues() {
        let root = std::env::temp_dir().join(format!("rustlua-verify-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("scripts/data")).unwrap();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/main.lua"),
            "function update() return 'ok' end\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\nname=main\n",
        )
        .unwrap();
        // 故意写坏一个脚本：缺少 end（第 3 行 EOF 报错）。
        std::fs::write(
            root.join("scripts/broken.lua"),
            "function update()\n  return 1\n",
        )
        .unwrap();
        // 工程自定义领域校验入口：报告一条含文件/对象/字段路径的问题。
        std::fs::write(
            root.join("scripts/validate_data.lua"),
            "local M = {}\n\
             function M.check()\n\
               return \"[data.demo] data/demo.lua:items[1].name: required non-empty string\"\n\
             end\n\
             return M\n",
        )
        .unwrap();

        let issues = verify_project(&root);
        let syntax = issues
            .iter()
            .find(|issue| issue.contains("broken.lua"))
            .unwrap_or_else(|| panic!("缺少脚本语法诊断: {issues:?}"));
        assert!(
            syntax.contains("语法错误") && syntax.contains(":3:"),
            "语法诊断需含类型与行号: {syntax}"
        );
        let domain = issues
            .iter()
            .find(|issue| issue.contains("data/demo.lua:items[1].name"))
            .unwrap_or_else(|| panic!("缺少领域数据诊断: {issues:?}"));
        assert!(
            domain.contains("[data.demo]"),
            "领域诊断需含模块归属: {domain}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M20-01：合法工程（含校验入口返回空）不应产生任何诊断。
    #[test]
    fn verify_project_accepts_valid_scripts_and_domain_data() {
        let root =
            std::env::temp_dir().join(format!("rustlua-verify-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("scripts/data")).unwrap();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/main.lua"),
            "function update() return 'ok' end\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/data/demo.lua"),
            "return { version = 1 }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/validate_data.lua"),
            "local M = {}\nfunction M.check() return \"\" end\nreturn M\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\nname=main\n",
        )
        .unwrap();
        assert!(
            verify_project(&root).is_empty(),
            "clean project must pass: {:?}",
            verify_project(&root)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_asset_refs_scans_3d_components() {
        let scene = "component 1 MeshRenderer mesh=assets/models/hero.glb\n\
                     component 1 Camera3D position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100\n\
                     component 2 MeshRenderer mesh=__ground_plane__\n";
        let refs = extract_asset_refs(scene);
        assert!(refs.contains(&"assets/models/hero.glb".to_owned()));
        assert!(!refs.contains(&"__ground_plane__".to_owned()));
        assert!(!refs.contains(&"assets/missing.glb".to_owned()));
    }

    #[test]
    fn verify_project_reports_missing_3d_mesh() {
        let root = std::env::temp_dir().join(format!("rustlua-verify-3d-{}", std::process::id()));
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::create_dir_all(root.join("assets/models")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/main.lua"),
            "function update() return 'ok' end\n",
        )
        .unwrap();
        // v2 scene referencing an existing and a missing 3D mesh.
        std::fs::write(root.join("assets/models/cube.glb"), b"fake glb bytes").unwrap();
        std::fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\nname=3d\n\
             component 1 MeshRenderer mesh=assets/models/cube.glb\n\
             component 2 MeshRenderer mesh=assets/models/missing.glb\n",
        )
        .unwrap();
        let missing = verify_project(&root);
        assert_eq!(
            missing.iter().filter(|m| m.contains("missing.glb")).count(),
            1,
            "missing 3D mesh reported: {missing:?}"
        );
        assert!(
            missing.iter().all(|m| !m.contains("cube.glb")),
            "existing 3D mesh must not be reported: {missing:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
