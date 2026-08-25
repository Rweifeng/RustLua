//! Dependency-graph extraction and resolution for the resource database.
//!
//! Pure logic (no I/O) shared by the editor, assetc, and game's runtime
//! verification: it scans scene/prefab component strings and Lua source for
//! references to other assets, and resolves them to [`AssetId`]s through the
//! database.

use crate::core::resource_db::{AssetId, ResourceDatabase};
use crate::core::resource_kind::ResourceKind;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A single extracted reference in its raw form (project-relative, forward
/// slashes). [`DependencyRef::Dynamic`] marks references that cannot be
/// resolved statically; they are recorded but never turned into edges.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum DependencyRef {
    /// Literal asset path, e.g. `assets/textures/player.png`.
    Asset(PathBuf),
    /// Lua `require("name")` sibling module → `scripts/<name>.lua`.
    LuaModule(String),
    /// Scene-load literal → `scenes/<name>.scene`.
    SceneName(String),
    /// Reference that cannot be resolved statically (dynamic variable).
    Dynamic(String),
}

/// A reference that has no matching registered record (file missing, outside
/// the scanned roots, or misspelled). Callers treat it as a broken link.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnresolvedRef {
    pub path: PathBuf,
    pub raw: String,
}

/// Extracts raw references from scene/prefab text or Lua source.
///
/// Pure function, quote-aware: Lua scanning only matches inside string literals
/// so comments and docstrings never produce references. Scene/prefab text is
/// tab-separated with unquoted component fields, so its scan stays "any
/// position" exactly like the runtime `verify_project` already does.
pub fn extract_refs(text: &str, kind: ResourceKind) -> Vec<DependencyRef> {
    match kind {
        ResourceKind::Scene | ResourceKind::Prefab => extract_scene_assets(text),
        ResourceKind::LuaScript => extract_lua_refs(text),
        _ => Vec::new(),
    }
}

/// Resolves raw references to registered asset IDs, reporting any that have no
/// matching record. `self_id` is used to drop self-references (an asset naming
/// itself would otherwise create a self-loop). Missing-case references fall
/// back to a case-insensitive lookup for Windows authoring differences.
pub fn resolve_refs(
    refs: &[DependencyRef],
    db: &ResourceDatabase,
    self_id: AssetId,
) -> (Vec<AssetId>, Vec<UnresolvedRef>) {
    let mut ids = Vec::new();
    let mut unresolved = Vec::new();
    let mut seen = HashSet::new();
    for reference in refs {
        let (candidate, raw) = match reference {
            DependencyRef::Asset(path) => (path.clone(), path.to_string_lossy().into_owned()),
            DependencyRef::LuaModule(name) => {
                let path = PathBuf::from(format!("scripts/{name}.lua"));
                (path, format!("require(\"{name}\")"))
            }
            DependencyRef::SceneName(name) => {
                let path = PathBuf::from(format!("scenes/{name}.scene"));
                (path, format!("scene.load(\"{name}\")"))
            }
            DependencyRef::Dynamic(_) => continue,
        };
        let id = db
            .find_by_path(&candidate)
            .ok()
            .flatten()
            .map(|record| record.id)
            .or_else(|| db.find_by_path_ci(&candidate));
        let Some(id) = id else {
            unresolved.push(UnresolvedRef {
                path: candidate,
                raw,
            });
            continue;
        };
        if id == self_id {
            continue;
        }
        if seen.insert(id) {
            ids.push(id);
        }
    }
    (ids, unresolved)
}

/// Recomputes dependency edges for every container asset (Scene/Prefab/LuaScript),
/// overwriting the previous edges each time so the operation is idempotent.
/// `read_source` reads a project-relative source file; returning `None` means
/// the file is gone and yields no edges. Unresolved references are returned so
/// callers can surface them as broken links.
pub fn recompute_edges(
    db: &mut ResourceDatabase,
    read_source: &dyn Fn(&Path) -> Option<String>,
) -> Vec<UnresolvedRef> {
    let containers: Vec<(AssetId, ResourceKind, PathBuf)> = db
        .iter()
        .filter(|record| {
            matches!(
                record.kind,
                ResourceKind::Scene | ResourceKind::Prefab | ResourceKind::LuaScript
            )
        })
        .map(|record| (record.id, record.kind, record.source_path.clone()))
        .collect();
    let refs_per_id: Vec<(AssetId, Vec<DependencyRef>)> = containers
        .into_iter()
        .map(|(id, kind, path)| {
            let refs = read_source(&path)
                .map(|text| extract_refs(&text, kind))
                .unwrap_or_default();
            (id, refs)
        })
        .collect();
    let mut unresolved = Vec::new();
    // resolve 用不可变借用，collect 完成后借用结束，再应用可变 set_dependencies。
    let resolved: Vec<(AssetId, Vec<AssetId>, Vec<UnresolvedRef>)> = refs_per_id
        .iter()
        .map(|(id, refs)| {
            let (ids, un_resolved) = resolve_refs(refs, db, *id);
            (*id, ids, un_resolved)
        })
        .collect();
    for (id, ids, un_resolved) in resolved {
        db.set_dependencies(id, ids)
            .expect("container still present");
        unresolved.extend(un_resolved);
    }
    unresolved
}

