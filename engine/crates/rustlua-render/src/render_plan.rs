//! docs/54 §4.3：一帧的**渲染通道计划**（`RenderPlan`）。
//!
//! §4.3 给出了目标渲染组织的形状：
//!
//! ```text
//! RenderPlan
//! ├── Shadow
//! ├── Depth / Opaque
//! ├── World Sprite / Billboard
//! ├── Transparent
//! ├── Legacy2DOverlay
//! ├── ScreenUI
//! └── PostProcess
//! ```
//!
//! 此前"这一帧要跑哪些通道、按什么顺序"只存在于 `Renderer::render_passes` 的一串 `if`：
//! 决策与执行混在一起，因此**没法在不启动 GPU 的情况下断言它**，"顺序对不对"只能靠肉眼看
//! 代码。本模块把决策抽成纯函数 [`crate::render_plan::RenderPlan::for_frame`]（输入是一帧内容的**事实**，输出是
//! 通道的**规范顺序**列表），渲染器改为按计划执行：每个代码块的开关条件都不再就地判断，
//! 而是 `plan.contains(...)`。于是"少跑了一个通道""顺序被换过"这类改动会在不启动 GPU 的
//! 单测里直接失败。
//!
//! 与本仓库其余收敛动作一致的几条边界：
//!
//! - 计划是**派生**的：它从帧内容算出来，不是第二份真源。渲染器不消费的通道不会出现在计划里。
//! - 计划**不写像素**：它只决定"跑不跑、按什么顺序跑"，每个通道内部的像素行为仍由原代码决定。
//! - 顺序是**规范顺序**（[`crate::render_plan::RenderPass::ALL`]），[`crate::render_plan::RenderPlan::is_canonical`] 是可断言的硬性质：
//!   同一通道至多一次、且严格按规范顺序出现。

use crate::layer::RenderLayer;

/// 一帧里的一条渲染通道。
///
/// 变体的**声明顺序就是规范执行顺序**（`Ord` 由声明顺序导出，[`crate::render_plan::RenderPlan::is_canonical`] 依赖这一点）。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RenderPass {
    /// 清屏（`ClearPolicy::Color` 或回滚档的历史默认色）。`ClearPolicy::None` 时这一条不出现。
    ///
    /// 严格说这是附件操作而不是绘制通道；蓝图里也没把它单列。保留成一条是因为"这一帧清不清屏"
    /// 同样是一帧内容的函数，且它是**唯一**能被计划漏掉却仍然改变画面的操作。
    Clear,
    /// 不透明世界 Sprite：在网格 pass **之前**写深度，因此不透明精灵能遮挡网格。
    ///
    /// 蓝图把 `World Sprite / Billboard` 排在 `Depth / Opaque` 之后；这里不透明段必须在网格
    /// 之前，否则共用深度附件时"精灵遮挡网格"就不成立。这是与蓝图的**有意偏离**（半透明段仍
    /// 在网格之后，与蓝图一致）。
    WorldSpriteOpaque,
    /// 阴影贴图（方向光）。
    Shadow,
    /// 深度 / 不透明网格。
    DepthOpaque,
    /// 世界 Sprite 通道：共用 3D 视图时画**半透明**段（测深度不写深度），否则画全部段
    /// （回滚档：纯 2D 画面只有精灵之间的画家顺序）。蓝图里的 `Transparent` 就是这里。
    WorldSprite,
    /// 旧 2D 叠加顶点，覆盖 `RenderLayer::{World2D, Overlay2D}` 两个桶（同一批顶点、同一条管线，
    /// 桶间按 [`crate::layer::OVERLAY_DRAW_ORDER`] 排序）。
    Legacy2DOverlay,
    /// 屏幕空间 UI 桶（`RenderLayer::ScreenUI`）。与 [`Self::Legacy2DOverlay`] 共用一次
    /// `begin_render_pass`，但它是**独立的通道**：蓝图把它单列，且"HUD 是否被其它内容盖住"
    /// 曾经是一个真实缺陷（docs/54 §8-⑤）。
    ScreenUI,
    /// 矢量文字轮廓（独立管线 + 独立顶点/字形/线段缓冲）。
    VectorText,
    /// 后处理（[`crate::post_process`]）：场景先画进离屏颜色纹理，再由全屏 pass 采样写进
    /// 目标附件。永远是最后一条——它消费的是"本帧已经画完的颜色"。
    PostProcess,
}

