//! Headless acceptance drive for M62：运行期改**预设实例**的矩形与显隐。
//!
//! 上一批（M61）把 `.prefab` 接进活世界：版面能进预设、能画也能点。但**代码构建的 UI**
//! 迁过来还差两件事，而这两件事缺一个就会退回"预设画一遍、动态层再盖一遍"的现状：
//!   * 列表行要按数据排位（背包 10 行、商店 9 行、任务 7 行）——版面在预设里，位置由数据定；
//!   * 一屏里"这一刻不该出现的那些节点"要能收起来（title_menu 现在靠
//!     `ui.scene_text_visible(name,false)` 逐名隐藏场景文字，正是这个缺口的症状）。
//!
//! 于是有了 `prefab.set_rect` 与 `prefab.set_visible`。这道门守的是它们的**语义**而不是语法：
//!
//!   ① `set_rect` 写四边之后，**宿主本帧用于绘制的矩形**跟着变（`prefab_ui_rects` 读的就是
//!      渲染消费的那份 `ui_layout_rects`，不是第二套布局计算）；子节点**整棵子树**跟着位移
//!      （父链解算本来就传下去）；世界的 `UiRectLayout` 组件同步改，于是"画面按 A 摆、脚本
//!      查到 B"不成立。
//!   ② `set_rect` 只改 `offsets`，预设里 authored 的 `anchors` **一个字都不动**：那条 stretch
//!      节点（`anchors=0,0,1,0`）写回 `0,300,0,340` 仍是通栏宽 1280——如果实现顺手把 anchors
//!      重置成 0，宽度会塌成 0，这条当场红。
//!   ③ `set_rect` 之后**命中框就是新矩形**：按新位置点得到、按旧位置点不到。
//!   ④ `set_visible(false)` 是**子树**语义且**画与命中同时失效**：藏行根 ⇒ 行里的图与字一起
//!      从读数里消失，原位置点下去一个回调都不派发（"看不见的按钮仍能被点中"在这条路上不
//!      成立，这是选"不建布局节点"而不是 `Visibility` 组件的全部理由——后者只管绘制）。
//!   ⑤ 显隐是**相对父**的（Unity `GameObject.SetActive` 同语义）：藏父之后再单独显示子，子
//!      仍不可见；显示父 ⇒ 整棵子树回来，且运行期改过的文字/矩形**不需要重设**（实体与组件
//!      从未销毁）。
//!   ⑥ 只动**本实例**的键：同屏还跑着第二个预设实例，改第一个实例的矩形不能把第二个实例的
//!      布局节点一起摘掉（并集表按槽位号键，误摘全量就是"另一块面板凭空消失"）。
//!   ⑦ 误用**点名报错**，不静默 no-op：写没有 `UiLayout` 的节点、不存在的节点、失效句柄，
//!      两个函数各自三条。静默的代价是"脚本以为改了、屏上纹丝不动"，只有真机才看得出来。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m62_prefab_write [-- <game_jhyt-root>]

use engine::host::RuntimeScene;
use rustlua_ecs::entity::EntityId;
use rustlua_input::input::{InputSnapshot, MouseButton};
use rustlua_ui::ui::UiText;
use rustlua_ui::ui_layout::{UiRect, UiRectLayout};
use std::{env, fs, path::PathBuf};

const VIEW_W: f32 = 1280.0;
const VIEW_H: f32 = 720.0;
const DT: f32 = 1.0 / 60.0;
/// 两位小数的十进制文本 ↔ 浮点解算之间只有结合律差异。
const EPS: f32 = 0.01;

const MENU_FILE: &str = "jhyt_m62_menu.prefab";
const OTHER_FILE: &str = "jhyt_m62_other.prefab";

