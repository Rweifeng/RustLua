//! 跨面板共享的编辑命令：撤销/重做、保存、场景切换、实体创建、Inspector 同步、对齐。
//!
//! 第 41 轮从单文件（4,604 行）拆成目录：本文件只留共享类型（`EditState` / `EditSnapshot` /
//! `DocumentChange` / `PendingChange` / `PendingDecision`）与跨模块再导出；命令按
//! 文档闸门 `document_gate`、编辑事务 `transaction`、文档读写 `document_io`、布局 `layout_ops`、
//! 实体 `entities`、组件 `components`、预设命令 `prefab_ops`、预设重建 `prefab_rebuild` 分家。
//! 拆分是纯搬运：只切行 + 补模块头，跨子模块用到的条目从"文件内私有"抬到 `pub(crate)`。

use crate::AlignMode;
use crate::EditorApp;
use crate::geometry::{encode_transform, parse_transform};
use crate::panels::inspector::diff_prefab_entity;
use crate::util::strip_verbatim_prefix;
use editor::{
    Collider3dFields, CommandKind, CommandResult, InspectorError, InspectorSnapshot, SceneDocument,
    TransformFields,
};
use eframe::egui;
use rustlua_animation::animation::AnimationStateMachine;
use rustlua_ecs::tilemap::Tilemap;
use rustlua_render::sprite::SpriteComponent;
use rustlua_scene::prefab::Prefab;
use rustlua_scene::scene::{SceneEntity, SceneEntityId};
use std::path::{Path, PathBuf};

/// E08 Inspector「脏编辑」标记：用户在一次连续交互中改动了缓冲字段，但
/// 尚未到提交边界（松手/文本提交/失焦）。编辑帧由 `mark_inspector_edit`
/// 把最新缓冲同步进一条自动事务并直写文档（驱动 Scene View revision 刷新），
/// 真正的历史/保存延迟到提交点。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EditState {
    pub pending: bool,
    /// 这条脏编辑是否已被消费进活动事务（防同一次拖动逐帧重复 begin）。
    pub consumed: bool,
}

/// E08 Inspector 缓冲基线：与文档上次同步（`sync_inspector`）或提交成功后的
/// 「已提交快照」。Inspector 面板编辑帧把整组缓冲字段与基线做差异比较，
/// 有差异才把整组字段直写文档（自动开事务、不写历史、不落盘）。
///
/// `InspectorSnapshot` 覆盖 `write_inspector` 的主要参数（name/transform/sprite/
/// lua/3D/UI 组件），其字段全部从 **EditorApp 当前缓冲**（`entity_name`/
/// `transform`/`transform3d`/`mesh_renderer`/`camera3d`/…/`text`/`image`）编码而来，
/// 与 `write_inspector` 写回的内容逐字段一致 —— 缓冲改动因此产生真实净 diff。
/// 面板显示层的裸引用/组件对象（`mesh_renderer` 的 `mesh=` 前缀）只作为「编码源」，
/// 不直接参与比较，避免显示层噪音造成空操作直写。
///
/// 写入端（`update_inspector_write`）要求写入字段与文档实际字段**逐字节一致**
/// 才能让 `write_inspector` 的 `Ok(*document != before)` 净差异判定正确：
/// - `mesh_renderer`：文档经 `MeshRenderer::to_component_str` 规范化
///   （`mesh=<path>`）。缓冲只存裸引用，比较前拼上 `mesh=` 前缀生成待写值；
///   实体**没有** MeshRenderer 组件时快照为 `None`，write 端会 clear（不会注入）。
/// - `Sprite` / `Light` / `Text` / `Image` / `UiLayout` / `Camera3D` / `WorldText` 都已迁到
///   schema 生成的字段编辑器（第 20–34 轮），因此**不在**这条缓冲直写路径里。
/// - `Persistent` 是**组件标记**，由通用组件管理（Add/Remove Component）维护，不在
///   这条缓冲直写路径里；Inspector 也不再提供"持久实体"复选框。
/// - `tilemap`/`animation`：从面板 `TilemapEditorState`/`AnimationEditorState`
///   实时编码的整串（`Some` 表示应保留/写回组件；None = 实体无该组件时不写）。
///
/// 附带实体存在性代理：`editing_entities` 在缓冲编辑期间携带全部被编辑实体
/// （主选 + Sprite 多选从属），以免目标实体被删除/撤销掉后误伤。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditSnapshot {
    pub doc_id: SceneEntityId,
    /// 目标（快照来源）实体是否仍存在于当前编辑文档。
    pub doc_present: bool,
    /// 面板主选要写回的实体 ID（可能已被撤销到不存在 —— `editing_entities` 兜底）。
    pub editing_entities: Vec<SceneEntityId>,
    pub snapshot: Option<InspectorSnapshot>,
    pub tilemap: Option<String>,
    pub animation: Option<String>,
}