/// Scans tab-separated scene/prefab text for `assets/` references, cutting at
/// the same terminator set the runtime uses (`;`/`:`/newline). Empty results
/// (a bare `assets/` before a terminator) are dropped; order is preserved with
/// duplicates removed.
fn extract_scene_assets(text: &str) -> Vec<DependencyRef> {
    let chars: Vec<char> = text.chars().collect();
    let mut refs = Vec::new();
    let mut seen = HashSet::new();
    let mut i = 0;
    while i < chars.len() {
        if i + 7 <= chars.len() && chars[i..i + 7].iter().collect::<String>() == "assets/" {
            let mut end = i;
            while end < chars.len() && !matches!(chars[end], ';' | ':' | '\n' | '\r') {
                end += 1;
            }
            let path: String = chars[i..end].iter().collect();
            if path != "assets/" && seen.insert(path.clone()) {
                refs.push(DependencyRef::Asset(PathBuf::from(path)));
            }
            i = end;
        } else {
            i += 1;
        }
    }
    refs
}

/// Scans Lua source with a quote-aware state machine: `--` comments are skipped
/// to end of line, and references are only read from string literals (or from
/// `scene.load(<variable>)`, which is recorded as [`DependencyRef::Dynamic`]).
fn extract_lua_refs(text: &str) -> Vec<DependencyRef> {
    let chars: Vec<char> = text.chars().collect();
    let mut refs = Vec::new();
    let mut seen = HashSet::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // `--` 注释到行尾：字符串内的 `--` 由字符串分支处理，不会走到这里。
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // `scene.load(<变量>)` 动态引用（字面量参数由字符串扫描处理）。
        if text[i..].starts_with("scene.load") {
            if let Some(reference) = scene_load_dynamic(&chars, i)
                && seen.insert(reference.clone())
            {
                refs.push(reference);
            }
            i += "scene.load".len();
            continue;
        }
        if c != '"' && c != '\'' {
            i += 1;
            continue;
        }
        // 字符串字面量：收集到匹配引号，`\` 转义跳过下一字符。
        let quote = c;
        let context = lua_context(&chars, i);
        let mut content = String::new();
        i += 1;
        let mut closed = false;
        while i < chars.len() {
            if chars[i] == '\\' {
                content.push(chars[i]);
                if let Some(&escaped) = chars.get(i + 1) {
                    content.push(escaped);
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            if chars[i] == quote {
                i += 1;
                closed = true;
                break;
            }
            content.push(chars[i]);
            i += 1;
        }
        if closed {
            for reference in lua_string_refs(&context, &content) {
                if seen.insert(reference.clone()) {
                    refs.push(reference);
                }
            }
        }
    }
    refs
}

/// Backward context token for the string starting at `quote_index`: skips
/// whitespace and an optional `(` so both `require("x")` and `require "x"`
/// yield `require`, and `scene.load("x")` yields `scene.load`.
fn lua_context(chars: &[char], quote_index: usize) -> String {
    let mut j = quote_index as isize - 1;
    let mut token: Vec<char> = Vec::new();
    while j >= 0 {
        let c = chars[j as usize];
        if c.is_whitespace() {
            if !token.is_empty() {
                break;
            }
            j -= 1;
        } else if c == '(' {
            j -= 1;
        } else if is_ident_char(c) {
            token.push(c);
            j -= 1;
        } else {
            break;
        }
    }
    token.reverse();
    token.into_iter().collect()
}

/// Detects `scene.load(<variable>)` and records the argument as [`DependencyRef::Dynamic`]
/// so the graph acknowledges the reference without resolving it.
fn scene_load_dynamic(chars: &[char], start: usize) -> Option<DependencyRef> {
    let mut j = start + "scene.load".len();
    while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
    }
    let mut arg_start = j;
    if j < chars.len() && chars[j] == '(' {
        j += 1;
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        arg_start = j;
    }
    if j < chars.len() && matches!(chars[j], '"' | '\'') {
        return None; // 字面量场景引用由字符串扫描处理。
    }
    let mut end = j;
    while end < chars.len() && is_ident_char(chars[end]) {
        end += 1;
    }
    if end <= arg_start {
        return None;
    }
    let name: String = chars[arg_start..end].iter().collect();
    let name = name.trim_end_matches([')', ',', ';']).to_owned();
    (!name.is_empty()).then_some(DependencyRef::Dynamic(name))
}

