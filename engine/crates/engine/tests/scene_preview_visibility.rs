//! 跨 crate 门：编辑器拖拽预览写进去的位姿，脚本侧必须**立刻读得到**。
//!
//! ## 这条门补的是什么
//!
//! `RuntimeScene::set_scene_entity_transform_preview` 是编辑器**每一次鼠标移动**都会调的
//! 热路径（`editor/src/scene_drag.rs`）：它把作者的场景实体 id（`SceneEntityId`）翻成
//! 运行期 ECS 句柄（`EntityId`），再就地改 `Transform`。整条链上有三处可以静静出错：
//!
//! 1. `scene_entities` 映射表里没有这个 id → 函数返回 `false`；
//! 2. 句柄在表里但世界里没有 `Transform` 组件 → 同样返回 `false`；
//! 3. 写进去了，但脚本侧读的是**另一个** `World`（两份状态）→ 返回值 `true`，脚本看不到。
//!
//! 第 3 种最恶劣：它不报错、不返回 false，只是"拖了没反应"。前两种至少返回了 `false`，
//! 但编辑器那边**丢弃了这个返回值**（`scene_drag.rs` 里 `for` 循环直接调用、不检查）。
//! 所以整条路径目前是"出错也无人知晓"。
//!
//! ## 为什么必须跨 crate
//!
//! `set_scene_entity_transform_preview` 住在 `engine`，`scene_entities` 表住在
//! `rustlua-ecs` 的 `EntityState`，而"脚本能读到"这件事只有把 VM 真的跑起来才知道。
//! 三个 crate 的状态对上了，用户的拖动才会动。

use engine::host::runtime_scene::RuntimeScene;
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::Scene;
use rustlua_scene::scene::SceneEntityId;
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

fn boot() -> RuntimeScene {
    let root = project_root();
    let scene = Scene::load(root.join("scenes/api_gameplay.scene")).expect("加载 api_gameplay");
    let mut runtime = RuntimeScene::from_scene(Some(&root), scene).expect("实例化 api_gameplay");
    runtime
        .run_script_steps(2, 1.0 / 60.0, &InputSnapshot::default())
        .expect("首两帧步进");
    runtime
}

/// 按**名字**读一批实体的位置（`(名字, x, y)` 三元组，排序后返回）。
///
/// 为什么不按键枚举世界：宿主侧 `scene_entity_handles` 返回的是 `encode_handle` 编码
/// （`index << 32 | generation`），而 Lua 侧 `entity.*` 收的是 `EntityId::to_lua_i64()`
/// —— **两套编码不同**，拿宿主编码去喂 Lua 只会读出 `nil`。
/// 名字是两边唯一的公共口径（`entity.find(name)`）。
///
/// 注意 `transform.get` 返回的是**一张表**（`{space,x,y,rotation,scale_x,scale_y}`），
/// 不是 `x, y` 两个返回值。
fn positions_of(runtime: &mut RuntimeScene, names: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for name in names {
        let raw = runtime
            .run_lua_console(&format!(
                "local e = entity.find('{name}')\n\
                 if not e then return 'gone' end\n\
                 local t = transform.get(e)\n\
                 if not t then return 'no-transform' end\n\
                 return string.format('{name}=%.3f,%.3f', t.x, t.y)"
            ))
            .unwrap_or_else(|error| panic!("读 `{name}` 位置失败：{error}"));
        out.push(raw.trim().to_owned());
    }
    out.sort();
    out
}

/// 有多少个具名实体恰好落在 `(x, y)`（容差 0.01）。
fn count_at_position(runtime: &mut RuntimeScene, names: &[String], x: f32, y: f32) -> usize {
    positions_of(runtime, names)
        .iter()
        .filter(|line| {
            line.rsplit('=')
                .next()
                .and_then(|pair| {
                    let mut it = pair.split(',');
                    let px: f32 = it.next()?.parse().ok()?;
                    let py: f32 = it.next()?.parse().ok()?;
                    Some((px - x).abs() < 0.01 && (py - y).abs() < 0.01)
                })
                .unwrap_or(false)
        })
        .count()
}

/// 场景里第一个有 `Transform` 的实体 id（作者侧 id，不是运行期句柄）。
fn first_scene_entity(runtime: &RuntimeScene) -> SceneEntityId {
    let mut handles = runtime.scene_entity_handles();
    handles.sort_by_key(|(id, _)| format!("{id:?}"));
    handles
        .first()
        .map(|(id, _)| *id)
        .expect("api_gameplay 应当至少实例化了一个场景实体")
}

