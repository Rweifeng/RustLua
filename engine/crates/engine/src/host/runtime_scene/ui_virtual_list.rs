//! M76-07：场景侧虚拟列表运行时（行池 + 滚动锚定 + 复用）。
//!
//! 每个带 `UiVirtualList` 组件的世界实体 = 一个虚拟列表。宿主每帧在布局解算**之前**
//! （`ensure_scene_ui_layout` 内、`sync_prefab_ui_nodes` 之后）做一件事：按列表的滚动
//! 偏移与视口算出可见窗口（含过扫），把窗口内的行实体材料化进世界与 `ui_scene_nodes`，
//! 窗口外的行实体回收。纯窗口数学在 `rustlua_ui::ui_virtual_list`，本模块只做载体接线。
//!
//! 三个机制各守一条（docs/76 §3.7 的验收门）：
//! - **行池**：在世行实体数 = 窗口大小（`VirtualList::window`），与数据行总数无关；
//!   5000 行数据只材料化"可见行 ± 1 屏"。
//! - **滚动锚定**：行矩形是**内容坐标**（y = stride × index，左右贴列表宽），滚动位移
//!   由 `ScrollView` 那条既有路径叠加；行不参与"内容尺寸"计算——滚动范围由组件总高
//!   （`total_height`）覆盖，否则范围会随窗口漂移、滚到中部就被夹死。
//! - **复用**：行下标 → 实体句柄的映射跨帧稳定：窗口内同一下标不重建实体（内容与位置
//!   对同一下标是常量），滚出一行回收一行、滚进一行新建一行，每帧材料化数 ≤ 窗口边界；
//!   滚到中部再回顶部，同一行的内容与位置逐字节一致（不串位）。
//!
//! 行模板 = 列表实体在场景里的**直接子实体**（导出端要求恰好一个、无嵌套，v1 单实体
//! 模板）。运行时克隆模板的 `Image`/`Text`——`Text` 里的 `{n}` 占位符替换为行下标；
//! 行 `UiLayout` 由窗口锚定重建（模板自身的作者布局只用于导出端推导行高）。
//!
//! 边界：数据行总数来自组件的 `count` 字段（运行期可变，改组件即改数据量）。Lua 侧
//! `ui.virtual_list` 查询 API 走的是另一套状态（`UiState.advanced`），本模块不消费它；
//! 两条路径的合流留作后续（脚本写组件 = 换数据量）。

use std::collections::BTreeSet;

use rustlua_ecs::entity::EntityId;
use rustlua_ecs::hierarchy::Parent;
use rustlua_ui::ui::UiImage;
use rustlua_ui::ui::UiText;
use rustlua_ui::ui_scene_layout::UiSceneNode;
use rustlua_ui::ui_virtual_list::{
    UiVirtualList, VirtualList, VirtualListConfig, row_layout, substitute_row,
};

use super::RuntimeScene;

/// 本帧虚拟列表的材料化统计。测试按它断言验收门（每帧只材料化可见行 ± 1 屏、
/// 总实体数与数据行数解耦）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct VirtualListFrame {
    /// 本帧新建的行实体数（滚一屏以内的帧 ≤ 窗口边界）。
    pub spawned: usize,
    /// 本帧回收的行实体数。
    pub despawned: usize,
    /// 本帧全部虚拟列表在世的行实体总数（与数据行总数解耦的读数）。
    pub live_rows: usize,
}

/// 一帧内从世界读出的虚拟列表输入（模板组件在布局解算前克隆）。
struct VirtualListInput {
    slot: u64,
    /// 行模板子实体占用的槽位（装配趟跳过模板：模板不画，只当克隆源）。
    template_slot: Option<u64>,
    config: VirtualListConfig,
    count: usize,
    template_image: Option<UiImage>,
    template_text: Option<UiText>,
}

