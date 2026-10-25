//! `assetc` 的单元测试（从 `lib.rs` 纯搬运，M77-05 §2）。
//!
//! 它仍是 crate 根的子模块，所以保留原来的 `use super::*;`：根模块的私有项照旧可见，
//! 搬到 `scan`/`deps`/`package` 的项经下面三条兄弟模块 glob 进入作用域。

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::*;
use crate::{deps::*, package::*, scan::*};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn package_dependency_index_resolves_dotted_require_to_subdirectory() {
    let root = temp_project("package-dependency-index-dotted");
    fs::create_dir_all(root.join("scripts/systems")).unwrap();
    fs::write(
        root.join("scripts/systems/farm.lua"),
        "local M = {}\nreturn M\n",
    )
    .unwrap();
    // 目录分隔的写法（老工程里也存在）走同一条换算，两者必须都连上。
    fs::create_dir_all(root.join("scripts/ui")).unwrap();
    fs::write(root.join("scripts/ui/hud.lua"), "return {}\n").unwrap();
    fs::write(
        root.join("scripts/main.lua"),
        "local farm = require('systems.farm')\nlocal hud = require('ui/hud')\nreturn farm, hud\n",
    )
    .unwrap();
    let out = std::env::temp_dir().join(format!(
        "rustlua-assetc-dependency-index-dotted-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = fs::remove_dir_all(&out);
    package(&root, &out, false, false, false).unwrap();
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
    let edges = index["edges"]["scripts/main.lua"].as_array().unwrap();
    for expected in ["scripts/systems/farm.lua", "scripts/ui/hud.lua"] {
        assert!(
            edges.iter().any(|path| path == expected),
            "require 边缺失 {expected}：{index}"
        );
    }
    assert!(
        index["opaque"].as_array().unwrap().is_empty(),
        "点号模块名不该把自己标成 opaque：{index}"
    );
    fs::remove_dir_all(&out).ok();
    cleanup(&root);
}

/// 声明的启动图必须随包，**哪怕它不在 `assets/` 下**（`splash/` 这类顶层目录）。
///
/// 漏了这一手的症状不是"打包失败"，而是运行期静默回落到编译期内置图（日志里只有一条
/// warn）——玩家看到的仍是引擎默认启动图，而包看着完全正常。
#[test]
fn declared_splash_outside_assets_is_packaged() {
    let root = temp_project("declared-splash");
    fs::create_dir_all(root.join("splash")).unwrap();
    // 与 `assets/textures/red-1x1.png` 同一份合法 PNG：内容必须逐字节进包。
    let png = fs::read(root.join("assets/textures/red-1x1.png")).unwrap();
    fs::write(root.join("splash/splash.png"), &png).unwrap();
    let config = fs::read_to_string(root.join("project.toml")).unwrap();
    fs::write(
        root.join("project.toml"),
        format!("{config}splash_image = \"splash/splash.png\"\n"),
    )
    .unwrap();

    let out = std::env::temp_dir().join(format!(
        "rustlua-assetc-declared-splash-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = fs::remove_dir_all(&out);
    package(&root, &out, false, false, false).unwrap();
    let manifest = fs::read_to_string(out.join("manifest.txt")).unwrap();
    assert!(
        manifest
            .lines()
            .any(|line| line.starts_with("splash/splash.png\t")),
        "声明的启动图必须进清单：{manifest}"
    );
    assert_eq!(fs::read(out.join("splash/splash.png")).unwrap(), png);

    fs::remove_dir_all(&out).ok();
    cleanup(&root);
}

/// 声明图越出工程根必须报错：`fs::copy` 会把项目外的文件悄悄塞进发布包。
#[test]
fn declared_splash_rejects_escaping_path() {
    let root = temp_project("declared-splash-escape");
    let config = fs::read_to_string(root.join("project.toml")).unwrap();
    fs::write(
        root.join("project.toml"),
        format!("{config}splash_image = \"../outside.png\"\n"),
    )
    .unwrap();
    let error = package(
        &root,
        &std::env::temp_dir().join(format!(
            "rustlua-assetc-declared-splash-escape-{}-{}",
            std::process::id(),
            unique_suffix()
        )),
        false,
        false,
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("splash_image") && error.contains("相对路径"),
        "{error}"
    );
    cleanup(&root);
}

fn temp_project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("rustlua-assetc-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("assets/textures")).unwrap();
    fs::create_dir_all(root.join("scenes")).unwrap();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(
        root.join("project.toml"),
        "[project]\nname = \"t\"\nversion = \"0.1.0\"\nengine = \"RustLua\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
    )
    .unwrap();
    fs::write(
        root.join("scripts/main.lua"),
        "-- ok\nfunction update() return 'ok' end\n",
    )
    .unwrap();
    fs::write(
        root.join("scenes/main.scene"),
        "scene_version=2\nname=main\n",
    )
    .unwrap();
    // 1x1 红色 PNG（有效可解码，与 game/assets/textures/red-1x1.png 一致）
    let png = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    fs::write(root.join("assets/textures/red-1x1.png"), png).unwrap();
    root
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be valid")
        .as_nanos()
}

fn sample_project_root() -> PathBuf {
    rustlua_infra::games_dir().join("examples").join("game_api")
}

fn copy_sample_project(label: &str) -> Option<PathBuf> {
    let source = sample_project_root();
    if !source.join("project.toml").is_file() {
        return None;
    }
    let root = std::env::temp_dir().join(format!(
        "rustlua-assetc-sample-{label}-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    copy_file(&source.join("project.toml"), &root.join("project.toml")).unwrap();
    copy_dir(&source.join("scripts"), &root.join("scripts")).unwrap();
    copy_dir(&source.join("scenes"), &root.join("scenes")).unwrap();
    copy_dir(&source.join("assets"), &root.join("assets")).unwrap();
    Some(root)
}

fn cleanup(root: &Path) {
    fs::remove_dir_all(root).ok();
}

fn replace_in_file(path: &Path, from: &str, to: &str) {
    let text = fs::read_to_string(path).unwrap();
    assert!(
        text.contains(from),
        "expected {} to contain {from}",
        path.display()
    );
    fs::write(path, text.replace(from, to)).unwrap();
}

#[test]
fn scan_registers_supported_and_marks_unsupported() {
    let root = temp_project("scan");
    let entries = scan(&root).unwrap();
    let supported: Vec<_> = entries
        .iter()
        .filter(|e| e.status != "unsupported")
        .collect();
    assert!(
        !supported.is_empty(),
        "should register at least one resource"
    );
    assert!(supported.iter().all(|e| e.asset_id.is_some()));
    assert!(entries.iter().any(|e| e.path.ends_with("red-1x1.png")));
    assert!(entries.iter().any(|e| e.path.ends_with("main.lua")));
    cleanup(&root);
}

#[test]
fn import_is_incremental_and_reuses_cache() {
    let root = temp_project("import");
    let first = import(&root).unwrap();
    assert!(first.iter().all(|o| o.error.is_none()), "first import ok");
    assert!(!first.is_empty());
    let second = import(&root).unwrap();
    assert!(
        second.iter().all(|o| o.reused),
        "second import reuses cache"
    );
    cleanup(&root);
}

#[test]
fn import_does_not_abort_on_single_bad_asset() {
    let root = temp_project("invalid-gltf");
    fs::write(root.join("assets/bad.glb"), b"invalid glb").unwrap();
    let outcomes = import(&root).unwrap();
    let bad = outcomes
        .iter()
        .find(|outcome| outcome.path.ends_with("bad.glb"))
        .expect("bad item present in outcomes");
    assert!(bad.error.is_some(), "bad.glb records an error");
    let good: Vec<_> = outcomes
        .iter()
        .filter(|outcome| !outcome.path.ends_with("bad.glb"))
        .collect();
    assert!(!good.is_empty(), "other assets still imported");
    assert!(good.iter().all(|outcome| outcome.error.is_none()));
    cleanup(&root);
}

#[test]
fn scan_reports_invalid_when_dependency_missing() {
    let root = temp_project("invalid");
    // 场景引用 assets/textures/red-1x1.png。
    fs::write(
        root.join("scenes/main.scene"),
        "[scene]\nSprite texture=assets/textures/red-1x1.png;size=32\n",
    )
    .unwrap();
    // 首次 scan：注册纹理并建立 scene→texture 依赖边。
    scan(&root).unwrap();
    // 删除被引用的纹理 → 再次 scan 应把消费者标 invalid（依赖缺失）。
    fs::remove_file(root.join("assets/textures/red-1x1.png")).unwrap();
    let entries = scan(&root).unwrap();
    let scene = entries
        .iter()
        .find(|e| e.path.ends_with("main.scene"))
        .expect("scene present in scan");
    assert_eq!(scene.status, "invalid");
    assert!(
        scene.error.as_deref().unwrap_or("").contains("缺失"),
        "error 应说明依赖缺失：{:?}",
        scene.error
    );
    cleanup(&root);
}

#[test]
fn check_deps_reports_full_chain_consumer_to_root() {
    let root = temp_project("checkdeps-chain");
    // 三层链路：main.scene → hero.prefab → red-1x1.png。
    fs::create_dir_all(root.join("assets/sprites")).unwrap();
    fs::write(
        root.join("scenes/main.scene"),
        "[scene]\nSprite texture=assets/sprites/hero.prefab;size=32\n",
    )
    .unwrap();
    fs::write(
        root.join("assets/sprites/hero.prefab"),
        "prefab_version=1\nSprite texture=assets/textures/red-1x1.png;size=32\n",
    )
    .unwrap();
    // 首次 scan 注册并建边。
    scan(&root).unwrap();
    // 删除根纹理 → 整个链路失效。
    fs::remove_file(root.join("assets/textures/red-1x1.png")).unwrap();
    let links = check_dependencies(&root).unwrap();
    let scene_link = links
        .iter()
        .find(|link| link.consumer.ends_with("main.scene"))
        .expect("scene 断链存在");
    assert_eq!(scene_link.chain.len(), 3, "完整链路含消费者与断链根");
    assert!(scene_link.chain[0].ends_with("main.scene"));
    assert!(scene_link.chain[1].ends_with("hero.prefab"));
    assert!(scene_link.chain[2].ends_with("red-1x1.png"));
    assert_eq!(scene_link.reason, "缺失");
    cleanup(&root);
}

#[test]
fn check_deps_truncates_on_cycle() {
    let root = temp_project("checkdeps-cycle");
    // a/b 两个 prefab 互相引用成环；a 依赖会缺失的纹理，依赖顺序让
    // walk 优先沿环下探（同 rank 取最后一个），从而触发循环截断。
    fs::write(
        root.join("assets/a.prefab"),
        "prefab_version=1\nSprite texture=assets/textures/red-1x1.png;size=32\nSprite texture=assets/b.prefab;size=32\n",
    )
    .unwrap();
    fs::write(
        root.join("assets/b.prefab"),
        "prefab_version=1\nSprite texture=assets/a.prefab;size=32\n",
    )
    .unwrap();
    scan(&root).unwrap();
    fs::remove_file(root.join("assets/textures/red-1x1.png")).unwrap();
    let links = check_dependencies(&root).unwrap();
    assert!(!links.is_empty(), "断链存在：{links:?}");
    assert!(
        links.iter().any(|link| link.reason == "循环依赖"),
        "成环下探应截断并标注循环依赖：{links:?}"
    );
    cleanup(&root);
}

#[test]
fn check_deps_empty_when_healthy() {
    let root = temp_project("checkdeps-ok");
    // 场景引用已存在的纹理 → 无 Invalid。
    fs::write(
        root.join("scenes/main.scene"),
        "[scene]\nSprite texture=assets/textures/red-1x1.png;size=32\n",
    )
    .unwrap();
    let links = check_dependencies(&root).unwrap();
    assert!(links.is_empty(), "健康项目无断链：{links:?}");
    cleanup(&root);
}

#[test]
fn clean_drops_import_cache() {
    let root = temp_project("clean");
    let outcomes = import(&root).unwrap();
    assert!(
        !outcomes.is_empty(),
        "import produced outcomes: {outcomes:?}"
    );
    // 用文件存在性断言（Windows 上对 Temp 目录 read_dir 有解析怪癖）
    for outcome in &outcomes {
        assert!(
            root.join(outcome.cache_path.as_ref().unwrap()).is_file(),
            "cache file exists after import: {:?}",
            outcome.cache_path
        );
    }
    clean(&root).unwrap();
    for outcome in &outcomes {
        assert!(
            !root.join(outcome.cache_path.as_ref().unwrap()).exists(),
            "cache file removed after clean: {:?}",
            outcome.cache_path
        );
    }
    cleanup(&root);
}

#[test]
fn package_copies_data_and_writes_manifest() {
    let root = temp_project("package");
    let out = std::env::temp_dir().join(format!("rustlua-assetc-out-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out);
    package(&root, &out, false, false, false).unwrap();
    assert!(out.join("project.toml").is_file());
    assert!(out.join("scripts/main.lua").is_file());
    assert!(out.join("assets/textures/red-1x1.png").is_file());
    assert!(out.join("dependencies.json").is_file());
    let manifest_text = fs::read_to_string(out.join("manifest.txt")).unwrap();
    assert_eq!(
        manifest_text.lines().next(),
        Some("assetc_manifest=1\tassetc-package-2")
    );
    assert!(
        manifest_text
            .lines()
            .any(|line| line.starts_with("dependencies.json\t")),
        "manifest includes the generated dependency index"
    );
    assert!(manifest_text.contains("red-1x1.png"), "manifest lists png");
    assert!(manifest_text.contains("main.lua"), "manifest lists lua");
    fs::remove_dir_all(&out).ok();
    cleanup(&root);
}

/// 微信形态配额门（docs/52）：`--drop-undeclared-fonts` 只留 `project.toml` 声明的
/// `font`/`font_fallback`；未声明字体二进制不随包发、也不进 `manifest.txt`。
/// 反面对照：不开旗标（PC 形态）时全量字体原样保留。
#[test]
fn package_drops_undeclared_fonts_when_requested() {
    let root = temp_project("package-font-drop");
    let mut toml = fs::read_to_string(root.join("project.toml")).unwrap();
    toml.push_str("font = \"assets/fonts/kept.ttf\"\nfont_fallback = \"\"\n");
    fs::write(root.join("project.toml"), toml).unwrap();
    fs::create_dir_all(root.join("assets/fonts")).unwrap();
    fs::write(root.join("assets/fonts/kept.ttf"), b"kept-font").unwrap();
    fs::write(root.join("assets/fonts/dead-weight.ttf"), b"dead").unwrap();
    fs::write(root.join("assets/fonts/LICENSE.txt"), b"lic").unwrap();
    let out = std::env::temp_dir().join(format!("rustlua-assetc-dropout-{}", std::process::id()));
    let out_keep =
        std::env::temp_dir().join(format!("rustlua-assetc-keepout-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out);
    let _ = fs::remove_dir_all(&out_keep);
    package(&root, &out, false, false, true).unwrap();
    assert!(out.join("assets/fonts/kept.ttf").is_file());
    assert!(!out.join("assets/fonts/dead-weight.ttf").exists());
    assert!(
        out.join("assets/fonts/LICENSE.txt").is_file(),
        "非字体文件（授权说明）不受剔除影响"
    );
    let manifest_text = fs::read_to_string(out.join("manifest.txt")).unwrap();
    assert!(
        !manifest_text.contains("dead-weight.ttf"),
        "清单不得再列被剔除的字体（运行时按清单校验）"
    );
    assert!(manifest_text.contains("kept.ttf"));
    // 对照：不开旗标 = 既有 PC 行为逐字不变。
    package(&root, &out_keep, false, false, false).unwrap();
    assert!(out_keep.join("assets/fonts/dead-weight.ttf").is_file());
    fs::remove_dir_all(&out).ok();
    fs::remove_dir_all(&out_keep).ok();
    cleanup(&root);
}

#[test]
fn package_dependency_index_records_static_edges_and_opaque_sources() {
    let root = temp_project("package-dependency-index");
    fs::write(
        root.join("scenes/main.scene"),
        "scene_version=2\nSprite texture=assets/textures/red-1x1.png;size=1\n",
    )
    .unwrap();
    fs::write(
        root.join("scripts/main.lua"),
        "local helper = require('helper')\nfunction update() return 'ok' end\n",
    )
    .unwrap();
    fs::write(root.join("scripts/helper.lua"), "return {}\n").unwrap();
    let out = std::env::temp_dir().join(format!(
        "rustlua-assetc-dependency-index-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = fs::remove_dir_all(&out);
    package(&root, &out, false, false, false).unwrap();
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
    assert_eq!(index["format"], 1);
    assert!(
        index["edges"]["scenes/main.scene"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "assets/textures/red-1x1.png")
    );
    assert!(
        index["edges"]["scripts/main.lua"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "scripts/helper.lua")
    );
    assert!(index["opaque"].as_array().unwrap().is_empty());
    fs::remove_dir_all(&out).ok();

    fs::write(
        root.join("scripts/main.lua"),
        "local next_scene = state.scene\nscene.load(next_scene)\n",
    )
    .unwrap();
    package(&root, &out, false, false, false).unwrap();
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
    assert!(
        index["opaque"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "scripts/main.lua")
    );
    fs::remove_dir_all(&out).ok();

    fs::write(
        root.join("scripts/main.lua"),
        "function update() audio.play('ui_click') end\n",
    )
    .unwrap();
    package(&root, &out, false, false, false).unwrap();
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
    // `audio.*` 不是同步 vfs 读取（运行期由宿主按名字取回），因此既不产边也不标 opaque：
    // 早先把它当"必须证明完整"的同步 API，代价是每个放过音效的脚本都拖垮全量预载。
    assert!(
        !index["opaque"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "scripts/main.lua")
    );
    assert!(index["edges"]["scripts/main.lua"].is_null());
    fs::remove_dir_all(&out).ok();

    // 查表路由 + 兜底字面量：兜底那一支是可证的真依赖，不该把文件标成 opaque。
    fs::write(
        root.join("scripts/main.lua"),
        "function on_click()\n  local target = maps.get(name)\n  return scene.load(target and target or \"main\")\nend\n",
    )
    .unwrap();
    package(&root, &out, false, false, false).unwrap();
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
    assert!(
        index["edges"]["scripts/main.lua"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "scenes/main.scene"),
        "兜底字面量应当成为真实场景边：{index}"
    );
    assert!(
        !index["opaque"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "scripts/main.lua")
    );
    fs::remove_dir_all(&out).ok();

    // 同文件常量实参：`prefab.spawn(PRESET_PATH)` 的路径在上一行是字面量，属可证依赖。
    fs::create_dir_all(root.join("assets/prefabs")).unwrap();
    fs::write(
        root.join("assets/prefabs/dialogue.prefab"),
        "prefab_version=1\n",
    )
    .unwrap();
    fs::write(
        root.join("scripts/main.lua"),
        "local PRESET_PATH = \"assets/prefabs/dialogue.prefab\"\nfunction boot()\n  return prefab.spawn(PRESET_PATH)\nend\n",
    )
    .unwrap();
    package(&root, &out, false, false, false).unwrap();
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
    assert!(
        index["edges"]["scripts/main.lua"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "assets/prefabs/dialogue.prefab"),
        "常量实参应当解析成真实预设边：{index}"
    );
    assert!(
        !index["opaque"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "scripts/main.lua")
    );
    fs::remove_dir_all(&out).ok();

    // 已退役的目录常量 + 变量拼路径：包内没有命中 ⇒ 运行期同样读不到，
    // 不该把整包拖成全量预载（真实工程里那是 M71 删掉的 `assets/ui/prefabs/*.ui.json`）。
    fs::write(
        root.join("scripts/main.lua"),
        "local DIR = \"assets/ui/prefabs/\"\nfunction load(name)\n  return data.load(DIR .. name .. \".ui.json\")\nend\n",
    )
    .unwrap();
    package(&root, &out, false, false, false).unwrap();
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
    assert!(
        !index["opaque"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "scripts/main.lua")
    );
    fs::remove_dir_all(&out).ok();

    // 拼出来的路径**确实**指向包内文件：前缀展开成完整命中集，同样是可证边。
    fs::create_dir_all(root.join("assets/ui/prefabs")).unwrap();
    fs::write(root.join("assets/ui/prefabs/dialogue.ui.json"), "{}\n").unwrap();
    fs::write(root.join("assets/ui/prefabs/hud.ui.json"), "{}\n").unwrap();
    package(&root, &out, false, false, false).unwrap();
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
    let edges = index["edges"]["scripts/main.lua"].as_array().unwrap();
    for expected in [
        "assets/ui/prefabs/dialogue.ui.json",
        "assets/ui/prefabs/hud.ui.json",
    ] {
        assert!(
            edges.iter().any(|path| path == expected),
            "目录前缀应当展开成 {expected}：{index}"
        );
    }
    assert!(
        !index["opaque"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "scripts/main.lua")
    );
    fs::remove_dir_all(&out).ok();
    cleanup(&root);
}

#[test]
fn package_dependency_index_ignores_bare_assets_prefix_literal() {
    let root = temp_project("package-bare-assets-prefix");
    // 只有"一级子目录"的目录引用才算可证闭包；裸 `"assets/"` 是**校验字面量**
    // （`string.find(path, "assets/", 1, true)`），展开它把整包挂成这个文件的依赖。
    assert_eq!(
        asset_prefix_refs("local ok = string.find(p, \"assets/\", 1, true)\n"),
        Vec::<String>::new()
    );
    assert_eq!(
        asset_prefix_refs("local dir = \"assets/art/tilesets/\"\n"),
        vec!["assets/art/tilesets/".to_owned()]
    );
    fs::create_dir_all(root.join("assets/art/tilesets")).unwrap();
    fs::write(root.join("assets/art/tilesets/ground.png"), b"png").unwrap();
    fs::write(root.join("assets/fonts-unused.ttf"), b"ttf").unwrap();
    fs::write(
        root.join("scripts/main.lua"),
        "local PATTERN = \"assets/\"\n\
         function update()\n\
         \x20 return string.find(PATTERN, \"assets/\", 1, true) ~= nil\n\
         end\n",
    )
    .unwrap();
    let out = std::env::temp_dir().join(format!(
        "rustlua-assetc-bare-prefix-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = fs::remove_dir_all(&out);
    package(&root, &out, false, false, false).unwrap();
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
    assert!(
        index["edges"]["scripts/main.lua"].is_null(),
        "裸 `assets/` 不该产生任何边（否则整包进闭包）：{index}"
    );
    fs::remove_dir_all(&out).ok();
    cleanup(&root);
}

#[test]
fn package_compresses_supported_assets_without_compressing_lua() {
    let root = temp_project("package-compress-assets");
    let out = std::env::temp_dir().join(format!(
        "rustlua-assetc-compress-out-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = fs::remove_dir_all(&out);
    package(&root, &out, true, false, false).unwrap();
    let png = fs::read(out.join("assets/textures/red-1x1.png")).unwrap();
    let lua = fs::read(out.join("scripts/main.lua")).unwrap();
    assert!(png.starts_with(rustlua_assets::pack::PACK_MAGIC));
    assert!(!lua.starts_with(rustlua_assets::pack::PACK_MAGIC));
    fs::remove_dir_all(&out).ok();
    cleanup(&root);
}

/// 压缩白名单的口径：资产按格式压缩，代码（Lua/场景）走加密通道而非这里。
/// 新增音频格式时必须同时改 `ResourceKind::from_path` 与这里，否则新格式会
/// 以原始字节进包——体积问题只在发布期才暴露。
#[test]
fn compressible_assets_cover_every_packaged_asset_format() {
    for name in [
        "a.png", "a.jpg", "a.jpeg", "a.wav", "a.ogg", "a.mp3", "a.ttf", "a.otf", "a.glb", "A.MP3",
    ] {
        assert!(is_compressible_asset(Path::new(name)), "{name} 应可压缩");
    }
    for name in ["a.lua", "a.scene", "a.json", "a.txt"] {
        assert!(!is_compressible_asset(Path::new(name)), "{name} 不应进压缩");
    }
}

/// 打包输出目录不得与项目目录相同、也不得位于 `scenes/`/`scripts/`/`assets/`
/// 之内：那几个目录正是打包输入，产物写进去会自我复制，下次打包还会把产物当输入。
#[test]
fn package_rejects_output_inside_data_dirs_or_equal_to_project() {
    let root = temp_project("package-outguard");
    let error = package(&root, &root, false, false, false).expect_err("输出=项目 必须被拒绝");
    assert!(error.to_string().contains("相同"), "{error}");

    for sub in ["assets", "scripts", "scenes"] {
        let nested = root.join(sub).join("dist");
        let error = package(&root, &nested, false, false, false)
            .expect_err("输出位于项目数据目录内部 必须被拒绝");
        assert!(error.to_string().contains("项目数据目录"), "{error}");
        assert!(!nested.exists(), "被拒绝的打包不应留下输出目录");
    }

    // 带 `..` 的等价写法也要被识别（词法比较即可，不需要目录存在）。
    let dotted = root.join("assets").join("dist").join("..").join("dist");
    let error = package(&root, &dotted, false, false, false).expect_err("`..` 绕不过数据目录检查");
    assert!(error.to_string().contains("项目数据目录"), "{error}");
    cleanup(&root);
}

/// 发布脚本的真实布局（`<project>/builds/<platform>/.staging-*/pack`）
/// 必须**允许**：`scan`/`collect_data_files` 只遍历 `project.toml` 与
/// `scenes/`/`scripts/`/`assets/`，因此 `builds/` 下的产物不会被当成项目数据。
/// 回归背景：更严格的「输出不能位于项目内部」规则曾让 `package.ps1 -SelfTest`
/// 直接失败（`assetc error: 输出目录不能位于项目目录内部`）。
#[test]
fn package_allows_output_inside_project_builds_dir() {
    let root = temp_project("package-builds-out");
    let out = root
        .join("builds")
        .join("pc")
        .join("jianghu_youtian")
        .join(".staging-20260101-000000-abcdef")
        .join("pack");
    package(&root, &out, false, false, false).expect("builds/ 下的输出必须被允许");
    assert!(out.join("scripts/main.lua").is_file());
    assert!(out.join("manifest.txt").is_file());
    // 项目数据子目录没有被产物污染（打包输入仍是纯项目数据）。
    assert!(!root.join("assets").join("builds").exists());
    cleanup(&root);
}

/// `canonical_or_absolute` 在输出目录尚不存在时也要给出可比较的绝对路径。
#[test]
fn canonical_or_absolute_resolves_missing_paths() {
    let root = temp_project("canonical");
    let missing = root.join("does").join("not").join("exist");
    assert!(!missing.exists());
    let resolved = canonical_or_absolute(&missing).unwrap();
    assert!(resolved.is_absolute(), "{}", resolved.display());
    assert!(resolved.ends_with(Path::new("does/not/exist")));
    // 已存在的路径解析为规范路径（Windows 上会去掉 8.3 短名/大小写差异）。
    let existing = canonical_or_absolute(&root).unwrap();
    assert_eq!(existing, fs::canonicalize(&root).unwrap());
    cleanup(&root);
}

/// 路径越界判定（纯函数）：符号链接安全校验的核心。
#[test]
fn path_containment_detects_escapes() {
    let root = Path::new("/project");
    assert!(!path_escapes_root(root, Path::new("/project/assets/a.png")));
    assert!(!path_escapes_root(root, Path::new("/project")));
    assert!(path_escapes_root(root, Path::new("/project2/assets/a.png")));
    assert!(path_escapes_root(root, Path::new("/etc/passwd")));
    assert!(path_escapes_root(
        root,
        Path::new("/project/../outside.png")
    ));
}

/// 指向项目外的符号链接必须在打包时报错，而不是被 `fs::copy` 跟随复制进发布包。
/// Windows 上创建符号链接需要开发者模式/管理员权限，无权限时跳过。
#[test]
fn collect_files_rejects_symlink_escaping_the_project() {
    let root = temp_project("symlink-escape");
    let outside = std::env::temp_dir().join(format!(
        "rustlua-assetc-outside-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    fs::create_dir_all(&outside).unwrap();
    let secret = outside.join("secret.txt");
    fs::write(&secret, "outside the project").unwrap();

    let link = root.join("assets").join("linked-secret.txt");
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_file(&secret, &link).is_ok();
    #[cfg(not(windows))]
    let created = std::os::unix::fs::symlink(&secret, &link).is_ok();
    if !created {
        // 无符号链接权限：本用例在该环境下无法构造，跳过。
        fs::remove_dir_all(&outside).ok();
        cleanup(&root);
        return;
    }

    let mut files = Vec::new();
    let error =
        collect_files(&root.join("assets"), &mut files).expect_err("越界符号链接必须被拒绝");
    assert!(error.to_string().contains("项目目录外"), "{error}");
    // 同时确认整条打包路径也会失败（不会写出半个发布目录）。
    let out = std::env::temp_dir().join(format!(
        "rustlua-assetc-symlink-out-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let error = package(&root, &out, false, false, false).expect_err("打包必须拒绝越界链接");
    assert!(error.to_string().contains("项目目录外"), "{error}");

    fs::remove_file(&link).ok();
    fs::remove_dir_all(&out).ok();
    fs::remove_dir_all(&outside).ok();
    cleanup(&root);
}

/// 开发用文件（预览拼图 / 美术清单）不进打包清单，游戏资源照常进。
#[test]
fn dev_artifacts_are_not_packaged() {
    let root = temp_project("dev-artifacts");
    let art = root.join("assets").join("art").join("items");
    fs::create_dir_all(&art).unwrap();
    fs::write(art.join("lingzhi.png"), b"png").unwrap();
    fs::write(art.join("_preview.png"), b"png").unwrap();
    fs::write(art.join("preview.png"), b"png").unwrap();
    fs::write(art.join("manifest.json"), b"{}").unwrap();
    let title = root.join("assets").join("art").join("title");
    fs::create_dir_all(&title).unwrap();
    fs::write(title.join("season_preview.png"), b"png").unwrap();
    fs::write(title.join("title_bg_pixel.png"), b"png").unwrap();
    // 作物出图集的中间帧（`crops/<crop>/stage_<n>.png`）不进包，图集本体必须进包。
    let crops = root.join("assets").join("art").join("crops");
    fs::create_dir_all(crops.join("cabbage")).unwrap();
    fs::write(crops.join("cabbage").join("stage_1.png"), b"png").unwrap();
    fs::write(crops.join("manor_crops.png"), b"png").unwrap();

    let mut files = Vec::new();
    collect_files(&root.join("assets"), &mut files).unwrap();
    let names: Vec<String> = files
        .iter()
        .map(|(path, _)| path.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert!(names.contains(&"lingzhi.png".to_owned()), "{names:?}");
    // `title_bg_pixel.png` 里含 "pixel" 不含 "preview"：必须留在包里（不能误伤）。
    assert!(
        names.contains(&"title_bg_pixel.png".to_owned()),
        "{names:?}"
    );
    assert!(!names.contains(&"_preview.png".to_owned()), "{names:?}");
    assert!(!names.contains(&"preview.png".to_owned()), "{names:?}");
    assert!(!names.contains(&"manifest.json".to_owned()), "{names:?}");
    assert!(
        !names.contains(&"season_preview.png".to_owned()),
        "{names:?}"
    );
    assert!(
        names.contains(&"manor_crops.png".to_owned()),
        "图集本体必须进包：{names:?}"
    );
    assert!(
        !names.contains(&"stage_1.png".to_owned()),
        "crops 中间帧不许进包：{names:?}"
    );

    // 判定函数本身的边界。
    assert!(is_dev_artifact(Path::new("assets/art/items/_overview.png")));
    assert!(is_dev_artifact(Path::new("assets/art/ui/manifest.json")));
    assert!(is_dev_artifact(Path::new(
        "assets/art/crops/cabbage/stage_1.png"
    )));
    // 只有 `crops/<crop>/` 下的 `stage_*.png` 归开发产物：图集本体与别处的 stage_ 文件不受影响。
    assert!(!is_dev_artifact(Path::new(
        "assets/art/crops/manor_crops.png"
    )));
    assert!(!is_dev_artifact(Path::new("assets/art/items/stage_1.png")));
    assert!(!is_dev_artifact(Path::new("assets/art/items/wood.png")));
    assert!(!is_dev_artifact(Path::new(
        "assets/art/title/title_bg_pixel.png"
    )));
    cleanup(&root);
}

#[test]
fn benchmark_compression_reports_asset_kinds() {
    let root = temp_project("benchmark-compression");
    let entries = benchmark_compression(&root).unwrap();
    let texture = entries
        .iter()
        .find(|entry| entry.kind == "texture")
        .unwrap();
    assert_eq!(texture.files, 1);
    assert!(texture.original_bytes > 0);
    assert!(texture.compressed_bytes > 0);
    cleanup(&root);
}

#[test]
fn sample_project_import_checkdeps_and_package_pass() {
    let Some(root) = copy_sample_project("workflow") else {
        return;
    };
    let outcomes = import(&root).unwrap();
    assert!(
        outcomes.iter().all(|outcome| outcome.error.is_none()),
        "sample project import should succeed: {outcomes:?}"
    );

    let broken = check_dependencies(&root).unwrap();
    assert!(
        broken.is_empty(),
        "sample project should have no broken dependency chain: {broken:?}"
    );

    let out = std::env::temp_dir().join(format!(
        "rustlua-assetc-sample-pack-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = fs::remove_dir_all(&out);
    let manifest = package(&root, &out, false, false, false).unwrap();
    assert!(
        manifest.entries.contains_key("scenes/api_gameplay.scene"),
        "packaged manifest should include production scene"
    );
    assert!(
        manifest
            .entries
            .contains_key("assets/prefabs/slime_green.prefab"),
        "packaged manifest should include prefab"
    );
    assert!(
        out.join("scripts/main.lua").is_file(),
        "packaged scripts should be copied"
    );
    fs::remove_dir_all(&out).ok();
    cleanup(&root);
}

#[test]
fn sample_project_rename_breaks_dependencies_until_consumers_are_updated() {
    let Some(root) = copy_sample_project("rename-fix") else {
        return;
    };
    let old_path = root.join("assets/textures/slime.png");
    let new_rel = "assets/textures/slime-renamed.png";
    let new_path = root.join(new_rel);

    let outcomes = import(&root).unwrap();
    assert!(
        outcomes.iter().all(|outcome| outcome.error.is_none()),
        "sample project import should succeed before rename: {outcomes:?}"
    );

    fs::rename(&old_path, &new_path).unwrap();

    let broken = check_dependencies(&root).unwrap();
    assert!(
        !broken.is_empty(),
        "renaming a referenced texture should surface broken links"
    );
    assert!(
        broken.iter().all(|link| link.reason == "缺失"),
        "renamed-away texture should report missing root: {broken:?}"
    );
    let consumers: Vec<_> = broken.iter().map(|link| link.consumer.clone()).collect();
    assert!(
        consumers.contains(&PathBuf::from("assets/prefabs/slime_green.prefab")),
        "green prefab should become invalid after texture rename: {consumers:?}"
    );
    assert!(
        consumers.contains(&PathBuf::from("assets/prefabs/slime_red.prefab")),
        "red prefab should become invalid after texture rename: {consumers:?}"
    );
    assert!(
        consumers.contains(&PathBuf::from("scenes/api_gameplay.scene")),
        "gameplay scene should become invalid after texture rename: {consumers:?}"
    );

    // 修复：**逐个消费方**把旧路径改成新路径。消费方清单**从断链报告里取**，
    // 而不是手抄一份文件列表——示例工程每加一个引用者，手抄清单就会悄悄失效。
    for consumer in &consumers {
        replace_in_file(&root.join(consumer), "assets/textures/slime.png", new_rel);
    }

    let outcomes = import(&root).unwrap();
    assert!(
        outcomes.iter().all(|outcome| outcome.error.is_none()),
        "sample project import should succeed after fix: {outcomes:?}"
    );
    let broken = check_dependencies(&root).unwrap();
    assert!(
        broken.is_empty(),
        "updating all consumers should repair broken links: {broken:?}"
    );

    cleanup(&root);
}

// ---------------------------------------------------------------- 批 A：RLB 单包

/// 建一个"有文本 + 有二进制 + 有依赖索引"的小工程，供打包门使用。
fn bundle_project(label: &str) -> PathBuf {
    let root = temp_project(label);
    fs::create_dir_all(root.join("assets/ui")).unwrap();
    fs::write(
        root.join("scripts/data.lua"),
        "return { title = '测试标题', items = { 'a', 'b' } }\n".repeat(20),
    )
    .unwrap();
    fs::write(
        root.join("scripts/main.lua"),
        "local data = require('data')\nfunction update() return data.title end\n",
    )
    .unwrap();
    fs::write(
        root.join("assets/ui/panel.txt"),
        "nine_slice=1\nborder=4\n".repeat(10),
    )
    .unwrap();
    root
}

/// 启动闭包优先排序（`docs/67` §9 的性能项）：boot 期读的条目必须排在组内最前面。
///
/// 这条只校验**排序性质**（纯函数、零依赖、秒级）；"实际少解几个块"的读数需要
/// WebGL 产物 + 浏览器 harness（重，单独跑）。
#[test]
fn boot_closure_sorts_first() {
    let mut paths = vec![
        "assets/art/ui/hud/a.png".to_owned(),
        "scripts/data/items.lua".to_owned(),
        "scripts/main.lua".to_owned(),
        "scenes/main.scene".to_owned(),
        "project.toml".to_owned(),
        "atlas_redirect.json".to_owned(),
    ];
    paths.sort_by(|a, b| {
        boot_priority(a)
            .cmp(&boot_priority(b))
            .then_with(|| a.cmp(b))
    });
    let head: Vec<&str> = paths.iter().take(4).map(String::as_str).collect();
    assert_eq!(
        head,
        vec![
            "atlas_redirect.json",
            "project.toml",
            "scenes/main.scene",
            "scripts/main.lua"
        ],
        "启动闭包必须全部排在前面：{paths:?}"
    );
    // 0 档 = 四个锚点（字典序）；1 档 = 其余脚本；2 档 = 美术等
    assert_eq!(
        paths[4], "scripts/data/items.lua",
        "1 档是其余脚本：{paths:?}"
    );
    assert_eq!(paths[5], "assets/art/ui/hud/a.png", "2 档才是美术");
}

/// 单包产物 = 骨架 + 容器 + 报告 + 清单，且目录形态的数据目录**全部消失**。
#[test]
fn bundle_package_leaves_only_skeleton() {
    let root = bundle_project("bundle-skeleton");
    let out = std::env::temp_dir().join(format!(
        "rustlua-assetc-bundle-skeleton-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = fs::remove_dir_all(&out);
    let report = package_bundled(&root, &out, false, false, false, &[]).unwrap();

    assert_eq!(report.mode, "single");
    assert!(report.entries >= 4, "条目数 {report:?}");
    assert!(report.blocks >= 1);
    assert!(report.ratio_milli < 1000, "必须比原始更小：{report:?}");
    assert!(report.bundle_path.starts_with("bundles/all-"));

    // 骨架在，数据目录不在。
    assert!(out.join("project.toml").is_file());
    assert!(out.join("manifest.txt").is_file());
    assert!(out.join(PACKAGE_REPORT_FILE).is_file());
    assert!(out.join(&report.bundle_path).is_file());
    for gone in ["scenes", "scripts", "assets"] {
        assert!(!out.join(gone).exists(), "{gone}/ 不该留在输出根");
    }

    // 清单只列骨架/容器/报告，且校验和与磁盘一致。
    let manifest = fs::read_to_string(out.join("manifest.txt")).unwrap();
    let lines: Vec<&str> = manifest.lines().collect();
    assert!(lines[0].starts_with("assetc_manifest=1\t"));
    assert_eq!(lines.len(), 4, "清单应当只有 3 个条目：{manifest}");
    for line in &lines[1..] {
        let mut parts = line.split('\t');
        let relative = parts.next().unwrap();
        let size: u64 = parts.next().unwrap().parse().unwrap();
        let checksum = u64::from_str_radix(parts.next().unwrap(), 16).unwrap();
        let bytes = fs::read(out.join(relative)).unwrap();
        assert_eq!(bytes.len() as u64, size, "{relative}");
        assert_eq!(fnv1a(&bytes), checksum, "{relative}");
    }

    fs::remove_dir_all(&out).ok();
    cleanup(&root);
}

/// 等价性门（`docs/67` §10.4）：目录形态与单包形态**逐条目字节相等**。
///
/// 这条把"容器不改变内容"变成机器可证，比任何体积报告都重要。
#[test]
fn bundle_package_matches_loose_output() {
    use rustlua_infra::bundle::{BundleIndex, BundleReader, MemSource};

    let root = bundle_project("bundle-equivalence");
    let loose = std::env::temp_dir().join(format!(
        "rustlua-assetc-bundle-loose-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let packed = std::env::temp_dir().join(format!(
        "rustlua-assetc-bundle-packed-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = fs::remove_dir_all(&loose);
    let _ = fs::remove_dir_all(&packed);

    package(&root, &loose, false, false, false).unwrap();
    let report = package_bundled(&root, &packed, false, false, false, &[]).unwrap();

    let bytes = fs::read(packed.join(&report.bundle_path)).unwrap();
    let index = BundleIndex::parse(&bytes).unwrap();
    let mut reader = BundleReader::new(index, MemSource::new(bytes));

    // 目录形态里除清单外的每个文件，都必须在容器里有一份字节相同的条目。
    let mut files = Vec::new();
    collect_files(&loose, &mut files).unwrap();
    let mut compared = 0;
    for (path, _) in files {
        let relative = path
            .strip_prefix(&loose)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "manifest.txt" {
            continue; // 两种形态的清单本来就不同（bundle 级 vs 逐文件）
        }
        let expected = fs::read(&path).unwrap();
        let actual = reader
            .read(&relative)
            .unwrap()
            .unwrap_or_else(|| panic!("容器缺少条目 {relative}"));
        assert_eq!(actual, expected, "条目 {relative} 字节不一致");
        compared += 1;
    }
    assert_eq!(compared as u64, report.entries, "条目数必须守恒");
    // 反向：容器里没有多余条目（index.paths ⊆ 目录形态的文件集）。
    for entry in reader.index().paths() {
        assert!(
            loose.join(entry).exists(),
            "容器里多出目录形态没有的条目 {entry}"
        );
    }
    reader.verify_all().unwrap();

    fs::remove_dir_all(&loose).ok();
    fs::remove_dir_all(&packed).ok();
    cleanup(&root);
}

/// 确定性门（`docs/67` §4.5）：同一输入连打两次，容器逐字节相同。
#[test]
fn bundle_package_is_deterministic() {
    let root = bundle_project("bundle-determinism");
    let mut previous: Option<Vec<u8>> = None;
    for round in 0..2 {
        let out = std::env::temp_dir().join(format!(
            "rustlua-assetc-bundle-det-{round}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&out);
        let report = package_bundled(&root, &out, false, false, false, &[]).unwrap();
        let bytes = fs::read(out.join(&report.bundle_path)).unwrap();
        if let Some(before) = &previous {
            assert_eq!(before, &bytes, "两次打包的容器字节必须相同");
        }
        previous = Some(bytes);
        fs::remove_dir_all(&out).ok();
    }
    cleanup(&root);
}

/// 报告里的数字必须与容器本身对得上（否则体积报告就是装饰）。
#[test]
fn bundle_report_matches_container() {
    use rustlua_infra::bundle::BundleIndex;

    let root = bundle_project("bundle-report");
    let out = std::env::temp_dir().join(format!(
        "rustlua-assetc-bundle-report-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = fs::remove_dir_all(&out);
    let report = package_bundled(&root, &out, false, false, false, &[]).unwrap();
    let bytes = fs::read(out.join(&report.bundle_path)).unwrap();
    let index = BundleIndex::parse(&bytes).unwrap();

    assert_eq!(report.entries, index.len() as u64);
    assert_eq!(report.blocks, index.blocks().len() as u64);
    assert_eq!(
        report.bundle_bytes,
        bytes.len() as u64,
        "报告的落盘字节必须等于容器文件大小"
    );
    // 容器名里的 8 位 hex 取自整包 FNV：名字换了内容一定换了（CDN/微信缓存失效的依据）。
    let expected_name = format!(
        "{BUNDLE_DIR}/all-{:08x}.rlb",
        rustlua_infra::bundle::checksum(&bytes) as u32
    );
    assert_eq!(report.bundle_path, expected_name);

    // 报告自身可被读回（发布面板/CI 依赖这份 JSON）。
    let text = fs::read_to_string(out.join(PACKAGE_REPORT_FILE)).unwrap();
    let parsed: PackageReport = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed, report);

    fs::remove_dir_all(&out).ok();
    cleanup(&root);
}
