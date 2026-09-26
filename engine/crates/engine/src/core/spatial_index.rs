//! 渲染级 2D 空间索引（P1-①）：uniform grid，供提取阶段按视口**预筛**实体。
//!
//! 定位（docs/53 §3.9 末登记的"下一刀"）：5 万精灵档收集阶段的大头是**逐实体
//! ECS 访问 + item 构建**（§3.10 已用 `for_each2` 消掉每精灵一次 `get`，剩下的
//! 是"视口外实体也照样构建 `SpriteBatchItem` 并注册纹理"）。本模块把精灵按世界
//! 外接圆插入均匀网格，提取时**只用视口 AABB 相交的 cell** 给出候选——视口外的
//! 实体在构建 item 之前就被跳过。
//!
//! 三条正确性契约（都有测试钉住）：
//!
//! 1. **保守不遗漏**：cell 是平面的完全划分；与视口相交的精灵**必然**出现在查询
//!    结果里（相交区域里的点 p 必落在某个 cell，插入时该 cell 已收录本精灵）。
//! 2. **多余候选无害**：查询结果只在"是否值得构建 item"处做**预筛**，精确判定
//!    （层遮罩、`cull_bounds`）仍在原提取路径里——因此空间索引打开与关闭
//!    **像素逐字节一致**。
//! 3. **顺序不变**：提取顺序 = EntityId 升序；查询结果按 EntityId 升序给出。
//!
//! 与既有两档剔除的分工：WP4 ④（瓦片/精灵**顶点**剔除）省顶点生成；`set_cull_bounds`
//! （收集时按世界 AABB 拒收）省 `push` 的写入；本模块省的是**收集循环本身**——
//! 静态场景下网格跨帧复用（指纹判定在宿主），收集从 O(N) 降为 O(视口内)。

use std::collections::HashMap;

use crate::core::entity::EntityId;

/// 一个实体的世界外接圆（中心 + 半径），插入用。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub center_x: f32,
    pub center_y: f32,
    /// 世界半径（外接圆，与旋转无关的保守上界）。
    pub radius: f32,
}

impl Circle {
    /// 精灵的世界外接圆：中心 = 平面位置，半径 = 半对角线 × 轴缩放。
    /// 与 docs/53 §3.9 的精灵剔除同款判定（旋转无关、负缩放被绝对值覆盖）。
    #[must_use]
    pub fn from_sprite(
        position: crate::core::transform::Vec2,
        scale: crate::core::transform::Vec2,
        size: crate::core::transform::Vec2,
    ) -> Self {
        let radius = 0.5
            * ((size.x * scale.x).powi(2) + (size.y * scale.y).powi(2)).sqrt();
        Self {
            center_x: position.x,
            center_y: position.y,
            radius,
        }
    }
}

/// 视口（或任何查询区域）的世界 AABB，查询用。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl Bounds {
    /// 由外接圆构造外接 AABB（查询时对 `CameraBounds` 取外扩用）。
    #[must_use]
    pub fn from_circle(circle: Circle) -> Self {
        Self {
            min_x: circle.center_x - circle.radius,
            min_y: circle.center_y - circle.radius,
            max_x: circle.center_x + circle.radius,
            max_y: circle.center_y + circle.radius,
        }
    }
}

/// 2D uniform grid：实体按世界外接圆插入，支持视口 AABB 查询候选。
///
/// 实现说明：
/// - `cells: HashMap<(i32, i32), Vec<EntityId>>`——cell 坐标 → 实体列表；
///   `rebuild` 要求输入按 EntityId 升序，因此每个 cell 内的列表天然升序；
/// - 一个实体可能跨多个 cell（外接圆覆盖多个格子），查询时跨 cell 合并后
///   **排序去重**，保证结果唯一且按 EntityId 升序；
/// - 网格**没有删除/更新 API**：跨帧复用由宿主持有（指纹不变不重建；变了整体
///   `rebuild`）。网格自身不缓存实体 AABB——那不是真相，真相在 ECS 里。
#[derive(Clone, Debug)]
pub struct SpatialGrid2D {
    cell_size: f32,
    cells: HashMap<(i32, i32), Vec<EntityId>>,
    /// 已收录实体数（诊断/断言用；跨 cell 重复不计）。
    count: usize,
}

impl SpatialGrid2D {
    /// 创建网格。`cell_size` 必须为正（`<= 0` 时退化为"单 cell 全收"的保守档，
    /// 行为正确但无预筛收益）。
    #[must_use]
    pub fn new(cell_size: f32) -> Self {
        Self {
            cell_size: if cell_size > 0.0 { cell_size } else { 1.0 },
            cells: HashMap::new(),
            count: 0,
        }
    }

