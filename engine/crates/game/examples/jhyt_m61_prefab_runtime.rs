//! Headless acceptance drive for M61：`.prefab` 作为 UI 预设的**运行时载体**（引擎桥）。
//!
//! 上一批（M59）把对话版面搬进 `assets/ui/prefabs/*.ui.json`，由 Lua 逐节点调
//! `ui.panel/ui.text` 画——版面进了数据文件，但编辑器读不懂那份数据：Scene 视图认的是
//! 引擎 `.prefab` 实体树（`UiLayout`/`Text`/`Image`/`UiEvents` 组件），双击 `.prefab`
//! 才有所见即所得。这一批把载体换成 `.prefab`，于是"在 Scene 视图里拖"与"运行期一次
//! `prefab.spawn`"用的是同一份文件、同一套组件字符串。
//!
//! 这道门守的是那条链真的通到底：
//!   ① `prefab.spawn` 走 VFS 读文件（三个宿主唯一可读的通路）、解析实体树、把组件落进
//!      **活世界**（不是编辑侧的 `Scene`）；`prefab.root/node` 与 `entity.find`（名字登记表）同源；
//!      坏数据（悬空 `parent`、文件不存在）在加载期报错并点名文件，不是静返回 nil；
//!   ② **画得出来**：宿主本帧真正用于绘制场景 UI 的矩形（`RuntimeScene::prefab_ui_rects`
//!      读的就是 `ui_layout_rects` 那份）逐值等于预设里的 `UiLayout`；子节点矩形 = 父矩形
//!      原点 + 自身偏移（spawn 落的 `Parent` 组件与布局父链都对得上）；**没有 `UiLayout`
//!      的 UI 实体不参与布局解算 ⇒ 不会被画**（这条作者规则要有读数，否则"少一张图"无从解释）；
//!   ③ **点得中**：合成左键落在按钮矩形内 → `UiEvents` 的 Lua 回调按实体名收到；落在矩形外
//!      → 收不到（命中框与绘制框同源，见 `compute_click_targets`）；
//!   ④ **写得进**：`prefab.set_text` 改的是渲染读的那份 `Text` 组件本身，不是副本；给没有
//!      `Text` 的节点、不存在的节点/句柄改字都要**报错**，不能静默 no-op；
//!   ⑤ **收得干净**：`prefab.despawn` 之后实体不在、布局节点也撤了、点击不再派发；
//!      **别批不受影响**——M63b 起门面自己的 UI 也是 spawn 出来的（`assets/prefabs/title_menu.prefab`），
//!      同一个世界里因此住着两批实例，本门的两条读数是"本批归零"与"别批逐值不变"（并表/摘表
//!      若按 index 段批量摘、或碰坏了别批的布局快照，第二条会红）；场景按钮 `BtnStart` 的命中框
//!      在 spawn 前后都照常派发——它现在正是门面预设实例的矩形，⓪ 因此同时是"迁移没改命中框"的证据；
//!   ⑥ **换得了场景**：`scene.load` 会把 ECS 世界整个重建，运行期实例随之全部失效，因此实例登记表
//!      与它的布局节点表必须跟着世界一起清（旧句柄 `despawn` 幂等返回 false、`root`/`node` 点名
//!      "没有 handle"，本批名字不再出现在矩形读数里——留下的槽位会被新世界的无关实体复用）；
//!      而门面在回来时要**重新铺一份**（`on_create` → `title_menu.reset()`），矩形与切换前逐值相同。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m61_prefab_runtime [-- <game_jhyt-root>]

use engine::{
    core::{
        entity::EntityId,
        input::{InputSnapshot, MouseButton},
        ui::UiText,
        ui_layout::UiRect,
    },
    host::RuntimeScene,
};
use std::{env, fs, path::PathBuf};

/// 设计分辨率 = 宿主视口（`RuntimeScene` 的默认视口，门面 UI 就按这套像素作者化）。
const VIEW_W: f32 = 1280.0;
const VIEW_H: f32 = 720.0;
const DT: f32 = 1.0 / 60.0;
/// 断言容差：预设里是两位小数的十进制文本，解算只有浮点结合律差异。
const EPS: f32 = 0.01;