/// 驱动预设：全屏根 + 一行（图 + 点击）+ 行里的格与标签 + **一张没有 `UiLayout` 的图**
/// （⑦ 用它守"没有矩形可写"）+ 一条 stretch 通栏（② 用它守"anchors 不许被重置"）。
const MENU: &str = concat!(
    "prefab_version=1\n",
    "asset_id=11111111-2222-4333-8444-333333333333\n",
    "name=M62Menu\n",
    "root=1\n",
    "entity\t1\tM62Root\t\n",
    "component\t1\tTransform\tposition=0.000000000,0.000000000,0.000000000;",
    "rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=1.000000000,1.000000000,1.000000000\n",
    "component\t1\tUiLayout\tanchors=0.0000,0.0000,1.0000,1.0000;offsets=0.00,0.00,0.00,0.00;",
    "min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000\n",
    "entity\t2\tM62Row\t1\n",
    "component\t2\tImage\ttexture=assets/art/ui/menu/ui_menu_btn_wuxia_9s.png;",
    "color=1.000,1.000,1.000;nine_slice=18,18,18,18\n",
    "component\t2\tUiEvents\tevent=click;function=m62_click\n",
    "component\t2\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;offsets=60.00,600.00,260.00,660.00;",
    "min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000\n",
    "entity\t3\tM62Cell\t2\n",
    "component\t3\tImage\ttexture=assets/art/ui/menu/ui_menu_lantern.png;color=1.000,1.000,1.000\n",
    "component\t3\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;offsets=10.00,10.00,50.00,30.00;",
    "min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000\n",
    "entity\t4\tM62Label\t2\n",
    "component\t4\tText\ttext=第一行;font_size=24.00;color=0.949,0.918,0.847;font=\n",
    "component\t4\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;offsets=120.00,17.00,200.00,47.00;",
    "min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000\n",
    "entity\t5\tM62Bare\t1\n",
    "component\t5\tImage\ttexture=assets/art/ui/menu/ui_menu_seal_jianghu.png;color=1.000,1.000,1.000\n",
    "entity\t6\tM62Stretch\t1\n",
    "component\t6\tImage\ttexture=assets/art/ui/menu/ui_menu_divider_bamboo.png;color=1.000,1.000,1.000\n",
    "component\t6\tUiLayout\tanchors=0.0000,0.0000,1.0000,0.0000;offsets=0.00,300.00,0.00,340.00;",
    "min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000\n",
);

/// 第二个实例：另一个文件、另一套实体名，只为守 ⑥（改第一个实例不许摘掉它的布局节点）。
const OTHER: &str = concat!(
    "prefab_version=1\n",
    "asset_id=11111111-2222-4333-8444-444444444444\n",
    "name=M62Other\n",
    "root=1\n",
    "entity\t1\tM62OtherRoot\t\n",
    "component\t1\tUiLayout\tanchors=0.0000,0.0000,1.0000,1.0000;offsets=0.00,0.00,0.00,0.00;",
    "min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000\n",
    "entity\t2\tM62OtherBtn\t1\n",
    "component\t2\tImage\ttexture=assets/art/ui/menu/ui_menu_btn_wuxia_9s.png;color=1.000,1.000,1.000\n",
    "component\t2\tUiEvents\tevent=click;function=m62_click\n",
    "component\t2\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;offsets=900.00,600.00,1000.00,660.00;",
    "min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000\n",
);

/// ① 的期望矩形（写回 `M62Row` 到 300,200,500,260 之后）：行的**子节点跟着位移**。
/// `M62Stretch` 不在这张表里：② 会把它另写一次，两处期望值不同（各自都单独断言）。
const AFTER_RECT: &[(&str, [f32; 4])] = &[
    ("M62Row", [300.0, 200.0, 200.0, 60.0]),
    ("M62Cell", [310.0, 210.0, 40.0, 20.0]),
    ("M62Label", [420.0, 217.0, 80.0, 30.0]),
];

