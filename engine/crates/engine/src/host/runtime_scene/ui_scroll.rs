//! M76-02：实体 UI 的滚动与裁剪。
//!
//! 布局解算之后做三件事（顺序固定）：
//! 1. **内容尺寸**：滚动节点的内容包围盒 = 其子孙节点布局矩形的最大右下角，不含滚动节点自身；
//! 2. **偏移**：`ui_scroll_offsets`（交互树按名字写）按轴向平移子孙矩形，越界值夹回范围；
//! 3. **裁剪**：每个节点拿到所有滚动祖先视口矩形的交集，绘制与命中共用这份矩形。
//!
//! 裁剪矩形存在 `ui_clip_rects` 而不是写进 `SceneDraw`：渲染层没有 scissor 字段，
//! 图像按矩形裁 UV、文字按字形过滤（部分重叠的字形整块取舍），三条宿主走同一份网格。
//!
//! 第 4 件事（M79）：**视口外条目不进装配**（[`RuntimeScene::scrolled_away_slots`]），
//! 长列表的每帧代价从"总项数"降到"窗口项数"。

use std::collections::{BTreeMap, BTreeSet};

use rustlua_ui::ui_interaction::UiRect;
use rustlua_ui::ui_interaction::UiScroll;
use rustlua_ui::ui_scene_layout::ResolvedUiNode;
use rustlua_ui::ui_scroll_view::ScrollView;

use super::RuntimeScene;

/// 滚出视口判定的余量（设计像素）：条目矩形先放大这个数再与裁剪矩形求交。
const SCROLL_CULL_MARGIN_PX: f32 = 1.0;

impl RuntimeScene {
    /// 在 `ensure_scene_ui_layout` 完成解析后调用：算滚动范围、平移子孙、生成裁剪矩形。
    pub(super) fn apply_scroll_layout(&mut self, resolved: &mut BTreeMap<u64, ResolvedUiNode>) {
        let scrolls: BTreeMap<u64, bool> = self.script.with_world(|world| {
            let mut found = BTreeMap::new();
            world.for_each::<ScrollView>(|id, scroll| {
                found.insert(id.index() as u64, scroll.horizontal);
            });
            found
        });
        if scrolls.is_empty() {
            self.ui_scroll_ranges.clear();
            self.ui_clip_rects.clear();
            self.ui_scroll_shifts.clear();
            return;
        }
        let base: BTreeMap<u64, UiRect> = resolved
            .iter()
            .map(|(slot, node)| {
                (
                    *slot,
                    UiRect::new(node.rect.x, node.rect.y, node.rect.w, node.rect.h),
                )
            })
            .collect();
        let names = self.script.entity_names_by_slot();

        let mut ranges: BTreeMap<u64, UiScroll> = BTreeMap::new();
        for (&slot, &horizontal) in &scrolls {
            let Some(viewport) = base.get(&slot).copied() else {
                // 没布局的滚动容器没有视口，滚不动；交互树里也就没有可滚动节点。
                continue;
            };
            let mut content = 0.0_f32;
            for (&child_slot, rect) in &base {
                if child_slot == slot || !self.is_layout_descendant(child_slot, slot) {
                    continue;
                }
                let extent = if horizontal {
                    rect.x + rect.w - viewport.x
                } else {
                    rect.y + rect.h - viewport.y
                };
                content = content.max(extent);
            }
            let range = if horizontal {
                UiScroll::horizontal(content).with_viewport(viewport.w)
            } else {
                UiScroll::vertical(content).with_viewport(viewport.h)
            };
            ranges.insert(slot, range);
        }

        // 偏移按实体名存取；内容变短时把越界偏移夹回去并写回，交互树下一帧才看到真值。
        for (&slot, range) in &mut ranges {
            let Some(name) = names.get(&slot) else {
                continue;
            };
            let requested = self.ui_scroll_offsets.get(name).copied().unwrap_or(0.0);
            let viewport = base.get(&slot).copied().unwrap_or(UiRect::ZERO);
            let clamped = requested.clamp(0.0, range.max_offset(viewport));
            range.offset = clamped;
            if clamped != requested {
                self.ui_scroll_offsets.insert(name.clone(), clamped);
            }
        }

        // 每个节点的累计平移与裁剪矩形：walk 父链，遇到滚动祖先就累加/求交。
        // 先算平移再求交：外层滚动节点的视口自身也可能被更外层推着走。
        let mut shifts: BTreeMap<u64, [f32; 2]> = BTreeMap::new();
        let mut clips: BTreeMap<u64, UiRect> = BTreeMap::new();
        for &slot in resolved.keys() {
            let shift = self.accumulated_scroll_shift(slot, &ranges);
            let mut clip: Option<UiRect> = None;
            let mut ancestor = self.layout_parent(slot);
            while let Some(parent) = ancestor {
                if ranges.contains_key(&parent)
                    && let Some(viewport) = base.get(&parent)
                {
                    let viewport =
                        viewport.translated(self.accumulated_scroll_shift(parent, &ranges));
                    clip = Some(match clip {
                        Some(current) => current.intersect(viewport).unwrap_or(UiRect::ZERO),
                        None => viewport,
                    });
                }
                ancestor = self.layout_parent(parent);
            }
            shifts.insert(slot, shift);
            if let Some(clip) = clip {
                clips.insert(slot, clip);
            }
        }

        for (&slot, node) in resolved.iter_mut() {
            if let Some(shift) = shifts.get(&slot) {
                node.rect.x += shift[0];
                node.rect.y += shift[1];
            }
        }
        self.ui_scroll_ranges = ranges;
        self.ui_clip_rects = clips;
        self.ui_scroll_shifts = shifts;
    }

