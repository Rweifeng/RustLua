//! 世界数据域的能力单元（`docs/62` P1 ⑤-4c）：资源句柄、数据表缓存与两类 schema 校验器。
//!
//! ## 为什么在 `rustlua-gameplay`
//!
//! 这四个字段服务的 `asset` / `data` / `map.state` 绑定读取的是**工程数据**，而
//! `map` 的持久状态又必须经过 [`crate::save_store::SaveStore`] 落盘——数据表、地图状态与
//! 存档本来就是同一个域。⑤-4a/4b 把字段收进本结构并换成 [`Handle`]，⑤-4c 把绑定
//! （[`crate::world_data_lua`]）一起搬过来，`rustlua-scripting` 从此只持有句柄。
//!
//! 字段 `pub(crate)`：读写它的绑定与本结构同在一个 crate；宿主（scripting）只需要句柄与
//! [`WorldDataState::clear_for_scene_transition`]，不必逐字段转手。

use std::collections::HashMap;

use rustlua_lua_api::Handle;

/// 世界数据域的运行期状态。
#[derive(Debug, Default)]
pub struct WorldDataState {
    /// `asset.*` 登记的具名资源句柄（名字 → 工程内相对路径）。
    pub(crate) assets: HashMap<String, String>,
    /// `data.*` 读过的数据表缓存（按路径索引，解析后的 JSON）。
    pub(crate) data_cache: HashMap<String, serde_json::Value>,
    /// `data.schema(path, spec)` 注册的数据表校验器；场景切换时清空。
    pub(crate) schema_defs: HashMap<String, serde_json::Value>,
    /// `map.schema(name, spec)` 注册的分地图持久状态校验器（B-3）：按**地图名**索引，
    /// 且刻意跨过场景切换保留（玩家来回换图时校验规则不该丢）。
    pub(crate) map_state_schemas: HashMap<String, serde_json::Value>,
}

impl WorldDataState {
    /// 建一份能力单元（`what` 用于重入报错）。
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "world_data")
    }

    /// 场景切换：清空**数据表**校验器。
    ///
    /// 与旧 `ScriptState` 内联字段的语义逐字一致——`map_state_schemas` 刻意保留：
    /// 地图状态校验器按地图名注册，与"当前场景加载了哪些数据表"无关，来回换图时
    /// 丢掉它会让第二次进图的状态写入突然失去校验（B-3 的原始动机）。
    pub fn clear_for_scene_transition(&mut self) {
        self.schema_defs.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_transition_clears_data_schemas_but_keeps_map_schemas() {
        let mut state = WorldDataState::default();
        state.schema_defs.insert(
            "data/items.json".to_owned(),
            serde_json::json!({"type": "object"}),
        );
        state
            .map_state_schemas
            .insert("manor".to_owned(), serde_json::json!({"type": "object"}));
        state.clear_for_scene_transition();
        assert!(state.schema_defs.is_empty(), "数据表校验器随场景清空");
        assert_eq!(
            state.map_state_schemas.len(),
            1,
            "地图状态校验器必须跨场景保留（B-3）"
        );
    }

    #[test]
    fn handle_shares_state_with_the_host() {
        let handle = WorldDataState::handle();
        handle
            .borrow_mut()
            .unwrap()
            .assets
            .insert("hero".to_owned(), "asset:art/hero.png".to_owned());
        let clone = handle.clone();
        assert_eq!(
            clone
                .borrow()
                .unwrap()
                .assets
                .get("hero")
                .map(String::as_str),
            Some("asset:art/hero.png")
        );
    }
}
