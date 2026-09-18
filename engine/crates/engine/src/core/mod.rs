//! Core types for the Rust + Lua game engine.

/// Named animation states driven by sprite-sheet frame clips.
pub mod animation;
/// 3D skeletal animation component driven by glTF skins and clips.
pub mod animation3d;
/// Texture atlas regions and named Sprite sub-assets.
pub mod atlas;
/// Named sound-effect playback backed by rodio.
pub mod audio;
/// WP4：2D 相机控制器（跟随 / 死区 / 平滑 / 抖动）。
pub mod camera;
/// Perspective camera and view-frustum primitives for 3D rendering.
/// 打包时字符收集：扫描场景 Text 与 Lua 字符串，产出去重的字符集供字形预热。
pub mod char_collect;
/// Deferred world mutations for safe ECS system iteration.
pub mod commands;
/// E21: typed schema registry over serialized component strings.
pub mod component_schema;
/// Lightweight JSON-schema validation for `data` tables loaded via Lua.
pub mod data_schema;
/// WP3 原型：dense 值数组 + sparse 槽位索引的组件存储（待 A/B 基准裁定是否替换）。
pub mod dense_storage;
/// Dependency-graph extraction, resolution, propagation, and cycle detection.
pub mod dependency;
/// E27: versioned, fault-isolated editor plugin registry.
pub mod editor_plugins;
/// Generational entity handles and allocation.
pub mod entity;
/// Typed, frame-bounded event queues.
pub mod events;
pub mod font;
/// 打包时 TrueType 字体子集化：把字体裁剪到用到的字符，缩小发布体积。
pub mod font_subset;
/// Gameplay data types for clocks, items, and inventories.
pub mod gameplay;
/// CPU-side glTF/GLB mesh and scene asset importer.
pub mod gltf_asset;
/// Grid helpers for tile-based world/cell conversion and facing.
pub mod grid;
/// Content fingerprinting shared by the import cache and resource database.
pub mod hash;
/// WP2：空间父子关系（`Parent`）与世界变换传播（`WorldTransform`）。
pub mod hierarchy;
/// Incremental and atomic asset import cache.
pub mod import_cache;
/// Importer trait, builtin registry, and batch runner.
pub mod importer;
/// Platform-independent keyboard, mouse, and cursor snapshots.
pub mod input;
/// WP6：渲染层（World2D / Overlay2D / ScreenUI）的显式说法。
pub mod layer;
/// Directional light component for 3D scenes.
pub mod light;
/// Logging and context-rich engine errors.
pub mod logging;
/// Coordinate, camera, and color-space conventions for 2D rendering.
pub mod math2d;
/// Coordinate, quaternion, and matrix primitives for 3D rendering.
pub mod math3d;
/// 游戏工程路径解析（默认仓库内 examples）。
pub mod paths;
/// WP2/WP9：场景空间层级迁移命令（dry-run / 写回 / 备份 / 一键回滚）。
pub mod scene_migration;
pub use paths::fixture_games_dir;
pub use paths::games_dir;
/// GPU-friendly CPU mesh and material data for 3D rendering.
pub mod mesh3d;
/// 发布资源编解码（压缩 + 加密）。
pub mod pack;
/// 确定性数据并行执行器（纯分块任务，可回滚为串行）。
pub mod par;
/// Pooled particle bursts with spawn/update/recycle.
pub mod particle;
/// Runtime frame and update performance sampling.
pub mod performance;
/// Rapier-backed rigid bodies, colliders, sensors, and collision events.
pub mod physics;
pub mod physics3d;
/// docs/54 §8-⑥：2D/3D 物理契约的**对拍**测试（只在测试构建里编译）。
#[cfg(test)]
mod physics_parity;
/// 拾取的共享投影/查询链：屏幕 → 视口 → 相机射线 → 世界查询（docs/54 §8-⑦）。
pub mod picking;
/// Prefab extraction, instantiation, overrides, and rebuild policy.
pub mod prefab;
/// E24: structured preview diagnostics with stable categories and deduping.
pub mod preview_diagnostics;
/// `project.toml` / `release.toml` 的键读取（工程配置解析唯一入口）。
pub mod project_config;
/// Persistent project asset identities, import states, dependencies, and cache metadata.
pub mod resource_db;
/// Unified coarse-grained resource families shared by editor, assetc, and engine.
pub mod resource_kind;
/// Versioned save-data serialization with corrupt/version rejection.
pub mod save;
/// Versioned scene serialization and strict version validation boundary.
pub mod scene;
/// Shared extraction of serialized 3D render components.
pub mod scene_extract;
/// 文档（编辑态）的空间语义：文档世界变换与"目标世界 → 新局部"反解（docs/54 §8-⑦）。
pub mod scene_space;
/// Deterministic stage-based system scheduling.
pub mod scheduler;
/// Lua VM and host API bridge.
pub mod script;
/// 工程级设置文件 `<project>/settings.toml`（音频分组音量等玩家偏好）。
pub mod settings;
/// WP2：统一的局部/世界变换与父先序传播。
pub mod spatial_transform;
/// CPU-side 2D Sprite geometry and UV generation.
pub mod sprite;
/// CPU-side batching and sorting for 2D Sprites.
pub mod sprite_batch;
/// Fixed-rate simulation timing primitives.
pub mod text;
/// PNG decoding and CPU-side texture asset representation.
pub mod texture;
/// Grid tile maps with a visual layer and a separate collision layer.
pub mod tilemap;
pub mod time_step;
/// 2D transform and velocity components with a minimal integration system.
pub mod transform;
/// 3D transform component and model-matrix construction.
/// WP1：加载边界的类型化场景编译。
pub mod typed_scene;
/// Screen-space UI scene components (Text / Image).
pub mod ui;
/// E26: pure UI animation tracks and isolated preview playback.
pub mod ui_animation;
/// E23: minimal one-way ViewModel property bindings.
pub mod ui_binding;
/// E22: structured UI event bindings.
pub mod ui_events;
/// E15/E16/E17 UI layout layer: Anchor+Offset, Container auto-layout, Canvas.
pub mod ui_layout;
/// E25: resolved-layout cache with subtree-aware invalidation.
pub mod ui_layout_cache;
/// UI 焦点与滚动导航原语（键盘/手柄列表导航）。
pub mod ui_navigation;
/// E17: parent-chain and container resolution over serialized UI nodes.
pub mod ui_scene_layout;
/// E20: reusable button visual-state configuration.
pub mod ui_states;
/// 玩家数据目录（P3）：存档与玩家设置写到哪里（发布包 → 用户目录）。
pub mod user_dir;
/// Window and application lifecycle primitives.
pub mod window;
/// Entity world and type-erased component storage.
pub mod world;
/// WP4 增补：世界空间文字组件（锚在世界位置、走同一套字形管线）。
pub mod world_text;

