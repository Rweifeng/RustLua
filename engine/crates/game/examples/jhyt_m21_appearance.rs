//! Headless acceptance drive for M21 主角形象系统（appearance system）.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` and
//! reproduces the M21 acceptance paths:
//!
//! - 数据：`data/appearances.lua` 的可选形象都能在 `data/characters.lua` 里找到
//!   且 `role="player"`；默认形象唯一。
//! - 场景：7 张可玩场景的 `JianghuPlayer` 都声明了真实形象（不再是 `__white__` 灰盒）。
//! - 运行期：`player.apply()` 把单帧 + 序列帧挂到实体上，成功后 `animation.get` = `idle_s`。
//! - 移动：按住方向键 → `walk_<dir>`，松手 → `idle_<dir>`；朝向按世界 y 向上换算
//!   （up→n / down→s / left→w / right→e）。
//! - 动作：`farm` 翻土播 `hoe_*`、`combat` 普攻播 `attack_*`，播完自动落回 walk/idle。
//! - 存档：切换形象写进 `state.appearance`，第二 runtime 读档后形象一致、
//!   且实体上挂的是女性形象的条带；未知 id 一律拒绝且不改变当前形象。
//!
//! Run:
//!   cargo run -p game --example jhyt_m21_appearance -- <game_jhyt-root>