/// ② 的期望矩形：逐条直接来自下面那份预设的 `anchors`/`offsets`。
/// `M61Label` 的 x/y 是**父（`M61Btn` 原点 60,600）+ 自身偏移 120,17** ——父链走通才会是这个数。
const EXPECTED: &[(&str, [f32; 4])] = &[
    ("M61Root", [0.0, 0.0, 1280.0, 720.0]),
    ("M61Btn", [60.0, 600.0, 200.0, 60.0]),
    ("M61Label", [180.0, 617.0, 80.0, 30.0]),
];

/// `M61Btn` 的矩形中心（③ 的"内"探针）与画面左上角（"外"探针，不在任何 UI 实体的矩形里）。
const BTN_CENTER: (f64, f64) = (160.0, 630.0);
const OUTSIDE: (f64, f64) = (40.0, 40.0);
/// 门面按钮 `BtnStart` 的矩形中心：M63b 起它来自 spawn 的门面预设（矩形与迁移前的
/// `main.scene` 逐值相同），⑤ 用它证明"撤掉自己那一批"没关掉别批的派发通道。
const SCENE_BTN_CENTER: (f64, f64) = (640.0, 412.0);

const FIXTURE_FILE: &str = "jhyt_m61_menu.prefab";
const BROKEN_FILE: &str = "jhyt_m61_broken.prefab";

/// ①②③④⑤ 的驱动预设：一个全屏根 + 一个按钮（图片 + 点击事件）+ 按钮里的文字 +
/// 一张**故意不带 `UiLayout`** 的图（② 用它守"没有布局就不画"这条规则）。
const FIXTURE: &str = concat!(
    "prefab_version=1\n",
    "asset_id=11111111-2222-4333-8444-111111111111\n",
    "name=M61Menu\n",
    "root=1\n",
    "entity\t1\tM61Root\t\n",
    "component\t1\tTransform\tposition=0.000000000,0.000000000,0.000000000;",
    "rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=1.000000000,1.000000000,1.000000000\n",
    "component\t1\tUiLayout\tanchors=0.0000,0.0000,1.0000,1.0000;offsets=0.00,0.00,0.00,0.00;",
    "min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000\n",
    "entity\t2\tM61Btn\t1\n",
    "component\t2\tImage\ttexture=assets/art/ui/menu/ui_menu_btn_wuxia_9s.png;",
    "color=1.000,1.000,1.000;nine_slice=18,18,18,18\n",
    "component\t2\tUiEvents\tevent=click;function=m61_click\n",
    "component\t2\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;offsets=60.00,600.00,260.00,660.00;",
    "min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000\n",
    "entity\t3\tM61Label\t2\n",
    "component\t3\tText\ttext=继续;font_size=24.00;color=0.949,0.918,0.847;font=\n",
    "component\t3\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;offsets=120.00,17.00,200.00,47.00;",
    "min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000\n",
    "entity\t4\tM61Bare\t1\n",
    "component\t4\tImage\ttexture=assets/art/ui/menu/ui_menu_lantern.png;color=1.000,1.000,1.000\n",
);

