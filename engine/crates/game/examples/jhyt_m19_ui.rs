//! Headless acceptance drive for M19-02 《江湖有田》游戏 UI、音频与新手引导.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` and
//! reproduces the M19-02 acceptance paths:
//!
//! - ①音频：16 个自研合成 MP3 存在（ID3/MPEG 帧头检查、非空；采样率/位深/声道由生成器保证）；Lua 侧
//!   `audio.play/music/stop_music/volume` 与 `audio.sfx/play_bgm` 调用路径
//!   在无音频设备时也不抛错（pcall 静默降级），同一曲目不重启。
//! - ②HUD 文案包含生命/体力/内力/时间/天气/金钱/快捷栏字段。
//! - ③背包（I）/任务（L）/暂停（P）三个面板可打开、选择/使用/装备/调音量、
//!   关闭；面板可见性与 `state.mode` 一致；UI 输入不落到世界（位置不变）。
//! - ④last_action 错误文案映射（not_enough/tired/full/... 与未知值原样显示）。
//! - ⑤新手引导：新档从第 0 步开始、同一步重复触发不重复推进、十步剧本推进
//!   与一次性奖励幂等、存档往返后步骤保持。
//! - ⑥回归锚点：农田循环经 input/farm 真实路径仍可用、建造模式 HUD 文案仍含
//!   字面量 `building`（jhyt_m18_build 的既有断言）、统一数据校验仍通过。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m19_ui [-- <game_jhyt-root>]

