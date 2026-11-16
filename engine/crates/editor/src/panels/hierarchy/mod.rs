//! 行3 层级面板（资源面板右侧）：场景切换/新建 + 实体树 + 项目信息。

use crate::EditorApp;
use crate::util::{display_path, strip_verbatim_prefix};
use editor::{ReparentPosition, SelectionSource, prefab_graph};
use eframe::egui;
use rustlua_assets::resource_db::AssetId;
use rustlua_ecs::transform::Transform3D;
use rustlua_scene::prefab::Prefab;
use rustlua_scene::scene::SceneEntityId;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::tree;
use model::HierarchyNav;

mod model;
mod navigation;
mod panel;
mod persist;
mod scene_ops;
#[cfg(test)]
mod tests;

pub(crate) use model::{
    HierarchyFilter, HierarchyLowerCache, HierarchyMatchIndex, HierarchyRow, HierarchyVisibleCache,
    RowHeightProbe, hierarchy_row_height,
};