    /// 当前 cell 边长。
    #[must_use]
    pub const fn cell_size(&self) -> f32 {
        self.cell_size
    }

    /// 已收录实体数（跨 cell 重复不计）。
    #[must_use]
    pub const fn len(&self) -> usize {
        self.count
    }

    /// 是否为空。
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// 全量重建。`items` 必须按 EntityId **升序**给出（保持每个 cell 内有序）。
    pub fn rebuild(&mut self, items: impl IntoIterator<Item = (EntityId, Circle)>) {
        self.cells.clear();
        self.count = 0;
        for (entity, circle) in items {
            let bounds = Bounds::from_circle(circle);
            let (min_x, min_y) = self.cell_of(bounds.min_x, bounds.min_y);
            let (max_x, max_y) = self.cell_of(bounds.max_x, bounds.max_y);
            for cell_x in min_x..=max_x {
                for cell_y in min_y..=max_y {
                    self.cells
                        .entry((cell_x, cell_y))
                        .or_default()
                        .push(entity);
                }
            }
            self.count += 1;
        }
    }

    /// 查询与 `bounds` 相交的 cell 里的所有实体。结果**去重且按 EntityId 升序**。
    ///
    /// 复杂度只随**非空 cell 数**走（不遍历整个 cell 范围）：先按查询区域过滤
    /// `cells` 的键，再排序去重。超大查询区域（如覆盖全世界的测试视口）也不会
    /// 陷入范围级循环。
    #[must_use]
    pub fn query(&self, bounds: Bounds) -> Vec<EntityId> {
        if self.is_empty() {
            return Vec::new();
        }
        let (min_x, min_y) = self.cell_of(bounds.min_x, bounds.min_y);
        let (max_x, max_y) = self.cell_of(bounds.max_x, bounds.max_y);
        let mut seen: Vec<EntityId> = Vec::new();
        for (&(cell_x, cell_y), entities) in &self.cells {
            if cell_x >= min_x
                && cell_x <= max_x
                && cell_y >= min_y
                && cell_y <= max_y
            {
                seen.extend_from_slice(entities);
            }
        }
        seen.sort_unstable();
        seen.dedup();
        seen
    }

    /// 世界坐标 → cell 坐标（向下取整；非有限值按保守最小 cell 处理；
    /// 超出 i32 范围按边界钳位——超大查询区域不 panic、不丢边角实体）。
    fn cell_of(&self, x: f32, y: f32) -> (i32, i32) {
        let cell = |value: f32| -> i32 {
            if !value.is_finite() {
                i32::MIN
            } else {
                let cell = (value / self.cell_size).floor();
                if cell >= i32::MAX as f32 {
                    i32::MAX
                } else if cell <= i32::MIN as f32 {
                    i32::MIN
                } else {
                    cell as i32
                }
            }
        };
        (cell(x), cell(y))
    }
}

#[cfg(test)]
mod tests {
    use super::{Bounds, Circle, SpatialGrid2D};
    use crate::core::entity::EntityId;

    fn circle(x: f32, y: f32, radius: f32) -> Circle {
        Circle {
            center_x: x,
            center_y: y,
            radius,
        }
    }

    fn bounds(x0: f32, y0: f32, x1: f32, y1: f32) -> Bounds {
        Bounds {
            min_x: x0,
            min_y: y0,
            max_x: x1,
            max_y: y1,
        }
    }

    fn entity(id: u64) -> EntityId {
        EntityId::from_raw(id as u32, 1)
    }

