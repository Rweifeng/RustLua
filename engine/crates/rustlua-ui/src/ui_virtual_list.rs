//! M76-07：虚拟列表的可见窗口与槽位回收。
//!
//! 纯逻辑：按固定行高把 1000+ 项映射成「可见区间 + 有限槽位」。渲染层只画窗口内的
//! 槽位，滚出窗口的 key 释放回池子；key 稳定时同一项始终复用自己的槽位。

/// 行高、间距与过扫（窗口上下各多绑几项，避免快速滚动露白）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VirtualListConfig {
    pub item_height: f32,
    pub spacing: f32,
    pub overscan: usize,
}

impl Default for VirtualListConfig {
    fn default() -> Self {
        Self {
            item_height: 24.0,
            spacing: 0.0,
            overscan: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VirtualWindow {
    pub start: usize,
    pub end: usize,
    pub offset: f32,
    pub total_height: f32,
}

impl VirtualWindow {
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VirtualList {
    config: VirtualListConfig,
    total: usize,
    viewport: f32,
    offset: f32,
}

impl VirtualList {
    pub fn new(config: VirtualListConfig, total: usize, viewport: f32) -> Self {
        let mut list = Self {
            config,
            total,
            viewport: viewport.max(0.0),
            offset: 0.0,
        };
        list.offset = list.clamp_offset(list.offset);
        list
    }

    fn stride(&self) -> f32 {
        (self.config.item_height + self.config.spacing).max(0.0)
    }

    pub fn total_height(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        self.stride() * self.total as f32 - self.config.spacing
    }

    fn max_offset(&self) -> f32 {
        (self.total_height() - self.viewport).max(0.0)
    }

    fn clamp_offset(&self, offset: f32) -> f32 {
        offset.clamp(0.0, self.max_offset())
    }

    pub fn offset(&self) -> f32 {
        self.offset
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn set_total(&mut self, total: usize) {
        self.total = total;
        self.offset = self.clamp_offset(self.offset);
    }

    pub fn set_viewport(&mut self, viewport: f32) {
        self.viewport = viewport.max(0.0);
        self.offset = self.clamp_offset(self.offset);
    }

    /// 返回实际应用的位移（滚动到底时可能小于请求值）。
    pub fn scroll_by(&mut self, delta: f32) -> f32 {
        let before = self.offset;
        self.offset = self.clamp_offset(self.offset + delta);
        self.offset - before
    }

    pub fn scroll_to(&mut self, offset: f32) {
        self.offset = self.clamp_offset(offset);
    }

    /// 把第 `index` 项滚进视口；已完整可见时不动。返回是否发生了滚动。
    pub fn scroll_to_index(&mut self, index: usize) -> bool {
        if index >= self.total {
            return false;
        }
        let stride = self.stride();
        let top = stride * index as f32;
        let bottom = top + self.config.item_height;
        let next = if top < self.offset {
            top
        } else if bottom > self.offset + self.viewport {
            bottom - self.viewport
        } else {
            return false;
        };
        let clamped = self.clamp_offset(next);
        let moved = (clamped - self.offset).abs() > f32::EPSILON;
        self.offset = clamped;
        moved
    }

    /// 含过扫的可见窗口，`end` 为开区间。
    pub fn window(&self) -> VirtualWindow {
        if self.total == 0 || self.viewport <= 0.0 {
            return VirtualWindow {
                start: 0,
                end: 0,
                offset: self.offset,
                total_height: self.total_height(),
            };
        }
        let stride = self.stride().max(0.001);
        let first = (self.offset / stride).floor() as usize;
        let visible = (self.viewport / stride).ceil() as usize + 1;
        let start = first.saturating_sub(self.config.overscan).min(self.total);
        let end = (first + visible + self.config.overscan).min(self.total);
        VirtualWindow {
            start,
            end,
            offset: self.offset,
            total_height: self.total_height(),
        }
    }

    /// 第 `index` 项的局部矩形 `[x=0, y, w=0, h=item_height]`（y 为内容坐标）。
    pub fn item_rect(&self, index: usize) -> [f32; 4] {
        [
            0.0,
            self.stride() * index as f32,
            0.0,
            self.config.item_height,
        ]
    }
}

/// 固定槽位池：槽位数不随总项数增长。key 未变时复用原槽位，避免重建控件状态。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotPool {
    slots: Vec<Option<String>>,
}

impl SlotPool {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: vec![None; capacity],
        }
    }

    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    pub fn bound(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    /// 只保留 `keys` 对应的槽位；新 key 优先复用空槽，不够才扩池。
    /// 返回与 `keys` 同序的槽位下标。
    pub fn bind(&mut self, keys: &[&str]) -> Vec<usize> {
        for slot in &mut self.slots {
            if let Some(key) = slot.as_deref()
                && !keys.contains(&key)
            {
                *slot = None;
            }
        }
        let mut assigned = Vec::with_capacity(keys.len());
        for key in keys {
            let existing = self
                .slots
                .iter()
                .position(|slot| slot.as_deref() == Some(*key));
            let index = match existing {
                Some(index) => index,
                None => {
                    let free = self.slots.iter().position(Option::is_none);
                    match free {
                        Some(index) => {
                            self.slots[index] = Some((*key).to_owned());
                            index
                        }
                        None => {
                            self.slots.push(Some((*key).to_owned()));
                            self.slots.len() - 1
                        }
                    }
                }
            };
            assigned.push(index);
        }
        assigned
    }
}

/// 一帧的绑定结果：屏幕项、稳定 key、槽位与局部矩形。
#[derive(Debug, Clone, PartialEq)]
pub struct BoundItem {
    pub index: usize,
    pub key: String,
    pub slot: usize,
    pub rect: [f32; 4],
}

pub fn bind_window(
    list: &VirtualList,
    window: VirtualWindow,
    keys: &[String],
    pool: &mut SlotPool,
) -> Vec<BoundItem> {
    let visible = keys
        .iter()
        .enumerate()
        .skip(window.start)
        .take(window.len())
        .map(|(index, key)| (index, key.as_str()))
        .collect::<Vec<_>>();
    let names = visible.iter().map(|(_, key)| *key).collect::<Vec<_>>();
    let slots = pool.bind(&names);
    visible
        .into_iter()
        .zip(slots)
        .map(|((index, key), slot)| BoundItem {
            index,
            key: key.to_owned(),
            slot,
            rect: list.item_rect(index),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousand_items_only_bind_a_bounded_window() {
        let list = VirtualList::new(VirtualListConfig::default(), 1000, 240.0);
        let window = list.window();
        assert_eq!(window.start, 0);
        assert!(window.len() <= 12 + 1 + 2 * 2, "window too big: {window:?}");
        assert_eq!(window.total_height, 24.0 * 1000.0);

        let keys = (0..1000)
            .map(|index| format!("item-{index}"))
            .collect::<Vec<_>>();
        let mut pool = SlotPool::default();
        let bound = bind_window(&list, window, &keys, &mut pool);
        assert_eq!(bound.len(), window.len());
        assert!(pool.capacity() <= window.len(), "槽位池不应随总项数增长");
        assert_eq!(bound[0].key, "item-0");
    }

    #[test]
    fn scrolling_reuses_slots_and_keeps_stable_keys() {
        let mut list = VirtualList::new(VirtualListConfig::default(), 1000, 240.0);
        let keys = (0..1000)
            .map(|index| format!("item-{index}"))
            .collect::<Vec<_>>();
        let mut pool = SlotPool::default();
        let first = bind_window(&list, list.window(), &keys, &mut pool);
        let item_slot = first
            .iter()
            .find(|item| item.key == "item-5")
            .map(|item| item.slot)
            .unwrap();
        list.scroll_by(72.0);
        let second = bind_window(&list, list.window(), &keys, &mut pool);
        let still = second.iter().find(|item| item.key == "item-5");
        assert_eq!(still.map(|item| item.slot), Some(item_slot));
        assert!(
            pool.capacity() <= list.window().len(),
            "槽位池应贴合当前窗口，不得随滚动无界增长"
        );
        assert_eq!(list.offset(), 72.0);
    }

    #[test]
    fn scroll_to_index_clamps_and_reports_movement() {
        let mut list = VirtualList::new(VirtualListConfig::default(), 1000, 240.0);
        assert!(!list.scroll_to_index(2), "已完整可见的项不应滚动");
        assert!(list.scroll_to_index(100), "视口外的项必须滚动");
        assert_eq!(list.offset(), 24.0 * 100.0 + 24.0 - 240.0);
        assert!(list.scroll_to_index(999));
        assert_eq!(list.offset(), 24.0 * 1000.0 - 240.0);
    }

    #[test]
    fn end_and_empty_are_safe() {
        let mut list = VirtualList::new(VirtualListConfig::default(), 10, 100.0);
        list.scroll_by(10_000.0);
        assert_eq!(list.offset(), 140.0);
        assert_eq!(list.window().end, 10);
        let mut empty = VirtualList::new(VirtualListConfig::default(), 0, 100.0);
        assert!(empty.window().is_empty());
        assert!(!empty.scroll_to_index(0));
        assert_eq!(empty.scroll_by(50.0), 0.0);
        empty.set_total(3);
        assert_eq!(empty.window().len(), 3);
    }

    /// 1000 项 × 1000 帧的滚动读数（`cargo test -- --nocapture` 可见）。
    /// 只断言结构性上界；时间读数用于基线，不做阈值断言（CI 机器间不可比）。
    #[test]
    fn thousand_item_scroll_frames_stay_bounded() {
        let viewport = 720.0;
        let mut list = VirtualList::new(VirtualListConfig::default(), 1000, viewport);
        let keys = (0..1000)
            .map(|index| format!("item-{index}"))
            .collect::<Vec<_>>();
        let mut pool = SlotPool::default();
        let frames = 1000usize;
        let start = std::time::Instant::now();
        let mut bound_total = 0usize;
        for _ in 0..frames {
            bound_total += bind_window(&list, list.window(), &keys, &mut pool).len();
            list.scroll_by(7.0);
            if list.offset() + viewport >= list.total_height() {
                list.scroll_to(0.0);
            }
        }
        let elapsed = start.elapsed();
        let per_frame = bound_total / frames;
        eprintln!(
            "1000 项虚拟列表：{frames} 帧，平均绑定 {per_frame} 项/帧，槽位池 {}，平均 {:?}/帧",
            pool.capacity(),
            elapsed / frames as u32
        );
        assert!(per_frame <= 40, "窗口项数失控：{per_frame}");
        assert!(pool.capacity() <= 40, "槽位池失控：{}", pool.capacity());
    }
}
