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
    /// 发布包模式：目录为 assetc package 生成的发布资源目录，启动期严格校验资源。
    pub pack: bool,
    /// 仅校验资源完整性后退出（不弹窗口），供 CI/打包自测使用。
    pub check: bool,
    /// 关闭 CPU 视锥剔除（M8-08 基线回退）。
    pub no_cull: bool,
    /// 关闭实例化（M8-08 基线回退）。
    pub no_instance: bool,
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
            "--help" | "-h" => {
                println!(
                    "Usage: game [--project <game-project-root>] [--pack <release-pack>] [--check] [--log-filter <tracing-filter>]\n\
                     \n\
                     --project <root>  加载开发游戏工程（含 .cache 等本地工作目录）\n\
                     --pack <dir>      加载 assetc package 生成的发布资源目录（严格校验资源完整性）\n\
                     --check           仅校验资源完整性后退出（exit 0/1，不弹窗口）"
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
        let game = dir.join("game").join("project.toml");
        if game.is_file() {
            return Some(dir.join("game"));
        }
    }
    None
}

/// 校验项目/发布包资源完整性：项目清单、主脚本、启动场景及场景引用的资源。
/// 返回缺失项列表（空表示完整）。
pub fn verify_project(root: &Path) -> Vec<String> {
    let mut missing = Vec::new();
    let project_toml = root.join("project.toml");
    if !project_toml.is_file() {
        missing.push(format!("缺少项目清单 {}", project_toml.display()));
        return missing;
    }
    let Ok(text) = fs::read_to_string(&project_toml) else {
        missing.push(format!("无法读取项目清单 {}", project_toml.display()));
        return missing;
    };
    if let Some(script) = extract_key(&text, "main_script") {
        let path = root.join(script);
        if !path.is_file() {
            missing.push(format!("缺少主脚本 {}", path.display()));
        }
    }
    if let Some(scene) = extract_key(&text, "startup_scene") {
        let path = root.join(scene);
        if !path.is_file() {
            missing.push(format!("缺少启动场景 {}", path.display()));
        } else if let Ok(scene_text) = fs::read_to_string(&path) {
            for asset in extract_asset_refs(&scene_text) {
                let asset_path = root.join(&asset);
                if !asset_path.is_file() {
                    let message = format!("场景引用的资源缺失 {}", asset);
                    if !missing.contains(&message) {
                        missing.push(message);
                    }
                }
            }
        }
    }
    missing
}

/// 从 `key = "value"` 样式的项目清单文本中提取字段值。
fn extract_key(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix(&format!("{key} ="))
            .map(|rest| rest.trim().trim_matches('"').to_owned())
    })
}

/// 提取场景文本中以 `assets/` 开头、到 `;`/`:`/换行为止的资源引用。
/// 共享 `engine::core::dependency` 提取器，保证运行时校验与依赖图同源。
fn extract_asset_refs(text: &str) -> Vec<String> {
    use engine::core::dependency::DependencyRef;
    use engine::core::resource_kind::ResourceKind;
    engine::core::dependency::extract_refs(text, ResourceKind::Scene)
        .into_iter()
        .filter_map(|reference| match reference {
            DependencyRef::Asset(path) => Some(path.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{extract_asset_refs, extract_key, verify_project};

    #[test]
    fn extract_key_parses_quoted_project_fields() {
        let text = "[project]\nname = \"p2-game\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n";
        assert_eq!(
            extract_key(text, "main_script"),
            Some("scripts/main.lua".to_owned())
        );
        assert_eq!(
            extract_key(text, "startup_scene"),
            Some("scenes/main.scene".to_owned())
        );
        assert_eq!(extract_key(text, "missing"), None);
    }

    #[test]
    fn extract_asset_refs_scans_component_strings() {
        let scene = "component 1 Sprite assets/textures/player.png;size=32\n\
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
        let root = std::env::temp_dir().join(format!("p2-verify-{}", std::process::id()));
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scenes/main.scene"),
            "component 1 Sprite assets/textures/player.png\ncomponent 2 Sprite assets/textures/player.png\n",
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
        let root = std::env::temp_dir().join(format!("p2-verify-ok-{}", std::process::id()));
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
            "scene_version=1\nname=main\n",
        )
        .unwrap();
        assert!(verify_project(&root).is_empty());
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
        let root = std::env::temp_dir().join(format!("p2-verify-3d-{}", std::process::id()));
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