/// 悬空 `parent`：子指向一个不存在的实体。`Prefab::from_text` 在解析期就拒绝，
/// 门的断言是"错误里点名了是哪一份文件的哪一行数据"，而不是崩或静默。
const BROKEN: &str = concat!(
    "prefab_version=1\n",
    "asset_id=11111111-2222-4333-8444-222222222222\n",
    "name=M61Broken\n",
    "root=1\n",
    "entity\t1\tBrokenRoot\t\n",
    "entity\t2\tBrokenChild\t99\n",
);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = resolve_project()?;
    let fixture_dir = project.join(".data");
    fs::create_dir_all(&fixture_dir)?;
    let fixture_rel = format!(".data/{FIXTURE_FILE}");
    let broken_rel = format!(".data/{BROKEN_FILE}");
    fs::write(fixture_dir.join(FIXTURE_FILE), FIXTURE)?;
    fs::write(fixture_dir.join(BROKEN_FILE), BROKEN)?;
    // 门面场景（project.toml 的 startup_scene）+ 工程脚本：与发布包同一条启动路径。
    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    frame(&mut runtime)?;
    // ⓪ 与 ⑤ 要点门面场景自己的按钮：`main.scene` 的 `UiEvents` 绑的是全局 `on_click`，
    // 让它跑真身会**开一局新游戏**、重建世界，之后门读到的就是下一屏而不是门面。
    // 因此临时把全局回调换成记录器（与 spawn 的按钮同一个表），跑完换回去。
    lua_run(
        &mut runtime,
        "m61_clicks = {}\nm61_real_on_click = on_click\n\
         function on_click(name) m61_clicks[#m61_clicks + 1] = name end",
    )?;
    println!("  门面场景 = {:?}", lua_str(&mut runtime, "map.current()")?);

    banner("⓪ 基线：场景自己的 UI 命中框（spawn 之前先跑一次，红了就不是这批的锅）");
    click_dispatches(&mut runtime, SCENE_BTN_CENTER, &["BtnStart"], "⓪ 门面基线")?;
    println!("  scene click({SCENE_BTN_CENTER:?}) → [\"BtnStart\"]");
    // M63b 起门面自己的 UI 也是 spawn 出来的（`assets/prefabs/title_menu.prefab`），
    // 于是本门的世界里同时住着**两批**实例：门面的 17 个 + 下面 fixture 的 4 个。
    // 先在这里记下门面那批的原点，⑤ 用它证明 `despawn` 只摘自己那一批。
    let menu_before = runtime.prefab_ui_rects([VIEW_W, VIEW_H]);
    println!(
        "  门面预设实例（spawn 之前）= {} 个：{}",
        menu_before.len(),
        rects_summary(&menu_before)
    );
    if menu_before.is_empty() {
        println!("  （门面未用预设：⑤ 的\"别批不受影响\"会退化成\"世界里不该有矩形\"，仍然成立）");
    }
    // 本批 fixture 的节点名前缀，②⑤ 用它把两批读数分开。
    const FIXTURE_PREFIX: &str = "M61";

    banner("① 预设文件 → 活世界实体：prefab.spawn / root / node 同源");
    lua_run(
        &mut runtime,
        "function m61_click(name) m61_clicks[#m61_clicks + 1] = name end",
    )?;
    let handle = lua_num(&mut runtime, &format!("prefab.spawn('{fixture_rel}')"))?;
    println!("  handle = {handle}");
    assert_str(
        &mut runtime,
        &format!("tostring(prefab.root({handle}) == entity.find('M61Root'))"),
        "true",
    )?;
    assert_str(
        &mut runtime,
        &format!("tostring(prefab.node({handle}, 'M61Btn') == entity.find('M61Btn'))"),
        "true",
    )?;
    assert_str(
        &mut runtime,
        &format!("tostring(entity.is_alive(prefab.node({handle}, 'M61Label')))"),
        "true",
    )?;
    // 坏数据与缺失文件都要**响亮**失败：静默返回 nil 的表现是"屏上少一块面板、日志一行没有"。
    // 复用 `query_err`（被接受就直接红）：Lua 里 `a .. b ~= nil` 会解析成 `(a .. b) ~= nil`
    // （拼接的优先级高于比较），在脚本内自己拼"ok|命中"少写一层括号就读成一个光秃秃的
    // `true`，负例等于没测——这批第一版就是这么"过"的。
    for (expr, needle) in [
        (format!("prefab.spawn('{broken_rel}')"), BROKEN_FILE),
        (
            "prefab.spawn('.data/jhyt_m61_nope.prefab')".to_owned(),
            "jhyt_m61_nope",
        ),
    ] {
        let got = query_err(&mut runtime, &expr)?;
        if !got.contains(needle) {
            return Err(format!("{expr} 的错误信息里没有点名 {needle:?}：实得 {got:?}").into());
        }
        println!("  {expr} → 报错点名 {needle:?}");
    }

    banner("② 画得出来：宿主本帧用于绘制场景 UI 的矩形 = 预设里的 UiLayout");
    frame(&mut runtime)?;
    let rects = runtime.prefab_ui_rects([VIEW_W, VIEW_H]);
    for (name, expect) in EXPECTED {
        let found = rects
            .iter()
            .find(|(rect_name, _)| rect_name == name)
            .map(|(_, rect)| [rect.x, rect.y, rect.w, rect.h]);
        let Some(found) = found else {
            return Err(format!(
                "{name} 没有解析出矩形（spawn 的 UI 不会被画）；本帧读数 = {}",
                rects_summary(&rects)
            )
            .into());
        };
        for (axis, (want, got)) in expect.iter().zip(found.iter()).enumerate() {
            if (want - got).abs() > EPS {
                return Err(format!(
                    "{name} 矩形第 {axis} 项（0=x 1=y 2=w 3=h）= {got}，预设里是 {want}"
                )
                .into());
            }
        }
    }
    if rects.iter().any(|(name, _)| name == "M61Bare") {
        return Err(
            "M61Bare 没有 UiLayout 组件却解算出了矩形：与渲染的\"无布局不画\"口径不一致".into(),
        );
    }
    println!(
        "  rects = {}（M61Bare 无 UiLayout ⇒ 不参与布局解算，故不在读数里）",
        rects_summary(&rects)
    );

    banner("③ 点得中：命中框与绘制框同源，矩形内收得到、矩形外收不到");
    click_dispatches(&mut runtime, BTN_CENTER, &["M61Btn"], "③ 按钮矩形内")?;
    click_dispatches(&mut runtime, OUTSIDE, &[], "③ 按钮矩形外")?;
    println!("  内 → [\"M61Btn\"]，外 → []（一次点击只派发一个回调）");

    banner("④ 写得进：prefab.set_text 改的是渲染读的那份 Text 组件");
    let label_id = lua_num(&mut runtime, &format!("prefab.node({handle}, 'M61Label')"))?;
    if scene_text(&mut runtime, label_id as i64)? != "继续" {
        return Err("spawn 之后读到的 Text 组件文字不是预设里那份".into());
    }
    lua_run(
        &mut runtime,
        &format!("prefab.set_text({handle}, 'M61Label', '继续游戏（运行期改的字）')"),
    )?;
    if scene_text(&mut runtime, label_id as i64)? != "继续游戏（运行期改的字）" {
        return Err("prefab.set_text 之后 Text 组件没变：写的是副本而不是世界里的组件".into());
    }
    // 三条负例：写错对象要报错。静默 no-op 的表现是"脚本以为改了、屏上还是旧字"。
    for (expr, needle) in [
        (
            format!("prefab.set_text({handle}, 'M61Btn', 'x')"),
            "没有 Text 组件",
        ),
        (
            format!("prefab.set_text({handle}, 'NoSuchNode', 'x')"),
            "里没有节点",
        ),
        (
            "prefab.set_text(-7, 'M61Label', 'x')".to_string(),
            "没有 handle",
        ),
    ] {
        let got = query_err(&mut runtime, &expr)?;
        if !got.contains(needle) {
            return Err(format!("{expr} 的错误信息里没有 {needle:?}：实得 {got:?}").into());
        }
    }
    println!("  写入生效，三条误用各自点名");

    banner("⑤ 收得干净：despawn 只撤自己那一批，别批的实例与矩形一个不少");
    let btn_id = lua_num(&mut runtime, &format!("prefab.node({handle}, 'M61Btn')"))?;
    assert_str(
        &mut runtime,
        &format!("tostring(prefab.despawn({handle}))"),
        "true",
    )?;
    // 实体没了（按 despawn 前拿到的句柄验），句柄也不再认得（`prefab.node` 必须**报错**，
    // 静默返回 nil 会让脚本以为实例还在）。
    assert_str(
        &mut runtime,
        &format!("tostring(entity.is_alive({btn_id}))"),
        "false",
    )?;
    let after_node = query_err(&mut runtime, &format!("prefab.node({handle}, 'M61Btn')"))?;
    if !after_node.contains("没有 handle") {
        return Err(format!(
            "despawn 之后 prefab.node({handle}, …) 的错误信息里没有点名失效句柄：实得 {after_node:?}"
        )
        .into());
    }
    frame(&mut runtime)?;
    let after = runtime.prefab_ui_rects([VIEW_W, VIEW_H]);
    // ⑤-a 本批那 4 个（3 个有矩形 + 无布局的 M61Bare 不在读数里）一个不剩。
    let mine = rects_matching(&after, |name| name.starts_with(FIXTURE_PREFIX));
    if !mine.is_empty() {
        return Err(format!("despawn 之后仍有本批的布局矩形：{}", rects_summary(&mine)).into());
    }
    // ⑤-b 别批（门面自己的预设实例）一个不少、逐值不变。despawn 走的是"按 handle 记的
    // 实体清单批量删"，若它按 index 段批量摘布局快照、或把并表的节点整批撤了，红的就是这一条
    // ——上一版断言的是"世界里不该有矩形"，那在门面也改用预设之后就不再是真相，
    // 而"只摘自己那一批"才是本门真正要守的东西。
    let others = rects_matching(&after, |name| !name.starts_with(FIXTURE_PREFIX));
    assert_same_rects(&menu_before, &others, "despawn 之后别批")?;
    println!(
        "  本批撤干净（{} 个矩形归零）；别批 {} 个矩形逐值不变：{}",
        EXPECTED.len(),
        others.len(),
        rects_summary(&others)
    );
    // despawn 幂等 + 派发通道一起关掉。
    assert_str(
        &mut runtime,
        &format!("tostring(prefab.despawn({handle}))"),
        "false",
    )?;
    click_dispatches(&mut runtime, BTN_CENTER, &[], "⑤ despawn 之后按原位置")?;
    click_dispatches(
        &mut runtime,
        SCENE_BTN_CENTER,
        &["BtnStart"],
        "⑤ 门面场景 UI 未被污染",
    )?;
    println!("  本批撤干净；门面预设的按钮命中框仍派发 [\"BtnStart\"]（撤一批没关掉派发通道）");

    banner("⑥ 换场景：实例登记表随世界一起清 —— 旧句柄撤不动也不留幽灵矩形，门面重新铺回来");
    // 走**真实**切换路径：`scene.load` 只挂起请求，由宿主在步进里应用
    // （`transition.rs`：换 `World::new()` 并按域清理）。这里原地重载门面那张 `main`——
    // 重建世界这件事与切去哪张图无关，而原地重载不需要任何玩法前置状态。
    let handle2 = lua_num(&mut runtime, &format!("prefab.spawn('{fixture_rel}')"))?;
    frame(&mut runtime)?;
    if rects_matching(&runtime.prefab_ui_rects([VIEW_W, VIEW_H]), |name| {
        name.starts_with(FIXTURE_PREFIX)
    })
    .is_empty()
    {
        return Err("⑥ 第二次 spawn 的实例没解算出矩形：切换之后没有可对比的读数".into());
    }
    assert_str(&mut runtime, "scene.load('main')", "true")?;
    frame(&mut runtime)?;
    // 旧句柄：登记表已随世界清空 ⇒ `despawn` 是**幂等 false**。不幂等的表现是拿着死句柄去
    // 新世界上删实体 ⇒ Lua 报错，而报错的位置是"离开门面的那次点击"（M20-09c 的原始红）。
    assert_str(
        &mut runtime,
        &format!("tostring(prefab.despawn({handle2}))"),
        "false",
    )?;
    for expr in [
        format!("prefab.root({handle2})"),
        format!("prefab.node({handle2}, 'M61Btn')"),
    ] {
        let got = query_err(&mut runtime, &expr)?;
        if !got.contains("没有 handle") {
            return Err(format!("{expr} 的错误信息里没有点名失效句柄：实得 {got:?}").into());
        }
    }
    // 幽灵矩形：布局节点表是按**槽位号**索引的，槽位会被新世界的无关实体复用，
    // 所以旧实例的名字必须一个都不在（留着既不画该画的，又可能顶掉别人的矩形）。
    let after_switch = runtime.prefab_ui_rects([VIEW_W, VIEW_H]);
    let ghosts = rects_matching(&after_switch, |name| name.starts_with(FIXTURE_PREFIX));
    if !ghosts.is_empty() {
        return Err(format!("换场景之后仍有旧实例的布局矩形：{}", rects_summary(&ghosts)).into());
    }
    // 门面自己重新铺回来：`on_create` → `title_menu.reset()` 按**当前世界**重铺一份，
    // 矩形与切换前逐值相同。脚本只认"句柄非 nil"时这一条是空的（整屏门面消失、一行日志
    // 没有——最难发现的那种坏），因此它同时是脚本侧失配守住的读数。
    assert_same_rects(&menu_before, &after_switch, "换场景之后的门面")?;
    println!(
        "  旧句柄失效（despawn=false、root/node 各自点名）；门面重新铺回 {} 个矩形",
        after_switch.len()
    );
    // 换回门面自己的回调（记录器只在门内有效）。
    lua_run(&mut runtime, "on_click = m61_real_on_click")?;

    println!("jhyt_m61_prefab_runtime_ok project={}", project.display());
    let _ = fs::remove_file(fixture_dir.join(FIXTURE_FILE));
    let _ = fs::remove_file(fixture_dir.join(BROKEN_FILE));
    Ok(())
}

