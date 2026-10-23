//! 资源**导入层**：把源文件（png/wav/lua/gltf/atlas/ttf/scene/prefab/anim）校验并加工成运行时能
//! 直接读的产物。
//!
//! 这一层**只在构建工具与编辑器里编译**：`assetc`（打包/导入 CLI）、`editor`（资源面板），
//! 以及两个演示 examples。浏览器产物（`game_web`）与引擎库（`engine`）都不依赖本 crate ——
//! 它们是"读产物"的一侧，读的是 [`rustlua_assets::resource_db`] 与 `rltex`。
//!
//! 拆分动机、代价与验收方式见 `docs/62` §6 P1 与 `tasks/done/P1-降耦合收口.md` 的阶段 ③。
//!
//! # 模块
//!
//! | 模块 | 职责 | 依赖的重东西 |
//! |---|---|---|
//! | [`importers`] | 九个内置 [`Importer`](rustlua_assets::importer::Importer) 实现 + 注册表组装 | `image` / `rodio` / `ab_glyph` / `mlua` |
//! | `batch` | 批量导入驱动（逐项状态写回资源库，一项坏不打断整批） | — |
//! | `atlas_plan` | 图集规划：配置 → 任务 → 装箱 | `image` |
//! | `atlas_pack` | 装箱器本体（矩形打包 + 页面合成） | `image` |
//! | `import_cache` | 导入缓存的读写与指纹 | `image` |
//!
//! [`Importer`]: rustlua_assets::importer::Importer

pub mod atlas_pack;
pub mod atlas_plan;
pub mod batch;
pub mod char_collect;
pub mod environment;
pub mod import_cache;
pub mod importers;

pub use batch::{
    ImportItemResult, ImportSummary, run_import_batch, run_import_batch_with_progress,
};
pub use importers::{builtin, core_importers};

/// 完整导入注册表：本 crate 的九个核心导入器 + `rustlua-scene` 的 Scene/Prefab 导入器。
///
/// 为什么组装点在**这里**而不是 `rustlua-scene`：组装需要同时看见"九个核心导入器"（住本
/// crate，因为要 `image`/`rodio`）和"场景导入器"（住 `rustlua-scene`，因为要用 `Scene`/
/// `Prefab` 类型）。scene 一旦反过来依赖本 crate，运行时就会被拖进导入层的编译图 —— 那正是
/// 这次拆分要拆掉的东西。所以：`scene` 提供零件，本 crate 提供装配。
///
/// 所有需要导入 `.scene` / `.prefab` 的调用方（`assetc`、`editor`、缓存示例）都必须用这个
/// 函数，否则这两类文件会报"不支持的资源类型"（`builtin()` 不含它们）。
#[must_use]
pub fn full_registry() -> rustlua_assets::importer::ImporterRegistry {
    let mut importers: Vec<&'static dyn rustlua_assets::importer::Importer> =
        core_importers().into();
    importers.extend(rustlua_scene::asset_import::scene_importers());
    rustlua_assets::importer::ImporterRegistry::from_importers(&importers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_assets::importer::ImporterRegistry;
    use rustlua_assets::resource_kind::ResourceKind;
    use std::path::Path;

    /// 装配出来的注册表必须**十一种 kind 齐全**：九个核心 + Scene/Prefab。
    ///
    /// 这条测试原先住在 `rustlua_scene::asset_import`（装配点在那里）。搬家时说清了原因
    /// 才不留空洞：装配需要同时看见两侧的零件，而核心零件要 `image`/`rodio`，scene 不能依赖
    /// 它们 —— 所以装配点必须在导入层，测试跟着走。
    ///
    /// 这份清单同时也是**回归门**：`ResourceKind::from_path` 认了扩展名却没有导入器时，
    /// 扫描会把该类型的每一份文件报成「不支持的资源类型」（P3-R02 的 `.material` 就这样
    /// 漏过一次，示例项目 25 份材质全红）。新增 kind 必须同时在这里加一条。
    #[test]
    fn full_registry_includes_core_and_scene_importers() {
        let registry: ImporterRegistry = full_registry();
        for kind in [
            ResourceKind::Texture,
            ResourceKind::Audio,
            ResourceKind::LuaScript,
            ResourceKind::Scene,
            ResourceKind::Prefab,
            ResourceKind::Model,
            ResourceKind::Atlas,
            ResourceKind::Font,
            ResourceKind::Environment,
            ResourceKind::Material,
            ResourceKind::Animation,
        ] {
            let importer = registry.get(kind).expect("full importer exists");
            assert_eq!(importer.kind(), kind);
            assert!(!importer.extensions().is_empty());
        }
        assert!(registry.get(ResourceKind::Other).is_none());
    }

    /// `builtin()` 是"只要普通资源"的那一半：**必须**不含 Scene/Prefab。
    ///
    /// 这条断言是给未来的人看的：谁要是把 scene 导入器挪进 `builtin()`，`rustlua-assets-import`
    /// 就会多依赖一层 scene（今天已经依赖，但那是因为 `full_registry`；`builtin()` 用它没有
    /// 理由），而更重要的是 `assetc`/`editor` 里"用 builtin 导场景会失败"这条既有契约会消失。
    #[test]
    fn builtin_has_no_scene_or_prefab_importers() {
        let registry = builtin();
        assert!(registry.get(ResourceKind::Scene).is_none());
        assert!(registry.get(ResourceKind::Prefab).is_none());
        // 反过来：普通资源照旧能按扩展名找到（证明 builtin 是活的，不是空表）。
        assert_eq!(
            registry.from_path(Path::new("hero.png")).map(|i| i.kind()),
            Some(ResourceKind::Texture)
        );
        assert_eq!(
            registry
                .from_path(Path::new("main.scene"))
                .map(|i| i.kind()),
            None,
            "按扩展名解析查不到 —— 这正是'用 builtin 导场景会报不支持'那条既有契约"
        );
    }
}
