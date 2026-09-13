//! UI 焦点与滚动导航原语（M20-02）。
//!
//! 纯逻辑模块：不依赖 egui / 渲染，供运行时 UI、编辑器预览与 headless 测试
//! 共用。职责分两块：
//!
//! * [`FocusRing`]：一组可聚焦项的**方向键空间导航**与 Tab 式循环，支持
//!   环绕（到边界后回到对侧），空集/单项均安全。
//! * [`ScrollState`]：列表滚动偏移的钳制、按行/按页步进、以及「把某项滚入
//!   视口」的最小滚动量计算。
//!
//! 坐标约定：`[x, y, w, h]`，原点左上、y 向下（与 egui 屏幕坐标一致）。

/// 方向键导航方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavDirection {
    Up,
    Down,
    Left,
    Right,
}

/// 一个可聚焦项（稳定 id + 屏幕像素矩形）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FocusItem {
    pub id: u64,
    pub rect: [f32; 4],
}

impl FocusItem {
    fn center(&self) -> (f32, f32) {
        (
            self.rect[0] + self.rect[2] / 2.0,
            self.rect[1] + self.rect[3] / 2.0,
        )
    }

    fn overlaps_along(&self, other: &Self, axis: Axis) -> bool {
        match axis {
            // 垂直方向移动时要求水平投影有重叠（同一列/相邻列）。
            Axis::Vertical => {
                let (a0, a1) = (self.rect[0], self.rect[0] + self.rect[2]);
                let (b0, b1) = (other.rect[0], other.rect[0] + other.rect[2]);
                a0 < b1 && b0 < a1
            }
            Axis::Horizontal => {
                let (a0, a1) = (self.rect[1], self.rect[1] + self.rect[3]);
                let (b0, b1) = (other.rect[1], other.rect[1] + other.rect[3]);
                a0 < b1 && b0 < a1
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

/// 焦点环：维护当前焦点并按空间位置解析方向键目标。
#[derive(Debug, Clone, Default)]
pub struct FocusRing {
    items: Vec<FocusItem>,
    current: Option<usize>,
}

impl FocusRing {
    /// 用一组可聚焦项构造；初始焦点为空（首帧或 Tab 时落到第一项）。
    #[must_use]
    pub fn new(items: Vec<FocusItem>) -> Self {
        Self {
            items,
            current: None,
        }
    }

    /// 重新设置可聚焦项；若当前焦点项不复存在则清空焦点。
    pub fn set_items(&mut self, items: Vec<FocusItem>) {
        let current_id = self.current().map(|item| item.id);
        self.items = items;
        self.current = None;
        if let Some(id) = current_id {
            self.set_current(id);
        }
    }

    /// 可聚焦项数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 是否没有可聚焦项。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 当前焦点项。
    #[must_use]
    pub fn current(&self) -> Option<FocusItem> {
        self.current
            .and_then(|index| self.items.get(index))
            .copied()
    }

    /// 按 id 指定焦点（不存在返回 false）。
    pub fn set_current(&mut self, id: u64) -> bool {
        match self.items.iter().position(|item| item.id == id) {
            Some(index) => {
                self.current = Some(index);
                true
            }
            None => false,
        }
    }

    /// 清空焦点。
    pub fn clear_focus(&mut self) {
        self.current = None;
    }

    /// 聚焦第一项（打开面板/进入列表时调用）。
    pub fn focus_first(&mut self) -> Option<FocusItem> {
        self.current = (!self.items.is_empty()).then_some(0);
        self.current()
    }

    /// 方向键移动焦点：优先同方向空间最近项；该方向没有候选时环绕到极值项。
    pub fn move_focus(&mut self, direction: NavDirection) -> Option<FocusItem> {
        if self.items.is_empty() {
            self.current = None;
            return None;
        }
        let Some(current_index) = self.current else {
            return self.focus_first();
        };
        let current = self.items[current_index];
        let (cx, cy) = current.center();
        let axis = match direction {
            NavDirection::Up | NavDirection::Down => Axis::Vertical,
            NavDirection::Left | NavDirection::Right => Axis::Horizontal,
        };

        let mut best: Option<(usize, f32, f32)> = None;
        for (index, candidate) in self.items.iter().enumerate() {
            if index == current_index {
                continue;
            }
            let (x, y) = candidate.center();
            // 方向过滤：必须在目标方向的半平面内。
            let forward = match direction {
                NavDirection::Up => cy - y,
                NavDirection::Down => y - cy,
                NavDirection::Left => cx - x,
                NavDirection::Right => x - cx,
            };
            if forward <= 0.5 {
                continue;
            }
            // 主距离 = 行进方向距离；次距离 = 垂直方向距离；重叠投影优先。
            let (primary, secondary) = match axis {
                Axis::Vertical => (forward, (x - cx).abs()),
                Axis::Horizontal => (forward, (y - cy).abs()),
            };
            let aligned = candidate.overlaps_along(&current, axis);
            let key = (primary, if aligned { 0.0 } else { secondary });
            match best {
                Some((_, best_primary, best_secondary))
                    if (best_primary, best_secondary) <= key => {}
                _ => best = Some((index, key.0, key.1)),
            }
        }

        if let Some((index, _, _)) = best {
            self.current = Some(index);
            return self.current();
        }

        // 环绕：优先在**同一行/列**内回到对侧极值（网格光标语义）；若该行/
        // 列只有当前项，则退化为全体的对侧极值，保证焦点不会卡在边界。
        let project = |item: &FocusItem, reversed: bool| -> f32 {
            let (x, y) = item.center();
            match (direction, reversed) {
                (NavDirection::Up, false) => y,
                (NavDirection::Down, false) => -y,
                (NavDirection::Left, false) => x,
                (NavDirection::Right, false) => -x,
                (NavDirection::Up, true) => -y,
                (NavDirection::Down, true) => y,
                (NavDirection::Left, true) => -x,
                (NavDirection::Right, true) => x,
            }
        };
        let pick_extreme = |items: &[FocusItem], skip: usize, reversed: bool| {
            items
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != skip)
                .min_by(|(_, a), (_, b)| {
                    project(a, reversed)
                        .partial_cmp(&project(b, reversed))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(index, _)| index)
        };
        let same_line: Vec<FocusItem> = self
            .items
            .iter()
            .filter(|candidate| candidate.overlaps_along(&current, axis))
            .copied()
            .collect();
        // 注意：skip 必须是 same_line 内的下标（不是全量列表下标）。
        let same_line_current = same_line
            .iter()
            .position(|item| item.id == current.id)
            .unwrap_or(usize::MAX);
        let wrapped = pick_extreme(&same_line, same_line_current, true)
            .map(|index| same_line[index].id)
            .and_then(|id| self.items.iter().position(|item| item.id == id))
            .or_else(|| {
                let all = self.items.clone();
                pick_extreme(&all, current_index, true)
            });
        if let Some(index) = wrapped {
            self.current = Some(index);
        }
        self.current()
    }

    /// Tab 式循环（阅读顺序：上→下、左→右）。`forward=false` 反向。
    pub fn cycle(&mut self, forward: bool) -> Option<FocusItem> {
        if self.items.is_empty() {
            self.current = None;
            return None;
        }
        let next = match self.current {
            None => {
                if forward {
                    0
                } else {
                    self.items.len() - 1
                }
            }
            Some(index) => {
                if forward {
                    (index + 1) % self.items.len()
                } else {
                    (index + self.items.len() - 1) % self.items.len()
                }
            }
        };
        self.current = Some(next);
        self.current()
    }
}

/// 列表滚动状态（内容坐标：0 在顶部，向下为正）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScrollState {
    offset: f32,
    viewport: f32,
    content: f32,
}

impl ScrollState {
    /// 新建滚动状态；`viewport`/`content` 为非负像素高度。
    #[must_use]
    pub fn new(viewport: f32, content: f32) -> Self {
        let mut state = Self::default();
        state.set_extents(viewport, content);
        state
    }

    /// 更新视口与内容高度（内容变化/窗口缩放后调用），并重新钳制偏移。
    pub fn set_extents(&mut self, viewport: f32, content: f32) {
        self.viewport = viewport.max(0.0);
        self.content = content.max(0.0);
        self.offset = self.offset.clamp(0.0, self.max_offset());
    }

    /// 当前偏移。
    #[must_use]
    pub fn offset(&self) -> f32 {
        self.offset
    }

    /// 最大偏移（内容不足一屏时为 0，不会出现空白滚动）。
    #[must_use]
    pub fn max_offset(&self) -> f32 {
        (self.content - self.viewport).max(0.0)
    }

    /// 按像素滚动；返回实际生效的增量（已钳制）。
    pub fn scroll_by(&mut self, delta: f32) -> f32 {
        let before = self.offset;
        self.offset = (self.offset + delta).clamp(0.0, self.max_offset());
        self.offset - before
    }

    /// 按行滚动（`lines` 可为负；`line_step` 为一行高度）。
    pub fn scroll_lines(&mut self, lines: f32, line_step: f32) -> f32 {
        self.scroll_by(lines * line_step.max(0.0))
    }

    /// 翻页（`pages` 可为负）。
    pub fn page(&mut self, pages: f32) -> f32 {
        self.scroll_by(pages * self.viewport)
    }

    /// 当前可见内容区间 `[start, end)`。
    #[must_use]
    pub fn visible_range(&self) -> (f32, f32) {
        (self.offset, (self.offset + self.viewport).min(self.content))
    }

    /// 让 `[item_top, item_top + item_height]` 进入视口；返回是否发生了滚动。
    ///
    /// 已完整可见时不滚动（避免方向键在列表中抖动）。
    pub fn scroll_to_visible(&mut self, item_top: f32, item_height: f32) -> bool {
        let item_height = item_height.max(0.0);
        let item_bottom = item_top + item_height;
        let view_top = self.offset;
        let view_bottom = self.offset + self.viewport;
        let target = if item_top < view_top {
            item_top
        } else if item_bottom > view_bottom {
            item_bottom - self.viewport
        } else {
            return false;
        };
        let before = self.offset;
        self.offset = target.clamp(0.0, self.max_offset());
        (self.offset - before).abs() > f32::EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::{FocusItem, FocusRing, NavDirection, ScrollState};

    fn grid_items() -> Vec<FocusItem> {
        // 3×2 网格：id 1..6，格 100×40，间距 10。
        let mut items = Vec::new();
        let mut id = 1;
        for row in 0..2 {
            for column in 0..3 {
                items.push(FocusItem {
                    id,
                    rect: [column as f32 * 110.0, row as f32 * 50.0, 100.0, 40.0],
                });
                id += 1;
            }
        }
        items
    }

    #[test]
    fn focus_moves_spatially_and_wraps_at_edges() {
        let mut ring = FocusRing::new(grid_items());
        assert_eq!(ring.focus_first().map(|item| item.id), Some(1));

        // 右移 → 同行的右侧邻居；左移在边界环绕到该行最右。
        assert_eq!(ring.move_focus(NavDirection::Right).map(|i| i.id), Some(2));
        assert_eq!(ring.move_focus(NavDirection::Right).map(|i| i.id), Some(3));
        assert_eq!(ring.move_focus(NavDirection::Right).map(|i| i.id), Some(1));
        assert_eq!(ring.move_focus(NavDirection::Left).map(|i| i.id), Some(3));

        // 下移 → 同列下一行；再下移环绕回第一行同列。
        ring.set_current(2);
        assert_eq!(ring.move_focus(NavDirection::Down).map(|i| i.id), Some(5));
        assert_eq!(ring.move_focus(NavDirection::Down).map(|i| i.id), Some(2));
        assert_eq!(ring.move_focus(NavDirection::Up).map(|i| i.id), Some(5));
    }

    #[test]
    fn focus_ring_handles_empty_single_and_tab_cycle() {
        let mut empty = FocusRing::new(Vec::new());
        assert!(empty.is_empty());
        assert!(empty.focus_first().is_none());
        assert!(empty.move_focus(NavDirection::Down).is_none());
        assert!(empty.cycle(true).is_none());

        // 单项：方向键与 Tab 都停在唯一项上。
        let mut single = FocusRing::new(vec![FocusItem {
            id: 7,
            rect: [0.0, 0.0, 10.0, 10.0],
        }]);
        assert_eq!(single.focus_first().map(|i| i.id), Some(7));
        assert_eq!(single.move_focus(NavDirection::Up).map(|i| i.id), Some(7));
        assert_eq!(single.cycle(true).map(|i| i.id), Some(7));

        // Tab 循环正反向 + 环绕。
        let mut ring = FocusRing::new(grid_items());
        ring.clear_focus();
        assert_eq!(ring.cycle(true).map(|i| i.id), Some(1));
        assert_eq!(ring.cycle(true).map(|i| i.id), Some(2));
        assert_eq!(ring.cycle(false).map(|i| i.id), Some(1));
        assert_eq!(ring.cycle(false).map(|i| i.id), Some(6));

        // 指定不存在的 id 不改动焦点；聚焦项消失后自动淡出。
        assert!(!ring.set_current(999));
        assert_eq!(ring.current().map(|i| i.id), Some(6));
        ring.set_items(vec![FocusItem {
            id: 42,
            rect: [0.0, 0.0, 1.0, 1.0],
        }]);
        assert!(ring.current().is_none());
        assert!(ring.set_current(42));
    }

    #[test]
    fn scroll_clamps_and_pages() {
        let mut scroll = ScrollState::new(100.0, 250.0);
        assert_eq!(scroll.max_offset(), 150.0);
        assert_eq!(scroll.scroll_by(30.0), 30.0);
        assert_eq!(scroll.offset(), 30.0);
        // 到顶/到底都钳制，且不会出现空白滚动。
        assert_eq!(scroll.scroll_by(-100.0), -30.0);
        assert_eq!(scroll.offset(), 0.0);
        assert_eq!(scroll.scroll_by(1000.0), 150.0);
        assert_eq!(scroll.offset(), 150.0);
        assert_eq!(scroll.visible_range(), (150.0, 250.0));
        // 翻页 = 一个视口高度。
        assert_eq!(scroll.page(-1.0), -100.0);
        assert_eq!(scroll.offset(), 50.0);
        // 内容不足一屏：无偏移。
        scroll.set_extents(100.0, 60.0);
        assert_eq!(scroll.max_offset(), 0.0);
        assert_eq!(scroll.offset(), 0.0);
        assert_eq!(scroll.scroll_lines(3.0, 20.0), 0.0);
    }

    #[test]
    fn scroll_to_visible_uses_minimal_scroll() {
        let mut scroll = ScrollState::new(100.0, 400.0);
        // 完全可见 → 不滚动。
        assert!(!scroll.scroll_to_visible(10.0, 20.0));
        assert_eq!(scroll.offset(), 0.0);
        // 下方项 → 只滚到其底边贴住视口底。
        assert!(scroll.scroll_to_visible(150.0, 30.0));
        assert_eq!(scroll.offset(), 80.0);
        // 上方项 → 滚到其顶边贴住视口顶。
        assert!(scroll.scroll_to_visible(20.0, 30.0));
        assert_eq!(scroll.offset(), 20.0);
        // 超出内容末尾也被钳制。
        assert!(scroll.scroll_to_visible(395.0, 30.0));
        assert_eq!(scroll.offset(), 300.0);
    }
}
