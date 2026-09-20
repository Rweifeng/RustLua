//! 旧组件名的统一迁移入口（docs/55 §4.1：typed schema / 迁移 / 运行时**不漂移**）。
//!
//! 起因是一个真实缺陷：同一个"实体级脚本"概念在仓库里有**三个名字**——
//! 运行时与场景加载器读 `Script`（`file=scripts/x.lua`），编辑器 Inspector 写的是
//! `LuaScript`（**裸路径**），schema 注册表里叫 `EntityScript`。于是"Inspector 里填了
//! 脚本路径、运行时什么都不发生"，而且没有任何代码把旧名字翻译过来。
//!
//! 这个模块只做一件事：把场景里的旧写法改写成运行时真正读取的写法，**并且不猜**——
//! 旧负载解析不出来时原样保留、计入 `untouched`，绝不为了让报告好看而丢数据。
//!
//! 迁移是**幂等**的：第二次跑不会改动任何东西，也不会把 `migrated` 再数一遍。
//!
//! 与 [`crate::core::scene_migration`] 的分工：那个模块负责"空间语义迁移"这种**危险、会写盘**
//! 的迁移（dry-run / 备份 / 一键回滚）；这里是**打开场景时的内存内改名**，不写盘、不备份，
//! 由调用方决定"要不要落盘"（编辑器把改动标成未保存，用户保存后才生效）。

use crate::core::{
    entity_script::{EntityScript, SCRIPT_COMPONENT},
    scene::Scene,
};

/// 编辑器历史上写出的实体级脚本组件名（负载是裸路径）。
pub const LEGACY_LUA_SCRIPT_COMPONENT: &str = "LuaScript";

/// 一次迁移的结果，供编辑器显示与日志使用。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComponentMigrationReport {
    /// 已改写为新组件（含"裸路径 → `file=` 载荷"的转换）。
    pub migrated: usize,
    /// 新组件已存在于同一实体：保留新值、丢弃旧键。
    pub conflicts: usize,
    /// 旧负载无法解析：**原样保留**，不做任何猜测。
    pub untouched: usize,
}

impl ComponentMigrationReport {
    /// 有没有发生过任何需要用户知道的事。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// 处理过的旧组件总数。
    #[must_use]
    pub fn total(&self) -> usize {
        self.migrated + self.conflicts + self.untouched
    }

    /// 给状态栏/日志用的一句话。
    #[must_use]
    pub fn describe(&self) -> String {
        if self.is_empty() {
            return "没有需要迁移的旧组件".to_owned();
        }
        let mut parts = Vec::new();
        if self.migrated > 0 {
            parts.push(format!("迁移 {} 个", self.migrated));
        }
        if self.conflicts > 0 {
            parts.push(format!("保留新值 {} 个", self.conflicts));
        }
        if self.untouched > 0 {
            parts.push(format!("无法解析而原样保留 {} 个", self.untouched));
        }
        format!(
            "旧脚本组件 {LEGACY_LUA_SCRIPT_COMPONENT} → {SCRIPT_COMPONENT}：{}",
            parts.join("，")
        )
    }
}

/// 把 `LuaScript` 迁移成 `Script`（幂等）。
pub fn migrate_legacy_components(scene: &mut Scene) -> ComponentMigrationReport {
    let mut report = ComponentMigrationReport::default();
    for entity in scene.entities.values_mut() {
        let Some(legacy) = entity.components.remove(LEGACY_LUA_SCRIPT_COMPONENT) else {
            continue;
        };
        let Some(script) = parse_legacy_payload(&legacy) else {
            // 解析不出来：放回去、计数、不猜（场景内容一字不改）。
            report.untouched += 1;
            entity
                .components
                .insert(LEGACY_LUA_SCRIPT_COMPONENT.to_owned(), legacy);
            continue;
        };
        if entity.components.contains_key(SCRIPT_COMPONENT) {
            // 新组件已在场：保留它（它才是运行时读的），旧键丢弃。
            report.conflicts += 1;
            continue;
        }
        entity
            .components
            .insert(SCRIPT_COMPONENT.to_owned(), script.to_component_str());
        report.migrated += 1;
    }
    report
}