impl EditSnapshot {
    /// “空”占位：无文档/无主选时使用（doc_id 取 max，保证与任何真实快照不等）。
    pub fn empty() -> Self {
        Self {
            doc_id: SceneEntityId(u64::MAX),
            doc_present: false,
            editing_entities: Vec::new(),
            snapshot: None,
            tilemap: None,
            animation: None,
        }
    }

    pub fn clear(&mut self) {
        *self = Self::empty();
    }
}

/// 一次已提交的文档变更类别（E02）：驱动 `on_document_changed` 的统一刷新流程。
///
/// 打开/新建/恢复、Undo/Redo、运行时 Stop 会把 `current_scene` 整个替换为新快照；
/// 实体删除/重命名/Prefab 重建在 history 闭包内写克隆文档后同样需要统一收尾，
/// 确保 Hierarchy / Inspector / Scene View / Game 预览显示同一份数据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DocumentChange {
    /// 打开磁盘场景/Prefab 文档（替换当前编辑文档）。
    Opened,
    /// 新建空白场景（未保存）。
    Created,
    /// 删除实体（含其子树）。
    EntityDeleted,
    /// 重命名实体。
    EntityRenamed,
    /// 变更 UI Canvas 根节点标记。
    UiRootChanged,
    /// 重挂载实体到新父节点。
    EntityReparented,
    /// 重新应用 / 写回 Prefab 源后重建实例（实体整体被替换）。
    PrefabApply,
    /// Undo：整文档回退到历史快照。
    Undo,
    /// Redo：整文档前进到历史快照。
    Redo,
    /// 退出 Play：编辑文档恢复为 Stop 时取回的编辑器快照。
    PlaybackStopped,
}

/// E06：一次待确认的文档离开操作。任何可能离开当前编辑文档的入口
/// （场景/Prefab 打开、新建场景、崩溃快照恢复、切换项目、关闭编辑器）
/// 都会经 `request_document_change` 请求。若当前编辑文档 dirty，该操作被挂起，
/// 等顶层「保存并继续 / 放弃并继续 / 取消」弹窗裁决后再执行对应 perform；
/// 若 clean 则立即执行，行为与 E06 之前完全一致。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PendingChange {
    /// 打开另一份场景 / Prefab 文档（相对项目根路径）。
    OpenResource { relative_path: PathBuf },
    /// 新建空白场景（尚未保存）。
    CreateScene { name: String },
    /// E19: create an isolated Widget document.
    CreateWidget { name: String },
    /// 恢复一条崩溃恢复快照。
    Recover {
        candidate: editor::RecoveryCandidate,
    },
    /// 切换游戏项目（新建 / 打开 / 最近项目；走后台导入）。
    SwitchProject { path: String },
    /// 关闭编辑器窗口（用户点窗口 X / 菜单退出）。
    QuitEditor,
}

impl PendingChange {
    /// 弹窗标题下对离开动作的短描述。
    pub(crate) fn action_label(&self) -> String {
        match self {
            Self::OpenResource { relative_path } => {
                format!("打开 {}", relative_path.display())
            }
            Self::CreateScene { name } => format!("新建场景 {name}"),
            Self::CreateWidget { name } => format!("新建控件 {name}"),
            Self::Recover { .. } => "恢复崩溃快照".to_owned(),
            Self::SwitchProject { path } => format!("切换到项目：{}", path),
            Self::QuitEditor => "关闭编辑器".to_owned(),
        }
    }
}

/// E06 确认窗的用户决策（由顶层弹窗的按钮触发；独立成枚举便于回归测试）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingDecision {
    /// 「保存并继续」：保存成功才执行离开操作；失败则停留并保留 dirty。
    Save,
    /// 「放弃并继续」：不保存，直接执行离开操作。
    Discard,
    /// 「取消」：取消挂起的离开操作，文档 / 选择 / 视图 / 历史保持不变。
    Cancel,
}

mod components;
mod document_gate;
mod document_io;
mod entities;
mod layout_ops;
mod prefab_ops;
mod prefab_rebuild;
#[cfg(test)]
mod tests;
mod transaction;

// 跨模块命令入口：其余模块一律经 `crate::actions::` 引用这些名字。
#[cfg(test)]
pub(crate) use layout_ops::{distribute_targets, sorted_sprite_order_updates};
pub(crate) use prefab_rebuild::{
    apply_prefab_instance_metadata, build_prefab_from_instance_root, collect_prefab_rebuild_state,
};