use rustlua_input::input::{InputSnapshot, KeyCode};
use rustlua_scripting::script::LuaRuntime;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 自研合成音频清单（tools/gen_audio.py 生成；名称即运行时键名）。
const BGM_CLIPS: [&str; 4] = ["bgm_title", "bgm_manor", "bgm_village", "bgm_wilds"];
const SFX_CLIPS: [&str; 12] = [
    "ui_click",
    "ui_confirm",
    "ui_cancel",
    "sfx_hoe",
    "sfx_plant",
    "sfx_water",
    "sfx_harvest",
    "sfx_hit",
    "sfx_quest",
    "sfx_sleep",
    "sfx_craft",
    // M22：面板选中项变化播 ui_move（panel_nav.lua 调用；目录有 ui_move.mp3）。
    "ui_move",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        rustlua_infra::games_dir()
            .join("examples")
            .join("game_jhyt")
    });
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let main_script = project.join("scripts/main.lua");
    if !main_script.is_file() {
        return Err(format!("missing {}", main_script.display()).into());
    }
    let save_path = fresh_save_path(&project, "jhyt_m19_ui.sav");
    let project_arg = project.clone();

    banner("M19-02a 音频资源：16 个自研合成 MP3 存在（ID3/MPEG 帧头检查、非空）+ 调用路径不抛错");
    let audio_dir = project.join("assets").join("audio");
    let mut clips = 0;
    for name in BGM_CLIPS.iter().chain(SFX_CLIPS.iter()) {
        let path = audio_dir.join(format!("{name}.mp3"));
        let info = inspect_mp3(&path)?;
        println!(
            "{name}.mp3 bytes={} id3={} frame_sync={}",
            info.bytes, info.id3, info.frame_sync
        );
        clips += 1;
    }
    if clips != 16 {
        return Err(format!("expected 16 clips, inspected {clips}").into());
    }

    let (mut runtime, host) = new_runtime(&project_arg, &save_path)?;
    enter_playable(&mut runtime, &main_script)?;
    // Lua 侧调用路径：pcall 包裹后必须返回 true（无设备/缺文件都不得中断玩法）。
    assert_str(
        &mut runtime,
        "(function() local ok = pcall(function() \
           audio.volume(0.6); audio.play('ui_click'); audio.music('bgm_title'); \
           audio.music('bgm_title'); audio.stop_music(); audio.play('__missing__') \
         end); return tostring(ok) end)()",
        "true",
    )?;
    // audio.sfx / play_bgm 薄包装：永不抛错；同一曲目不重启，切图才换曲。
    assert_str(
        &mut runtime,
        "tostring(require('core.audio').sfx('ui_click'))",
        "true",
    )?;
    // enter_playable_map 已按地图起播 BGM。
    assert_str(
        &mut runtime,
        "require('core.audio').current_bgm()",
        "bgm_manor",
    )?;
    assert_str(&mut runtime, "require('core.state').mode", "Playing")?;
    assert_str(
        &mut runtime,
        "tostring((require('core.audio').play_bgm('village')))",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('core.audio').play_bgm('village'))",
        "same",
    )?;
    assert_str(
        &mut runtime,
        "require('core.audio').current_bgm()",
        "bgm_village",
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('core.audio').play_bgm(nil))",
        "unknown_map",
    )?;
    // 未登记地图回落到 wilds（新增地图不会静音）。
    assert_str(
        &mut runtime,
        "tostring((require('core.audio').play_bgm('some_new_map')))",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "require('core.audio').current_bgm()",
        "bgm_wilds",
    )?;
    assert_str(
        &mut runtime,
        "tostring((require('core.audio').play_bgm('manor')))",
        "true",
    )?;
    // 音量入口：模块是唯一事实来源（原始 audio.volume 只用于上面的 pcall 探针）。
    assert_str(
        &mut runtime,
        "tostring(require('core.audio').set_volume(0.6))",
        "true",
    )?;
    assert_num(&mut runtime, "require('core.audio').volume()", 0.6)?;
    // 音量随存档持久化（provider "audio"）。
    assert_str(
        &mut runtime,
        "(function() require('core.audio').set_volume(0.45); \
          local snap = require('core.save_manager').capture(); \
          require('core.audio').set_volume(1.0); \
          require('core.save_manager').restore(snap); \
          return tostring(require('core.audio').volume()) end)()",
        "0.45",
    )?;

    banner("M19-02b HUD：生命/体力/内力/时间/天气/金钱/快捷栏字段");
    // 验收项①的口径是"这些数据在 HUD 上看得见"，不是"字段名逐个印成标签"：
    // M33 把面板拆成 6 行后，时间行写作 `第1天 春 00:00`（不再打"时间"二字），
    // 地图名改由场景自带的 `HudTitle` 显示，"快捷栏"这个纯标签也不再生占预算——
    // 因此这里改成"标签词表 + 时刻格式"两路检查，数据一条不少。
    assert_str(
        &mut runtime,
        "(function() local t = require('ui.hud').text(); \
          for _, w in ipairs({ '生命', '体力', '内力', '天气', '金钱', '武器', '工具', '种子' }) do \
            if not t:find(w, 1, true) then return 'missing:' .. w end end \
          if not t:find('第%d+天') then return 'missing:day' end \
          if not t:find('%d+:%d%d') then return 'missing:clock' end \
          return 'ok' end)()",
        "ok",
    )?;
    assert_num(&mut runtime, "#(require('ui.hud').status_lines())", 6.0)?;
    let hud_line = query_str(&mut runtime, "require('ui.hud').text()")?;
    println!("hud.text() = {hud_line}");
    // update() 返回值仍携带 HUD 文案（既有 headless 断言的兼容面）。
    assert_str(
        &mut runtime,
        "require('ui.input').update():find('气血', 1, true) ~= nil and 'ok' or 'none'",
        "ok",
    )?;

    // **快捷栏读回真实几何**（2026-10-05）：上面那条 `text()` 只证明"招式这格数据在 HUD
    // 里看得见"，证明不了"卡片画在哪"。2×4 之前的单行排法把 8 张 48px 卡画到设计 x=1404，
    // 而 `layout.DESIGN_W` 是 1280 —— 第 6 张被切一半、7/8 完全看不见，**而且两侧都不报错**
    // （`layout.audit()` 只查 `M.PANEL` 注册过的面板，招式卡是 `ui.image` 不是面板）。
    // M70/B7：卡底已搬进 `hud.prefab`（HudSkillCard1..8，几何与 `skill_slots()` 同源公式），
    // 每帧由 `hud.draw` 按槽数 `set_visible` 显隐；几何正确性与画布内已由 `jhyt_m70_hud_prefab`
    // 黄金表逐块对账覆盖。这里守住迁移后的新形态：**每个玩家槽位都有一张卡底实体**。
    //
    // 张数的来源从 `data.skills.ids()` 换成 `hud.skill_slots()`：卡片从此按
    // `data/skills.lua` 的**玩家按键绑定表**排（cast → dash → guard），不再是招式字典序。
    // 字典序那次扩招把「普攻」排到第 8 张、还把 Boss 专属的「山魈冲撞」画成了玩家的卡，
    // 而触屏要表达"第 3 张 = 弦月斩"根本没有依据 —— 现在这个依据就是那张表。
    let skill_bar = query_str(
        &mut runtime,
        "(function() local slots = require('ui.hud').skill_slots(); \
          local h = require('ui.hud').preset_ref(); \
          if not h then return 'preset_not_spawned' end; \
          for i = 1, #slots do \
            local e = prefab.node(h, 'HudSkillCard' .. i); \
            if not entity.is_alive(e) then return 'not_found:card' .. i end; \
          end; \
          return 'ok|cards=' .. #slots end)()",
    )?;
    if !skill_bar.starts_with("ok|") {
        return Err(format!("招式快捷栏卡底实体缺失（预设未按槽数铺满）: {skill_bar}").into());
    }
    println!("skill_bar = {skill_bar}");
    // HUD 必须为**每一个玩家槽位**都画一张卡（漏一张 = 那一招在触屏上根本没有入口）。
    let slot_count = query_num(&mut runtime, "#(require('data.skills').player_slots())")? as i64;
    assert_num(
        &mut runtime,
        "#(require('ui.hud').skill_slots())",
        slot_count as f64,
    )?;
    // `hud.reset()` 必须把**画过的每一张**都收掉：曾经只清 1..5 号 id，招式扩到 8 张后
    // 6..8 在切场景/切模式时留在屏上（贴图不在每帧登记表里，画过就一直在）。
    // M70/B7：卡底在预设常驻（reset 不清），动态层只余招式图标/名，reset 收的就是它们。
    assert_str(
        &mut runtime,
        "(function() require('ui.hud').reset(); \
          for i = 1, #require('ui.hud').skill_slots() do \
            if ui.image_get('hud:skill_icon' .. i) then return 'leftover:icon' .. i end end; \
          return 'ok' end)()",
        "ok",
    )?;
    // 收完再画一遍，后面的面板小节仍在玩法态 HUD 上跑。
    assert_str(
        &mut runtime,
        "require('ui.input').update():find('气血', 1, true) ~= nil and 'ok' or 'none'",
        "ok",
    )?;

    banner("M34-05 触屏招式卡：点第 i 张 = 注入第 i 项动作，且真的出了它绑的那一招");
    // 无头宿主没有指针事件，而 `M.update()` 在 `app.platform()` 报告 `desktop` 时整条早退，
    // 所以这里走 `virtual_pad.handle_pointer()` 这条**测试注入口**：它和 `update()` 共用
    // 同一段命中测试代码，只是坐标由调用方给、且不受平台门限制。
    // 每按一张前做三件事（都在 `tap_skill_card` 里），否则这道门会假绿：
    //   * `set_input(default)` 清掉上一张的**按下边沿**（真实宿主每帧换新快照 = 同一口径；
    //     不清则 `pressed_keys` 累积，`input.lua` 那条"一帧只出一招"永远命中第 1 张）；
    //   * `combat.reset()` 清空攻击/位移记录（不清则分辨不出"这张卡出了招"和"上一张的残留"）；
    //   * `inner_power = 100` 把内力灌满（付不起 → `player_attack` 返回 false，
    //     那就成了"卡没问题、池子空了"的假红）。
    for index in 1..=slot_count {
        let want = query_str(
            &mut runtime,
            &format!(
                "(function() local s = require('ui.hud').skill_slots()[{index}]; \
                  local def = require('data.skills').get(s.skill); \
                  return s.action .. '|' .. s.skill .. '|' .. tostring(def.kind) end)()"
            ),
        )?;
        let mut parts = want.split('|');
        let action = parts.next().unwrap_or_default().to_owned();
        let skill = parts.next().unwrap_or_default().to_owned();
        let kind = parts.next().unwrap_or_default().to_owned();
        // 格挡是"按住才站得住"，所以那一格观测的是姿态本身；其余观测出的那招的 id。
        let expected = if kind == "guard" {
            format!("{action}|true")
        } else {
            format!("{action}|{skill}")
        };
        let got = tap_skill_card(&mut runtime, &host, index)?;
        if got != expected {
            return Err(format!(
                "第 {index} 张招式卡（{want}）注入后没有出对应的招：期望 {expected}，实得 {got}"
            )
            .into());
        }
        println!("skill_card {index} = {got} (kind={kind})");
    }
    println!("touch_skill_cards={slot_count}");
    // 快点击口径：按下与抬起落在**同一帧**时（web/微信的触摸适配层就是这样——touchStart 与
    // touchEnd 之间没跑到 rAF，`mouse_just_pressed` 和 `mouse_just_released` 同帧为真），
    // 命中测试必须当场补一次释放。不补的话 `card_down`/`action_down`/`panel_action_down`
    // 会一直留着、那只键**永远松不掉**（格挡永久贴住、面板十字键一直往下滚）。
    // 必须**一族一测、测完就 release_all**：只数"最后一族"是瞎的——下一族的任一次命中都会
    // 走进 `if *_down then if released then release` 那支，把上一族漏下的键顺手擦掉
    // （实测：删掉招式卡的同帧补释放，全量末尾读数是 `0|`，删的是门该咬的那一族却看不见）。
    // 反面对照（2026-10-05，逐族口径）：删掉 `hit_test_cards` 里的 `if released then card_release`
    // → 招式卡一族报 `1|guard`——其余六张被后一张的命中循环顺手释放掉了，只有最后一张没人接手，
    // 这正是"永久贴住"的真实形状（长按格挡会一直挡着）。
    for (family, tap_loop, actions) in [
        (
            "招式卡",
            "for _, slot in ipairs(require('ui.hud').skill_slots()) do \
               vp.handle_pointer(slot.x + slot.w / 2, slot.y + slot.h / 2, true, true) end",
            "'attack','skill','skill_2','skill_3','skill_4','dash','guard'",
        ),
        (
            "动作盘",
            "local m = require('ui.layout').TOUCH.more; \
               local mx, my = m.x + m.size / 2, m.y + m.size / 2; \
               for _, slot in ipairs(vp.action_slots()) do \
                 vp.handle_pointer(mx, my, true, true); \
                 vp.handle_pointer(slot.x, slot.y, true, true) end",
            "'sleep','build','place','practice','demolish','use','gift','seed','respawn'",
        ),
        (
            "面板动作条",
            "local st = require('core.state'); \
               for _, mode in ipairs({ 'Inventory', 'Shop', 'Pause' }) do \
                 st.mode = mode; \
                 for _, slot in ipairs(vp.panel_action_slots()) do \
                   vp.handle_pointer(slot.x, slot.y, true, true) end end; \
                 st.mode = 'Playing'",
            "'interact','seed','place','volume_up','volume_down'",
        ),
    ] {
        let got = query_str(
            &mut runtime,
            &format!(
                "(function() local vp = require('ui.virtual_pad'); \
                  local held = 0; local names = {{}}; \
                  {tap_loop}; \
                  for _, action in ipairs({{ {actions} }}) do \
                    if input.action(action) then held = held + 1; names[#names + 1] = action end \
                  end; \
                  vp.release_all(); \
                  return held .. '|' .. table.concat(names, ',') end)()"
            ),
        )?;
        if !got.starts_with("0|") {
            return Err(format!(
                "{family} 同帧按下+抬起后仍有键处在按住态（那只手指永远松不掉）: {got}"
            )
            .into());
        }
        println!("coalesced_tap {family} = {got}");
    }

    banner("M34-06 触屏「更多」键 + 动作盘：九格各自动作、盘随点击展开/收起");
    // 与批 1 同一个注入口（`virtual_pad.handle_pointer`），差别在对象：「更多」键 + 3×3 动作盘。
    // **通用口径只测注入**：按下 → 该动作 `down=true` 且其余八格全 `false` → 抬手。
    // 为什么通用循环故意不跑 `ui.input.update()`：`睡觉` 那格一进分发就 `day.sleep()`，
    // 而本小节跑在 `set_save_path`（下面的临时档）之前 —— 真跑一次会把开发档写脏。
    // "注入之后游戏真的做了那件事"由下面两条**显式效果探针**（运气 +5 内力 / 建造翻转建造模式）
    // 证明；其余七格的效果由各自的键盘门覆盖（m18/m28 按 B/F/X/U/T/C/G/R 逐条断言），
    // 触屏→动作 与 动作→效果 两段接起来才是"触屏 = 键鼠"。
    let action_count = query_num(&mut runtime, "#require('ui.virtual_pad').action_slots()")?;
    if action_count != 9.0 {
        return Err(format!("动作盘应是 3×3 = 9 格，实得 {action_count}").into());
    }
    for index in 1..=action_count as i64 {
        let want = query_str(
            &mut runtime,
            &format!("require('ui.virtual_pad').action_slots()[{index}].action"),
        )?;
        let got = tap_action_disk(&mut runtime, &host, index, false)?;
        let expected = format!("{want}|true|0");
        if got != expected {
            return Err(format!(
                "动作盘第 {index} 格（{want}）注入后动作不对：期望 {expected}，实得 {got}（串键/漏键）"
            )
            .into());
        }
        println!("action_disk {index} = {got}");
    }
    // 「更多」键：点一下展开、再点一下收起（展开态由 `is_action_open()` 报，不靠截图）。
    let more_toggle = query_str(
        &mut runtime,
        "(function() local vp = require('ui.virtual_pad'); local m = require('ui.layout').TOUCH.more; \
          local cx, cy = m.x + m.size / 2, m.y + m.size / 2; \
          local first = vp.handle_pointer(cx, cy, true, false); \
          vp.handle_pointer(cx, cy, false, true); \
          local opened = tostring(vp.is_action_open()); \
          vp.handle_pointer(cx, cy, true, false); \
          vp.handle_pointer(cx, cy, false, true); \
          return tostring(first) .. '|' .. opened .. '|' .. tostring(vp.is_action_open()) end)()",
    )?;
    if more_toggle != "more|true|false" {
        return Err(format!(
            "「更多」键开关不对：期望 more|true|false（点一下展开、再点收起），实得 {more_toggle}"
        )
        .into());
    }
    println!("more_key = {more_toggle}");
    // 按下一格后盘自动收起（一次点击 = 一个动作，九块盘不常驻盖住世界画面）。
    let _ = tap_action_disk(&mut runtime, &host, 1, false)?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.virtual_pad').is_action_open())",
        "false",
    )?;
    // 面板态：「更多」与盘全部退出（HUD 在面板态还画着卡片，点它该做列表操作而不是出手）。
    let more_in_panel = query_str(
        &mut runtime,
        "(function() local vp = require('ui.virtual_pad'); local m = require('ui.layout').TOUCH.more; \
          require('core.state').mode = 'Inventory'; \
          local fired = vp.handle_pointer(m.x + m.size / 2, m.y + m.size / 2, true, false); \
          require('core.state').mode = 'Playing'; \
          return tostring(fired) .. '|' .. tostring(vp.is_action_open()) end)()",
    )?;
    if more_in_panel != "nil|false" {
        return Err(
            format!("面板态点「更多」不该有反应：期望 nil|false，实得 {more_in_panel}").into(),
        );
    }
    println!("more_in_panel = {more_in_panel}");
    // 效果探针一（运气，动作盘第 4 格）：内力 +5，与键盘按 Q 同一读数。
    lua(&mut runtime, "require('core.state').inner_power = 50")?;
    let _ = tap_action_disk(&mut runtime, &host, 4, true)?;
    assert_num(&mut runtime, "require('core.state').inner_power", 55.0)?;
    // 效果探针二（建造，第 2 格）：翻转建造模式，探针后复原。
    let _ = tap_action_disk(&mut runtime, &host, 2, true)?;
    assert_str(
        &mut runtime,
        "tostring(require('systems.building').build_mode())",
        "true",
    )?;
    let _ = tap_action_disk(&mut runtime, &host, 2, true)?;
    assert_str(
        &mut runtime,
        "tostring(require('systems.building').build_mode())",
        "false",
    )?;
    println!("touch_action_disk=9");

    banner("M34-07 触屏面板动作条：装备/选种/换页/数量·音量± 逐格注入 + 三条真实分发");
    // 批 3 的对象是**面板态**里"键盘有键、触屏没有入口"的五个动作（E 装备 / C 选种 /
    // F 换页 / +/- 数量与音量）。它们与动作盘共用一个注入口（`virtual_pad.handle_pointer`），
    // 差别在**每个模式的格数不同**（背包 2 / 商店 3 / 暂停 2），所以逐模式验三件事：
    //   ① 格数——模式名一旦拼错，`panel_action_slots()` 返回空表，下面的循环零次也能"绿"；
    //      反面对照（2026-10-05）：`layout.TOUCH.panel_actions.modes` 里把 `Inventory` 写成
    //      `InventoryX` → `Inventory 面板动作条应是 2 格，实得 0`（几何门 ⑧ 看不见这个错，
    //      它按表里**实际有的**模式名遍历，拼错的模式它照样数得出 2 格）。
    //   ② 编号口径——pad 的格心必须与 `layout.TOUCH.panel_actions` 的 cols×rows **行主序**
    //      逐格相等，否则几何门查的那些圈和手上点的那些圈不是同一个东西；
    //   ③ 逐格注入——按下 → 只有这一格的动作 down、同条其余格 0 个 → 抬手。
    // 通用循环照批 2 的口径不跑分发（分发会动音量/装备/商店页签，留给下面三条显式探针，
    // 每条探针后复原面板与原值，免得把开发档写脏）。
    let strips: [(&str, &[&str]); 3] = [
        ("Inventory", &["interact", "seed"]),
        ("Shop", &["place", "volume_up", "volume_down"]),
        ("Pause", &["volume_up", "volume_down"]),
    ];
    for (mode, actions) in strips {
        let count = query_num(
            &mut runtime,
            &format!(
                "(function() local st = require('core.state'); st.mode = '{mode}'; \
                  local n = #require('ui.virtual_pad').panel_action_slots(); \
                  st.mode = 'Playing'; return n end)()"
            ),
        )?;
        if count != actions.len() as f64 {
            return Err(format!(
                "{mode} 面板动作条应是 {} 格，实得 {count}（模式名拼错=静默没有入口）",
                actions.len()
            )
            .into());
        }
        for (offset, want) in actions.iter().enumerate() {
            let got = tap_panel_strip(&mut runtime, &host, mode, (offset + 1) as i64, false)?;
            let expected = format!("{want}|true|0");
            if got != expected {
                return Err(format!(
                    "{mode} 动作条第 {} 格（{want}）注入后动作不对：期望 {expected}，实得 {got}（串键/漏键）",
                    offset + 1
                )
                .into());
            }
            println!("panel_strip {mode} {} = {got}", offset + 1);
        }
    }
    // ② 行主序对账：第 index 格的圆心 = cols[((index-1) mod #cols)] × rows[((index-1) div #cols)]。
    // 反面对照（2026-10-05）：把 pad 里的 `cols[col]/rows[row]` 对调成 `cols[row]/rows[col]`
    // → `mismatch@2`。**买不到的部分**：这张格阵是 2×2 正方形，只把两层循环的**边界**对调
    // （`row = 1,#cols`）读数完全不变、门照绿——正方形阵列上"行列对调"只有落到取值下标才看得出。
    let strip_index = query_str(
        &mut runtime,
        "(function() local l = require('ui.layout').TOUCH.panel_actions; \
          local st = require('core.state'); st.mode = 'Shop'; \
          local slots = require('ui.virtual_pad').panel_action_slots(); st.mode = 'Playing'; \
          for index = 1, #slots do \
            local col = ((index - 1) % #l.cols) + 1; \
            local row = math.floor((index - 1) / #l.cols) + 1; \
            local slot = slots[index]; \
            if slot.x ~= l.cols[col] or slot.y ~= l.rows[row] then \
              return 'mismatch@' .. index \
            end \
          end; \
          return 'ok' end)()",
    )?;
    if strip_index != "ok" {
        return Err(format!(
            "面板动作条编号与 layout 的 cols×rows 行主序不一致：{strip_index}（画出来的格和点得到的格不是同一批）"
        )
        .into());
    }
    println!("panel_strip_index = {strip_index}");
    // 效果探针一（背包「装备」格）：触屏点第 1 格 → 面板真的装备了选中行，
    // 与 M19-02c 键盘按 E 的读数一致（tool=watering_can / last_action=equip_watering_can）。
    lua(&mut runtime, "require('ui.inventory_ui').open()")?;
    let can_row = row_of(&mut runtime, "ui.inventory_ui", "watering_can")?;
    lua(
        &mut runtime,
        &format!("require('ui.inventory_ui').select({})", can_row - 1),
    )?;
    let equip_fire = tap_panel_strip(&mut runtime, &host, "Inventory", 1, true)?;
    if equip_fire != "interact|true|0" {
        return Err(
            format!("背包「装备」格注入不对：期望 interact|true|0，实得 {equip_fire}").into(),
        );
    }
    assert_str(
        &mut runtime,
        "inv.equip_list(require('core.state').inventory).tool",
        "watering_can",
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "equip_watering_can",
    )?;
    lua(&mut runtime, "require('ui.inventory_ui').close()")?;
    // 效果探针二（商店「换页」格）：buy → sell（第 1 格 = place，键盘 F 同一动作）。
    lua(
        &mut runtime,
        "require('systems.shop').open_ui('zhao_stall')",
    )?;
    assert_str(&mut runtime, "require('systems.shop').tab()", "buy")?;
    let tab_fire = tap_panel_strip(&mut runtime, &host, "Shop", 1, true)?;
    if tab_fire != "place|true|0" {
        return Err(format!("商店「换页」格注入不对：期望 place|true|0，实得 {tab_fire}").into());
    }
    assert_str(&mut runtime, "require('systems.shop').tab()", "sell")?;
    lua(&mut runtime, "require('systems.shop').close_ui()")?;
    // 效果探针三（暂停「音量+」格）：0.5 → 0.6（+0.1 = `audio.VOLUME_STEP`），探针后回 0.5——
    // M19-02e 会把主音量随存档写盘，留在这里的不复原值会污染开发档。
    lua(&mut runtime, "require('core.audio').set_volume(0.5)")?;
    lua(&mut runtime, "require('ui.pause_menu').open()")?;
    let volume_before = query_num(&mut runtime, "require('core.audio').volume()")?;
    let volume_fire = tap_panel_strip(&mut runtime, &host, "Pause", 1, true)?;
    if volume_fire != "volume_up|true|0" {
        return Err(
            format!("暂停「音量+」格注入不对：期望 volume_up|true|0，实得 {volume_fire}").into(),
        );
    }
    let volume_after = query_num(&mut runtime, "require('core.audio').volume()")?;
    if (volume_after - (volume_before + 0.1)).abs() > 1e-6 {
        return Err(format!(
            "音量 + 格未生效（面板分发链路没吃到这个动作）: {volume_before} -> {volume_after}"
        )
        .into());
    }
    lua(
        &mut runtime,
        &format!("require('core.audio').set_volume({volume_before})"),
    )?;
    lua(&mut runtime, "require('ui.pause_menu').close()")?;
    println!("touch_panel_strip=7 dispatch=3");

    banner("M19-02c 背包面板：I 打开 → 选择/使用/装备 → 关闭");
    // 预置测试物品：回复药（可使用）+ 水壶（新档自带，可装备到 tool 槽）。
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'heal_pill', 1); require('core.state').energy = 50",
    )?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('i').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Inventory")?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.inventory_ui').ui_active())",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "(function() local h = require('ui.inventory_ui').preset_ref(); \
          return tostring(h ~= nil and entity.is_alive(prefab.node(h, 'InvTitle'))) end)()",
        "true",
    )?;
    // 面板打开期间 UI 输入不得落到世界（位置不变）。
    let (px, py) = player_pos(&mut runtime)?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('d').expect("QWERTY 字符必须有对应物理键"),
    )?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('w').expect("QWERTY 字符必须有对应物理键"),
    )?;
    let (px2, py2) = player_pos(&mut runtime)?;
    if (px - px2).abs() > f64::EPSILON || (py - py2).abs() > f64::EPSILON {
        return Err(format!("UI 面板期间玩家移动了: ({px},{py}) -> ({px2},{py2})").into());
    }
    // 数量/容量显示（文字内容由 jhyt_m67_inventory_prefab 黄金表覆盖，这里守实体存在）。
    assert_str(
        &mut runtime,
        "(function() local h = require('ui.inventory_ui').preset_ref(); \
          return tostring(h ~= nil and entity.is_alive(prefab.node(h, 'InvCap'))) end)()",
        "true",
    )?;
    // 装备：定位水壶行（1 基序号）→ 数字键选中 → E 装备。
    let water_row = row_of(&mut runtime, "ui.inventory_ui", "watering_can")?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char(digit(water_row)).expect("数字行号必须有对应主键区数字键"),
    )?;
    assert_num(
        &mut runtime,
        "require('ui.inventory_ui').index()",
        (water_row - 1) as f64,
    )?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "inv.equip_list(require('core.state').inventory).tool",
        "watering_can",
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "equip_watering_can",
    )?;
    // 换回锄头也必须走背包面板（2026-10-04）：`inv.equip` 只给槽位打标签，
    // 而行只来自槽位——只在新档写标签、不发放堆叠的工具（原先的锄头）在这里
    // 根本选不到，翻土会永久不可用。这条断言就是那个缺口的回归。
    let hoe_row = row_of(&mut runtime, "ui.inventory_ui", "hoe")?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char(digit(hoe_row)).expect("数字行号必须有对应主键区数字键"),
    )?;
    press(&mut runtime, &host, KeyCode::KeyE)?;
    assert_str(
        &mut runtime,
        "inv.equip_list(require('core.state').inventory).tool",
        "hoe",
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "equip_hoe",
    )?;
    // 使用：回复药 → 体力 50 → 90（成功路径消耗物品并给反馈文案）。
    let pill_row = row_of(&mut runtime, "ui.inventory_ui", "heal_pill")?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char(digit(pill_row)).expect("数字行号必须有对应主键区数字键"),
    )?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('u').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut runtime, "require('core.state').energy", 90.0)?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'heal_pill')",
        0.0,
    )?;
    assert_str(
        &mut runtime,
        "(function() local m = require('ui.inventory_ui').msg(); \
          return tostring(m ~= nil and m:find('使用', 1, true) ~= nil) end)()",
        "true",
    )?;
    // 不可使用物品的失败反馈：稻谷（无 use 字段）→ no_effect。
    let rice_row = row_of(&mut runtime, "ui.inventory_ui", "rice")?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char(digit(rice_row)).expect("数字行号必须有对应主键区数字键"),
    )?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('u').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_str(
        &mut runtime,
        "(function() local m = require('ui.inventory_ui').msg(); \
          return tostring(m ~= nil and m:find('没有使用效果', 1, true) ~= nil) end)()",
        "true",
    )?;
    // 关闭：面板与动态 UI 一起收起，模式回到 Playing。
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('i').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Playing")?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.inventory_ui').ui_active())",
        "false",
    )?;
    // 关闭后标题随根显隐收起（`set_visible(InvRoot,false)` 子树语义）——可见性由上面的
    // `ui_active()` 断言覆盖，标题文字内容由 jhyt_m67_inventory_prefab 黄金表覆盖。

    banner("M19-02d 任务面板（只读）：L 打开 → 列出任务与目标进度 → Q 关闭");
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('l').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Quest")?;
    // 面板行数必须等于任务注册表条目数（数据驱动：M19-01 追加订单/主线后仍有 8 条）。
    let quest_registry = query_num(&mut runtime, "return #require('data.quests.index').ids")?;
    assert_num(
        &mut runtime,
        "#(require('ui.quest_ui').rows())",
        quest_registry,
    )?;
    assert_str(
        &mut runtime,
        "(function() local h = require('ui.quest_ui').preset_ref(); \
          return tostring(h ~= nil and entity.is_alive(prefab.node(h, 'QuestTitle'))) end)()",
        "true",
    )?;
    // 进行中任务的目标进度可见（quest.list()/progress() 只读接口）。
    assert_str(
        &mut runtime,
        "(function() require('world.quest').accept('quest_herb'); \
          local mod = require('ui.quest_ui'); \
          for i, r in ipairs(mod.rows()) do \
            if r.id == 'quest_herb' then mod.select(i - 1) end end; \
          mod.render(); \
          local found = 'none'; \
          for _, o in ipairs(mod.objectives()) do \
            if o.target == 3 then found = tostring(o.current) .. '|' .. tostring(o.label) end end; \
          return found end)()",
        "0|采集灵芝",
    )?;
    assert_str(
        &mut runtime,
        "require('world.quest').state('quest_herb')",
        "in_progress",
    )?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('q').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Playing")?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.quest_ui').ui_active())",
        "false",
    )?;

    banner("M19-02e 暂停菜单：P 打开 → 音量 ± → 继续关闭");
    let volume_before = query_num(&mut runtime, "require('core.audio').volume()")?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('p').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Pause")?;
    // M21 主角形象：暂停菜单新增「形象」行（原 5 行 → 6 行），
    // 标签带当前形象的中文名，按确认在该行轮换形象。
    // M33：再加「帮助」行（原 6 行 → 7 行）——常驻 HUD 的键位通栏撤下后，
    // 完整操作说明住在这页里（`jhyt_m30_cjk_text` 断言它真的画出来了）。
    assert_num(&mut runtime, "#(require('ui.pause_menu').rows())", 7.0)?;
    assert_str(
        &mut runtime,
        "(function() for _, r in ipairs(require('ui.pause_menu').rows()) do \
           if r.key == 'appearance' then return r.label end end; return 'missing' end)()",
        "形象  青衫少侠",
    )?;
    assert_str(
        &mut runtime,
        "(function() for _, r in ipairs(require('ui.pause_menu').rows()) do \
           if r.key == 'help' then return r.label end end; return 'missing' end)()",
        "帮助",
    )?;
    assert_str(
        &mut runtime,
        "(function() local h = require('ui.pause_menu').preset_ref(); \
          return tostring(h ~= nil and entity.is_alive(prefab.node(h, 'PseTitle'))) end)()",
        "true",
    )?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('+').expect("QWERTY 字符必须有对应物理键"),
    )?;
    let volume_after = query_num(&mut runtime, "require('core.audio').volume()")?;
    if (volume_after - (volume_before + 0.1)).abs() > 1e-6 {
        return Err(format!("音量 + 未生效: {volume_before} -> {volume_after}").into());
    }
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('-').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(
        &mut runtime,
        "require('core.audio').volume()",
        volume_before,
    )?;
    // 保存进度（真实写盘路径）→ 菜单反馈。
    assert_str(
        &mut runtime,
        "tostring((require('ui.pause_menu').save_game()))",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "(function() local m = require('ui.pause_menu').msg(); \
          return tostring(m ~= nil and m:find('已保存', 1, true) ~= nil) end)()",
        "true",
    )?;
    // 读取进度：改动 → load_game() → provider 恢复 + 本图 overlay 重放，
    // 读取成功后自动回到玩法态（main.enter_playable_map 复位面板与模式）。
    lua(&mut runtime, "require('core.state').money = 999999")?;
    assert_str(
        &mut runtime,
        "tostring((require('ui.pause_menu').load_game()))",
        "true",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 120.0)?;
    assert_mode(&mut runtime, "Playing")?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.pause_menu').ui_active())",
        "false",
    )?;
    // 重新打开 → 继续游戏（首项）→ 关闭。
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('p').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Pause")?;
    press(&mut runtime, &host, KeyCode::Space)?;
    assert_mode(&mut runtime, "Playing")?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.pause_menu').ui_active())",
        "false",
    )?;

    banner("M19-02f last_action → 可读中文反馈（未知值带中文标签兜底，不再裸露英文键）");
    for (action, expected) in [
        ("not_enough", "材料/数量不足"),
        ("tired", "体力不足"),
        ("full", "背包已满"),
        ("wrong_season", "季节不符"),
        ("crafting", "制作中"),
        ("no_seed", "没有种子"),
    ] {
        assert_str(
            &mut runtime,
            &format!("require('ui.hud').feedback('{action}')"),
            expected,
        )?;
    }
    assert_str(
        &mut runtime,
        "require('ui.hud').feedback('build_blocked_solid@14,4')",
        "建造受阻：已有阻挡 @ 14,4",
    )?;
    assert_str(
        &mut runtime,
        "require('ui.hud').feedback('build_blocked_out_of_bounds@-3,-3')",
        "建造受阻：超出地图范围 @ -3,-3",
    )?;
    // M26：未知取值不再原样露出英文键（docs/37 4-13），改为带中文标签的兜底文案，
    // 原值仍保留在括号里便于诊断（`jhyt_m26_hud_feedback` 守住这条不变量）。
    assert_str(
        &mut runtime,
        "require('ui.hud').feedback('totally_unknown_action')",
        "未知反馈（totally_unknown_action）",
    )?;
    // 中文不溢出：M30 起按**引擎真实字宽**排版（`ui.text_fit` ← `ui.measure_text`），
    // 不再是"每个 CJK 记 2 单位"的启发式。CJK 在 14px 字号下的推进宽度 = 14 + 1（字距）.
    assert_num(
        &mut runtime,
        "require('ui.text_fit').width('江湖', 14)",
        30.0,
    )?;
    assert_num(
        &mut runtime,
        "require('ui.text_fit').width('ab', 14) < require('ui.text_fit').width('江湖', 14) and 1 or 0",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('ui.text_fit').fit('一二三四五六七八九十', 14, 60)",
        "一二三…",
    )?;
    // 裁剪结果必须真的放得下（含省略号）：这条不变量由 text_fit 自己保证。
    assert_str(
        &mut runtime,
        "tostring(require('ui.text_fit').width(\
require('ui.text_fit').fit('一二三四五六七八九十', 14, 60), 14) <= 60)",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "require('ui.text_fit').fit('abc', 14, 60)",
        "abc",
    )?;

    banner("M19-02g 新手引导：幂等推进 + 十六步剧本 + 存档往返");
    lua(&mut runtime, "require('ui.tutorial').reset()")?;
    assert_num(&mut runtime, "require('ui.tutorial').index()", 0.0)?;
    assert_str(&mut runtime, "require('ui.tutorial').step().id", "till")?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').prompt():find('引导 1/16', 1, true) ~= nil)",
        "true",
    )?;
    // 同一步重复触发不重复推进（幂等）。
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').on_action('till'))",
        "true",
    )?;
    assert_num(&mut runtime, "require('ui.tutorial').index()", 1.0)?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').on_action('till'))",
        "false",
    )?;
    assert_num(&mut runtime, "require('ui.tutorial').index()", 1.0)?;
    // 逐步推进到「交付」之前。
    for action in ["plant_seed_lingzhi", "water_lingzhi"] {
        assert_str(
            &mut runtime,
            &format!("tostring(require('ui.tutorial').on_action('{action}'))"),
            "true",
        )?;
    }
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').on_day_start())",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').on_enter('village'))",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').on_action('dialogue'))",
        "true",
    )?;
    // 接取任务（步骤 7）由任务状态轮询推进。
    assert_str(
        &mut runtime,
        "require('ui.tutorial').step().id",
        "accept_quest",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').on_action('none'))",
        "true",
    )?;
    assert_str(&mut runtime, "require('ui.tutorial').step().id", "deliver")?;
    // 存档往返：进度写 state.vars，随 state provider 恢复。
    assert_str(
        &mut runtime,
        "(function() local snap = require('core.save_manager').capture(); \
          local before = require('ui.tutorial').index(); \
          require('ui.tutorial').reset(); local zero = require('ui.tutorial').index(); \
          require('core.save_manager').restore(snap); \
          return tostring(before) .. '|' .. tostring(zero) .. '|' .. tostring(require('ui.tutorial').index()) end)()",
        "7|0|7",
    )?;
    // 交付步骤：目标达成并交付后由轮询推进。
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'lingzhi', 3)",
    )?;
    assert_str(
        &mut runtime,
        "tostring((require('world.quest').deliver('quest_herb')))",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').on_action('none'))",
        "true",
    )?;
    assert_str(&mut runtime, "require('ui.tutorial').step().id", "craft")?;
    // 既有九步链末步：制作收取 → 推进到阶段5 新增「首次种植闭环」教学段第 1 步。
    let pills_before = query_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'heal_pill')",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').on_action('craft_collect_heal_pill'))",
        "true",
    )?;
    // 0 基索引：craft=8（第 9 步），craft_collect 后推进到 9 = claim_seed（阶段5 新段第 1 步）。
    assert_num(&mut runtime, "require('ui.tutorial').index()", 9.0)?;
    assert_str(
        &mut runtime,
        "require('ui.tutorial').step().id",
        "claim_seed",
    )?;
    // 阶段5（docs/33 3.2）新增六步：领种子→翻土→播种→浇水→入睡→收获→终止发奖。
    for (action, next_id) in [
        ("select_seed_seed_radish", "till_loop"),
        ("till", "plant_loop"),
        ("plant_seed_radish", "water_loop"),
        ("water_radish", "sleep_loop"),
    ] {
        assert_str(
            &mut runtime,
            &format!("tostring(require('ui.tutorial').on_action('{action}'))"),
            "true",
        )?;
        assert_str(&mut runtime, "require('ui.tutorial').step().id", next_id)?;
    }
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').on_day_start())",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "require('ui.tutorial').step().id",
        "harvest_loop",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').on_action('harvest_radish'))",
        "true",
    )?;
    // finish 步骤是瞬态：advance 内 settle 当帧就把索引推到 16 并发奖——此时
    // `step()` 返回 nil，不能断言 `step().id == "done"`（会对 nil 索引崩溃）。
    // 直接断言终态：索引封顶 16、状态收尾、发奖幂等。
    assert_num(&mut runtime, "require('ui.tutorial').index()", 16.0)?;
    // 已完成后再触发不重复推进（幂等，事件直接拒绝）。
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').on_action('harvest_radish'))",
        "false",
    )?;
    assert_num(&mut runtime, "require('ui.tutorial').index()", 16.0)?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').status().active)",
        "false",
    )?;
    assert_str(&mut runtime, "require('ui.tutorial').prompt()", "nil")?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').status().rewarded)",
        "true",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'heal_pill')",
        pills_before + 1.0,
    )?;
    // 已完成后再触发不重复推进/发奖（幂等）。
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').on_action('craft_collect_heal_pill'))",
        "false",
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'heal_pill')",
        pills_before + 1.0,
    )?;
    // 老档（无 tutorial_step）不打扰：prompt 为 nil 且事件不写状态。
    assert_str(
        &mut runtime,
        "(function() require('core.state').vars['tutorial_step'] = nil; \
          return tostring(require('ui.tutorial').prompt()) .. '|' .. tostring(require('ui.tutorial').on_action('till')) \
            .. '|' .. tostring(require('core.state').vars['tutorial_step']) end)()",
        "nil|false|nil",
    )?;

    banner("M19-02h 回归锚点：农田循环 + 建造预览文案 + 统一数据校验");
    // 统一校验入口（含新增 tutorial 校验器）仍全通过。
    assert_str(
        &mut runtime,
        "(function() local report = require('validate_data').check({ strict = true }); \
          return report == '' and 'ok' or report end)()",
        "ok",
    )?;
    assert_str(
        &mut runtime,
        "(function() local ok, issues = require('data.tutorial').validate_all(); \
          return ok and 'ok' or table.concat(issues, ' | ') end)()",
        "ok",
    )?;
    // 建造模式 HUD 文案仍含字面量 building（jhyt_m18_build 的既有断言面）：
    // 放置一座药庐后，在其锚点格再次 can_place 必然冲突 "building"（确定性断言，不依赖
    // 玩家站位/朝向/碰撞推出——旧写法依赖"站在建筑下方朝上预览"，会随锚点格下方是否为
    // 可走格而漂移）。
    let (lx, ly) = find_legal_anchor(&mut runtime, "herb_hut")?;
    lua(
        &mut runtime,
        &format!("require('systems.building').place('herb_hut', {lx}, {ly})"),
    )?;
    assert_str(
        &mut runtime,
        &format!(
            "(function() local ok, c = require('systems.building').can_place('herb_hut', {lx}, {ly}); \
             for _, x in ipairs(c or {{}}) do if x.reason == 'building' then return 'ok' end end \
             return tostring(ok) end)()"
        ),
        "ok",
    )?;
    // M26：HUD 状态行的建造冲突改为中文（BUILD_REASON 表），不再拼英文 reason 键。
    // 断言真实渲染面 `hud.feedback_line()`（`build_blocked_<reason>@x,y` → 中文原因；
    // `input.update()` 的返回值不含反馈行——反馈行是独立 HUD 元素）。
    lua(
        &mut runtime,
        &format!("require('core.state').last_action = 'build_blocked_building@{lx},{ly}'"),
    )?;
    assert_str(
        &mut runtime,
        "require('ui.hud').feedback_line():match('已有建筑') ~= nil and 'ok' or 'none'",
        "ok",
    )?;
    lua(
        &mut runtime,
        "require('systems.building').set_build_mode(false)",
    )?;
    // 农田循环经 farm.act 真实路径仍可用（audio 接入未破坏成功分支）。
    let plot = find_farm_cell(&mut runtime)?;
    let (fx, fy) = plot;
    lua(
        &mut runtime,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'up'; \
             inv.equip(require('core.state').inventory, 'tool', 'hoe')",
            (fx as f32 + 0.5) * 32.0,
            (fy as f32 - 0.5) * 32.0
        ),
    )?;
    assert_str(&mut runtime, "require('systems.farm').act('up')", "till")?;
    assert_str(
        &mut runtime,
        "require('systems.farm').act('up'):match('^plant_') ~= nil and 'ok' or 'none'",
        "ok",
    )?;
    // 钉住晴天（M29 起天气按 `(seed, day, 季节)` 抽签）：雨天浇水的动作值是
    // `rain_watered`，不匹配下面的 `^water_` 前缀断言。
    lua(&mut runtime, "require('world.weather').set('sunny')")?;
    lua(
        &mut runtime,
        "inv.equip(require('core.state').inventory, 'tool', 'watering_can')",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.farm').act('up'):match('^water_') ~= nil and 'ok' or 'none'",
        "ok",
    )?;

    println!(
        "jhyt_m19_ui_ok project={} clips={} hud_lines={} inventory_rows={} quest_rows={} pause_rows={} tutorial_total={} volume={}",
        project.display(),
        clips,
        query_num(&mut runtime, "return #require('ui.hud').status_lines()")?,
        query_num(
            &mut runtime,
            "require('world.quest').accept('quest_spar'); return #require('ui.inventory_ui').rows()"
        )?,
        query_num(&mut runtime, "return #require('ui.quest_ui').rows()")?,
        query_num(&mut runtime, "return #require('ui.pause_menu').rows()")?,
        query_num(&mut runtime, "return #require('data.tutorial').steps")?,
        query_num(&mut runtime, "return require('core.audio').volume()")?,
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers ----------

/// MP3 容器自检（2026-10-01 音频统一 MP3 后取代当年的 OGG 检查）。
///
/// 注意口径与 OGG 时代一致：MP3 是压缩容器，**无解码器**时无法在 Rust 侧复核
/// 采样率/位深/声道与振幅——这些参数由生成器 `tools/gen_audio.py` 保证；
/// 这里独立复核的是：文件存在且非空（> 1KB）、有 ID3v2 头或 MPEG 帧同步，
/// 且帧同步字段是合法组合（版本/层位非 0、码率与采样下标不是保留值）。
struct Mp3Info {
    bytes: u64,
    id3: bool,
    frame_sync: bool,
}

/// ID3v2 标签头的 4 字节 synchsafe 大小（每字节只用低 7 位）。
fn id3v2_size(bytes: &[u8]) -> usize {
    if bytes.len() < 10 || &bytes[0..3] != b"ID3" {
        return 0;
    }
    ((bytes[6] as usize) << 21)
        | ((bytes[7] as usize) << 14)
        | ((bytes[8] as usize) << 7)
        | (bytes[9] as usize)
}

/// 一个合法 MPEG 音频帧头：0xFF 前 3 位同步 + 版本位非 0 + 层位非 0 +
/// 码率/采样下标不是全 1 或全 0（这两者在 MPEG 规范里是保留值）。
fn mp3_frame_header(bytes: &[u8]) -> bool {
    bytes.windows(4).any(|w| {
        w[0] == 0xFF
            && (w[1] & 0xE0) == 0xE0
            && (w[1] & 0x18) != 0
            && (w[1] & 0x06) != 0
            && (w[2] & 0xF0) != 0xF0
            && (w[2] & 0xF0) != 0
            && (w[2] & 0x0C) != 0x0C
    })
}

fn inspect_mp3(path: &Path) -> Result<Mp3Info, Box<dyn std::error::Error>> {
    let bytes = fs::read(path).map_err(|e| format!("missing {}: {e}", path.display()))?;
    if bytes.len() < 1024 {
        return Err(format!("mp3 too small: {} ({} bytes)", path.display(), bytes.len()).into());
    }
    let id3 = bytes.starts_with(b"ID3");
    // 只在前 8 KB 里找帧头：ID3v2 标签之后的第一个帧必然落在这一窗口内。
    let start = (id3v2_size(&bytes) + if id3 { 10 } else { 0 }).min(bytes.len());
    let window = &bytes[start..bytes.len().min(start + 8192)];
    let frame_sync = mp3_frame_header(window);
    if !frame_sync {
        return Err(format!("no mpeg frame header: {}", path.display()).into());
    }
    Ok(Mp3Info {
        bytes: bytes.len() as u64,
        id3,
        frame_sync,
    })
}

/// Creates a bare engine runtime pointing at the project scripts, audio assets and save file.
fn new_runtime(
    project: &Path,
    save_path: &Path,
) -> Result<(LuaRuntime, rustlua_input::host::HostInput), Box<dyn std::error::Error>> {
    let (installers, handles) = rustlua_gameplay::input_lua::registry_with_handles();
    let host = handles.input;
    let mut runtime = LuaRuntime::with_installers(installers)?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // 音频目录固定为 <project>/assets/audio（`rustlua-audio` 的 audio.rs）。
    handles
        .audio
        .with_mut(|audio| audio.set_asset_dir(project.join("assets").join("audio")));
    // M32 横屏：设计分辨率 1280×720（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok((runtime, host))
}

/// Runs the project script, then loads the `main` 门面 scene and enters 山庄.
fn enter_playable(
    runtime: &mut LuaRuntime,
    main_script: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_project_script(main_script)?;
    runtime.execute_console("scene.load('main')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(runtime, "main")?;
    lua(runtime, "scene.load('manor')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(runtime, "manor")?;
    assert_mode(runtime, "Playing")?;
    Ok(())
}

/// 面板内某个物品的行号（0 基 index + 1；用于按数字键选中）。
fn row_of(
    runtime: &mut LuaRuntime,
    module: &str,
    item_id: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    let got = query_num(
        runtime,
        &format!(
            "(function() for i, r in ipairs(require('{module}').rows()) do \
             if r.item == '{item_id}' then return i end end return -1 end)()"
        ),
    )?;
    if got < 1.0 {
        return Err(format!("{module}: no row for {item_id}").into());
    }
    Ok(got as usize)
}

/// 1..9 的按键字符（面板数字键上限；超出则报错，测试数据不应超过 9 行）。
fn digit(index: usize) -> char {
    match index {
        1..=9 => char::from_digit(index as u32, 10).unwrap_or('1'),
        _ => '9',
    }
}

/// 扫描当前地图，返回第一个对该建筑可放置的锚点（确定性，与 jhyt_m18_build 同法）。
fn find_legal_anchor(
    runtime: &mut LuaRuntime,
    building_id: &str,
) -> Result<(i64, i64), Box<dyn std::error::Error>> {
    let exp = format!(
        "(function() local b = require('systems.building'); \
         for y = 0, 21 do for x = 0, 39 do \
           if b.can_place('{building_id}', x, y) then return x .. ',' .. y end end end return 'none' end)()"
    );
    let got = query_str(runtime, &exp)?;
    let mut parts = got.split(',');
    let x = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("no legal anchor for {building_id}: {got}"))?;
    let y = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("no legal anchor for {building_id}: {got}"))?;
    Ok((x, y))
}

fn find_farm_cell(runtime: &mut LuaRuntime) -> Result<(i64, i64), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        "(function() for y = 4, 13 do for x = 14, 33 do \
           local p = require('systems.farm'); \
           if not p.is_plot(x, y) then return x .. ',' .. y end end end return 'none' end)()",
    )?;
    let mut parts = got.split(',');
    let x = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("no farm cell: {got}"))?;
    let y = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("no farm cell: {got}"))?;
    Ok((x, y))
}