/// 旧负载有两种历史写法：`file=...`（与运行时同形）与**裸路径**（编辑器 Inspector 写的）。
/// 两者都能解析才迁移；其余一律当作不可解析。
fn parse_legacy_payload(payload: &str) -> Option<EntityScript> {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('=') {
        return EntityScript::from_component_str(trimmed).ok();
    }
    let script = EntityScript {
        file: trimmed.to_owned(),
        enabled: true,
    };
    script.validate().ok().map(|()| script)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scene::SceneEntityId;

    fn scene_with(components: &[(&str, &str)]) -> Scene {
        let mut scene = Scene::new("migration");
        scene.add_entity(SceneEntityId(1), "E", None).unwrap();
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        for (name, payload) in components {
            entity
                .components
                .insert((*name).to_owned(), (*payload).to_owned());
        }
        scene
    }

    #[test]
    fn bare_legacy_path_becomes_the_runtime_component() {
        let mut scene = scene_with(&[(LEGACY_LUA_SCRIPT_COMPONENT, "scripts/player.lua")]);
        let report = migrate_legacy_components(&mut scene);
        assert_eq!(report.migrated, 1);
        assert_eq!(report.conflicts, 0);
        assert_eq!(report.untouched, 0);
        let components = &scene.entities[&SceneEntityId(1)].components;
        assert_eq!(
            components.get(SCRIPT_COMPONENT).map(String::as_str),
            Some("file=scripts/player.lua"),
            "迁移后的载荷必须是运行时解析器认得的 `file=` 形式"
        );
        assert!(!components.contains_key(LEGACY_LUA_SCRIPT_COMPONENT));
        // 运行时解析器真的能读回来（这是"迁移不是换个名字了事"的证据）。
        let parsed = EntityScript::from_component_str(&components[SCRIPT_COMPONENT]).unwrap();
        assert_eq!(parsed.file, "scripts/player.lua");
        assert!(parsed.enabled);
    }

    #[test]
    fn legacy_file_form_keeps_enabled_flag() {
        let mut scene = scene_with(&[(LEGACY_LUA_SCRIPT_COMPONENT, "file=a.lua;enabled=false")]);
        let report = migrate_legacy_components(&mut scene);
        assert_eq!(report.migrated, 1);
        assert_eq!(
            scene.entities[&SceneEntityId(1)].components[SCRIPT_COMPONENT],
            "file=a.lua;enabled=false"
        );
    }

    #[test]
    fn existing_runtime_component_wins_and_legacy_key_is_dropped() {
        let mut scene = scene_with(&[
            (LEGACY_LUA_SCRIPT_COMPONENT, "scripts/old.lua"),
            (SCRIPT_COMPONENT, "file=scripts/new.lua"),
        ]);
        let report = migrate_legacy_components(&mut scene);
        assert_eq!(report.conflicts, 1);
        assert_eq!(report.migrated, 0);
        let components = &scene.entities[&SceneEntityId(1)].components;
        assert_eq!(components[SCRIPT_COMPONENT], "file=scripts/new.lua");
        assert!(!components.contains_key(LEGACY_LUA_SCRIPT_COMPONENT));
    }

    #[test]
    fn unparsable_legacy_payload_is_kept_untouched() {
        // 绝对路径与含 `..` 的路径都是**非法**脚本路径：迁移不能把它们"修好"，
        // 更不能丢掉——原样留着，让校验器/审计去报。
        for legacy in ["/etc/passwd", "../outside.lua", ""] {
            let mut scene = scene_with(&[(LEGACY_LUA_SCRIPT_COMPONENT, legacy)]);
            let report = migrate_legacy_components(&mut scene);
            assert_eq!(report.untouched, 1, "legacy={legacy:?}");
            assert_eq!(report.migrated, 0, "legacy={legacy:?}");
            let components = &scene.entities[&SceneEntityId(1)].components;
            assert_eq!(
                components.get(LEGACY_LUA_SCRIPT_COMPONENT).map(String::as_str),
                Some(legacy),
                "无法解析的旧负载必须原样保留"
            );
            assert!(!components.contains_key(SCRIPT_COMPONENT));
        }
    }

    #[test]
    fn migration_is_idempotent_and_silent_on_clean_scenes() {
        let mut clean = scene_with(&[("Transform", "x=0;y=0")]);
        let before = clean.clone();
        let report = migrate_legacy_components(&mut clean);
        assert!(report.is_empty());
        assert_eq!(clean, before, "没有旧组件的场景必须逐字段不变");

        let mut scene = scene_with(&[(LEGACY_LUA_SCRIPT_COMPONENT, "scripts/a.lua")]);
        let first = migrate_legacy_components(&mut scene);
        let after_first = scene.clone();
        let second = migrate_legacy_components(&mut scene);
        assert_eq!(first.migrated, 1);
        assert!(second.is_empty(), "第二次跑不该再迁移一遍");
        assert_eq!(scene, after_first, "幂等：第二次不改动任何东西");
    }

    #[test]
    fn migrated_scene_compiles_into_the_runtime_component_set() {
        // 端到端：旧场景 → 迁移 → **加载器真的把它编成 Script 组件**。
        // 这条把"改名"与"运行时会读到"两件事连起来，避免只测到字符串层面。
        use crate::core::typed_scene::{ComponentId, TypedScene};
        let mut scene = scene_with(&[(LEGACY_LUA_SCRIPT_COMPONENT, "scripts/ai/patrol.lua")]);
        let report = migrate_legacy_components(&mut scene);
        assert_eq!(report.migrated, 1);
        let typed = TypedScene::compile_v2(&scene).expect("typed compile");
        assert!(
            typed.entities[0].components.contains_key(&ComponentId::Script),
            "迁移后的组件必须被加载器编译成 Script"
        );
    }

    #[test]
    fn report_describes_what_happened() {
        let report = ComponentMigrationReport {
            migrated: 2,
            conflicts: 1,
            untouched: 3,
        };
        let text = report.describe();
        assert!(text.contains("迁移 2 个"), "{text}");
        assert!(text.contains("保留新值 1 个"), "{text}");
        assert!(text.contains("原样保留 3 个"), "{text}");
        assert_eq!(report.total(), 6);
        assert!(ComponentMigrationReport::default().describe().contains("没有需要迁移"));
    }
}