// ---------- 驱动 ----------

/// 一次左键点击：置好光标与按下边沿跑一步，返回**实际派发出的回调目标名**（按派发顺序）。
///
/// 走的是**真实**通道（宿主每帧 `compute_click_targets` → `process_ui_clicks` → 取全局回调），
/// 所以这一条同时证明"预设里的 `UiEvents` 落成了运行时事件"。
///
/// 这里**不**断言数量：`矩形外`与`despawn 之后`两路探针期望的就是**空**（一道只会数"恰好一个"
/// 的探针，在负例上必然红——本批第一版就是这么把自己绊倒的，红在③的"外"探针上，
/// 看起来像命中框坏了）。数量与名单都交给调用方比。
fn click(
    runtime: &mut RuntimeScene,
    point: (f64, f64),
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let (x, y) = point;
    lua_run(runtime, "m61_clicks = {}")?;
    let mut input = InputSnapshot::default();
    input.cursor_moved(x, y);
    input.mouse_event(MouseButton::Left, true);
    runtime
        .run_script_steps(1, DT, &input)
        .map_err(|error| format!("点击帧失败: {error}"))?;
    let names = lua_str(
        runtime,
        "(function() local out = {} for i = 1, #m61_clicks do out[#out + 1] = tostring(m61_clicks[i]) end return table.concat(out, ',') end)()",
    )?;
    // 松开：否则下一帧的 `mouse_just_pressed` 边沿会重复派发。
    let mut up = InputSnapshot::default();
    up.cursor_moved(x, y);
    up.mouse_event(MouseButton::Left, false);
    runtime
        .run_script_steps(1, DT, &up)
        .map_err(|error| format!("松开帧失败: {error}"))?;
    Ok(if names.is_empty() {
        Vec::new()
    } else {
        names.split(',').map(str::to_owned).collect()
    })
}

