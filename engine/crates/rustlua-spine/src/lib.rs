//! M84 Spine 2D 运行时接入：Spine 4.3.x → 引擎世界通道。
//!
//! 分层：`backend/` 隔离第三方 crate，`player/` 只认中性类型，
//! 渲染 / 组件 / Lua 三层不直接引用 `spine2d` 的类型（见 M84-00 §4.1）。
//!
//! 模块职责：
//! - [`model`]：中性数据模型（渲染输出、清单、组件载荷、事件）。
//! - [`backend`]：唯一允许出现 `spine2d` 类型的隔离层。
//! - [`player`]：播放层（`SpineData` / `SpinePlayer`），作为 ECS 组件挂载。
//! - [`spine_state`]：跨帧的脚本可见事件队列。
//! - [`spine_lua`]：`spine.*` Lua 表。
//! - [`scene_codec`]：`SpineSkeleton` 的组件载荷校验 codec。

pub mod backend;
pub mod model;
pub mod player;
pub mod scene_codec;
pub mod spine_lua;
pub mod spine_state;

pub use model::{
    SpineAttachment, SpineBlend, SpineDraw, SpineError, SpineEvent, SpineEventKind, SpineManifest,
    SpineRenderOutput, SpineSkeletonComponent, SpineVertex,
};
pub use player::{SpineData, SpinePlayer};
pub use spine_state::SpineState;

/// 导入期的中性校验入口：只验"这份字节能不能被运行时读出来"，不构造骨架、不留状态。
///
/// 导入层（`rustlua-assets-import`）因此**不需要**依赖 `spine2d`——backend 隔离层之外
/// 不该出现第三方类型，这条规则连校验路径也不例外。
pub mod validate {
    use crate::model::SpineError;

    /// 校验 `.skel` 二进制骨骼。
    pub fn skeleton_bytes(bytes: &[u8]) -> Result<(), SpineError> {
        crate::backend::BackendData::validate_skeleton_bytes(bytes)
    }

    /// 校验 Spine JSON 骨骼。
    pub fn skeleton_json_bytes(bytes: &[u8]) -> Result<(), SpineError> {
        crate::backend::BackendData::validate_skeleton_json_bytes(bytes)
    }

    /// 校验 `.spineatlas` 图集文本。
    pub fn atlas_text(text: &str) -> Result<(), SpineError> {
        crate::backend::BackendData::validate_atlas_text(text)
    }

    /// 拒绝本管线尚未实现的骨架特性（目前只有 clipping）。
    ///
    /// 必须在**导入期**调用：P1/P2 不实现裁剪，静默放行会让"该裁没裁"变成运行时
    /// 才发现的画面错误。返回的错误带 `skin/slot/attachment` 三处定位，作者能直接在
    /// Spine 编辑器里找到那一条。
    pub fn reject_unsupported_features(
        skeleton_bytes: &[u8],
        binary: bool,
    ) -> Result<(), SpineError> {
        let clipping = crate::backend::BackendData::clipping_attachments(skeleton_bytes, binary)?;
        let mut iter = clipping.into_iter();
        let Some(first) = iter.next() else {
            return Ok(());
        };
        let count = 1 + iter.count();
        Err(SpineError::UnsupportedClipping { count, first })
    }
}