impl RenderPass {
    /// 规范执行顺序。渲染器按这个顺序执行，计划也必须按这个顺序列出通道。
    pub const ALL: [Self; 9] = [
        Self::Clear,
        Self::WorldSpriteOpaque,
        Self::Shadow,
        Self::DepthOpaque,
        Self::WorldSprite,
        Self::Legacy2DOverlay,
        Self::ScreenUI,
        Self::VectorText,
        Self::PostProcess,
    ];

    /// 稳定短名（日志与诊断用）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::WorldSpriteOpaque => "world_sprite_opaque",
            Self::Shadow => "shadow",
            Self::DepthOpaque => "depth_opaque",
            Self::WorldSprite => "world_sprite",
            Self::Legacy2DOverlay => "legacy_2d_overlay",
            Self::ScreenUI => "screen_ui",
            Self::VectorText => "vector_text",
            Self::PostProcess => "post_process",
        }
    }

    /// 这条通道覆盖哪些 2D 分桶（空 = 不是 2D 分桶通道）。
    #[must_use]
    pub const fn overlay_layers(self) -> &'static [RenderLayer] {
        match self {
            Self::Legacy2DOverlay => &[RenderLayer::World2D, RenderLayer::Overlay2D],
            Self::ScreenUI => &[RenderLayer::ScreenUI],
            _ => &[],
        }
    }
}

/// 一帧内容的**事实**：计划函数的唯一输入。
///
/// 全是计数/布尔，没有几何本体——因此计划可以在单测里被穷举，而不需要 GPU 或场景。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameFacts {
    /// 本帧清屏（宿主声明了 `ClearPolicy::Color`，或没有声明主视图 ⇒ 历史默认色）。
    pub clear: bool,
    /// 本帧有 3D 网格可画（阴影与深度/不透明通道因此存在）。
    pub has_3d_draws: bool,
    /// 世界 Sprite 顶点数（0 ⇒ 整条世界 Sprite 通道不存在）。
    pub world_sprites: usize,
    /// 世界 Sprite 里有**不透明**分段（`depth_write`）。没有不透明段就不需要"网格之前"那一遍。
    pub world_sprite_opaque_segment: bool,
    /// 世界 Sprite 与网格**共用**相机/深度附件（`WorldSpriteView::shared_depth`）。
    pub world_sprite_shared_depth: bool,
    /// `RenderLayer::World2D` 桶的 draw 数。
    pub world_2d_draws: usize,
    /// `RenderLayer::Overlay2D` 桶的 draw 数。
    pub overlay_2d_draws: usize,
    /// `RenderLayer::ScreenUI` 桶的 draw 数。
    pub screen_ui_draws: usize,
    /// 本帧有矢量文字轮廓。
    pub vector_text: bool,
    /// 本帧开后处理（宿主 `set_post_process(Some(..))`）。
    ///
    /// 开后处理时场景画进离屏颜色纹理，因此**本帧一定会清屏**（离屏 pass 没有"上一帧内容"可叠）
    /// ——见 [`crate::render_plan::RenderPlan::for_frame`] 里对 `clear` 的处理。
    pub post_process: bool,
}

/// 本帧要跑的通道，按规范顺序排列。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RenderPlan {
    passes: Vec<RenderPass>,
}

impl RenderPlan {
    /// 什么都不跑（没有可画内容、也不清屏时的计划）。
    #[must_use]
    pub const fn empty() -> Self {
        Self { passes: Vec::new() }
    }

    /// 从帧内容事实算出计划。
    #[must_use]
    pub fn for_frame(facts: FrameFacts) -> Self {
        let mut passes = Vec::with_capacity(Self::ALL_LEN);
        // 开后处理时场景先画进**离屏**颜色纹理，那张附件上没有"上一帧留下的内容"，
        // 因此 `ClearPolicy::None` 无法照原义执行：本帧必须清屏（渲染器会就此告警一次）。
        if facts.clear || facts.post_process {
            passes.push(RenderPass::Clear);
        }
        // 不透明世界 Sprite 只有在"与网格共用深度"时才有意义：独立深度附件下精灵与网格本来
        // 就不共享深度空间，提前一遍只是空转。
        if facts.world_sprites > 0
            && facts.world_sprite_shared_depth
            && facts.world_sprite_opaque_segment
        {
            passes.push(RenderPass::WorldSpriteOpaque);
        }
        if facts.has_3d_draws {
            passes.push(RenderPass::Shadow);
            passes.push(RenderPass::DepthOpaque);
        }
        if facts.world_sprites > 0 {
            passes.push(RenderPass::WorldSprite);
        }
        if facts.world_2d_draws + facts.overlay_2d_draws > 0 {
            passes.push(RenderPass::Legacy2DOverlay);
        }
        if facts.screen_ui_draws > 0 {
            passes.push(RenderPass::ScreenUI);
        }
        if facts.vector_text {
            passes.push(RenderPass::VectorText);
        }
        if facts.post_process {
            passes.push(RenderPass::PostProcess);
        }
        Self { passes }
    }