/// 点击 `point`，并要求派发名单逐字等于 `want`（空表就是"一下都没派发"）。
fn click_dispatches(
    runtime: &mut RuntimeScene,
    point: (f64, f64),
    want: &[&str],
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = click(runtime, point)?;
    let want: Vec<String> = want.iter().map(|name| (*name).to_owned()).collect();
    if got != want {
        return Err(format!("{label}：点在 {point:?} 的派发名单是 {got:?}，期望 {want:?}").into());
    }
    Ok(())
}

/// 推进一帧：先走真实渲染路径（视口与 UI 布局解算只在那条路径上发生），再跑一个固定步。
fn frame(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    runtime.vertices(VIEW_W as u32, VIEW_H as u32)?;
    runtime
        .run_script_steps(1, DT, &InputSnapshot::default())
        .map_err(|error| error.into())
}

/// 读活世界里那个实体的 `Text` 组件内容（渲染画的正是这一份）。
///
/// Lua 侧的实体 id 是 `index << 32 | generation`，这里按同一编码还原句柄。
fn scene_text(
    runtime: &mut RuntimeScene,
    lua_id: i64,
) -> Result<String, Box<dyn std::error::Error>> {
    let raw = lua_id as u64;
    let entity = EntityId::from_raw((raw >> 32) as u32, raw as u32);
    let text = runtime.with_script(|script| {
        script.with_world(|world| {
            world
                .get::<UiText>(entity)
                .ok()
                .flatten()
                .map(|text| text.text.clone())
        })
    });
    text.ok_or_else(|| format!("实体 {lua_id} 上没有 Text 组件").into())
}