fn player_pos(runtime: &mut LuaRuntime) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let x = query_num(runtime, "require('player.player').position().x")?;
    let y = query_num(runtime, "require('player.player').position().y")?;
    Ok((x, y))
}

/// Simulates a single key press: one frame with the key down, one clean frame.
fn press(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    key: KeyCode,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = InputSnapshot::default();
    input.key_event(key, true);
    host.set_input(input);
    runtime.call_update()?;
    host.set_input(InputSnapshot::default());
    runtime.call_update()?;
    Ok(())
}

/// 点第 `index` 张招式卡（触屏命中测试注入口），返回 `"<动作>|<观测到的那招>"`。
///
/// 一次"点击"在这一个 Lua 块里走完（按下 → 一帧输入分发 → 观测 → 抬起），因为只有
/// `ui.input.update()` 才会把注入的键读成动作；格挡那格额外多走一步 `combat.tick()`
/// ——姿态是 tick 结算的，不吃 tick 就永远读到 false。
fn tap_skill_card(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    index: i64,
) -> Result<String, Box<dyn std::error::Error>> {
    host.set_input(InputSnapshot::default());
    query_str(
        runtime,
        &format!(
            "(function() local slot = require('ui.hud').skill_slots()[{index}]; \
              if not slot then return 'no_slot' end; \
              require('core.state').inner_power = 100; \
              require('systems.combat').reset(); \
              local cx, cy = slot.x + slot.w / 2, slot.y + slot.h / 2; \
              if require('ui.virtual_pad').handle_pointer(cx, cy, true, false) ~= slot.action then \
                return 'no_hit' \
              end; \
              require('ui.input').update(); \
              if slot.hold then \
                require('systems.combat').tick(); \
                local up = tostring(require('systems.combat').is_guarding()); \
                require('ui.virtual_pad').handle_pointer(cx, cy, false, true); \
                require('ui.input').update(); \
                require('systems.combat').tick(); \
                if require('systems.combat').is_guarding() then return 'guard_stuck' end; \
                return slot.action .. '|' .. up \
              end; \
              local atk = require('systems.combat').last_attack(); \
              local dash = require('systems.combat').last_dash(); \
              local got = 'none'; \
              if atk then got = tostring(atk.skill) elseif dash then got = tostring(dash.skill) end; \
              require('ui.virtual_pad').handle_pointer(cx, cy, false, true); \
              return slot.action .. '|' .. got end)()"
        ),
    )
}