    fn layout_parent(&self, slot: u64) -> Option<u64> {
        self.ui_scene_nodes.get(&slot).and_then(|node| node.parent)
    }

    /// 祖先链里所有滚动节点对本节点的累计平移（不含本节点自己的滚动偏移）。
    fn accumulated_scroll_shift(&self, slot: u64, ranges: &BTreeMap<u64, UiScroll>) -> [f32; 2] {
        let mut shift = [0.0_f32, 0.0];
        let mut cursor = self.layout_parent(slot);
        let mut guard = 0usize;
        while let Some(ancestor) = cursor {
            if let Some(range) = ranges.get(&ancestor) {
                if range.horizontal {
                    shift[0] -= range.offset;
                } else {
                    shift[1] -= range.offset;
                }
            }
            guard += 1;
            if guard > self.ui_scene_nodes.len() {
                break;
            }
            cursor = self.layout_parent(ancestor);
        }
        shift
    }

    /// `child` 是否在 `ancestor` 的布局子树里（父链闭环时按"不是"处理）。
    fn is_layout_descendant(&self, child: u64, ancestor: u64) -> bool {
        let mut cursor = self.layout_parent(child);
        let mut guard = 0usize;
        while let Some(slot) = cursor {
            if slot == ancestor {
                return true;
            }
            guard += 1;
            if guard > self.ui_scene_nodes.len() {
                return false;
            }
            cursor = self.layout_parent(slot);
        }
        false
    }

    /// 本帧**完全滚出视口**的场景 UI 槽位（放大 [`SCROLL_CULL_MARGIN_PX`] 后仍不相交）。
    ///
    /// 为什么绘制趟已经"交集为空就不写顶点"了还要这一层：早退发生在**装配之后**。
    /// 进趟之前每条目都要全额付一次：实体遍历、组件克隆（`UiImage`/`UiText` 整份 clone）、
    /// 贴图重定向查表、状态贴图查名，文字那条更要紧——`prewarm_frame_glyphs` 会把
    /// 拿到的**每一条**文字都栅格化进图集，滚出视口的 988 项照样栅格化。
    /// 1000 项列表里只有十几项真的进网格，其余的代价过去每帧都付。
    ///
    /// 判据与绘制趟同源（同一份 `ui_layout_rects` ∩ `ui_clip_rects`），但先把条目矩形
    /// 放大 1 px 再判：边界条目宁可多留进装配（那里绘制趟仍会按真实交集裁），
    /// 也不许把一个本帧画得出来的条目摘掉——症状会是"滚动时边缘闪一下白"，
    /// 而那正是最难复现、最难归因的一类红。
    ///
    /// 没有滚动容器时返回空集（逐帧行为与今天逐字节一致）。
    pub(crate) fn scrolled_away_slots(&self) -> BTreeSet<u64> {
        let mut away = BTreeSet::new();
        if self.ui_scroll_ranges.is_empty() {
            return away;
        }
        for (slot, node) in &self.ui_layout_rects {
            let Some(clip) = self.ui_clip_rects.get(slot) else {
                continue;
            };
            let inflated = UiRect::new(
                node.rect.x - SCROLL_CULL_MARGIN_PX,
                node.rect.y - SCROLL_CULL_MARGIN_PX,
                node.rect.w + SCROLL_CULL_MARGIN_PX * 2.0,
                node.rect.h + SCROLL_CULL_MARGIN_PX * 2.0,
            );
            if inflated.intersect(*clip).is_none() {
                away.insert(*slot);
            }
        }
        away
    }
}