    /// 规范顺序里的通道条数（用于预分配）。
    pub const ALL_LEN: usize = RenderPass::ALL.len();

    /// 本帧的通道，按规范顺序。
    #[must_use]
    pub fn passes(&self) -> &[RenderPass] {
        &self.passes
    }

    /// 本帧是否要跑这条通道。渲染器里每个代码块的开关条件都是它。
    #[must_use]
    pub fn contains(&self, pass: RenderPass) -> bool {
        self.passes.contains(&pass)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.passes.len()
    }

    /// 是否严格按规范顺序、且同一通道至多一次。
    ///
    /// 这是计划的**硬性质**（由构造方式保证），写成可断言的函数是为了让"以后有人手写一份
    /// 计划"这种改动立刻失败，而不是等到像素不对。
    #[must_use]
    pub fn is_canonical(&self) -> bool {
        self.passes.windows(2).all(|pair| pair[0] < pair[1])
    }

    /// 把执行到的通道序列与本计划核对。返回第一个不一致的地方（`None` = 完全一致）。
    ///
    /// 渲染器每帧调它一次：计划驱动的代码块如果被条件绕开（例如 `world_view` 为 `None`），
    /// 就在这里留下可断言的差异，而不是静默少画一个通道。
    #[must_use]
    pub fn mismatch(&self, executed: &[RenderPass]) -> Option<String> {
        if self.passes.len() != executed.len() {
            return Some(format!(
                "planned {} passes ({:?}) but executed {} ({:?})",
                self.passes.len(),
                Self::names(&self.passes),
                executed.len(),
                Self::names(executed)
            ));
        }
        for (planned, actual) in self.passes.iter().zip(executed) {
            if planned != actual {
                return Some(format!(
                    "planned order {:?} but executed {:?}",
                    Self::names(&self.passes),
                    Self::names(executed)
                ));
            }
        }
        None
    }