fn rects_summary(rects: &[(String, UiRect)]) -> String {
    if rects.is_empty() {
        return "<空>".to_owned();
    }
    rects
        .iter()
        .map(|(name, rect)| format!("{name}=({},{},{},{})", rect.x, rect.y, rect.w, rect.h))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 按名字筛矩形读数。⑤⑥ 用节点名前缀把"本批"与"别批"分开——同一个世界里现在住着两批
/// 实例（门面的预设 + 本门的 fixture），"整个世界该是空的"那种断言已经不是真相。
fn rects_matching<F>(rects: &[(String, UiRect)], f: F) -> Vec<(String, UiRect)>
where
    F: Fn(&str) -> bool,
{
    rects.iter().filter(|(name, _)| f(name)).cloned().collect()
}

/// 两次读数之间**同一批**矩形逐值不变：数量、名字集合与四条边都比。顺序不比
/// （顺序由实体槽位决定，那是 m63 ④ 的账，不是这条的）。
fn assert_same_rects(
    want: &[(String, UiRect)],
    got: &[(String, UiRect)],
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if got.len() != want.len() {
        return Err(format!(
            "{label}的矩形数量从 {} 变成 {}：实得 {}",
            want.len(),
            got.len(),
            rects_summary(got)
        )
        .into());
    }
    for (name, before) in want {
        let found = got
            .iter()
            .find(|(got_name, _)| got_name == name)
            .map(|(_, rect)| [rect.x, rect.y, rect.w, rect.h])
            .ok_or_else(|| format!("{label}少了 {name}"))?;
        for (axis, (was, now)) in [before.x, before.y, before.w, before.h]
            .iter()
            .zip(found.iter())
            .enumerate()
        {
            if (was - now).abs() > EPS {
                return Err(format!(
                    "{label}的 {name} 矩形第 {axis} 项（0=x 1=y 2=w 3=h）= {now}，之前是 {was}"
                )
                .into());
            }
        }
    }
    Ok(())
}

// ---------- Lua 取值 ----------

fn resolve_project() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let project = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| engine::core::games_dir().join("examples").join("game_jhyt"));
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    Ok(project)
}