use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

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
    let save_path = fresh_save_path(&project);
    let project_arg = project.clone();

    banner("M21-01a 形象数据：可选形象都指向 data/characters.lua 的 role=player");
    let mut runtime = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut runtime, &main_script)?;
    stub_missing_input_settings(&mut runtime)?;
    assert_str(
        &mut runtime,
        "require('data.appearances').default_id()",
        "player_male",
    )?;
    let options = query_i64(&mut runtime, "#(require('data.appearances').list())")?;
    if options < 2 {
        return Err(format!("appearances.list() = {options}, want >= 2").into());
    }
    let resolved = query_str(
        &mut runtime,
        "local a = require('data.appearances'); local c = require('data.characters'); \
         for _, entry in ipairs(a.list()) do \
           local art = c.get(entry.id); \
           if not art or art.role ~= 'player' then return 'fail:' .. entry.id end \
         end; return 'ok'",
    )?;
    if resolved != "ok" {
        return Err(format!("appearance → character 交叉校验失败：{resolved}").into());
    }
    let data_check = query_str(&mut runtime, "return require('validate_data').check()")?;
    if !data_check.is_empty() {
        return Err(format!("validate_data 报告问题：\n{data_check}").into());
    }

    banner("M21-01b 进入山庄：主角拿到真实形象 + 序列帧（不再是灰盒）");
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;
    assert_mode(&mut runtime, "Playing")?;
    lua(&mut runtime, "return require('world.weather').set('sunny')")?;
    frames(&mut runtime, 2)?;
    let status = query_str(
        &mut runtime,
        "local s = jianghu_appearance(); \
         return string.format('%s|%s|%s|%s', s.appearance, tostring(s.animated), tostring(s.clip), tostring(s.facing))",
    )?;
    if status != "player_male|true|idle_s|s" {
        return Err(
            format!("主角形象状态 = {status:?}, want \"player_male|true|idle_s|s\"").into(),
        );
    }
    let texture = query_str(
        &mut runtime,
        "return sprite.get(entity.find('JianghuPlayer')).texture",
    )?;
    if !texture.contains("player_male") {
        return Err(format!("主角贴图 = {texture:?}, want 含 player_male").into());
    }
    // animation_sheet 是**真正渲染**的那张图（Animation 覆盖 Sprite.texture）
    let sheet = current_sheet(&mut runtime)?;
    if sheet != "idle_s.png" {
        return Err(format!("主角渲染条带 = {sheet:?}, want \"idle_s.png\"").into());
    }
    let clips = query_i64(
        &mut runtime,
        "#(animation.clips(entity.find('JianghuPlayer')))",
    )?;
    if clips != 56 {
        return Err(format!(
            "主角 clip 数 = {clips}, want 56（8 向 × 7 动作，docs/51 v0.3 由 4 向扩展）"
        )
        .into());
    }

    banner("M21-01c 移动：walk_<dir> ↔ idle_<dir>，朝向按世界 y 向上换算");
    for (key, want_walk) in [
        (KeyCode::KeyD, "walk_e"),
        (KeyCode::KeyA, "walk_w"),
        (KeyCode::KeyW, "walk_n"),
        (KeyCode::KeyS, "walk_s"),
    ] {
        hold(&mut runtime, key, 4)?;
        let clip = current_clip(&mut runtime)?;
        if clip != want_walk {
            return Err(format!("按住 {key:?} 时 clip = {clip:?}, want {want_walk:?}").into());
        }
        release(&mut runtime, 3)?;
        let idle = current_clip(&mut runtime)?;
        let want_idle = want_walk.replace("walk_", "idle_");
        if idle != want_idle {
            return Err(format!("松开 {key:?} 后 clip = {idle:?}, want {want_idle:?}").into());
        }
    }

    banner("M21-01d 动作：翻土 → hoe_*，普攻 → attack_*，播完自动落回 idle");
    // 翻土：站在药田内 (16,5) 面向下方（s）→ 目标格 (16,4)。
    // facing_cell 的 down 步进是 (0,-1)（世界 y 向上），所以站在目标格北侧一格。
    lua(
        &mut runtime,
        "local p = require('player.player'); p.teleport(16 * 32 + 16, 5 * 32 + 16); \
         local st = require('core.state'); st.facing = 'down'; st.selected_tool = 'hoe'",
    )?;
    let action = query_str(&mut runtime, "return require('systems.farm').act('down')")?;
    if action != "till" {
        return Err(format!("farm.act 返回 {action:?}, want \"till\"").into());
    }
    let hoe_clip = current_clip(&mut runtime)?;
    if hoe_clip != "hoe_s" {
        return Err(format!("翻土时 clip = {hoe_clip:?}, want \"hoe_s\"").into());
    }
    // 非循环条带播完（4 帧 @8fps = 0.5s = 30 帧）后必须落回 idle_s。
    frames(&mut runtime, 40)?;
    let after = current_clip(&mut runtime)?;
    if after != "idle_s" {
        return Err(format!("翻土结束后 clip = {after:?}, want \"idle_s\"").into());
    }
    // 普攻：起手就应切到 attack_<dir>（技能 id 用 data/skills.lua 的普攻 `strike`）。
    let attacked = query_str(
        &mut runtime,
        "return tostring(require('systems.combat').player_attack('strike'))",
    )?;
    if attacked != "true" {
        return Err(format!("player_attack 返回 {attacked:?}, want \"true\"").into());
    }
    let attack_clip = current_clip(&mut runtime)?;
    if attack_clip != "attack_s" {
        return Err(format!("普攻时 clip = {attack_clip:?}, want \"attack_s\"").into());
    }
    let attack_sheet = current_sheet(&mut runtime)?;
    if attack_sheet != "attack_s.png" {
        return Err(format!("普攻渲染条带 = {attack_sheet:?}, want \"attack_s.png\"").into());
    }
    frames(&mut runtime, 40)?;

    banner("M21-01e 换装：切到青衣女侠 → 贴图/条带立刻换，未知 id 被拒");
    let bad = query_str(
        &mut runtime,
        "return tostring(jianghu_set_appearance('no_such_appearance'))",
    )?;
    if bad != "false" {
        return Err(format!("未知形象应被拒，却返回 {bad:?}").into());
    }
    assert_str(
        &mut runtime,
        "require('core.state').appearance",
        "player_male",
    )?;
    let switched = query_str(
        &mut runtime,
        "return tostring(jianghu_set_appearance('player_female'))",
    )?;
    if switched != "true" {
        return Err(format!("切换到 player_female 返回 {switched:?}").into());
    }
    let female_texture = query_str(
        &mut runtime,
        "return sprite.get(entity.find('JianghuPlayer')).texture",
    )?;
    if !female_texture.contains("player_female") {
        return Err(format!("换装后贴图 = {female_texture:?}, want 含 player_female").into());
    }
    let female_state = query_str(
        &mut runtime,
        "local s = jianghu_appearance(); return string.format('%s|%s', s.appearance, tostring(s.clip))",
    )?;
    if female_state != "player_female|idle_s" {
        return Err(format!("换装后状态 = {female_state:?}, want \"player_female|idle_s\"").into());
    }

    banner("M21-01f 游戏中入口：暂停菜单「形象」行显示中文名并可就地轮换");
    lua(
        &mut runtime,
        "local st = require('core.state'); st.appearance = 'player_male'; \
         require('player.player').apply()",
    )?;
    let menu = query_str(
        &mut runtime,
        "local pm = require('ui.pause_menu'); pm.open(); \
         local label, key = 'missing', nil; \
         for _, r in ipairs(pm.rows()) do \
           if r.key == 'appearance' then label = r.label end \
         end; \
         local count = #pm.rows(); \
         pm.close(); \
         return string.format('%d|%s', count, label)",
    )?;
    if menu != "7|形象  青衫少侠" {
        return Err(format!("暂停菜单形象行 = {menu:?}, want \"7|形象  青衫少侠\"").into());
    }
    // 走到「形象」行按确认 → 轮换到下一个可选形象（男 → 女）。
    let cycled = query_str(
        &mut runtime,
        "local pm = require('ui.pause_menu'); pm.open(); \
         for i, r in ipairs(pm.rows()) do if r.key == 'appearance' then pm.select(i - 1) end end; \
         local ok = pm.confirm(); \
         local msg = tostring(pm.msg()); \
         pm.close(); \
         return string.format('%s|%s|%s', tostring(ok), require('core.state').appearance, msg)",
    )?;
    if cycled != "true|player_female|形象：青衣女侠" {
        return Err(format!(
            "暂停菜单轮换形象 = {cycled:?}, want \"true|player_female|形象：青衣女侠\""
        )
        .into());
    }

    banner("M21-01g 存档：读档后形象一致，实体挂的是所选形象的条带");
    save_game(&mut runtime)?;
    let mut second = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut second, &main_script)?;
    stub_missing_input_settings(&mut second)?;
    click_button(&mut second, "BtnContinue")?;
    assert_scene_is(&mut second, "manor")?;
    frames(&mut second, 2)?;
    assert_str(
        &mut second,
        "require('core.state').appearance",
        "player_female",
    )?;
    let restored = query_str(
        &mut second,
        "local s = jianghu_appearance(); \
         local tex = sprite.get(entity.find('JianghuPlayer')).texture; \
         return string.format('%s|%s|%s', s.appearance, tostring(s.animated), tex:match('[^/]+$'))",
    )?;
    if restored != "player_female|true|static_s.png" {
        return Err(
            format!("读档后形象 = {restored:?}, want \"player_female|true|static_s.png\"").into(),
        );
    }
    // 女装条带确实来自 player_female 目录（animation_sheet 是实际渲染的那张）。
    let clip_sheet = query_i64(
        &mut second,
        "#(animation.clips(entity.find('JianghuPlayer')))",
    )?;
    let texture2 = query_str(
        &mut second,
        "return sprite.get(entity.find('JianghuPlayer')).texture",
    )?;
    let sheet2 = current_sheet(&mut second)?;
    if !texture2.contains("player_female") || sheet2 != "idle_s.png" {
        return Err(format!(
            "读档后贴图/条带 = {texture2:?}/{sheet2:?}, want 含 player_female 且 idle_s.png"
        )
        .into());
    }

    println!(
        "jhyt_m21_appearance_ok project={} appearance={} clips={} options={} tile={}",
        project.display(),
        query_str(&mut second, "require('core.state').appearance")?,
        clip_sheet,
        options,
        texture2
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers ----------

fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
}