/// 拖拽预览写进去的位置，脚本侧必须**读得到**同一组值。
///
/// 这里刻意不去比"某个特定句柄"：宿主侧 `RuntimeScene::scene_entity_handles` 返回的是
/// `encode_handle` 的编码（`index << 32 | generation`），而 Lua 侧 `entity.*` 用的是
/// `EntityId::to_lua_i64()` ——**两套编码不同**，跨过去比句柄只会比错对象。
///
/// 改为按**位置**反查：把目标挪到一个几乎不可能撞上的坐标，然后问脚本"有没有实体在那儿"。
/// 这同时覆盖了最初的第三种失败（预览写进另一个 `World`）：那种情况下脚本世界里
/// 一个实体都不会出现这个坐标。
#[test]
fn a_drag_preview_is_visible_to_lua_immediately() {
    let mut runtime = boot();
    let scene_id = first_scene_entity(&runtime);

    // 一组非整十、且不可能与场景默认值撞上的坐标。
    let (want_x, want_y) = (137.5_f32, 41.25_f32);
    let applied = runtime.set_scene_entity_transform_preview(
        scene_id,
        rustlua_math::math2d::Vec2::new(want_x, want_y),
        0.0,
        rustlua_math::math2d::Vec2::new(1.0, 1.0),
    );
    assert!(
        applied,
        "预览写入应当成功（实体 {scene_id:?} 有 Transform 且在映射表里）"
    );

    let names: Vec<String> = runtime.named_entity_names();
    assert!(!names.is_empty(), "场景应当有具名实体");
    let hits = count_at_position(&mut runtime, &names, want_x, want_y);

    assert_eq!(
        hits, 1,
        "脚本侧应当恰好有一个实体落在预览坐标 ({want_x}, {want_y})，实际 {hits}。\
         0 说明预览写进了另一个 World（编辑器与脚本看到的不是同一份状态）；\
         >1 说明有别的实体被一起动了"
    );
}

/// 不存在的场景实体必须返回 `false`，而不是 panic 或静默成功。
///
/// 编辑器那边目前**不检查这个返回值**，所以这里的契约就是唯一防线：
/// 如果它 panic，编辑器会在用户拖到一个已删除实体时整个崩掉；
/// 如果它静默返回 `true`，编辑器会以为写成功了。
#[test]
fn preview_of_an_unknown_scene_entity_reports_false() {
    let mut runtime = boot();

    // 一个几乎不可能被分配的 id。
    let bogus = SceneEntityId(u64::MAX);
    let applied = runtime.set_scene_entity_transform_preview(
        bogus,
        rustlua_math::math2d::Vec2::new(1.0, 2.0),
        0.0,
        rustlua_math::math2d::Vec2::new(1.0, 1.0),
    );
    assert!(
        !applied,
        "对不存在的场景实体写预览应当返回 false（否则编辑器以为写成功了）"
    );
}

/// 预览写入只动**一个**实体，别的实体的位置必须原样。
///
/// 这条防的是"映射表错位"：`scene_entities` 是 `BTreeMap<SceneEntityId, EntityId>`，
/// 如果某个环节把两者搞混（例如按索引而不是按 id 查），预览会写到**相邻实体**身上——
/// 表现为"拖动 A，B 跟着动"。这类错误在返回值上完全看不出来（`true`，因为确实写成功了）。
///
/// 判据用"位置快照"而不是句柄：把全场景位置收成一个排序后拼起来的字符串，
/// 预览前后对比，**只允许差一行**（就是被拖的那个）。
#[test]
fn a_drag_preview_moves_exactly_one_entity() {
    let mut runtime = boot();
    let mut handles = runtime.scene_entity_handles();
    handles.sort_by_key(|(id, _)| format!("{id:?}"));
    assert!(
        handles.len() >= 2,
        "api_gameplay 应当至少有两个场景实体，实际 {}",
        handles.len()
    );

    let target_id = handles[0].0;

    // 全场景位置快照（排序后拼接，位置唯一标识一个实体）。
    let names: Vec<String> = runtime.named_entity_names();
    assert!(!names.is_empty(), "场景应当有具名实体");
    let snapshot = |runtime: &mut RuntimeScene| -> Vec<String> { positions_of(runtime, &names) };

    let before = snapshot(&mut runtime);
    let applied = runtime.set_scene_entity_transform_preview(
        target_id,
        rustlua_math::math2d::Vec2::new(777.0, 888.0),
        0.0,
        rustlua_math::math2d::Vec2::new(1.0, 1.0),
    );
    assert!(applied, "预览写入应当成功");
    let after = snapshot(&mut runtime);

    assert_eq!(
        after.len(),
        before.len(),
        "实体数量变了：{} → {}（预览不该增删实体）",
        before.len(),
        after.len()
    );
    let changed: Vec<_> = before
        .iter()
        .zip(after.iter())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(
        changed.len(),
        1,
        "预览应当只移动一个实体，实际有 {} 个位置变了：{changed:?}。\
         多于一个说明 `scene_entities` 的 id → 句柄映射错位（表现为\"拖 A，B 跟着动\"）",
        changed.len()
    );
}