/// 行移动前/后的中心（③ 的两个探针）；`M62OtherBtn` 的中心（⑥）。
const ROW_OLD_CENTER: (f64, f64) = (160.0, 630.0);
const ROW_NEW_CENTER: (f64, f64) = (400.0, 230.0);
const OTHER_CENTER: (f64, f64) = (950.0, 630.0);
/// main.scene 的 `BtnStart`（460,380,820,444）中心：⑧ 用它证明场景 UI 没被污染。
const SCENE_BTN_CENTER: (f64, f64) = (640.0, 412.0);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = resolve_project()?;
    let fixture_dir = project.join(".data");
    fs::create_dir_all(&fixture_dir)?;
    fs::write(fixture_dir.join(MENU_FILE), MENU)?;
    fs::write(fixture_dir.join(OTHER_FILE), OTHER)?;
    let menu_rel = format!(".data/{MENU_FILE}");
    let other_rel = format!(".data/{OTHER_FILE}");

    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    frame(&mut runtime)?;
    // 门面场景的 `UiEvents` 绑全局 `on_click`，让它跑真身会开一局新游戏、重建世界。
    lua_run(
        &mut runtime,
        "m62_clicks = {}\nm62_real_on_click = on_click\n\
         function on_click(name) m62_clicks[#m62_clicks + 1] = name end\n\
         function m62_click(name) m62_clicks[#m62_clicks + 1] = name end",
    )?;

    banner("① set_rect：宿主绘制的矩形跟着改，子树整棵位移，世界的 UiRectLayout 同步");
    let menu = lua_num(&mut runtime, &format!("prefab.spawn('{menu_rel}')"))?;
    let other = lua_num(&mut runtime, &format!("prefab.spawn('{other_rel}')"))?;
    println!("  handle 菜单={menu} 另一实例={other}");
    frame(&mut runtime)?;
    // 起点必须先是预设里那份，否则"改完等于期望"可能一直是期望。
    expect_rects(
        &mut runtime,
        &[
            ("M62Row", [60.0, 600.0, 200.0, 60.0]),
            ("M62Cell", [70.0, 610.0, 40.0, 20.0]),
            ("M62Stretch", [0.0, 300.0, 1280.0, 40.0]),
        ],
        "① spawn 之后（预设原值）",
    )?;
    let row_id = lua_num(&mut runtime, &format!("prefab.node({menu}, 'M62Row')"))?;
    lua_run(
        &mut runtime,
        &format!("prefab.set_rect({menu}, 'M62Row', 300, 200, 500, 260)"),
    )?;
    frame(&mut runtime)?;
    expect_rects(&mut runtime, AFTER_RECT, "① set_rect 之后")?;
    // 一动不如一静：没参与本次写入的节点不许被牵连位移。
    expect_rects(
        &mut runtime,
        &[("M62Stretch", [0.0, 300.0, 1280.0, 40.0])],
        "① 未参与写入的节点照旧",
    )?;
    let stored = world_layout(&mut runtime, row_id as i64)?;
    if stored.offsets != [300.0, 200.0, 500.0, 260.0] {
        return Err(format!(
            "世界的 UiRectLayout offsets = {:?}，与画面（300,200,500,260）不同源",
            stored.offsets
        )
        .into());
    }
    println!(
        "  世界组件里也是 offsets={:?}（entity 侧与画面同一个数）",
        stored.offsets
    );

    banner("② 只改 offsets：预设里 authored 的 anchors 一个字都不动");
    // stretch 通栏写回 0,250,0,290：anchors=(0,0,1,0) 下宽度仍是父宽 1280。
    // 若实现把 anchors 一起重置成 (0,0,0,0)，右缘就变成 0+0 ⇒ 宽 0，这条当场红。
    lua_run(
        &mut runtime,
        &format!("prefab.set_rect({menu}, 'M62Stretch', 0, 250, 0, 290)"),
    )?;
    frame(&mut runtime)?;
    expect_rects(
        &mut runtime,
        &[("M62Stretch", [0.0, 250.0, 1280.0, 40.0])],
        "② stretch 节点写四边之后",
    )?;
    let stretch_id = lua_num(&mut runtime, &format!("prefab.node({menu}, 'M62Stretch')"))?;
    let stretch = world_layout(&mut runtime, stretch_id as i64)?;
    if stretch.anchors != [0.0, 0.0, 1.0, 0.0] {
        return Err(format!(
            "set_rect 把 anchors 改成了 {:?}（预设里是 0,0,1,0；通栏不该被降级成左上角定宽）",
            stretch.anchors
        )
        .into());
    }
    println!(
        "  anchors 仍是 {:?}（宽 1280 由父矩形撑出来）",
        stretch.anchors
    );

    banner("③ 命中框 = 新矩形：按新位置点得到、按旧位置点不到");
    click_dispatches(&mut runtime, ROW_NEW_CENTER, &["M62Row"], "③ 新矩形内")?;
    click_dispatches(&mut runtime, ROW_OLD_CENTER, &[], "③ 旧位置")?;
    println!("  新 → [\"M62Row\"]，旧 → []");

    banner("④ set_visible(false)：子树一起消失，画与命中同时失效");
    lua_run(
        &mut runtime,
        &format!("prefab.set_text({menu}, 'M62Label', '第一行（运行期）')"),
    )?;
    lua_run(
        &mut runtime,
        &format!("prefab.set_visible({menu}, 'M62Row', false)"),
    )?;
    frame(&mut runtime)?;
    let hidden = runtime.prefab_ui_rects([VIEW_W, VIEW_H]);
    for name in ["M62Row", "M62Cell", "M62Label"] {
        if hidden.iter().any(|(rect_name, _)| rect_name == name) {
            return Err(format!(
                "藏了行根 {name} 之后它仍在绘制读数里：{}",
                rects_summary(&hidden)
            )
            .into());
        }
    }
    // 没被牵连的：根与通栏照常（只摘本应摘的键）。
    expect_rects(
        &mut runtime,
        &[
            ("M62Root", [0.0, 0.0, 1280.0, 720.0]),
            ("M62Stretch", [0.0, 250.0, 1280.0, 40.0]),
        ],
        "④ 藏行之后仍在的节点",
    )?;
    click_dispatches(&mut runtime, ROW_NEW_CENTER, &[], "④ 藏行之后按原位置")?;
    println!(
        "  读数里只剩 {}；新矩形中心点下去派发 []（画与命中同源，同一次失效）",
        rects_summary(&hidden)
    );

    banner("⑤ 显隐相对父：藏父时单独显示子不生效；显示父 ⇒ 子树回来且不用重设");
    lua_run(
        &mut runtime,
        &format!("prefab.set_visible({menu}, 'M62Label', true)"),
    )?;
    frame(&mut runtime)?;
    let still = runtime.prefab_ui_rects([VIEW_W, VIEW_H]);
    if still.iter().any(|(name, _)| name == "M62Label") {
        return Err(
            "父还藏着就显示子节点：Unity 语义下子的显隐是相对父的，这里不该出现 M62Label".into(),
        );
    }
    println!("  藏父 + 显示子 → M62Label 仍不在读数里（相对父）");
    lua_run(
        &mut runtime,
        &format!("prefab.set_visible({menu}, 'M62Row', true)"),
    )?;
    frame(&mut runtime)?;
    expect_rects(&mut runtime, AFTER_RECT, "⑤ 显示行根之后（矩形不用重设）")?;
    // ② 写过的通栏也还在写后的位置：显示子树不会把别的节点回滚成预设原值。
    expect_rects(
        &mut runtime,
        &[("M62Stretch", [0.0, 250.0, 1280.0, 40.0])],
        "⑤ 同实例内另一处已写过的矩形",
    )?;
    let label_id = lua_num(&mut runtime, &format!("prefab.node({menu}, 'M62Label')"))? as i64;
    if scene_text(&mut runtime, label_id)? != "第一行（运行期）" {
        return Err("显示回来之后运行期改的文字丢了：隐藏动到了实体/组件".into());
    }
    println!("  显示行根 → 行/格/标签回到 set_rect 之后的位置，文字仍是运行期那份");

    banner("⑥ 只动本实例：第二个预设实例的布局节点不许被牵连");
    frame(&mut runtime)?;
    expect_rects(
        &mut runtime,
        &[("M62OtherBtn", [900.0, 600.0, 100.0, 60.0])],
        "⑥ 另一个实例",
    )?;
    click_dispatches(
        &mut runtime,
        OTHER_CENTER,
        &["M62OtherBtn"],
        "⑥ 另一个实例的按钮",
    )?;
    // 再改一次第一个实例，另一份照旧（重建走"摘本实例的键"而不是清空整表）。
    lua_run(
        &mut runtime,
        &format!("prefab.set_rect({menu}, 'M62Row', 60, 600, 260, 660)"),
    )?;
    frame(&mut runtime)?;
    expect_rects(
        &mut runtime,
        &[
            ("M62Row", [60.0, 600.0, 200.0, 60.0]),
            ("M62OtherBtn", [900.0, 600.0, 100.0, 60.0]),
        ],
        "⑥ 回到预设原值 + 另一实例照旧",
    )?;
    println!("  两块的读数互不干涉");

    banner("⑦ 误用点名报错，不静默 no-op");
    // `set_rect` 的三条：写没有 `UiLayout` 的节点、写不存在的节点、写失效句柄。
    for (expr, needle) in [
        (
            format!("prefab.set_rect({menu}, 'M62Bare', 0, 0, 10, 10)"),
            "没有 UiLayout 组件",
        ),
        (
            format!("prefab.set_rect({menu}, 'Nope', 0, 0, 1, 1)"),
            "里没有节点",
        ),
        (
            "prefab.set_rect(-7, 'M62Row', 0, 0, 1, 1)".to_owned(),
            "没有 handle",
        ),
    ] {
        let got = query_err(&mut runtime, &expr)?;
        if !got.contains(needle) {
            return Err(format!("{expr} 的错误里没点名 {needle:?}：实得 {got:?}").into());
        }
        println!("  {expr} → 点名 {needle:?}");
    }
    // 显隐对"没有 UiLayout 的节点"必须**成功**：它本来就不画，但藏/显它的子树合法。
    // 反过来说这条也是一道门——若实现让 set_visible 依赖矩形，这里就会红。
    lua_run(
        &mut runtime,
        &format!("prefab.set_visible({menu}, 'M62Bare', false)"),
    )?;
    lua_run(
        &mut runtime,
        &format!("prefab.set_visible({menu}, 'M62Bare', true)"),
    )?;
    println!("  set_visible 对无 UiLayout 的节点同样可用（显隐不依赖矩形）");
    for (expr, needle) in [
        (
            format!("prefab.set_visible({menu}, 'Nope', false)"),
            "里没有节点",
        ),
        (
            "prefab.set_visible(-7, 'M62Row', false)".to_owned(),
            "没有 handle",
        ),
    ] {
        let got = query_err(&mut runtime, &expr)?;
        if !got.contains(needle) {
            return Err(format!("{expr} 的错误里没点名 {needle:?}：实得 {got:?}").into());
        }
        println!("  {expr} → 点名 {needle:?}");
    }

    banner("⑧ 收得干净：despawn 之后场景 UI 与另一个实例照常");
    assert_str(
        &mut runtime,
        &format!("tostring(prefab.despawn({menu}))"),
        "true",
    )?;
    frame(&mut runtime)?;
    let after = runtime.prefab_ui_rects([VIEW_W, VIEW_H]);
    for name in ["M62Row", "M62Cell", "M62Label", "M62Stretch", "M62Root"] {
        if after.iter().any(|(rect_name, _)| rect_name == name) {
            return Err(format!(
                "despawn 之后本实例的 {name} 仍在绘制读数里：{}",
                rects_summary(&after)
            )
            .into());
        }
    }
    expect_rects(
        &mut runtime,
        &[("M62OtherBtn", [900.0, 600.0, 100.0, 60.0])],
        "⑧ despawn 一份之后另一份照旧",
    )?;
    click_dispatches(&mut runtime, ROW_NEW_CENTER, &[], "⑧ 已销毁的实例按位置")?;
    click_dispatches(
        &mut runtime,
        SCENE_BTN_CENTER,
        &["BtnStart"],
        "⑧ 门面场景 UI 未被污染",
    )?;
    println!("  摘得掉本实例、留下别的实例与场景那份");
    lua_run(&mut runtime, "on_click = m62_real_on_click")?;
    println!("jhyt_m62_prefab_write_ok project={}", project.display());
    let _ = fs::remove_file(fixture_dir.join(MENU_FILE));
    let _ = fs::remove_file(fixture_dir.join(OTHER_FILE));
    Ok(())
}

