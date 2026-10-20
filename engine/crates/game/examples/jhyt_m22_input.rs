//! Headless acceptance drive for M22/W1 —— 输入层**物理键身份**收口。
//!
//! 把「宿主喂入的键位 → 引擎物理键 → 动作 → Lua」整条链锁成一条断言链，覆盖
//! 四类历史缺陷（均已复现过，本示例把它们钉成回归）：
//!
//! 1. **键身份按字符编码**（旧 `InputKey`）：`Shift+1` 与 `1` 是两个键、Tab/Enter/F5
//!    根本无法表达、方向键只能靠内部码位 `Other(0x0010_0001)`。现在 `KeyCode` 是
//!    与 winit 0.30 逐项对齐的 **194 项物理键表**，文本形式 = 变体名小写。
//! 2. **同一物理位置在不同键盘布局下变成不同键**：引擎只认物理键（宿主读
//!    `PhysicalKey::Code`），AZERTY 的 W 位置永远是 `KeyW`；字符只在**兼容入口**
//!    （`input.is_down("w")`、v1 设置文件）里按 US-QWERTY 换算。
//! 3. **拼错的键名静默变成「某个未知键」**（旧 `parse_key` 落 `Other(0)`）：现在
//!    `input.is_down('not-a-key')` 是**明确报错**，并把原值回显在错误里。
//! 4. **设置文件里的键位写法与运行时两套**：现在是 v1 读入 → 物理键 → v2 写出，
//!    有损换算（`key:+` 只能落到 `=` 那个物理键）必须给出**可显示的中文诊断**，
//!    而不是悄悄改掉玩家的键位。
//!
//! 另外锁住三处**单一事实来源**（这是本工作包真正要防的漂移）：
//! * 引擎键域 = `KeyCode::ALL`，宿主 winit 表与编辑器 egui 表都从它派生
//!   （`host::keymap` 的覆盖性单测 + `editor::panels::keymap` 的往返单测）；
//! * 默认动作表在**键盘**上零跨动作冲突（一个物理键只属于一个动作），写出的文本里
//!   **不含任何 v1 码位痕迹**；手柄侧的刻意别名（`pad:0` 同服 `confirm`/`interact`、
//!   左摇杆同服 `move_*`/`ui_*`）是设备别名，不算冲突（见 `ActionMap::with_defaults`）；
//! * Lua 侧 `input.bindings()` / `action_map_text()` 与宿主读到的 `ActionMap`
//!   是**同一份实例**。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m22_input [-- <game_jhyt-root>]