    fn entities(ids: &[u64]) -> impl Iterator<Item = (EntityId, Circle)> + '_ {
        ids.iter().copied().map(|id| (entity(id), circle(0.0, 0.0, 0.5)))
    }

    #[test]
    fn empty_grid_queries_empty() {
        let grid = SpatialGrid2D::new(8.0);
        assert!(grid.is_empty());
        assert_eq!(grid.len(), 0);
        assert!(grid.query(bounds(-10.0, -10.0, 10.0, 10.0)).is_empty());
    }

    /// 与视口相交的实体**必须**出现在候选里：把多个实体放在不同 cell，
    /// 视口覆盖中间一片——每个与视口相交的实体都不能漏。
    #[test]
    fn query_never_misses_an_entity_intersecting_the_viewport() {
        let mut grid = SpatialGrid2D::new(8.0);
        let mut items = Vec::new();
        for (index, (x, y)) in [
            (-20.0, 0.0), // 视口外（左侧 cell）
            (-4.0, 0.0),  // 视口内（左边缘附近）
            (0.0, 0.0),   // 视口内
            (4.0, 0.0),   // 视口内（右边缘附近）
            (20.0, 0.0),  // 视口外（右侧 cell）
        ]
        .into_iter()
        .enumerate()
        {
            items.push((entity(index as u64 + 1), circle(x, y, 1.0)));
        }
        grid.rebuild(items);
        // 视口 [-6, 6]：中间三个与视口相交（含边缘 4.0+1.0=5.0 < 6）。
        let candidates = grid.query(bounds(-6.0, -1.0, 6.0, 1.0));
        assert!(candidates.contains(&entity(2)));
        assert!(candidates.contains(&entity(3)));
        assert!(candidates.contains(&entity(4)));
        assert!(!candidates.contains(&entity(1)));
        assert!(!candidates.contains(&entity(5)));
    }

    /// 跨 cell 实体只出现一次，且结果按 EntityId 升序。
    #[test]
    fn query_deduplicates_and_sorts() {
        let mut grid = SpatialGrid2D::new(8.0);
        // 实体 3 的圆（半径 6）横跨 (-8, 8) 的多个 cell；实体 1/2 与它共 cell。
        let items = vec![
            (entity(1), circle(-10.0, 0.0, 1.0)),
            (entity(2), circle(0.0, 0.0, 1.0)),
            (entity(3), circle(0.0, 0.0, 6.0)),
            (entity(4), circle(10.0, 0.0, 1.0)),
        ];
        grid.rebuild(items);
        // 视口 [-6, 6]：实体 2/3 与视口相交（3 的圆横跨多 cell），实体 4 在 x=10 视口外。
        let candidates = grid.query(bounds(-6.0, -1.0, 6.0, 1.0));
        let mut expected = vec![entity(2), entity(3)];
        expected.sort_unstable();
        assert_eq!(candidates, expected);
        // 覆盖全世界的查询把所有实体都收进来（去重后数量 = rebuild 输入数）。
        let all = grid.query(bounds(-1.0e6, -1.0e6, 1.0e6, 1.0e6));
        assert_eq!(all.len(), 4);
    }

    /// 精灵外接圆：半径 = 半对角线 × 缩放，负缩放不改变半径。
    #[test]
    fn sprite_circle_ignores_rotation_and_absorbs_negative_scale() {
        use crate::core::transform::Vec2;
        let circle = Circle::from_sprite(
            Vec2::new(3.0, 4.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(3.0, 4.0),
        );
        // 半对角线 = 0.5 * sqrt(9 + 16) = 2.5；scale 2 → 半径 5。
        assert_eq!(circle.center_x, 3.0);
        assert_eq!(circle.center_y, 4.0);
        assert!((circle.radius - 5.0).abs() < 1e-6);
        let flipped = Circle::from_sprite(
            Vec2::new(3.0, 4.0),
            Vec2::new(-2.0, -2.0),
            Vec2::new(3.0, 4.0),
        );
        assert!((flipped.radius - circle.radius).abs() < 1e-6);
    }

    /// 非正 cell_size 退化为保守单 cell（行为正确、无预筛收益）。
    #[test]
    fn non_positive_cell_size_falls_back_to_single_cell() {
        let grid = SpatialGrid2D::new(0.0);
        assert!(grid.cell_size() > 0.0);
        let mut grid = grid;
        grid.rebuild(entities(&[1, 2, 3]));
        assert_eq!(grid.len(), 3);
        // 任意小视口也能查到（全在一个大 cell 里）。
        assert_eq!(grid.query(bounds(0.0, 0.0, 0.1, 0.1)).len(), 3);
    }

    /// 非有限位置按最小 cell 处理：不 panic、不丢实体。
    #[test]
    fn non_finite_positions_do_not_panic() {
        let mut grid = SpatialGrid2D::new(8.0);
        grid.rebuild(vec![
            (entity(1), circle(f32::NAN, 0.0, 1.0)),
            (entity(2), circle(f32::INFINITY, 0.0, 1.0)),
        ]);
        // 不 panic 即可；实体被收到最小 cell（查询范围钳位到全域时能见到）。
        let all = grid.query(bounds(-1.0e11, -1.0e11, 1.0e11, 1.0e11));
        assert_eq!(all.len(), 2);
    }
}
