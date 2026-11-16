//! 行3 属性面板（Inspector）：实体层级/命名、组件管理、Transform/Camera/Light/Collider 编辑。

use crate::ComponentClipboard;
use crate::EditorApp;
use crate::panels::tilemap_tools;
use crate::util::{component_display_name, strip_verbatim_prefix};
use editor::{Collider3dFields, ContainerPreset, InspectorError, InspectorState};
use eframe::egui;
use rustlua_scene::prefab::Prefab;
use rustlua_scene::scene::{SceneEntity, SceneEntityId};
use rustlua_ui::ui_animation::{UiEasing, UiKeyframe};
use std::{collections::BTreeSet, fs, path::Path, time::SystemTime};

struct ComponentDescriptor {
    name: &'static str,
    /// E13：Add Component 菜单按类别分组显示。
    category: ComponentCategory,
}

/// E13：组件类别（Add Component 分组 + 未来按类别过滤的扩展点）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ComponentCategory {
    /// 变换与相机：2D 与 3D 共用同一套组件（第 57 轮统一后的唯一入口）。
    Spatial,
    Gameplay,
    Rendering,
    ThreeD,
    Ui,
}

impl ComponentCategory {
    /// 分组顺序（菜单与 Add Component 共用一处，避免两处顺序漂移）。
    const ALL: &'static [Self] = &[
        Self::Spatial,
        Self::Gameplay,
        Self::Rendering,
        Self::ThreeD,
        Self::Ui,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Spatial => "变换与相机",
            Self::Gameplay => "玩法",
            Self::Rendering => "渲染",
            Self::ThreeD => "3D",
            Self::Ui => "UI",
        }
    }
}

/// E13：注册表驱动 Add Component——新组件只需在此登记名称与类别，
/// 默认值在 `add_component` 的 match 中补充（类型化 setter 或字面字符串）。
///
/// `菜单栏的「组件」菜单直接由这张表生成`（见 `crate::menu`），因此新增组件既不用改
/// 巨型 match，也不用改菜单。
const COMPONENT_REGISTRY: &[ComponentDescriptor] = &[
    // 第 57 轮：**唯一的**变换与相机（旧名 `Transform3D`/`Camera2D`/`Camera3D` 已从引擎删除，
    // 因此它们既不在本表、也不再能被加载）。默认值见 `add_component` 的对应分支：
    // `Transform` = 引擎的单位变换，`Camera` = 透视相机 + 判别字段。
    ComponentDescriptor {
        name: "Transform",
        category: ComponentCategory::Spatial,
    },
    ComponentDescriptor {
        name: "Camera",
        category: ComponentCategory::Spatial,
    },
    ComponentDescriptor {
        name: "Sprite",
        category: ComponentCategory::Rendering,
    },
    ComponentDescriptor {
        name: "Tilemap",
        category: ComponentCategory::Gameplay,
    },
    // 运行时切场景不销毁：无载荷标记组件，挂在父节点上会级联到整棵子树。
    // 由运行时 `Scene::is_entity_persistent_with_ancestors` 解释；Inspector 不再有复选框。
    ComponentDescriptor {
        name: rustlua_scene::scene::PERSISTENT_COMPONENT,
        category: ComponentCategory::Gameplay,
    },
    ComponentDescriptor {
        name: "Animation",
        category: ComponentCategory::Gameplay,
    },
    // P3-E01-2：`.anim` 时间线播放器（与状态机 `Animation`、骨骼 `Animation3D` 并列）。
    ComponentDescriptor {
        name: "AnimationPlayer",
        category: ComponentCategory::Gameplay,
    },
    ComponentDescriptor {
        name: "MeshRenderer",
        category: ComponentCategory::ThreeD,
    },
    ComponentDescriptor {
        name: "Light",
        category: ComponentCategory::ThreeD,
    },
    ComponentDescriptor {
        name: "Collider3d",
        category: ComponentCategory::ThreeD,
    },
    ComponentDescriptor {
        name: "Text",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "WorldText",
        category: ComponentCategory::Rendering,
    },
    ComponentDescriptor {
        name: "Image",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "UiLayout",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "Container",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "LayoutItem",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "UiEvents",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "UiBindings",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "ButtonStates",
        category: ComponentCategory::Ui,
    },
    ComponentDescriptor {
        name: "UiAnimation",
        category: ComponentCategory::Ui,
    },
];

/// 组件菜单的分组视图：`(类别标签, 类别下的组件名)`。
///
/// 菜单栏（`crate::menu` 的「组件」菜单）用它生成子菜单——**注册表是唯一来源**，
/// 因此新增组件只改注册表一处，菜单与 Inspector 的 Add Component 同时生效。
#[must_use]
pub(crate) fn component_menu_groups() -> Vec<(&'static str, Vec<&'static str>)> {
    ComponentCategory::ALL
        .iter()
        .copied()
        .map(|category| {
            let names = COMPONENT_REGISTRY
                .iter()
                .filter(|descriptor| descriptor.category == category)
                .map(|descriptor| descriptor.name)
                .collect();
            (category.label(), names)
        })
        .filter(|(_, names): &(&str, Vec<&str>)| !names.is_empty())
        .collect()
}

/// Inspector Override 摘要用的源预制体 缓存条目。
pub(crate) struct CachedPrefabSource {
    /// (文件长度, mtime)。`None` = 读取元数据失败（例如文件不存在）。
    pub(crate) stamp: Option<(u64, Option<SystemTime>)>,
    pub(crate) prefab: Prefab,
}

/// 源预制体 的失效戳：长度 + mtime。任一变化即视为需要重载。
fn prefab_stamp(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()))
}

mod actions;
mod helpers;
mod panel;
mod prefab;
#[cfg(test)]
mod tests;
mod ui_editors;

// Prefab diff / 字段取值这些纯函数住在 `helpers`（只算文本，不碰文档）。
use helpers::{
    canvas_scale_mode_label, collider3d_shape_label, compact_value, container_align_label,
    container_justify_label, container_kind_label,
};
// 外部调用点（`crate::actions`）沿用 `panels::inspector::…` 路径。
pub(crate) use helpers::{diff_prefab_entity, prefab_override_side_value};

impl EditorApp {
    /// 注册表里是否有这个组件（Add Component 的两个入口共用同一条判定与文案）。
    pub(crate) fn component_registered(name: &str) -> bool {
        COMPONENT_REGISTRY
            .iter()
            .any(|descriptor| descriptor.name == name)
    }
}