/// Turns a Lua string literal plus its call context into references: `require`
/// and `scene.load` literals are module/scene references; any literal containing
/// `assets/` is an asset path reference. Other strings are ignored.
fn lua_string_refs(context: &str, content: &str) -> Vec<DependencyRef> {
    let mut refs = Vec::new();
    let trimmed = content.trim();
    if context.ends_with("require") && !trimmed.is_empty() {
        refs.push(DependencyRef::LuaModule(trimmed.to_owned()));
    }
    if context.ends_with("scene.load") && !trimmed.is_empty() {
        refs.push(DependencyRef::SceneName(trimmed.to_owned()));
    }
    if let Some(index) = content.find("assets/") {
        let path = content[index..].trim_end_matches([';', ':', '\n', '\r']);
        refs.push(DependencyRef::Asset(PathBuf::from(path)));
    }
    refs
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 提取 Asset 引用的路径列表（用于断言）。
    fn asset_paths(refs: &[DependencyRef]) -> Vec<String> {
        refs.iter()
            .filter_map(|reference| match reference {
                DependencyRef::Asset(path) => Some(path.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect()
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustlua-m12-02-{label}-{}", std::process::id()))
    }

    #[test]
    fn scene_refs_sprite_image_animation_tilemap_mesh() {
        // 用 game_test 真实组件行：Sprite 首字段无 key=、Animation 冒号分隔、
        // Tilemap texture=、MeshRenderer mesh=。
        let scene = "scene_version=2\nname=main\n\
            component 1 Animation clip:idle:assets/textures/hero-idle.png:4:4:true;clip:move:assets/textures/hero-move.png:4:8:true\n\
            component 1 Sprite assets/textures/player.png;size=32\n\
            component 2 Tilemap texture=assets/textures/tileset.png;tiles=1111111\n\
            component 4 MeshRenderer mesh=assets/models/m8-test-triangle.glb\n\
            component 6 Animation3D mesh=assets/models/kaykit-warrior.glb;clip=Idle;looped=true\n";
        let refs = extract_refs(scene, ResourceKind::Scene);
        let assets = asset_paths(&refs);
        for expected in [
            "assets/textures/hero-idle.png",
            "assets/textures/hero-move.png",
            "assets/textures/player.png",
            "assets/textures/tileset.png",
            "assets/models/m8-test-triangle.glb",
            "assets/models/kaykit-warrior.glb",
        ] {
            assert!(
                assets.contains(&expected.to_owned()),
                "missing {expected}: {assets:?}"
            );
        }
        assert_eq!(assets.len(), 6, "dedup + exact extraction: {assets:?}");
    }

    #[test]
    fn prefab_uses_same_format() {
        let prefab = "name=demo\ncomponent 1 Sprite assets/textures/player.png;size=32\n";
        let refs = extract_refs(prefab, ResourceKind::Prefab);
        assert_eq!(
            asset_paths(&refs),
            vec!["assets/textures/player.png".to_owned()]
        );
    }

    #[test]
    fn lua_require_modules() {
        let lua = "local game = require(\"game\")\nlocal demo3d = require('demo3d')\nlocal hud = require(\"ui/hud\")\n";
        let refs = extract_refs(lua, ResourceKind::LuaScript);
        let modules: Vec<_> = refs
            .iter()
            .filter_map(|reference| match reference {
                DependencyRef::LuaModule(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(modules, vec!["game", "demo3d", "ui/hud"]);
    }

    #[test]
    fn lua_scene_load_literal_only() {
        let lua = "scene.load(\"main\")\nscene.load('arena')\nscene.load(state.scene_name)\n";
        let refs = extract_refs(lua, ResourceKind::LuaScript);
        let scenes: Vec<_> = refs
            .iter()
            .filter_map(|reference| match reference {
                DependencyRef::SceneName(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(scenes, vec!["main", "arena"]);
        let dynamic: Vec<_> = refs
            .iter()
            .filter_map(|reference| match reference {
                DependencyRef::Dynamic(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(dynamic, vec!["state.scene_name"], "变量参数记录为动态");
    }

    #[test]
    fn lua_asset_paths() {
        let lua = "mesh.set(id, \"assets/models/m8-test-triangle.glb\")\n";
        let refs = extract_refs(lua, ResourceKind::LuaScript);
        assert_eq!(
            asset_paths(&refs),
            vec!["assets/models/m8-test-triangle.glb".to_owned()]
        );
    }

    #[test]
    fn excludes_ground_plane() {
        let scene = "component 1 MeshRenderer mesh=__ground_plane__\n\
            component 2 MeshRenderer mesh=assets/models/kaykit-warrior.glb\n";
        let refs = extract_refs(scene, ResourceKind::Scene);
        let assets = asset_paths(&refs);
        assert_eq!(assets, vec!["assets/models/kaykit-warrior.glb".to_owned()]);
    }

    #[test]
    fn comment_aware_scan() {
        // 注释里的 require / assets 引用不产出。
        let lua = "-- local x = require(\"ghost\")\n\
            -- mesh.set(id, \"assets/textures/ghost.png\")\n\
            local ok = require(\"ok\")\n";
        let refs = extract_refs(lua, ResourceKind::LuaScript);
        assert!(refs.iter().all(|reference| match reference {
            DependencyRef::LuaModule(name) => name == "ok",
            _ => false,
        }));
    }

    #[test]
    fn dedupe_preserves_order() {
        let scene = "Sprite assets/textures/a.png\nSprite assets/textures/a.png\nSprite assets/textures/b.png\n";
        let refs = extract_refs(scene, ResourceKind::Scene);
        assert_eq!(
            asset_paths(&refs),
            vec![
                "assets/textures/a.png".to_owned(),
                "assets/textures/b.png".to_owned()
            ]
        );
    }

    #[test]
    fn resolve_self_cycle_excluded() {
        let root = temp_root("self");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "scene-1")
            .unwrap();
        // 场景文本里 scene.load("main") 指向自己 → 排除。
        let refs = vec![DependencyRef::SceneName("main".to_owned())];
        let (ids, unresolved) = resolve_refs(&refs, &db, scene);
        assert!(ids.is_empty(), "self-loop dropped");
        assert!(unresolved.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_asset_and_report_unresolved() {
        let root = temp_root("resolve");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let tex = db
            .register(
                "assets/textures/player.png",
                ResourceKind::Texture,
                "texture-1",
            )
            .unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "scene-1")
            .unwrap();
        let refs = vec![
            DependencyRef::Asset(PathBuf::from("assets/textures/player.png")),
            DependencyRef::Asset(PathBuf::from("assets/textures/ghost.png")),
            DependencyRef::Asset(PathBuf::from("assets/textures/PLAYER.PNG")), // CI 兜底
        ];
        let (ids, unresolved) = resolve_refs(&refs, &db, scene);
        assert_eq!(ids, vec![tex], "asset resolved; CI fallback also resolves");
        assert_eq!(unresolved.len(), 1, "only ghost.png is broken");
        assert_eq!(
            unresolved[0].path,
            PathBuf::from("assets/textures/ghost.png")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recompute_edges_reads_sources_and_writes_graph() {
        let root = temp_root("edges");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let tex = db
            .register(
                "assets/textures/player.png",
                ResourceKind::Texture,
                "texture-1",
            )
            .unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "scene-1")
            .unwrap();
        let lua = db
            .register("scripts/main.lua", ResourceKind::LuaScript, "script-1")
            .unwrap();
        // require("game") 指向的模块：真实项目中会随扫描注册，测试需同步注册。
        let game = db
            .register("scripts/game.lua", ResourceKind::LuaScript, "script-1")
            .unwrap();
        let sources = [
            (
                PathBuf::from("scenes/main.scene"),
                "component 1 Sprite assets/textures/player.png;size=32\n".to_owned(),
            ),
            (
                PathBuf::from("scripts/main.lua"),
                "local game = require(\"game\")\n".to_owned(),
            ),
            (
                PathBuf::from("scripts/game.lua"),
                "function update() end\n".to_owned(),
            ),
        ];
        let read = |path: &Path| -> Option<String> {
            sources
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, text)| text.clone())
        };
        let unresolved = recompute_edges(&mut db, &read);
        assert!(unresolved.is_empty(), "all references resolvable");
        assert_eq!(db.get(scene).unwrap().dependencies, vec![tex]);
        // Lua 引用 scripts/game.lua。
        assert_eq!(db.get(lua).unwrap().dependencies, vec![game]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recompute_edges_reports_unresolved() {
        let root = temp_root("edges-unresolved");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "scene-1")
            .unwrap();
        let read = |_path: &Path| {
            Some("component 1 Sprite assets/textures/player.png;size=32\n".to_owned())
        };
        let unresolved = recompute_edges(&mut db, &read);
        assert_eq!(unresolved.len(), 1);
        assert_eq!(
            unresolved[0].path,
            PathBuf::from("assets/textures/player.png")
        );
        assert_eq!(db.get(scene).unwrap().dependencies, Vec::<AssetId>::new());
        let _ = std::fs::remove_dir_all(&root);
    }
}