    fn names(passes: &[RenderPass]) -> Vec<&'static str> {
        passes.iter().map(|pass| pass.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_frame() -> FrameFacts {
        FrameFacts {
            clear: true,
            has_3d_draws: true,
            world_sprites: 4,
            world_sprite_opaque_segment: true,
            world_sprite_shared_depth: true,
            world_2d_draws: 2,
            overlay_2d_draws: 1,
            screen_ui_draws: 3,
            vector_text: true,
            post_process: true,
        }
    }

    /// 全内容的一帧必须**正好**是规范顺序，一条不多一条不少。
    #[test]
    fn full_frame_plan_follows_the_canonical_order() {
        let plan = RenderPlan::for_frame(full_frame());
        assert_eq!(
            plan.passes(),
            [
                RenderPass::Clear,
                RenderPass::WorldSpriteOpaque,
                RenderPass::Shadow,
                RenderPass::DepthOpaque,
                RenderPass::WorldSprite,
                RenderPass::Legacy2DOverlay,
                RenderPass::ScreenUI,
                RenderPass::VectorText,
                RenderPass::PostProcess,
            ]
        );
        assert!(plan.is_canonical());
        assert_eq!(
            RenderPlan::ALL_LEN,
            RenderPass::ALL.len(),
            "ALL_LEN 必须与规范顺序一致"
        );
        assert_eq!(RenderPass::ALL.len(), 9, "通道数变化必须同步改渲染器");
    }

    /// 空帧：什么都不跑（连清屏都没有 ⇒ 宿主声明了 `ClearPolicy::None`）。
    #[test]
    fn empty_frame_plans_nothing() {
        let plan = RenderPlan::for_frame(FrameFacts::default());
        assert!(plan.is_empty());
        assert!(plan.is_canonical());
        assert_eq!(plan, RenderPlan::empty());
    }

    /// 开后处理 ⇒ 场景画进离屏附件，那张附件上没有"上一帧内容"可叠，因此**必须清屏**，
    /// 即使宿主声明了 `ClearPolicy::None`（渲染器会就此告警一次）。
    #[test]
    fn post_process_forces_a_clear_and_is_always_last() {
        let plan = RenderPlan::for_frame(FrameFacts {
            clear: false,
            post_process: true,
            world_2d_draws: 1,
            ..FrameFacts::default()
        });
        assert_eq!(
            plan.passes(),
            [
                RenderPass::Clear,
                RenderPass::Legacy2DOverlay,
                RenderPass::PostProcess
            ],
            "后处理必须清屏且永远排在最后"
        );
    }

    /// 三条容易写错的边界：不透明段需要共用深度、2D 桶各自独立成通道、World2D/Overlay2D 合成一条。
    #[test]
    fn plan_separates_the_conditions_that_used_to_live_in_the_render_loop() {
        // ① 只有精灵、没网格 ⇒ 没有阴影/深度通道，也没有"网格之前"那一遍。
        let sprites_only = RenderPlan::for_frame(FrameFacts {
            world_sprites: 1,
            world_sprite_opaque_segment: true,
            world_sprite_shared_depth: false,
            ..FrameFacts::default()
        });
        assert_eq!(
            sprites_only.passes(),
            [RenderPass::WorldSprite],
            "独立深度附件下不该有 WorldSpriteOpaque"
        );

        // ② 共用深度但只有半透明段 ⇒ 同样不需要提前那一遍。
        let translucent_only = RenderPlan::for_frame(FrameFacts {
            world_sprites: 1,
            world_sprite_opaque_segment: false,
            world_sprite_shared_depth: true,
            ..FrameFacts::default()
        });
        assert!(!translucent_only.contains(RenderPass::WorldSpriteOpaque));

        // ③ World2D 与 Overlay2D 合成 Legacy2DOverlay，ScreenUI 独立；只有 ScreenUI 时
        //    不该顺带跑一条空的 Legacy2DOverlay。
        let overlay_only = RenderPlan::for_frame(FrameFacts {
            overlay_2d_draws: 1,
            ..FrameFacts::default()
        });
        assert_eq!(overlay_only.passes(), [RenderPass::Legacy2DOverlay]);
        let ui_only = RenderPlan::for_frame(FrameFacts {
            screen_ui_draws: 1,
            ..FrameFacts::default()
        });
        assert_eq!(ui_only.passes(), [RenderPass::ScreenUI]);
        assert_eq!(
            RenderPass::Legacy2DOverlay.overlay_layers(),
            [RenderLayer::World2D, RenderLayer::Overlay2D]
        );
        assert_eq!(
            RenderPass::ScreenUI.overlay_layers(),
            [RenderLayer::ScreenUI]
        );
        assert!(RenderPass::Shadow.overlay_layers().is_empty());
    }

    /// 计划恒为规范顺序：把 bool 事实穷举一遍，`is_canonical` 必须无例外成立。
    #[test]
    fn every_fact_combination_yields_a_canonical_plan() {
        for bits in 0u32..(1 << 7) {
            let facts = FrameFacts {
                clear: bits & 1 != 0,
                has_3d_draws: bits & 2 != 0,
                world_sprites: usize::from(bits & 4 != 0),
                world_sprite_opaque_segment: bits & 8 != 0,
                world_sprite_shared_depth: bits & 16 != 0,
                world_2d_draws: usize::from(bits & 32 != 0),
                overlay_2d_draws: 0,
                screen_ui_draws: 0,
                vector_text: false,
                post_process: bits & 64 != 0,
            };
            let plan = RenderPlan::for_frame(facts);
            assert!(plan.is_canonical(), "{facts:?} ⇒ {:?}", plan.passes());
        }
    }

    /// 核对函数必须真的能发现差异（否则"计划=执行"只是个恒真的断言）。
    #[test]
    fn mismatch_reports_missing_extra_and_reordered_passes() {
        let plan = RenderPlan::for_frame(FrameFacts {
            clear: true,
            has_3d_draws: true,
            ..FrameFacts::default()
        });
        let executed = plan.passes().to_vec();
        assert!(plan.mismatch(&executed).is_none());

        // 少一条 / 多一条 / 顺序颠倒都必须报出来。
        assert!(plan.mismatch(&executed[1..]).is_some());
        let mut extra = executed.clone();
        extra.push(RenderPass::VectorText);
        assert!(plan.mismatch(&extra).is_some());
        let mut swapped = executed.clone();
        swapped.swap(0, 1);
        let message = plan.mismatch(&swapped).expect("顺序颠倒必须被发现");
        assert!(message.contains("planned order"), "{message}");
    }
}
