//! schema 注册表与运行时组件名的**跨 crate 对拍**（WP11 拆分后唯一能同时看见两侧的落点）。
//!
//! 放在 engine 的集成测试而不是 `rustlua-ecs` 的单测里，是因为这条门要同时 import
//! `rustlua_ecs::component_schema` 与 `rustlua_scene::typed_scene`，而后者依赖前者：
//! 在 ecs 的测试里会出现两个版本的 `rustlua_ecs` 类型对不上。engine 依赖两侧且只有一份，
//! 因此这里是唯一能做机械化对拍的地方。

use engine::core::{component_schema::builtin_registry, typed_scene::ComponentId};

/// **运行时认识的每个组件名都必须有 schema**（第 39 轮）：机械化地防住"键名漂移"。
///
/// 逐个写断言挡不住历史上的第三例（`Particle` 那次就是漏了）——注册表描述了一个**没人消费**
/// 的名字时 schema 空转，真实工程里的组件被报"没有 schema"时实例失去校验。所以这里直接遍历
/// `ComponentId::ALL`：新增运行时组件却忘了注册 schema，这条立刻判红。
#[test]
fn every_component_id_the_runtime_knows_has_a_schema() {
    let registry = builtin_registry();
    let missing: Vec<&str> = ComponentId::ALL
        .iter()
        .map(|id| id.as_str())
        .filter(|name| !registry.contains_key(name))
        .collect();
    assert!(
        missing.is_empty(),
        "运行时认识这些组件名，但 schema 注册表里没有：{missing:?}"
    );
    // 名字还必须**唯一**：一个 `ComponentId` 一个名字（重复会让上面那条门形同虚设）。
    let mut names: Vec<&str> = ComponentId::ALL.iter().map(|id| id.as_str()).collect();
    names.sort_unstable();
    let unique = names.len();
    names.dedup();
    assert_eq!(names.len(), unique, "ComponentId::as_str 出现了重复名字");
    // 第 39 轮修掉的漂移名不得复活。
    assert!(
        !registry.contains_key("Particle"),
        "旧键名 `Particle` 不得复活：运行时读的是 `ParticleEmitter`"
    );
    assert!(registry.contains_key("ParticleEmitter"));
}
