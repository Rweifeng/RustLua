//! 跨 crate 门：脚本销毁实体后，宿主侧那些**按实体记账**的旁路状态必须跟着消失。
//!
//! ## 为什么这条门不能放在单个 crate 里
//!
//! `entity.destroy` 的完整语义横跨三个 crate：
//!
//! 1. `rustlua-ecs`：`EntityState` 清理名字/血量/标签，并往
//!    `pending_physics_removals` 记一笔"这个实体的刚体要摘"；
//! 2. `rustlua-physics`：刚体本身住在 `PhysicsState`，脚本够不到它；
//! 3. 宿主 `physics_driver`：在推进物理前统一消费那张登记表。
//!
//! `rustlua-ecs` 里有一条 `destroy_bookkeeping_and_scene_transition`，但它测的是
//! **队列本身的语义**（入队 / 取走即清 / 切场景清空），全部由手工调用驱动。
//! 它证明不了最后一环：**真的调 `entity.destroy` 之后，物理世界里那个刚体是不是真的没了**。
//!
//! 这一环正是"倒置"式解耦最容易断的地方——ECS 只往队列里记一笔就返回，
//! 谁忘了消费、消费早了一步或晚了一步，队列测试全是绿的，而游戏里表现为
//! "怪物死掉了还挡路"。只有把三段接起来跑才有意义。
//!
//! ## 反过来也要测
//!
//! 只测"销毁后刚体少了"会漏掉另一种失败：**消费太勤**，把活着的也摘了。
//! 所以下面同时断言"只少一个"。

use engine::host::runtime_scene::RuntimeScene;
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::Scene;
use std::path::PathBuf;

fn project_root() -> PathBuf {
    let root = rustlua_infra::games_dir().join("examples/game_api");
    assert!(
        root.join("project.toml").is_file(),
        "演示工程不存在：{}",
        root.display()
    );
    root
}

fn boot(name: &str) -> RuntimeScene {
    let root = project_root();
    let scene = Scene::load(root.join("scenes").join(format!("{name}.scene"))).expect("加载场景");
    let mut runtime = RuntimeScene::from_scene(Some(&root), scene).expect("实例化场景");
    // 两帧：`on_create` 建实体、首帧物理把碰撞体铺出来。
    runtime
        .run_script_steps(2, 1.0 / 60.0, &InputSnapshot::default())
        .expect("首两帧步进");
    runtime
}

/// 2D 碰撞体数量（`collider_debug_shapes` 与刚体一一对应）。
fn collider_count(runtime: &RuntimeScene) -> usize {
    runtime.collider_debug_shapes().len()
}

/// 场景里的 `Farmer` 带 `Collider` 组件，是这三条门的目标实体。
const TARGET: &str = "Farmer";

/// 取目标实体的 Lua 句柄（十进制 id 字符串）。
fn target_handle(runtime: &mut RuntimeScene) -> String {
    let raw = runtime
        .run_lua_console(&format!("return entity.find('{TARGET}')"))
        .expect("entity.find 应可用");
    let raw = raw.trim().to_owned();
    assert_ne!(raw, "nil", "场景里应有名为 {TARGET} 的实体");
    raw
}

/// 脚本 `entity.destroy` → 宿主消费登记 → 物理碰撞体消失，且 ECS 账本同步。
#[test]
fn destroying_an_entity_closes_the_loop_through_physics_and_ecs() {
    let mut runtime = boot("api_gameplay");

    let handle = target_handle(&mut runtime);
    let before_colliders = collider_count(&runtime);
    assert!(
        before_colliders >= 1,
        "场景应当至少有一个 2D 碰撞体，实际 {before_colliders}"
    );
    let alive_before = runtime
        .run_lua_console(&format!("return entity.is_alive({handle})"))
        .expect("is_alive");
    assert_eq!(alive_before.trim(), "true", "销毁前目标应当活着");

    runtime
        .run_lua_console(&format!("entity.destroy({handle})"))
        .expect("entity.destroy 应被接受");
    // 一步：`step_physics` 在推进前消费 `pending_physics_removals`。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("销毁后步进");

    // ① ECS 账本：`is_alive` 变 false。
    let alive_after = runtime
        .run_lua_console(&format!("return entity.is_alive({handle})"))
        .expect("is_alive");
    assert_eq!(
        alive_after.trim(),
        "false",
        "销毁后 `entity.is_alive` 仍为 true——`entity.destroy` 的记账没生效"
    );

    // ② 物理账本：碰撞体少了。这是本门的关键一环——ECS 只登记，摘刚体是宿主的事。
    let after_colliders = collider_count(&runtime);
    assert!(
        after_colliders < before_colliders,
        "销毁后物理碰撞体数没有减少：{before_colliders} → {after_colliders}。\
         `pending_physics_removals` 没被消费，或消费发生在碰撞体建立之前"
    );
}

/// 消费不许"多摘"：销毁一个实体，别的实体的碰撞体必须原样还在。
///
/// 这条防的是另一种失败——`take_physics_removals` 取走整批后如果处理写错
/// （例如把列表当成"清空全部"），队列语义测试照样绿，但整个场景的碰撞都没了。
#[test]
fn destroying_one_entity_leaves_the_other_colliders_alone() {
    let mut runtime = boot("api_gameplay");

    let before = collider_count(&runtime);
    assert!(
        before >= 2,
        "api_gameplay 的碰撞体太少（{before}），无法验证误伤"
    );

    let handle = target_handle(&mut runtime);
    runtime
        .run_lua_console(&format!("entity.destroy({handle})"))
        .expect("entity.destroy 应被接受");
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("销毁后步进");

    let after = collider_count(&runtime);
    assert_eq!(
        after,
        before - 1,
        "只该少一个碰撞体：销毁前 {before}，销毁后 {after}。\
         少得更多说明 `take_physics_removals` 的处理把别的刚体也摘了"
    );
}

/// 反复销毁同一个实体：第二次必须幂等，不能连带影响别的刚体。
///
/// 这是"登记-消费"模型的真实风险：`queue_physics_removal` 无条件 `push`。
/// 如果调用方不设防，同一个 id 会进队两次；宿主两次 `remove_body`（第二次是 no-op，
/// 无害）——但若实现里混用了索引或计数器，就可能把**别的**实体当成"已摘"。
#[test]
fn destroying_the_same_entity_twice_does_not_disturb_other_bodies() {
    let mut runtime = boot("api_gameplay");
    let before = collider_count(&runtime);
    let handle = target_handle(&mut runtime);

    runtime
        .run_lua_console(&format!("entity.destroy({handle})"))
        .expect("第一次销毁");
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("第一次销毁后步进");
    let after_first = collider_count(&runtime);
    assert_eq!(after_first, before - 1, "第一次销毁应只少一个碰撞体");

    // 第二次：允许报错（更严格），也允许静默成功（幂等）——但不许影响别的刚体。
    let _ = runtime.run_lua_console(&format!("entity.destroy({handle})"));
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("第二次销毁后步进");
    let after_second = collider_count(&runtime);
    assert_eq!(
        after_second, after_first,
        "重复销毁后碰撞体数又变了：{after_first} → {after_second}。\
         第二次 destroy 误伤了别的实体"
    );
}
