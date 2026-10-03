//! 文本度量的像素↔em 换算基准，render 与 ui 共用的单一事实源。
//!
//! 拆分后 `rustlua-render` 的 `text::EM_PX` 与 `rustlua-ui` 的 `ui::UI_TEXT_BASE_PX`
//! 各写了一份 `12.0`，只靠 render 里一条 `#[cfg(test)]` 断言维持同步，而那条断言又逼出
//! render 对 ui 的 dev-dependency（两条都不该存在：渲染的文本度量与 UI 组件没有依赖关系）。
//! 字面量收在本模块后，两个 crate 各自保留原来的公开名指向这里，既有引用路径一字不变。

/// `scale == 1.0` 时一个 em 的显示高度（像素），也是 SDF 字形光栅化的 em 尺寸。
///
/// 场景格式按这个值定稿（见 `docs/16-scene-format.md`）：`Text` 组件的 `font_size`
/// 除以它得到渲染 `scale`。改动会让既有场景的字号与字形采样口径一起漂移。
pub const TEXT_BASE_PX: f32 = 12.0;