/// 点动作盘第 `index` 格（先点「更多」展开），返回 `"<动作>|<该动作按下>|<其余格按下数>"`。
/// `dispatch = true` 时多跑一帧 `ui.input.update()`，让游戏侧的分发真的执行这个动作
/// （只验注入的通用循环传 `false`，见调用处注释）。
fn tap_action_disk(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    index: i64,
    dispatch: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    host.set_input(InputSnapshot::default());
    let step = if dispatch {
        "require('ui.input').update(); "
    } else {
        ""
    };
    query_str(
        runtime,
        &format!(
            "(function() local vp = require('ui.virtual_pad'); \
              local m = require('ui.layout').TOUCH.more; \
              vp.handle_pointer(m.x + m.size / 2, m.y + m.size / 2, true, false); \
              vp.handle_pointer(m.x + m.size / 2, m.y + m.size / 2, false, true); \
              if not vp.is_action_open() then return 'not_open' end; \
              local slot = vp.action_slots()[{index}]; \
              if not slot then return 'no_slot' end; \
              local fired = vp.handle_pointer(slot.x, slot.y, true, false); \
              {step} \
              local self_down = tostring(input.action(slot.action)); \
              local others = 0; \
              for _, s in ipairs(vp.action_slots()) do \
                if s.action ~= slot.action and input.action(s.action) then others = others + 1 end \
              end; \
              vp.handle_pointer(slot.x, slot.y, false, true); \
              return tostring(fired) .. '|' .. self_down .. '|' .. others end)()"
        ),
    )
}