/// Returns the engine core package version.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Per-project working directory holding local runtime data: the resource
/// database, import cache, and save data. Local and gitignored.
pub const PROJECT_WORK_DIR: &str = ".cache";

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn reports_package_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }

    /// M22（W5）：core 不得依赖 host。
    ///
    /// `core/settings.rs` 曾 `use crate::host::gamepad::GamepadDeadzone`（core 反向
    /// 依赖桌面/XInput 层，`docs/37` P1-1），这会让 core 无法独立编译到
    /// wasm/headless-only 目标。死区值类型已下沉到 `core::input`，这里加一条**架构
    /// 回归**：扫描 `src/core/**/*.rs` 源码，任何**代码行**出现 host 依赖路径的模块
    /// 都判失败（注释行忽略，便于就地写明原因）。
    ///
    /// 局限：只做字面量检查（`super::`/别名 `use` 这类绕法查不到），足以覆盖本仓库的
    /// 实际写法并防止再次引入。
    #[test]
    fn core_does_not_depend_on_host() {
        fn collect(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, files);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    files.push(path);
                }
            }
        }

        // 拼接 needle，避免本测试自己的源码文本被匹配到。
        let needle = ["crate", "::", "host"].concat();
        let core_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core");
        let mut files = Vec::new();
        collect(&core_dir, &mut files);
        assert!(
            !files.is_empty(),
            "未找到 core 源文件：{}",
            core_dir.display()
        );

        let offenders = files
            .iter()
            .filter_map(|path| {
                let text = std::fs::read_to_string(path).ok()?;
                let mut in_tests = false;
                let hit = text.lines().any(|line| {
                    let code = line.trim_start();
                    // 只看**生产代码**：注释行与 `#[cfg(test)]` 之后的测试模块都跳过
                    //（单元测试本来就允许在同一个 crate 内驱动 host 层）。
                    if code.starts_with("#[cfg(test)]") {
                        in_tests = true;
                    }
                    !in_tests && !code.starts_with("//") && code.contains(&needle)
                });
                hit.then(|| {
                    path.strip_prefix(&core_dir)
                        .unwrap_or(path)
                        .display()
                        .to_string()
                })
            })
            .collect::<Vec<_>>();
        assert!(
            offenders.is_empty(),
            "core 不得依赖 host（把平台类型下沉到 core 或改为泛型参数）：{offenders:?}"
        );
    }
}
