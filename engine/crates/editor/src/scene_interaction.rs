//! E11：Scene View 交互事件与工具状态机的可测试核心。
//!
//! 交互**词汇表**（[`InteractionEvent`]）与工具状态机（[`ToolStateMachine`]）在这里，
//! 命中测试与文档写入留在 `scene_view.rs` / shell。这样"事件 → 状态"的流转可以脱离
//! egui/GPU 独立单测。
//!
//! **已接入生产路径**（docs/55 `ER-07` 首批接线）：`EditorApp` 用
//! [`ToolStateMachine`] 持有"当前工具 + 交互阶段"，工具切换（快捷键/工具面板/布局恢复）
//! 与 Scene View 的指针事件都走它；`scene_view` 不再自己维护 `box_select_start`，
//! 框选起点直接读 [`ToolPhase::Marquee`]。
//!
//! 阶段判定的规则（由输入采集层提供 `hit`，状态机不自己做命中测试）：
//!
//! | 工具 | 按在实体上 | 按在空白处 |
//! |---|---|---|
//! | TilePaint | `Painting` | `Painting` |
//! | 其它 | `Dragging` | `Marquee` |
//!
//! 修饰键（Ctrl/Shift）**不**参与阶段判定：它们只决定选择是"加入"还是"替换"，
//! 属于选择语义；早期版本把它们编码进事件里，结果状态机与生产行为不一致。

use std::fmt;

/// Scene View 统一交互事件（输入采集层的输出词汇表）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InteractionEvent {
    /// 主键按下（画布坐标 + 是否命中实体）。
    ///
    /// `hit` 由采集层做命中测试后给出：状态机只关心"按在了东西上还是空白处"，
    /// 不重复实现 picking。
    PointerDown { pos: [f32; 2], hit: bool },
    /// 主键拖动（已按下期间的位置更新）。
    PointerDrag { pos: [f32; 2] },
    /// 主键释放（结束拖拽/框选）。
    PointerUp { pos: [f32; 2] },
    /// Escape：取消当前交互（拖拽取消 / 框选丢弃）。
    Cancel,
    /// 工具切换（QWER / 工具栏按钮）。
    ToolSelected(SceneTool),
    /// Tilemap 绘制笔触（仅 TilePaint 工具消费）。
    TileStroke { cell: [i32; 2] },
}

/// 场景工具（与 main.rs 的 `SceneTool` 对齐；独立定义以便单测脱离 egui）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SceneTool {
    #[default]
    Select,
    Move,
    Rotate,
    Scale,
    TilePaint,
}

impl fmt::Display for SceneTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Select => "Select",
            Self::Move => "Move",
            Self::Rotate => "Rotate",
            Self::Scale => "Scale",
            Self::TilePaint => "Tile Paint",
        })
    }
}

/// 交互阶段状态机：Idle → (Down) → Dragging/Marquee/Painting → (Up) → Idle。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ToolPhase {
    /// 无进行中交互。
    #[default]
    Idle,
    /// 拖动对象（Move/Rotate/Scale/Gizmo 共用；记录起始位置）。
    Dragging { start: [f32; 2], current: [f32; 2] },
    /// 框选（Select 工具 + Shift/Ctrl 或空白处按下）。
    Marquee { start: [f32; 2], current: [f32; 2] },
    /// Tilemap 连续绘制。
    Painting,
}

impl ToolPhase {
    /// 是否有进行中的连续交互（E08：活动阶段 = 活动事务窗口）。
    #[must_use]
    pub const fn is_active(self) -> bool {
        !matches!(self, Self::Idle)
    }

    /// 当前交互是否应产生选择变更（框选）。
    #[must_use]
    pub const fn affects_selection(self) -> bool {
        matches!(self, Self::Marquee { .. })
    }
}

/// 工具状态机：输入事件 → 阶段流转。纯函数式，不含渲染/文档写入。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ToolStateMachine {
    tool: SceneTool,
    phase: ToolPhase,
}

impl ToolStateMachine {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn tool(&self) -> SceneTool {
        self.tool
    }

    pub const fn phase(&self) -> ToolPhase {
        self.phase
    }