fn lua_run(runtime: &mut RuntimeScene, code: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime
        .with_script(|script| script.execute_console(code).map(|_| ()))
        .map_err(|error| error.into())
}

fn lua_str(runtime: &mut RuntimeScene, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    let text = runtime
        .with_script(|script| script.execute_console(&format!("return tostring({expr})")))?;
    Ok(text.trim().to_owned())
}

fn lua_num(runtime: &mut RuntimeScene, expr: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let text = lua_str(runtime, expr)?;
    text.parse::<f64>()
        .map_err(|error| format!("{expr} = {text:?} 不是数字: {error}").into())
}

fn assert_str(
    runtime: &mut RuntimeScene,
    expr: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = lua_str(runtime, expr)?;
    if got != expected {
        return Err(format!("{expr} = {got:?}, want {expected:?}").into());
    }
    Ok(())
}

/// 取一条 Lua 调用的错误信息（调用必须失败，否则这里直接报错）。
fn query_err(runtime: &mut RuntimeScene, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    let text = runtime.with_script(|script| {
        script.execute_console(&format!(
            "local ok, err = pcall(function() {expr} end)\n\
             if ok then return 'ACCEPTED' end\n\
             return tostring(err)"
        ))
    })?;
    let text = text.trim().to_owned();
    if text == "ACCEPTED" {
        return Err(format!("{expr} 本应报错却被接受").into());
    }
    Ok(text)
}

fn banner(title: &str) {
    println!("\n=== {title} ===");
}
