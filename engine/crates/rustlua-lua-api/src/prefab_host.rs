//! `prefab` 表的宿主端口：一组与领域无关的最小操作。
//!
//! `rustlua-ui::prefab_lua` 把这四个参数形状接到 Lua 的 `prefab.*`，而真正的世界装配由
//! 组合根（脚本层的 `PrefabOps`）实现本 trait。trait 住在契约层（叶子 crate）是为了让
//! 实现方 `rustlua-scripting` 不必依赖 `rustlua-ui`：`prefab` 表的 Lua 表面属于 UI 域，
//! 但"按路径造一个实例、按名字找节点、改文字/显隐/矩形"这组动作本身不含任何 UI 类型。

/// 组合根实现这组操作，UI 域只负责把它们接到 Lua 上。
///
/// 约定：`Err(String)` 里是**完整的** Lua 错误文案（含 `prefab.xxx` 前缀与参数细节）。这样
/// 迁移前后脚本看到的字串逐字相同，不必在两侧各拼一半。
pub trait PrefabHost {
    /// 从工程相对路径读 `.prefab` 并实例化，返回实例 handle。
    fn spawn(&self, path: &str) -> Result<i64, String>;
    /// 销毁实例；handle 不存在时返回 `false`（幂等，不报错）。
    fn despawn(&self, handle: i64) -> Result<bool, String>;
    /// 实例的根实体（Lua 侧 id）。
    fn root(&self, handle: i64) -> Result<i64, String>;
    /// 实例里某个名字的节点（Lua 侧 id）。
    fn node(&self, handle: i64, name: &str) -> Result<i64, String>;
    /// 改节点的场景 `Text` 组件文字（不动字体/字号/颜色）。
    fn set_text(&self, handle: i64, name: &str, text: &str) -> Result<(), String>;
    /// 显示/隐藏节点及其整棵子树。
    fn set_visible(&self, handle: i64, name: &str, visible: bool) -> Result<(), String>;
    /// 改节点 `UiLayout` 的四边（anchors 沿用预设里 authored 的值）。
    fn set_rect(&self, handle: i64, name: &str, rect: [f32; 4]) -> Result<(), String>;
}