    /// 处理一个交互事件，返回是否产生了状态变化。
    pub fn handle(&mut self, event: InteractionEvent) -> bool {
        match event {
            InteractionEvent::ToolSelected(tool) => {
                if self.tool != tool {
                    // 切换工具即取消进行中的交互。
                    self.tool = tool;
                    self.phase = ToolPhase::Idle;
                    true
                } else {
                    false
                }
            }
            InteractionEvent::Cancel => {
                if self.phase.is_active() {
                    self.phase = ToolPhase::Idle;
                    true
                } else {
                    false
                }
            }
            InteractionEvent::PointerDown { pos, hit } => {
                if self.phase.is_active() {
                    return false; // 已在交互中：忽略重复 Down。
                }
                self.phase = match self.tool {
                    // TilePaint 是模式化工具：按下即进入连续绘制。
                    SceneTool::TilePaint => ToolPhase::Painting,
                    // 按在实体上 = 拖动；按在空白处 = 框选（任何工具下都成立）。
                    _ if hit => ToolPhase::Dragging {
                        start: pos,
                        current: pos,
                    },
                    _ => ToolPhase::Marquee {
                        start: pos,
                        current: pos,
                    },
                };
                true
            }
            InteractionEvent::PointerDrag { pos } => match self.phase {
                ToolPhase::Dragging { start, .. } => {
                    self.phase = ToolPhase::Dragging {
                        start,
                        current: pos,
                    };
                    true
                }
                ToolPhase::Marquee { start, .. } => {
                    self.phase = ToolPhase::Marquee {
                        start,
                        current: pos,
                    };
                    true
                }
                _ => false,
            },
            InteractionEvent::PointerUp { .. } => {
                if self.phase.is_active() {
                    self.phase = ToolPhase::Idle;
                    true
                } else {
                    false
                }
            }
            InteractionEvent::TileStroke { .. } => self.phase == ToolPhase::Painting,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_on_entity_drags_and_release_idles() {
        let mut sm = ToolStateMachine::new();
        assert_eq!(sm.tool(), SceneTool::Select);
        assert!(!sm.phase().is_active());
        sm.handle(InteractionEvent::PointerDown {
            pos: [10.0, 20.0],
            hit: true,
        });
        assert!(sm.phase().is_active());
        sm.handle(InteractionEvent::PointerDrag { pos: [30.0, 40.0] });
        assert_eq!(
            sm.phase(),
            ToolPhase::Dragging {
                start: [10.0, 20.0],
                current: [30.0, 40.0]
            }
        );
        sm.handle(InteractionEvent::PointerUp { pos: [30.0, 40.0] });
        assert_eq!(sm.phase(), ToolPhase::Idle);
    }

    /// 按在空白处 → 框选（**任何工具**下都成立，修饰键不参与判定）。
    #[test]
    fn empty_space_click_starts_marquee_for_every_tool() {
        for tool in [
            SceneTool::Select,
            SceneTool::Move,
            SceneTool::Rotate,
            SceneTool::Scale,
        ] {
            let mut sm = ToolStateMachine::new();
            sm.handle(InteractionEvent::ToolSelected(tool));
            sm.handle(InteractionEvent::PointerDown {
                pos: [0.0, 0.0],
                hit: false,
            });
            assert!(
                sm.phase().affects_selection(),
                "{tool} 按空白处必须是框选，实际 {:?}",
                sm.phase()
            );
            sm.handle(InteractionEvent::Cancel);
            assert_eq!(sm.phase(), ToolPhase::Idle);
        }
    }

    #[test]
    fn tile_paint_tracks_painting_phase() {
        let mut sm = ToolStateMachine::new();
        sm.handle(InteractionEvent::ToolSelected(SceneTool::TilePaint));
        sm.handle(InteractionEvent::PointerDown {
            pos: [5.0, 5.0],
            hit: false,
        });
        assert_eq!(sm.phase(), ToolPhase::Painting);
        assert!(sm.handle(InteractionEvent::TileStroke { cell: [2, 3] }));
        sm.handle(InteractionEvent::PointerUp { pos: [5.0, 5.0] });
        assert!(!sm.handle(InteractionEvent::TileStroke { cell: [4, 4] }));
    }

    #[test]
    fn tool_switch_cancels_active_interaction() {
        let mut sm = ToolStateMachine::new();
        sm.handle(InteractionEvent::PointerDown {
            pos: [0.0, 0.0],
            hit: true,
        });
        assert!(sm.phase().is_active());
        sm.handle(InteractionEvent::ToolSelected(SceneTool::Rotate));
        assert_eq!(sm.tool(), SceneTool::Rotate);
        assert_eq!(sm.phase(), ToolPhase::Idle);
        // 选择相同工具不产生变化。
        assert!(!sm.handle(InteractionEvent::ToolSelected(SceneTool::Rotate)));
    }

    #[test]
    fn cancel_is_noop_when_idle_and_duplicate_down_ignored() {
        let mut sm = ToolStateMachine::new();
        assert!(!sm.handle(InteractionEvent::Cancel));
        sm.handle(InteractionEvent::PointerDown {
            pos: [1.0, 1.0],
            hit: true,
        });
        assert!(!sm.handle(InteractionEvent::PointerDown {
            pos: [2.0, 2.0],
            hit: true,
        }));
    }
}