impl RuntimeScene {
    /// 每帧在布局解算前调用：按窗口材料化/回收全部虚拟列表的行实体。
    ///
    /// 有意做成不可失败（世界操作异常时记 warning 并跳过该列表）：`ensure_scene_ui_layout`
    /// 在绘制趟之前逐帧调用，行缺失一帧是可恢复的，不值得把整帧 UI 炸掉。
    pub(super) fn reconcile_virtual_lists(&mut self, screen_viewport: [f32; 2]) {
        let inputs = self.collect_virtual_list_inputs();
        let names = self.script.entity_names_by_slot();
        let mut frame = VirtualListFrame::default();
        let mut template_slots = BTreeSet::new();

        // 清掉已不在世的列表（切场景/实体销毁后残留的行实体必须回收）。
        let live_names: BTreeSet<String> = inputs
            .iter()
            .filter_map(|input| names.get(&input.slot).cloned())
            .collect();
        let stale: Vec<String> = self
            .ui_virtual_rows
            .keys()
            .filter(|name| !live_names.contains(*name))
            .cloned()
            .collect();
        for name in stale {
            self.drain_virtual_rows(&name, &mut frame);
            self.ui_virtual_signatures.remove(&name);
        }

        for input in &inputs {
            let Some(name) = names.get(&input.slot).cloned() else {
                // 没有登记名的列表没有滚动偏移可读（交互树按名写偏移）——跳过，不悄悄材料化。
                continue;
            };
            if let Some(template_slot) = input.template_slot {
                template_slots.insert(template_slot);
            }
            let viewport_h = self.virtual_list_viewport(input.slot, screen_viewport);
            let signature = (
                input.count,
                input.config.item_height,
                input.config.spacing,
                input.config.overscan,
            );
            let offset = self.ui_scroll_offsets.get(&name).copied().unwrap_or(0.0);
            let mut list_state = VirtualList::new(input.config, input.count, viewport_h);
            list_state.scroll_to(offset);
            let window = list_state.window();

            // 数据签名变了（count/行高/间距/过扫）⇒ 整池重建，而不是逐行比对。
            if self.ui_virtual_signatures.get(&name) != Some(&signature) {
                self.drain_virtual_rows(&name, &mut frame);
                self.ui_virtual_signatures.insert(name.clone(), signature);
            }

            // 先算清单（回收 + 新建），再执行：`spawn_virtual_row` 是 `&mut self`
            // 方法调用，而 `entry` 的借用会挡到它——分两段做，语义不变（窗口数学
            // 与计数同原实现，行实体回收/材料化的顺序也保持"先回收后新建"）。
            let (departed, to_spawn) = {
                let rows = self.ui_virtual_rows.entry(name.clone()).or_default();
                // 回收离开窗口的行（先摘节点再 despawn，避免同槽位的残留节点被新行读到）。
                let departed: Vec<(usize, EntityId)> = rows
                    .iter()
                    .filter(|(index, _)| **index < window.start || **index >= window.end)
                    .map(|(index, handle)| (*index, *handle))
                    .collect();
                let mut to_spawn = Vec::new();
                for index in window.start..window.end {
                    if !rows.contains_key(&index) {
                        to_spawn.push(index);
                    }
                }
                (departed, to_spawn)
            };
            for (index, handle) in departed {
                self.ui_virtual_row_slots.remove(&(handle.index() as u64));
                self.ui_scene_nodes.remove(&(handle.index() as u64));
                self.script
                    .unregister_entity_name(&format!("{name}.Row{index}"), handle);
                let removed = self.script.with_world_mut(|world| {
                    world.despawn(handle).map_err(|error| error.to_string())
                });
                if let Err(error) = removed {
                    tracing::warn!(target: "ui", list = %name, index, "[virtual list] 行实体回收失败: {error}");
                }
                if let Some(rows) = self.ui_virtual_rows.get_mut(&name) {
                    rows.remove(&index);
                }
                frame.despawned += 1;
            }
            // 材料化进入窗口的行。
            for index in to_spawn {
                match self.spawn_virtual_row(&name, input, index) {
                    Ok(handle) => {
                        if let Some(rows) = self.ui_virtual_rows.get_mut(&name) {
                            rows.insert(index, handle);
                        }
                        frame.spawned += 1;
                    }
                    Err(error) => {
                        tracing::warn!(target: "ui", list = %name, index, "[virtual list] 行实体材料化失败: {error}");
                    }
                }
            }
            frame.live_rows += self
                .ui_virtual_rows
                .get(&name)
                .map_or(0, std::collections::BTreeMap::len);
        }
        self.ui_virtual_template_slots = template_slots;
        self.ui_virtual_frame = frame;
    }