// ---------- 驱动 ----------

/// 逐条比对 `want` 里的 (名字 → 矩形)：读数缺名字或任一项超出 `EPS` 都红，红话说清缺哪个、
/// 本帧实得哪一份（只看"第 3 项不对"看不出是父链断了还是偏移丢了）。
fn expect_rects(
    runtime: &mut RuntimeScene,
    want: &[(&str, [f32; 4])],
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let rects = runtime.prefab_ui_rects([VIEW_W, VIEW_H]);
    for (name, expect) in want {
        let found = rects
            .iter()
            .find(|(rect_name, _)| rect_name == name)
            .map(|(_, rect)| [rect.x, rect.y, rect.w, rect.h]);
        let Some(found) = found else {
            return Err(format!(
                "{label}：{name} 没有解析出矩形（spawn 的 UI 不会被画）；本帧读数 = {}",
                rects_summary(&rects)
            )
            .into());
        };
        for (axis, (want, got)) in expect.iter().zip(found.iter()).enumerate() {
            if (want - got).abs() > EPS {
                return Err(format!(
                    "{label}：{name} 矩形第 {axis} 项（0=x 1=y 2=w 3=h）= {got}，期望 {want}；本帧读数 = {}",
                    rects_summary(&rects)
                )
                .into());
            }
        }
    }
    Ok(())
}

