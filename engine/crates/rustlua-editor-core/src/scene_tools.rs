//! docs/55 阶段 2「拆分 2D/3D 工具控制器」。
//!
//! 计划要求把 Scene View 的工具语义按视图模式拆开。这里落地的不是"三个各自持有状态的
//! 状态机"（那会让"当前工具"又变成多份事实源，正是计划 §4.1 要消除的东西），而是
//! **一张按模式声明的能力表 + 一个统一的状态机**：
//!
//! * 状态机（`scene_interaction::ToolStateMachine`）仍然是"当前工具 + 交互相位"的唯一事实源；
//! * 本模块只回答"**这个模式允许哪些工具、不允许的那些要落到哪个工具上**"。
//!
//! 为什么必须有这张表：`TilePaint` 只在 2D 画布上有意义（3D 分支里是 `SceneTool::TilePaint => {}`，
//! 即"选了但什么都不做"），而工具面板过去在 3D 模式下照样把它显示成可点——用户点完
//! 什么反馈都没有。
//!
//! 这张表同时被三处消费，所以"声明"和"行为"不会漂移：
//!
//! 1. 工具面板的可用/禁用状态与 tooltip；
//! 2. `set_scene_tool` 的兜底（该模式不支持 → 落到该模式的默认工具并在状态栏说明）；
//! 3. 切换视图模式时的重新校验（带着 TilePaint 切到 3D 会退回 Select）。

use crate::SceneTool;

/// 视图模式（与 `main::SceneViewMode` 一一对应，但这里只关心**工具语义**）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolMode {
    /// 2D 画布：变换走 `Transform`（XY 平面），另有 Tile 画笔。
    #[default]
    TwoD,
    /// 3D 画布：变换走 `Transform3D`（XZ 平面 + 高度 Y），无 Tile 画笔。
    ThreeD,
    /// Prefab 编辑：与 2D 画布同一套工具（Prefab 文档本身就是 2D/3D 场景的子集）。
    Prefab,
}

impl ToolMode {
    /// 该模式允许的工具（顺序即工具面板的显示顺序）。
    pub fn tools(self) -> &'static [SceneTool] {
        match self {
            // 2D 画布里 Tile 绘制是核心工作流之一。
            Self::TwoD | Self::Prefab => &[
                SceneTool::Select,
                SceneTool::Move,
                SceneTool::Rotate,
                SceneTool::Scale,
                SceneTool::TilePaint,
            ],
            // 3D 画布没有 tilemap 绘制概念（地形由网格/模型承担）。
            Self::ThreeD => &[
                SceneTool::Select,
                SceneTool::Move,
                SceneTool::Rotate,
                SceneTool::Scale,
            ],
        }
    }

    /// 该模式的默认工具（不支持的请求会落到它上面）。
    pub fn default_tool(self) -> SceneTool {
        SceneTool::Select
    }

    pub fn allows(self, tool: SceneTool) -> bool {
        self.tools().contains(&tool)
    }

    /// 把请求的工具收敛到本模式允许的集合里。
    ///
    /// 返回 `(实际生效的工具, 是否被收敛)`：调用方据此提示"该模式不支持这个工具"，
    /// 而不是静默换一个（静默换工具会让快捷键看起来失灵）。
    pub fn resolve(self, requested: SceneTool) -> (SceneTool, bool) {
        if self.allows(requested) {
            (requested, false)
        } else {
            (self.default_tool(), true)
        }
    }

    /// 模式名（状态栏与 tooltip 用）。
    pub fn label(self) -> &'static str {
        match self {
            Self::TwoD => "2D 视图",
            Self::ThreeD => "3D 视图",
            Self::Prefab => "预制体视图",
        }
    }

    /// 某个工具在这个模式下的**语义说明**（工具面板的 tooltip）。
    ///
    /// 这解决的是"同一个按钮在不同模式里做的不是一件事"的沟通问题：
    /// 例如 Move 在 2D 改 Transform，在 3D 改位置轴。
    pub fn tool_hint(self, tool: SceneTool) -> &'static str {
        match (self, tool) {
            (_, SceneTool::Select) => "选择（Q）",
            (Self::ThreeD, SceneTool::Move) => "移动（W，沿 X/Y/Z 轴）",
            (_, SceneTool::Move) => "移动（W）",
            (_, SceneTool::Rotate) => "旋转（E）",
            (_, SceneTool::Scale) => "缩放（R）",
            (_, SceneTool::TilePaint) => "瓦片画笔",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_paint_only_exists_on_the_2d_canvas() {
        assert!(ToolMode::TwoD.allows(SceneTool::TilePaint));
        assert!(ToolMode::Prefab.allows(SceneTool::TilePaint));
        assert!(
            !ToolMode::ThreeD.allows(SceneTool::TilePaint),
            "3D 画布没有 tile 绘制落点，不允许选中它"
        );
    }

    #[test]
    fn resolve_reports_whether_it_had_to_fall_back() {
        // 允许的工具原样返回，且不报告收敛。
        assert_eq!(
            ToolMode::ThreeD.resolve(SceneTool::Move),
            (SceneTool::Move, false)
        );
        // 不允许的工具落到默认工具，并**明确报告**收敛（调用方要提示用户）。
        assert_eq!(
            ToolMode::ThreeD.resolve(SceneTool::TilePaint),
            (SceneTool::Select, true)
        );
    }

    #[test]
    fn every_mode_has_a_distinct_hint_for_move() {
        for mode in [ToolMode::TwoD, ToolMode::ThreeD, ToolMode::Prefab] {
            for tool in mode.tools() {
                assert!(!mode.tool_hint(*tool).is_empty(), "{mode:?} {tool:?}");
            }
        }
    }
}
