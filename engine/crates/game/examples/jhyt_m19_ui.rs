//! Headless acceptance drive for M19-02 《江湖有田》游戏 UI、音频与新手引导.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` and
//! reproduces the M19-02 acceptance paths:
//!
//! - ①音频：15 个自研合成 WAV 存在且为 22050Hz/16-bit/单声道/非空；Lua 侧
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

use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 自研合成音频清单（tools/gen_audio.py 生成；名称即运行时键名）。
const BGM_CLIPS: [&str; 4] = ["bgm_title", "bgm_manor", "bgm_village", "bgm_wilds"];
const SFX_CLIPS: [&str; 11] = [
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
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| engine::core::games_dir().join("examples").join("game_jhyt"));
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

    banner("M19-02a 音频资源：15 个 WAV 存在 + 22050Hz/16-bit/单声道/非空 + 调用路径不抛错");
    let audio_dir = project.join("assets").join("audio");
    let mut clips = 0;
    for name in BGM_CLIPS.iter().chain(SFX_CLIPS.iter()) {
        let path = audio_dir.join(format!("{name}.wav"));
        let info = inspect_wav(&path)?;
        println!(
            "{name}.wav ch={} rate={} bits={} frames={} bytes={} peak={}",
            info.channels, info.sample_rate, info.bits, info.frames, info.bytes, info.peak
        );
        clips += 1;
    }
    if clips != 15 {
        return Err(format!("expected 15 clips, inspected {clips}").into());
    }

    let mut runtime = new_runtime(&project_arg, &save_path)?;
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
    assert_str(
        &mut runtime,
        "(function() local t = require('ui.hud').text(); \
          for _, w in ipairs({ '生命', '体力', '内力', '时间', '天气', '金钱', '快捷栏', '武器', '种子', '地图' }) do \
            if not t:find(w, 1, true) then return 'missing:' .. w end end return 'ok' end)()",
        "ok",
    )?;
    assert_num(&mut runtime, "#(require('ui.hud').status_lines())", 4.0)?;
    let hud_line = query_str(&mut runtime, "require('ui.hud').text()")?;
    println!("hud.text() = {hud_line}");
    // update() 返回值仍携带 HUD 文案（既有 headless 断言的兼容面）。
    assert_str(
        &mut runtime,
        "require('ui.input').update():find('气血', 1, true) ~= nil and 'ok' or 'none'",
        "ok",
    )?;

    banner("M19-02c 背包面板：I 打开 → 选择/使用/装备 → 关闭");
    // 预置测试物品：回复药（可使用）+ 水壶（新档自带，可装备到 tool 槽）。
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'heal_pill', 1); require('core.state').energy = 50",
    )?;
    press(
        &mut runtime,
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
        "(function() local t = ui.get('bp:title'); \
          return tostring(t ~= nil and t.visible == true and t.text == '背包') end)()",
        "true",
    )?;
    // 面板打开期间 UI 输入不得落到世界（位置不变）。
    let (px, py) = player_pos(&mut runtime)?;
    press(
        &mut runtime,
        KeyCode::from_char('d').expect("QWERTY 字符必须有对应物理键"),
    )?;
    press(
        &mut runtime,
        KeyCode::from_char('w').expect("QWERTY 字符必须有对应物理键"),
    )?;
    let (px2, py2) = player_pos(&mut runtime)?;
    if (px - px2).abs() > f64::EPSILON || (py - py2).abs() > f64::EPSILON {
        return Err(format!("UI 面板期间玩家移动了: ({px},{py}) -> ({px2},{py2})").into());
    }
    // 数量/容量显示。
    assert_str(
        &mut runtime,
        "(function() local t = ui.get('bp:cap'); \
          return tostring(t ~= nil and t.text:find('容量', 1, true) ~= nil) end)()",
        "true",
    )?;
    // 装备：定位水壶行（1 基序号）→ 数字键选中 → E 装备。
    let water_row = row_of(&mut runtime, "ui.inventory_ui", "watering_can")?;
    press(
        &mut runtime,
        KeyCode::from_char(digit(water_row)).expect("数字行号必须有对应主键区数字键"),
    )?;
    assert_num(
        &mut runtime,
        "require('ui.inventory_ui').index()",
        (water_row - 1) as f64,
    )?;
    press(&mut runtime, KeyCode::KeyE)?;
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
    // 使用：回复药 → 体力 50 → 90（成功路径消耗物品并给反馈文案）。
    let pill_row = row_of(&mut runtime, "ui.inventory_ui", "heal_pill")?;
    press(
        &mut runtime,
        KeyCode::from_char(digit(pill_row)).expect("数字行号必须有对应主键区数字键"),
    )?;
    press(
        &mut runtime,
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
        "(function() local t = ui.get('bp:msg'); \
          return tostring(t ~= nil and t.text:find('使用', 1, true) ~= nil) end)()",
        "true",
    )?;
    // 不可使用物品的失败反馈：稻谷（无 use 字段）→ no_effect。
    let rice_row = row_of(&mut runtime, "ui.inventory_ui", "rice")?;
    press(
        &mut runtime,
        KeyCode::from_char(digit(rice_row)).expect("数字行号必须有对应主键区数字键"),
    )?;
    press(
        &mut runtime,
        KeyCode::from_char('u').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_str(
        &mut runtime,
        "(function() local t = ui.get('bp:msg'); \
          return tostring(t ~= nil and t.text:find('没有使用效果', 1, true) ~= nil) end)()",
        "true",
    )?;
    // 关闭：面板与动态 UI 一起收起，模式回到 Playing。
    press(
        &mut runtime,
        KeyCode::from_char('i').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Playing")?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.inventory_ui').ui_active())",
        "false",
    )?;
    assert_str(
        &mut runtime,
        "(function() local t = ui.get('bp:title'); return tostring(t ~= nil and t.visible == false) end)()",
        "true",
    )?;

    banner("M19-02d 任务面板（只读）：L 打开 → 列出任务与目标进度 → Q 关闭");
    press(
        &mut runtime,
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
        "(function() local t = ui.get('qst:title'); \
          return tostring(t ~= nil and t.visible == true and t.text == '任务日志') end)()",
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
        KeyCode::from_char('p').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Pause")?;
    // M21 主角形象：暂停菜单新增「形象」行（原 5 行 → 6 行），
    // 标签带当前形象的中文名，按确认在该行轮换形象。
    assert_num(&mut runtime, "#(require('ui.pause_menu').rows())", 6.0)?;
    assert_str(
        &mut runtime,
        "(function() for _, r in ipairs(require('ui.pause_menu').rows()) do \
           if r.key == 'appearance' then return r.label end end; return 'missing' end)()",
        "形象  青衫少侠",
    )?;
    assert_str(
        &mut runtime,
        "(function() local t = ui.get('pse:title'); \
          return tostring(t ~= nil and t.visible == true and t.text == '暂停') end)()",
        "true",
    )?;
    press(
        &mut runtime,
        KeyCode::from_char('+').expect("QWERTY 字符必须有对应物理键"),
    )?;
    let volume_after = query_num(&mut runtime, "require('core.audio').volume()")?;
    if (volume_after - (volume_before + 0.1)).abs() > 1e-6 {
        return Err(format!("音量 + 未生效: {volume_before} -> {volume_after}").into());
    }
    press(
        &mut runtime,
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
        "(function() local t = ui.get('pse:msg'); \
          return tostring(t ~= nil and t.text:find('已保存', 1, true) ~= nil) end)()",
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
        KeyCode::from_char('p').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Pause")?;
    press(&mut runtime, KeyCode::Space)?;
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

    banner("M19-02g 新手引导：幂等推进 + 十步剧本 + 存档往返");
    lua(&mut runtime, "require('ui.tutorial').reset()")?;
    assert_num(&mut runtime, "require('ui.tutorial').index()", 0.0)?;
    assert_str(&mut runtime, "require('ui.tutorial').step().id", "till")?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').prompt():find('引导 1/10', 1, true) ~= nil)",
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
    // 最后一步：制作收取 → 进入终止标记 → 完成并发一次性奖励。
    let pills_before = query_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'heal_pill')",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.tutorial').on_action('craft_collect_heal_pill'))",
        "true",
    )?;
    assert_num(&mut runtime, "require('ui.tutorial').index()", 10.0)?;
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
    // 重复触发末步不重复发奖。
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
    // 放置一座药庐后站在其下方朝上，预览必然给出 building 冲突。
    let (lx, ly) = find_legal_anchor(&mut runtime, "herb_hut")?;
    lua(
        &mut runtime,
        &format!("require('systems.building').place('herb_hut', {lx}, {ly})"),
    )?;
    lua(
        &mut runtime,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'up'; \
             require('systems.building').set_build_mode(true)",
            (lx as f32 + 0.5) * 32.0,
            (ly as f32 - 0.5) * 32.0
        ),
    )?;
    assert_str(
        &mut runtime,
        "require('systems.building').preview('up').conflicts[1].reason",
        "building",
    )?;
    // M26：HUD 状态行的建造冲突改为中文（BUILD_REASON 表），不再拼英文 reason 键。
    assert_str(
        &mut runtime,
        "require('ui.input').update():match('已有建筑') ~= nil and 'ok' or 'none'",
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

/// WAV 头部与样本自检（M19-02：格式与「非空」由 Rust 侧独立复核）。
struct WavInfo {
    channels: u16,
    sample_rate: u32,
    bits: u16,
    frames: u64,
    bytes: u64,
    peak: i32,
}

fn inspect_wav(path: &Path) -> Result<WavInfo, Box<dyn std::error::Error>> {
    let bytes = fs::read(path).map_err(|e| format!("missing {}: {e}", path.display()))?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format!("bad RIFF/WAVE header: {}", path.display()).into());
    }
    let total = bytes.len() as u64;
    let mut cursor = 12usize;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut audio_format = 0u16;
    let mut data_range: Option<(usize, usize)> = None;
    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let size = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into()?) as usize;
        let body = cursor + 8;
        if id == b"fmt " && body + 16 <= bytes.len() {
            audio_format = u16::from_le_bytes(bytes[body..body + 2].try_into()?);
            channels = u16::from_le_bytes(bytes[body + 2..body + 4].try_into()?);
            sample_rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into()?);
            bits = u16::from_le_bytes(bytes[body + 14..body + 16].try_into()?);
        } else if id == b"data" {
            let end = (body + size).min(bytes.len());
            data_range = Some((body, end));
        }
        cursor = body + size + (size % 2);
    }
    let (start, end) = data_range.ok_or_else(|| format!("no data chunk: {}", path.display()))?;
    if audio_format != 1 {
        return Err(format!("expected PCM, got format {audio_format}").into());
    }
    if channels != 1 || sample_rate != 22050 || bits != 16 {
        return Err(format!(
            "{}: expected ch=1 rate=22050 bits=16, got ch={channels} rate={sample_rate} bits={bits}",
            path.display()
        )
        .into());
    }
    let mut peak = 0i32;
    let mut loud = 0u64;
    let mut frames = 0u64;
    let mut index = start;
    while index + 2 <= end {
        let sample = i16::from_le_bytes(bytes[index..index + 2].try_into()?) as i32;
        peak = peak.max(sample.abs());
        if sample.abs() > 100 {
            loud += 1;
        }
        frames += 1;
        index += 2;
    }
    if frames == 0 {
        return Err(format!("empty audio data: {}", path.display()).into());
    }
    // 非空：至少 1% 的样本振幅 > 100。
    if loud * 100 < frames {
        return Err(format!(
            "audio too quiet: {} loud={loud} frames={frames}",
            path.display()
        )
        .into());
    }
    Ok(WavInfo {
        channels,
        sample_rate,
        bits,
        frames,
        bytes: total,
        peak,
    })
}

/// Creates a bare engine runtime pointing at the project scripts, audio assets and save file.
fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // 音频目录固定为 <project>/assets/audio（core/audio.rs）。
    runtime.set_audio_asset_dir(project.join("assets").join("audio"));
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(720, 1280);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
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
fn press(runtime: &mut LuaRuntime, key: KeyCode) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = InputSnapshot::default();
    input.key_event(key, true);
    runtime.set_input(input);
    runtime.call_update()?;
    runtime.set_input(InputSnapshot::default());
    runtime.call_update()?;
    Ok(())
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