/// 一次左键点击 → 实际派发出的回调目标名（走宿主真实通道：`compute_click_targets` →
/// `process_ui_clicks`）。不断言数量：负例探针要的就是空表。
fn click(
    runtime: &mut RuntimeScene,
    point: (f64, f64),
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let (x, y) = point;
    lua_run(runtime, "m62_clicks = {}")?;
    let mut input = InputSnapshot::default();
    input.cursor_moved(x, y);
    input.mouse_event(MouseButton::Left, true);
    runtime
        .run_script_steps(1, DT, &input)
        .map_err(|error| format!("点击帧失败: {error}"))?;
    let names = lua_str(
        runtime,
        "(function() local out = {} for i = 1, #m62_clicks do out[#out + 1] = tostring(m62_clicks[i]) end return table.concat(out, ',') end)()",
    )?;
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

/// 读活世界里那个实体的 `UiLayout` 组件（画面与脚本共用同一份数据才算同源）。
fn world_layout(
    runtime: &mut RuntimeScene,
    lua_id: i64,
) -> Result<UiRectLayout, Box<dyn std::error::Error>> {
    let raw = lua_id as u64;
    let entity = EntityId::from_raw((raw >> 32) as u32, raw as u32);
    let layout = runtime.with_script(|script| {
        script.with_world(|world| world.get::<UiRectLayout>(entity).ok().flatten().cloned())
    });
    layout.ok_or_else(|| format!("实体 {lua_id} 上没有 UiLayout 组件").into())
}

/// 读活世界里那个实体的 `Text` 组件内容（渲染画的正是这一份）。
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

// ---------- Lua 取值 ----------

fn resolve_project() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let project = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        rustlua_infra::games_dir()
            .join("examples")
            .join("game_jhyt")
    });
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