/// 点面板动作条第 `index` 格，返回 `"<动作>|<该动作按下>|<同条其余格按下数>"`。
///
/// 与动作盘的差别只有一处：这一条的格表按 `state.mode` 取（背包/商店/暂停各一条），
/// 所以块内先把 mode 设成 `mode`、收尾再复位 `Playing`。`dispatch = true` 时面板必须
/// **真的开着**（`*_ui.open()` / `open_ui()`），否则分发那一帧的 `heal_mode` 会发现
/// "mode 是面板态但没有面板"而复位，动作被吞掉。
fn tap_panel_strip(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    mode: &str,
    index: i64,
    dispatch: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    host.set_input(InputSnapshot::default());
    let step = if dispatch {
        "require('ui.input').update(); "
    } else {
        ""
    };
    query_str(
        runtime,
        &format!(
            "(function() local vp = require('ui.virtual_pad'); local st = require('core.state'); \
              st.mode = '{mode}'; \
              local slot = vp.panel_action_slots()[{index}]; \
              if not slot then return 'no_slot' end; \
              local fired = vp.handle_pointer(slot.x, slot.y, true, false); {step} \
              local self_down = tostring(input.action(slot.action)); local others = 0; \
              for _, s in ipairs(vp.panel_action_slots()) do \
                if s.action ~= slot.action and input.action(s.action) then others = others + 1 end \
              end; \
              vp.handle_pointer(slot.x, slot.y, false, true); st.mode = 'Playing'; \
              return tostring(fired) .. '|' .. self_down .. '|' .. others end)()"
        ),
    )
}