/// Runs the project script, then loads the `main` 门面 scene so BtnStart/BtnContinue
/// are live (mirrors jhyt_smoke).
fn enter_main_menu(
    runtime: &mut LuaRuntime,
    main_script: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_project_script(main_script)?;
    runtime.execute_console("scene.load('main')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(runtime, "main")?;
    Ok(())
}

fn click_button(runtime: &mut LuaRuntime, button: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("on_click('{button}')"))?;
    runtime.process_scene_transition()?;
    Ok(())
}

/// 按住方向键推进 `n` 帧（每帧都带同一个按键状态）。
fn hold(
    runtime: &mut LuaRuntime,
    key: KeyCode,
    n: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..n {
        let mut input = InputSnapshot::default();
        input.key_event(key, true);
        runtime.set_input(input);
        runtime.call_update()?;
    }
    Ok(())
}

/// 松开所有键推进 `n` 帧。
fn release(runtime: &mut LuaRuntime, n: usize) -> Result<(), Box<dyn std::error::Error>> {
    frames(runtime, n)
}

fn frames(runtime: &mut LuaRuntime, n: usize) -> Result<(), Box<dyn std::error::Error>> {
    runtime.set_input(InputSnapshot::default());
    for _ in 0..n {
        runtime.call_update()?;
    }
    Ok(())
}