use rustlua_input::input::{
    ACTION_MAP_VERSION, ACTION_MAP_VERSION_V1, ActionMap, Binding, InputSnapshot, KeyCode,
};
use rustlua_scripting::script::LuaRuntime;
use std::{env, fs, path::PathBuf};

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
    let project_arg = project.clone();

    // ---------------- ① 键身份表 ----------------
    banner("① 物理键身份：194 项全表、文本往返、v1 兼容换算");
    assert_eq!(
        KeyCode::ALL.len(),
        194,
        "引擎键域必须与 winit 0.30 的 KeyCode 逐项对齐"
    );
    let mut seen: Vec<&'static str> = Vec::with_capacity(KeyCode::ALL.len());
    for key in KeyCode::ALL {
        let text = key.to_text();
        if KeyCode::from_text(text) != Some(*key) {
            return Err(format!("{key:?} 的文本形式 {text:?} 不能解析回自己").into());
        }
        if seen.contains(&text) {
            return Err(format!("文本形式重复（设置文件会有歧义）：{text:?}").into());
        }
        seen.push(text);
        if Binding::parse(&format!("key:{text}")) != Ok(Binding::Key(*key)) {
            return Err(format!("key:{text} 不能解析为 {key:?}").into());
        }
    }
    // 文本形式是**变体名小写**：与平台库的 Debug 名一一对应，稳定且无歧义。
    for (key, text) in [
        (KeyCode::KeyE, "keye"),
        (KeyCode::Digit1, "digit1"),
        (KeyCode::ArrowUp, "arrowup"),
        (KeyCode::Space, "space"),
        (KeyCode::Escape, "escape"),
        (KeyCode::ShiftLeft, "shiftleft"),
        (KeyCode::F35, "f35"),
        (KeyCode::NumpadAdd, "numpadadd"),
        (KeyCode::IntlYen, "intlyen"),
    ] {
        if key.to_text() != text {
            return Err(format!("{key:?}.to_text() = {:?}, want {text:?}", key.to_text()).into());
        }
    }
    println!(
        "    键域 {} 项，文本形式两两不同；样例 keye/digit1/arrowup/space/shiftleft/f35 均一致",
        KeyCode::ALL.len()
    );

    // v1 兼容换算：字符/可读名/内部码位 → 物理键；带 Shift 的字符必须给出诊断。
    let mut notes = Vec::new();
    for (v1, expected) in [
        ("w", KeyCode::KeyW),
        ("E", KeyCode::KeyE),
        ("1", KeyCode::Digit1),
        ("up", KeyCode::ArrowUp),
        ("esc", KeyCode::Escape),
        ("space", KeyCode::Space),
        ("other:1048577", KeyCode::ArrowUp),
        ("other:1048580", KeyCode::ArrowRight),
        ("other:1", KeyCode::F1),
        ("+", KeyCode::Equal),
        (":", KeyCode::Semicolon),
    ] {
        match KeyCode::v1_key_code(v1, &mut notes) {
            Some(got) if got == expected => {}
            other => {
                return Err(format!("v1 键位 {v1:?} 换算成 {other:?}，期望 {expected:?}").into());
            }
        }
    }
    if KeyCode::v1_key_code("not-a-key", &mut notes).is_some() {
        return Err("v1 未知名不得落成某个「未知键」".into());
    }
    println!(
        "    v1 换算：w/E/1/up/esc/space/other:1048577/+ 全部落到物理键；未知名返回 None（不静默）"
    );

    // AZERTY 场景：物理键路径不经过字符。
    let defaults = ActionMap::with_defaults();
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyW, true);
    if !input.action_is_down(&defaults, "move_up") {
        return Err("KeyW 必须驱动 move_up（物理键路径）".into());
    }
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyZ, true);
    if input.action_is_down(&defaults, "move_up") {
        return Err("AZERTY 的 Z 字符位置不得被当成 W 物理键".into());
    }
    println!("    AZERTY：KeyW 驱动 move_up，KeyZ 不误触发（身份不经过字符）");

    // ---------------- ② 实际游戏工程：宿主 → 动作 → Lua ----------------
    banner("② 真实工程链路：默认动作表 → 物理键按下 → Lua 动作查询");
    let (installers, handles) = rustlua_gameplay::input_lua::registry_with_handles();
    let host = handles.input;
    let mut runtime = LuaRuntime::with_installers(installers)?;
    runtime.set_project_root(project.clone());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    runtime.execute_project_script(&main_script)?;

    // 默认表在**键盘**上零冲突：每个物理键只出现在一个动作里。
    //
    // 手柄侧的刻意别名（`pad:0` 同时服务 `confirm`/`interact`、左摇杆同时服务
    // `move_*` 与 `ui_*`）是 M20-08 的设计，不算冲突；因此这里断言的是
    // 「`conflicts()` 里不含任何 Key 绑定」这一真正的不变量，而**不是**
    // `validate_bindings()`（它服务编辑器保存路径，连同动作多绑定也拒）。
    let conflicts = host.action_map().conflicts();
    let key_conflicts: Vec<_> = conflicts
        .iter()
        .filter(|conflict| matches!(conflict.binding, Binding::Key(_)))
        .collect();
    if !key_conflicts.is_empty() {
        return Err(format!("默认动作表存在键盘跨动作冲突：{key_conflicts:?}").into());
    }
    let gamepad_aliases = conflicts.len();
    let text = runtime
        .execute_console("return input.action_map_text()")?
        .trim()
        .to_owned();
    if !text.starts_with(ACTION_MAP_VERSION) {
        return Err(format!("映射文本头必须是 {ACTION_MAP_VERSION}，实际 {text:?}").into());
    }
    if text.contains("other:") {
        return Err("v2 映射文本不得含 v1 内部码位（key:other:…）".into());
    }
    println!(
        "    默认动作表 {} 个动作、键盘零冲突（手柄刻意别名 {gamepad_aliases} 处），\
         文本头 = {ACTION_MAP_VERSION}，无 key:other: 痕迹",
        host.action_map().actions().len()
    );

    // 物理键按下 → Lua 动作查询（键盘与手柄语义一致由 M20-08 的单测覆盖）。
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyE, true);
    host.set_input(input);
    runtime.call_update()?;
    assert_lua_str(&mut runtime, "input.action('interact')", "true")?;
    assert_lua_str(&mut runtime, "input.action_pressed('interact')", "true")?;
    assert_lua_str(&mut runtime, "input.is_down('keye')", "true")?;
    assert_lua_str(&mut runtime, "input.is_down('e')", "true")?;
    assert_lua_str(&mut runtime, "input.is_down('keyw')", "false")?;
    host.set_input(InputSnapshot::default());
    runtime.call_update()?;

    // 未知名是**明确错误**并回显原值（旧实现静默落 Other(0)）。
    let error = runtime
        .execute_console("return input.is_down('not-a-key')")
        .err()
        .ok_or("未知名必须报错，而不是当成某个未知键")?;
    let rendered = format!("{error:?}");
    if !rendered.contains("not-a-key") {
        return Err(format!("错误信息必须回显原键名，实际 {rendered}").into());
    }
    println!("    物理键 → 动作 → Lua 一致；未知名报错并回显原值");

    // 重绑定：v1 写法读回 v2 物理键名；宿主与脚本共用同一份实例。
    assert_lua_str(
        &mut runtime,
        "tostring(input.rebind('interact', 'key:keyf'))",
        "true",
    )?;
    assert_lua_str(&mut runtime, "input.bindings('interact')[1]", "key:keyf")?;
    assert_lua_str(
        &mut runtime,
        "tostring(input.rebind('interact', 'key:f'))",
        "true",
    )?;
    assert_lua_str(&mut runtime, "input.bindings('interact')[1]", "key:keyf")?;
    if host.action_map().bindings("interact") != [Binding::Key(KeyCode::KeyF)] {
        return Err("宿主读到的映射必须与脚本一致（单一事实来源）".into());
    }
    println!("    重绑定：v1 写法与 v2 物理键名解析到同一物理键，宿主/脚本同源");

    // ---------------- ③ 设置持久化与 v1 迁移 ----------------
    banner("③ 设置持久化：v1 读入 → 物理键 → v2 写出 + 有损换算诊断");
    let settings_root = std::env::temp_dir().join(format!(
        "rustlua-m22-input-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    fs::create_dir_all(&settings_root)?;
    let (installers, handles) = rustlua_gameplay::input_lua::registry_with_handles();
    let host = handles.input;
    let mut runtime = LuaRuntime::with_installers(installers)?;
    runtime.set_project_root(project_arg.clone());
    runtime.set_settings_root(settings_root.clone());
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    runtime.execute_project_script(&main_script)?;

    // 写盘：`[input]` 段落在设置根，写出的文本是 v2 物理键。
    assert_lua_str(
        &mut runtime,
        "tostring(input.rebind('interact', 'key:f'))",
        "true",
    )?;
    assert_lua_str(
        &mut runtime,
        "tostring(select(1, input.save_settings()))",
        "true",
    )?;
    let settings_path = settings_root.join("project.toml");
    let written = fs::read_to_string(&settings_path)?;
    for required in [
        "[input]",
        ACTION_MAP_VERSION,
        "interact=key:keyf",
        "move_up=key:keyw",
        "ui_up=key:arrowup,padaxis:1+,pad:12",
    ] {
        if !written.contains(required) {
            return Err(format!("写出的 project.toml 缺少 {required:?}：\n{written}").into());
        }
    }
    // 文本必须等于当前内存映射的 `to_text`（宿主/脚本/磁盘三者同源）。
    let stored = extract_map_line(&written)?;
    if stored != host.action_map().to_text() {
        return Err(format!(
            "磁盘映射与内存映射不一致：\n磁盘 {:?}\n内存 {:?}",
            stored,
            host.action_map().to_text()
        )
        .into());
    }
    println!("    写盘 = v2 物理键文本，且与内存映射逐字相等（磁盘/宿主/脚本同源）");

    // v1 迁移：含一处在 v1 里「只能猜」的带 Shift 字符 → 必须给诊断。
    fs::write(
        &settings_path,
        format!(
            "{}\n[other]\nkeep = \"yes\"\n",
            v1_settings_block(
                "rustlua-actions-v1\\nmove_up=key:w\\nvolume_up=key:+\\nslot_1=key:1"
            )
        ),
    )?;
    assert_lua_str(
        &mut runtime,
        "tostring(select(1, input.load_settings()))",
        "true",
    )?;
    assert_lua_str(&mut runtime, "input.bindings('move_up')[1]", "key:keyw")?;
    assert_lua_str(&mut runtime, "input.bindings('slot_1')[1]", "key:digit1")?;
    assert_lua_str(&mut runtime, "input.bindings('volume_up')[1]", "key:equal")?;
    let notes = runtime
        .execute_console("return table.concat(input.migration_notes(), '\\n')")?
        .trim()
        .to_owned();
    if !notes.contains("key:+") || !notes.contains("key:equal") {
        return Err(format!("v1 有损换算必须给出诊断，实际 {notes:?}").into());
    }
    println!(
        "    v1 读入：move_up=key:w → key:keyw、slot_1=key:1 → key:digit1、key:+ → key:equal（附诊断）"
    );
    for line in notes.lines() {
        println!("      · {line}");
    }

    // 读 v1、写 v2：迁移后落盘即升级，且 `[other]` 段原样保留。
    assert_lua_str(
        &mut runtime,
        "tostring(select(1, input.save_settings()))",
        "true",
    )?;
    let migrated = fs::read_to_string(&settings_path)?;
    if !migrated.contains(ACTION_MAP_VERSION) || migrated.contains(ACTION_MAP_VERSION_V1) {
        return Err(format!("迁移后必须写 v2 头：\n{migrated}").into());
    }
    if !migrated.contains("[other]") || !migrated.contains(r#"keep = "yes""#) {
        return Err(format!("迁移不得吃掉其它段：\n{migrated}").into());
    }
    println!("    迁移落盘：版本头升级为 v2，未知段 [other] 原样保留");

    // 版本头不符 → 明确回退 + 可定位诊断（不崩溃、不静默）。
    fs::write(
        &settings_path,
        v1_settings_block("rustlua-actions-v9\\nmove_up=key:w"),
    )?;
    let fallback = runtime.execute_console(
        "local ok, why = input.load_settings()\n\
         assert(ok == false, '未知版本头必须失败')\n\
         return tostring(why)",
    )?;
    if !fallback.contains("rustlua-actions-v9") {
        return Err(format!("未知版本头的原因必须回显版本名，实际 {fallback:?}").into());
    }
    assert_lua_str(&mut runtime, "input.bindings('move_up')[1]", "key:keyw")?;
    println!("    未知版本头：明确失败 + 回显版本名 + 回退默认，不崩溃");

    let _ = fs::remove_dir_all(&settings_root);

    // ---------------- ④ Lua 侧键名映射不许与引擎 display_label 漂移 ----------------
    banner("④ 脚本侧键名显示：hints.lua 的映射与引擎 display_label 逐项对照");
    // 背景：`ui/hints.lua` 过去直接把 `key:` 后面的内容 `upper()` 当标签（当时是 `key:e`
    // 这种字符形式）。W1 把文本换成**物理键名**（`key:keye`）后，"W/A/S/D 移动"会变成
    // "KEYW/KEYA/KEYS/KEYD 移动"——`jhyt_m20_gamepad` 的文案断言即刻抓到这个回归。
    // 修法是让脚本认识物理键名，但脚本表与引擎 `Binding::display_label()` 是**两份**，
    // 会再次漂移，所以这里做机器对照：把全部 **194 个物理键**挨个喂给真实的 Lua
    // `hints.binding_hint`，逐项与引擎标签比对。
    //
    // 期望值：同一份 `KeyCode::ALL` 在两侧各算一次，**逐项**比对。
    // * 字母/数字/F 键/小键盘数字/`Lang<n>` → 两侧都按「推导规则」算（`keyw` → `W`）；
    // * 其余一律要求**逐字相等**，包括所有中文标签——引擎新增中文标签而 `hints.lua`
    //   没跟上时这条断言立刻变红（这正是要防的漂移）；
    // * 唯一例外是 `DEFERRED_HINTS`（极少被绑的编辑/输入法键，脚本仍显示机器名），
    //   而它的**上限被钉死**：引擎给某个键新补了标签却没补进 `hints.lua`，就会超出上限报错。
    let mut by_rule = 0usize;
    let mut exact = 0usize;
    let mut machine: Vec<String> = Vec::new();
    for key in KeyCode::ALL {
        let label = Binding::Key(*key).display_label();
        let name = key.to_text();
        let expected = if let Some(rule) = rule_derived_label(*key) {
            by_rule += 1;
            rule
        } else if DEFERRED_HINTS.contains(&name) {
            // 脚本对未收录键的兜底是 `to_text` 大写（`\ (ISO)`/`ろ`/`¥` 三个有专属条目，
            // 已在表里，因此不会走到这里）。
            machine.push(format!("{name}（引擎 {label:?}）"));
            name.to_ascii_uppercase()
        } else {
            exact += 1;
            label.clone()
        };
        assert_lua_str(
            &mut runtime,
            &format!("require('ui.hints').binding_hint('key:{name}')"),
            &expected,
        )?;
    }
    // 下限是**地板**不是目标：查表项 ~57、规则项 ~86。任一类掉下来都说明有键被
    // 悄悄挪进了兜底分支（那正是漂移的开端）。
    if exact < 50 || by_rule < 80 {
        return Err(format!("对照样本不足（exact={exact} by_rule={by_rule}）").into());
    }
    if machine.len() != DEFERRED_HINTS.len() {
        return Err(format!(
            "「脚本仍显示机器名」的键是 {} 项、白名单是 {} 项：{}——\
             请把它们补进 hints.lua 的 KEYBOARD_KEY_NAMES，并同步这份白名单",
            machine.len(),
            DEFERRED_HINTS.len(),
            machine.join("、")
        )
        .into());
    }
    // 老写法（v1 可读名）在脚本侧也要显示成同一套标签，否则老设置文件读进来后提示会退化。
    for (legacy, expected) in [
        ("key:w", "W"),
        ("key:e", "E"),
        ("key:1", "1"),
        ("key:up", "↑"),
        ("key:space", "空格"),
        ("key:esc", "Esc"),
        ("key:other:1048577", "键1048577"),
    ] {
        assert_lua_str(
            &mut runtime,
            &format!("require('ui.hints').binding_hint('{legacy}')"),
            expected,
        )?;
    }
    // 手柄绑定不受影响（同一条路径的另一半）。
    assert_lua_str(
        &mut runtime,
        "require('ui.hints').binding_hint('pad:0')",
        "A",
    )?;
    assert_lua_str(
        &mut runtime,
        "require('ui.hints').binding_hint('padaxis:1+')",
        "左摇杆→/↑",
    )?;
    // 默认绑定表里实际出现的键位必须全部走**查表**那一类（不能用 `upper()` 兜底）——
    // 提示文案里不会出现 `KEYW` 这种机器名。这条断言直接把用户可见文案钉死。
    let defaults = ActionMap::with_defaults();
    let mut default_keys = 0usize;
    for action in defaults.actions() {
        for binding in defaults.bindings(action) {
            let Binding::Key(code) = binding else {
                continue;
            };
            default_keys += 1;
            if let Some(rule) = rule_derived_label(*code) {
                // 字母/数字键走推导规则（`keyw` → `W`），这是设计如此、不是兜底。
                if rule != binding.display_label() {
                    return Err(format!(
                        "默认动作 {action} 的 {code:?} 推导标签 {rule:?} 与引擎标签 {:?} 不一致",
                        binding.display_label()
                    )
                    .into());
                }
                continue;
            }
            let label = binding.display_label();
            if label == code.to_text().to_ascii_uppercase() {
                return Err(format!(
                    "默认动作 {action} 的 {code:?} 在脚本侧会显示成机器名 {label:?}"
                )
                .into());
            }
        }
    }
    if default_keys < 20 {
        return Err(format!("默认表的键盘绑定只有 {default_keys} 项，样本不足").into());
    }
    println!(
        "    hints.lua：194 项逐一对照通过（查表逐字相等 {exact}、按规则推导 {by_rule}），\
         默认表 {default_keys} 个键盘绑定无机器名；白名单内 {} 项仍显示机器名\
         （numpad 编辑键 / Fn / 浏览器与媒体键 / 编辑·输入法键，清单见 DEFERRED_HINTS）；\
         v1 老写法与手柄绑定照旧",
        machine.len()
    );

    println!("\njhyt_m22_input_ok keys=194 actions=default settings=v1→v2");
    Ok(())
}

/// `hints.lua` **故意不收录**的物理键：这些键的引擎标签也是「机器名」（`numpadhash`）
/// 或很少被绑到的编辑/输入法/媒体键（引擎给了中文名，脚本仍显示 `to_text` 大写）。
///
/// 为提示文案多维护十几条中文名不划算；但**白名单必须与实际情况逐项相等**：升级 winit、
/// 或给某个键补上标签、或 `hints.lua` 少收了一条，条目数就会对不上，示例立刻报错要求补表
/// ——不给「反正会兜底」留后门。
const DEFERRED_HINTS: &[&str] = &[
    // 小键盘上的编辑/记忆键：引擎标签是 `to_text` 本身（无中文名可给），脚本 `upper()`。
    "numpadbackspace",
    "numpadclear",
    "numpadclearentry",
    "numpadhash",
    "numpadmemoryadd",
    "numpadmemoryclear",
    "numpadmemoryrecall",
    "numpadmemorystore",
    "numpadmemorysubtract",
    "numpadparenleft",
    "numpadparenright",
    "numpadstar",
    // 编辑/输入法/媒体键：引擎有中文名，脚本只显示机器名（极少被绑到）。
    "help",
    "convert",
    "nonconvert",
    "kanamode",
    "select",
    "again",
    "copy",
    "cut",
    "find",
    "open",
    "paste",
    "props",
    "undo",
    "hiragana",
    "katakana",
    // winit 的伪键与浏览器/系统键：引擎标签就是 `to_text`，脚本 `upper()` 一致。
    "fn",
    "fnlock",
    "meta",
    "hyper",
    "turbo",
    "abort",
    "resume",
    "suspend",
    "browserback",
    "browserfavorites",
    "browserforward",
    "browserhome",
    "browserrefresh",
    "browsersearch",
    "browserstop",
    "eject",
    "launchapp1",
    "launchapp2",
    "launchmail",
    "mediaplaypause",
    "mediaselect",
    "mediastop",
    "mediatracknext",
    "mediatrackprevious",
    "power",
    "sleep",
    "audiovolumedown",
    "audiovolumemute",
    "audiovolumeup",
    "wakeup",
];

/// 与 `scripts/ui/hints.lua` 的 `derive_key_name` **同规则**地算出标签。
///
/// 两个宿主（编辑器面板、游戏提示）都自己维护标签，靠这条函数把「能机械推导的部分」
/// 从「必须查表的部分」里分出来：能推导的按规则对照，剩下的必须逐字相等。
/// `Lang<n>`/`Intl*` 也算可推导——脚本对它们的规则是「首字母大写后原样保留」。
fn rule_derived_label(key: KeyCode) -> Option<String> {
    let text = key.to_text();
    if text.len() == 4 && text.starts_with("key") {
        return Some(text[3..].to_ascii_uppercase());
    }
    if let Some(digit) = text.strip_prefix("digit")
        && digit.len() == 1
        && digit.chars().all(|c| c.is_ascii_digit())
    {
        return Some(digit.to_owned());
    }
    if let Some(digit) = text.strip_prefix("numpad")
        && digit.len() == 1
        && digit.chars().all(|c| c.is_ascii_digit())
    {
        return Some(format!("小键盘 {digit}"));
    }
    if let Some(n) = text.strip_prefix('f')
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
    {
        return Some(format!("F{n}"));
    }
    if text.starts_with("lang") && text.len() > 4 && text[4..].chars().all(|c| c.is_ascii_digit()) {
        let mut chars = text.chars();
        let first = chars.next()?.to_ascii_uppercase();
        return Some(format!("{first}{}", chars.as_str()));
    }
    None
}

/// `[input]` 段（单行 TOML 基本字符串，`\n` 已转义）。
fn v1_settings_block(map: &str) -> String {
    format!("# 玩家设置\n[input]\nmap = \"{map}\"\n")
}

/// 从 project.toml 文本里取出 `map` 行的 TOML 转义文本并还原换行。
fn extract_map_line(text: &str) -> Result<String, Box<dyn std::error::Error>> {
    let start = text
        .find("map = \"")
        .ok_or("project.toml 里没有 map = \"…\"")?
        + "map = \"".len();
    let rest = &text[start..];
    let end = rest.find('"').ok_or("map 行的引号未闭合")?;
    Ok(rest[..end].replace("\\n", "\n"))
}

fn assert_lua_str(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = runtime
        .execute_console(&format!("return tostring({expr})"))?
        .trim()
        .to_owned();
    if got != expected {
        return Err(format!("{expr} = {got:?}, want {expected:?}").into());
    }
    Ok(())
}

fn banner(title: &str) {
    println!("\n=== {title} ===");
}