    /// 从世界读出全部虚拟列表的配置与行模板组件（一次只读借用）。
    fn collect_virtual_list_inputs(&self) -> Vec<VirtualListInput> {
        self.script.with_world(|world| {
            let parents: Vec<(EntityId, EntityId)> = {
                let mut out = Vec::new();
                world.for_each::<Parent>(|id, parent| out.push((id, parent.0)));
                out
            };
            let mut inputs = Vec::new();
            world.for_each::<UiVirtualList>(|id, list| {
                let template = parents
                    .iter()
                    .find(|(_, parent)| *parent == id)
                    .map(|(child, _)| *child);
                let template_slot = template.map(|entity| entity.index() as u64);
                let template_image = template
                    .and_then(|entity| world.get::<UiImage>(entity).ok().flatten().cloned());
                let template_text =
                    template.and_then(|entity| world.get::<UiText>(entity).ok().flatten().cloned());
                inputs.push(VirtualListInput {
                    slot: id.index() as u64,
                    template_slot,
                    config: list.config,
                    count: list.count,
                    template_image,
                    template_text,
                });
            });
            inputs
        })
    }

    /// 列表视口高：上一帧已解算的列表矩形优先（行不改变列表自身矩形），
    /// 首帧退回作者布局直接 resolve。
    fn virtual_list_viewport(&self, slot: u64, screen_viewport: [f32; 2]) -> f32 {
        if let Some(rect) = self.ui_layout_rects.get(&slot) {
            return rect.rect.h;
        }
        self.ui_scene_nodes
            .get(&slot)
            .and_then(|node| node.layout.as_ref())
            .map(|layout| layout.resolve(screen_viewport).h)
            .unwrap_or(0.0)
    }

    /// 新建一个窗口内的行实体：spawn 世界实体（模板 Image/Text + 锚定 UiLayout）、
    /// 登记行名 `{列表名}.Row{index}`、并表 `ui_scene_nodes`（键 = 活实体槽位，parent = 列表槽位）。
    fn spawn_virtual_row(
        &mut self,
        name: &str,
        input: &VirtualListInput,
        index: usize,
    ) -> Result<EntityId, String> {
        let layout = row_layout(input.config.item_height, input.config.spacing, index);
        let handle = self.script.with_world_mut(|world| {
            let id = world.spawn();
            world
                .insert(id, layout.clone())
                .map_err(|error| error.to_string())?;
            if let Some(text) = &input.template_text {
                let mut text = text.clone();
                text.text = substitute_row(&text.text, index);
                world.insert(id, text).map_err(|error| error.to_string())?;
            }
            if let Some(image) = &input.template_image {
                world
                    .insert(id, image.clone())
                    .map_err(|error| error.to_string())?;
            }
            Ok::<EntityId, String>(id)
        })?;
        let slot = handle.index() as u64;
        self.script
            .register_entity_name(handle, &format!("{name}.Row{index}"));
        self.ui_scene_nodes.insert(
            slot,
            UiSceneNode {
                parent: Some(input.slot),
                layout: Some(layout),
                container: None,
                layout_item: None,
                preferred_size: [0.0, 0.0],
                mask: None,
                canvas_group: None,
            },
        );
        self.ui_virtual_row_slots.insert(slot);
        Ok(handle)
    }

    /// 清空一个列表的全部行实体（切数据签名/列表消亡时）。
    fn drain_virtual_rows(&mut self, name: &str, frame: &mut VirtualListFrame) {
        let Some(rows) = self.ui_virtual_rows.remove(name) else {
            return;
        };
        for (index, handle) in rows {
            self.ui_virtual_row_slots.remove(&(handle.index() as u64));
            self.ui_scene_nodes.remove(&(handle.index() as u64));
            // 与逐行回收同一条：名字表不清会在槽位复用后把新实体认成旧行。
            self.script
                .unregister_entity_name(&format!("{name}.Row{index}"), handle);
            let removed = self
                .script
                .with_world_mut(|world| world.despawn(handle).map_err(|error| error.to_string()));
            if let Err(error) = removed {
                tracing::warn!(target: "ui", list = name, index, "[virtual list] 行实体回收失败: {error}");
            }
            frame.despawned += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_stats_default_to_zero() {
        let frame = VirtualListFrame::default();
        assert_eq!(frame.spawned, 0);
        assert_eq!(frame.despawned, 0);
        assert_eq!(frame.live_rows, 0);
    }
}