/// 当前主角动画 clip 名（未挂动画时为 "nil"）。
fn current_clip(runtime: &mut LuaRuntime) -> Result<String, Box<dyn std::error::Error>> {
    query_str(
        runtime,
        "local id = entity.find('JianghuPlayer'); \
         if not id then return 'nil' end; \
         local ok, clip = pcall(animation.get, id); \
         return ok and tostring(clip) or 'nil'",
    )
}

/// 当前**实际渲染**的条带文件名（Animation 的 clip sheet 覆盖 Sprite.texture）。
fn current_sheet(runtime: &mut LuaRuntime) -> Result<String, Box<dyn std::error::Error>> {
    query_str(
        runtime,
        "local id = entity.find('JianghuPlayer'); \
         if not id then return 'nil' end; \
         local ok, info = pcall(sprite.get, id); \
         if not ok or not info or not info.animation_sheet then return 'nil' end; \
         return tostring(info.animation_sheet:match('[^/]+$'))",
    )
}

/// 并发开发中的 M20-08「玩家改键」：`main.lua` 已经调用 `input.load_settings()`，
/// 但 `scripts/ui/input.lua` 目前还没实现它 —— 那会让**所有**验收驱动在 boot_game
/// 阶段就崩掉（与本驱动的 M21 主题无关）。这里只在缺失时补一个最小桩
/// （返回 true = "没有改键，用引擎默认"），对方实现落地后这段自动变成空操作。
fn stub_missing_input_settings(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(
        "local input = require('ui.input'); \
         if type(input.load_settings) ~= 'function' then \
           input.load_settings = function() return true, 'stub: input.load_settings 尚未实现' end \
         end",
    )?;
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

fn query_i64(runtime: &mut LuaRuntime, expr: &str) -> Result<i64, Box<dyn std::error::Error>> {
    let value = query_str(runtime, expr)?;
    value
        .parse::<i64>()
        .map_err(|e| format!("{value:?} not an integer: {e}").into())
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

fn save_game(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    let ok = query_str(
        runtime,
        "return tostring(require('core.save_manager').save())",
    )?;
    if ok != "true" {
        return Err(format!("save() returned {ok:?}").into());
    }
    Ok(())
}

fn banner(title: &str) {
    println!("\n=== {title} ===");
}

fn fresh_save_path(project: &Path) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("jhyt_m21_appearance.sav");
    let _ = fs::remove_file(&path);
    path
}