fn lua(runtime: &mut LuaRuntime, code: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(code)?;
    Ok(())
}

fn query_str(runtime: &mut LuaRuntime, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    let value = runtime.execute_console(expr)?;
    Ok(value.trim().to_owned())
}

fn query_num(runtime: &mut LuaRuntime, expr: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let value = query_str(runtime, expr)?;
    value
        .parse::<f64>()
        .map_err(|e| format!("{value:?} not a number: {e}").into())
}

fn assert_str(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(runtime, &format!("return tostring({expr})"))?;
    if got != expected {
        return Err(format!("{expr} = {got:?}, want {expected:?}").into());
    }
    Ok(())
}

fn assert_num(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_num(runtime, &format!("return {expr}"))?;
    if (got - expected).abs() > 1e-6 {
        return Err(format!("{expr} = {got}, want {expected}").into());
    }
    Ok(())
}

fn assert_scene_is(
    runtime: &mut LuaRuntime,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let scene = query_str(runtime, "scene.reload()")?;
    if scene != expected {
        return Err(format!("scene = {scene:?}, want {expected:?}").into());
    }
    Ok(())
}

fn assert_mode(runtime: &mut LuaRuntime, expected: &str) -> Result<(), Box<dyn std::error::Error>> {
    assert_str(runtime, "require('core.state').mode", expected)
}

fn banner(title: &str) {
    println!("\n=== {title} ===");
}

fn fresh_save_path(project: &Path, name: &str) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(name);
    let _ = fs::remove_file(&path);
    path
}
