//! Headless acceptance drive for M20-02 《江湖有田》音量分组混音与 UI 焦点/滚动导航.
//!
//! Loads `examples/game_jhyt/scripts/main.lua` into a bare `LuaRuntime` and
//! reproduces the M20-02 acceptance paths:
//!
//! - ①音量分组：`audio` Lua API（master/bgm/sfx/ui）独立可调、实际音量 = 主 ×
//!   分组、非法总线名报错、无输出设备时播放静默降级、缺文件才算失败。
//! - ②设置往返：`audio.settings_text()` / `audio.apply_settings()` 往返；版本头
//!   不符被拒且不改变音量。
//! - ③工程设置文件：启动时把 `<project>/project.toml` 的 `[audio]` 段应用到总线；
//!   暂停菜单「设置」页四路音量 ± 后可保存回文件，且保留注释与 `[input]` 段。
//! - ④面板焦点导航（方向键/W-S 走引擎 `ui.focus_*` 焦点环）：背包（2 列网格）、
//!   任务、暂停、对话、商店上下移动与边界环绕。
//! - ⑤列表滚动（引擎 `ui.scroll_*` ScrollState）：商店库存超过可视槽位时按选中
//!   行滚动窗口；内容不足一页时偏移恒为 0（不出现空白滚动）。
//! - ⑥回归锚点：统一数据校验、HUD 文案、主音量存档 provider 与分组音量互不干扰。
//!
//! 本示例会临时写入 `<project>/project.toml`（M20-02 新增的可选工程设置文件，
//! 用于验证启动加载与面板保存），**结束时按原样恢复**（原本不存在则删除），
//! 以保持仓库干净、重复运行结果确定。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m20_audio [-- <game_jhyt-root>]

use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 运行期写入的 project.toml：完整 `[project]` 段（main.lua 启动校验会读窗口
/// 尺寸/启动脚本）+ `[audio]` 段 + `[input]` 段（后者是后续任务 M20-07 的扩展位，
/// 本示例验证音频写入保序、不丢其他段）。
const SETTINGS_TEMPLATE: &str = "# M20-02 测试设置（示例结束时恢复）\n[project]\nname = \"game_jhyt\"\nversion = \"0.1.0\"\nengine = \"RustLua\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\nfont = \"assets/fonts/NotoSansSC-VF.ttf\"\nfont_fallback = \"\"\nfont_weight = 400.0\ntext_render_mode = \"vector\"\nwindow_width = 1280\nwindow_height = 720\n\n[audio]\nmaster = 0.5\nbgm = 0.25\nsfx = 0.75\nui = 0.5\n\n[input]\nmap = \"keep-me\"\n";

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
    let settings_path = project.join("project.toml");
    // 备份/恢复工程设置文件：示例写入的音频音量不得污染仓库。
    let _guard = SettingsGuard::capture(&settings_path)?;
    fs::write(&settings_path, SETTINGS_TEMPLATE)?;

    let save_path = fresh_save_path(&project, "jhyt_m20_audio.sav");
    let mut runtime = new_runtime(&project, &save_path)?;
    enter_playable(&mut runtime, &main_script)?;

    banner("M20-02a 音量分组：Master/BGM/SFX/UI 独立 + 实际音量 = 主 × 分组");
    // 启动时 main.lua boot_game → audio.load_settings() 已把 project.toml 应用到总线。
    assert_str(&mut runtime, "tostring(#audio.buses())", "4")?;
    assert_str(
        &mut runtime,
        "(function() local b = audio.buses(); return b[1] .. ',' .. b[4] end)()",
        "master,ui",
    )?;
    assert_num(&mut runtime, "require('core.audio').volume()", 0.5)?;
    assert_num(
        &mut runtime,
        "require('core.audio').bus_volume('bgm')",
        0.25,
    )?;
    assert_num(
        &mut runtime,
        "require('core.audio').bus_volume('sfx')",
        0.75,
    )?;
    assert_num(&mut runtime, "require('core.audio').bus_volume('ui')", 0.5)?;
    assert_num(
        &mut runtime,
        "require('core.audio').effective_volume('bgm')",
        0.125,
    )?;
    assert_num(
        &mut runtime,
        "require('core.audio').effective_volume('sfx')",
        0.375,
    )?;
    // 分组独立：改 UI 不影响 BGM/SFX；主音量联动全部分组。
    assert_str(
        &mut runtime,
        "tostring(require('core.audio').set_bus_volume('ui', 0.8))",
        "true",
    )?;
    assert_num(
        &mut runtime,
        "require('core.audio').bus_volume('bgm')",
        0.25,
    )?;
    assert_num(
        &mut runtime,
        "require('core.audio').effective_volume('ui')",
        0.4,
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('core.audio').set_volume(1.0))",
        "true",
    )?;
    assert_num(
        &mut runtime,
        "require('core.audio').effective_volume('ui')",
        0.8,
    )?;
    assert_num(
        &mut runtime,
        "require('core.audio').effective_volume('bgm')",
        0.25,
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('core.audio').set_volume(0.5))",
        "true",
    )?;
    // 钳制到 0..=1；非法总线名明确失败（查询 nil、设置 false、引擎直调报错）。
    assert_str(
        &mut runtime,
        "tostring(require('core.audio').set_bus_volume('ui', 5.0))",
        "true",
    )?;
    assert_num(&mut runtime, "require('core.audio').bus_volume('ui')", 1.0)?;
    assert_str(
        &mut runtime,
        "tostring(require('core.audio').set_bus_volume('nope', 0.5))",
        "false",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('core.audio').bus_volume('nope'))",
        "nil",
    )?;
    assert_str(
        &mut runtime,
        "tostring(pcall(function() return audio.bus_volume('nope') end))",
        "false",
    )?;
    // 无输出设备（headless）时播放静默降级：请求被接受，不算失败。
    assert_str(
        &mut runtime,
        "tostring(require('core.audio').sfx('ui_click'))",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('core.audio').ui('ui_click'))",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "tostring(require('core.audio').play_on('sfx', '__m20_missing__'))",
        "false",
    )?;
    assert_num(&mut runtime, "require('core.audio').bus_volume('ui')", 1.0)?;

    banner("M20-02b 音量设置文本往返 + 非法版本拒绝");
    assert_str(
        &mut runtime,
        "require('core.audio').settings_text():sub(1, 16)",
        "rustlua-audio-v1",
    )?;
    assert_str(
        &mut runtime,
        "(function() local a = require('core.audio'); local text = a.settings_text(); \
          a.set_bus_volume('bgm', 0.1); \
          local ok = select(1, a.apply_settings(text)); \
          local rejected, why = a.apply_settings('rustlua-audio-v0\\nbgm=0.9'); \
          return tostring(ok) .. '|' .. tostring(rejected) .. '|' .. tostring(#why > 0) \
            .. '|' .. tostring(math.abs(a.bus_volume('bgm') - 0.25) < 1e-6) end)()",
        "true|false|true|true",
    )?;
    // 引擎直调同样拒绝：版本不符返回 false + 原因（不 panic）。
    assert_str(
        &mut runtime,
        "tostring(select(2, audio.apply_settings('rustlua-audio-v0\\n')) ~= nil \
          and select(2, audio.apply_settings('rustlua-audio-v0\\n')):find('unsupported', 1, true) ~= nil)",
        "true",
    )?;

    banner("M20-02c 暂停菜单「设置」页：四路音量 ± → 保存 project.toml → 返回");
    press(
        &mut runtime,
        KeyCode::from_char('p').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Pause")?;
    // M19-02 回归：主菜单行数（设置页是子页，不改变主菜单行数）。
    // M21 主角形象：主菜单新增「形象」行 → 5 项变 6 项，音量行索引 3 → 4。
    // M33：再加「帮助」行（6 → 7）——帮助排在音量**之后**，所以这里的 select(4) 索引不动。
    assert_num(&mut runtime, "#(require('ui.pause_menu').rows())", 7.0)?;
    lua(&mut runtime, "require('ui.pause_menu').select(4)")?;
    assert_str(&mut runtime, "require('ui.pause_menu').item()", "volume")?;
    press(&mut runtime, KeyCode::Space)?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.pause_menu').settings_active())",
        "true",
    )?;
    assert_num(
        &mut runtime,
        "#(require('ui.pause_menu').settings_rows())",
        6.0,
    )?;
    assert_str(
        &mut runtime,
        "require('ui.pause_menu').settings_rows()[1].key",
        "master",
    )?;
    assert_str(
        &mut runtime,
        "(function() for _, r in ipairs(require('ui.pause_menu').settings_rows()) do \
           if r.key == 'bgm' then return tostring(r.label:find('25', 1, true) ~= nil) end end \
         return 'missing' end)()",
        "true",
    )?;
    // master 0.5 → +0.05 → 0.55
    press(
        &mut runtime,
        KeyCode::from_char('+').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(
        &mut runtime,
        "require('core.audio').bus_volume('master')",
        0.55,
    )?;
    // W/S 选择（设置页内部分发）→ bgm → +0.05 → 0.30
    press(
        &mut runtime,
        KeyCode::from_char('s').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_str(
        &mut runtime,
        "require('ui.pause_menu').settings_item()",
        "bgm",
    )?;
    press(
        &mut runtime,
        KeyCode::from_char('+').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut runtime, "require('core.audio').bus_volume('bgm')", 0.3)?;
    // 方向键移两行 → ui → -0.05 → 0.95（主菜单主音量 shortcut 不受影响）
    press(&mut runtime, KeyCode::ArrowDown)?;
    press(&mut runtime, KeyCode::ArrowDown)?;
    assert_str(
        &mut runtime,
        "require('ui.pause_menu').settings_item()",
        "ui",
    )?;
    press(
        &mut runtime,
        KeyCode::from_char('-').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut runtime, "require('core.audio').bus_volume('ui')", 0.95)?;
    // 方向键上一行 → sfx → -0.05 → 0.70
    press(&mut runtime, KeyCode::ArrowUp)?;
    assert_str(
        &mut runtime,
        "require('ui.pause_menu').settings_item()",
        "sfx",
    )?;
    press(
        &mut runtime,
        KeyCode::from_char('-').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut runtime, "require('core.audio').bus_volume('sfx')", 0.7)?;
    // 保存设置（第 5 行）→ 写回 project.toml，保留注释与 [input] 段。
    lua(&mut runtime, "require('ui.pause_menu').settings_select(4)")?;
    assert_str(
        &mut runtime,
        "require('ui.pause_menu').settings_item()",
        "save",
    )?;
    press(&mut runtime, KeyCode::Space)?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.pause_menu').settings_msg():find('已保存', 1, true) ~= nil)",
        "true",
    )?;
    let written = fs::read_to_string(&settings_path)?;
    for needle in [
        "# M20-02 测试设置（示例结束时恢复）",
        "master = 0.55",
        "bgm = 0.3",
        "sfx = 0.7",
        "ui = 0.95",
        "map = \"keep-me\"",
    ] {
        if !written.contains(needle) {
            return Err(format!("project.toml 缺少 {needle:?}:\n{written}").into());
        }
    }
    println!("project.toml after save:\n{written}");
    // 重新读盘 → 覆盖内存中的临时改动（设置文件是分组音量的权威来源）。
    assert_str(
        &mut runtime,
        "(function() local a = require('core.audio'); a.set_bus_volume('bgm', 0.9); \
          local ok = select(1, a.load_settings()); \
          return tostring(ok) .. '|' .. tostring(math.abs(a.bus_volume('bgm') - 0.3) < 1e-6) end)()",
        "true|true",
    )?;
    // Esc 返回主菜单（不关闭暂停），再确认继续游戏。
    press(&mut runtime, KeyCode::Escape)?;
    assert_str(
        &mut runtime,
        "tostring(require('ui.pause_menu').settings_active())",
        "false",
    )?;
    assert_mode(&mut runtime, "Pause")?;
    // M21：主菜单 7 行（新增「形象」行 + M33 的「帮助」行）。
    assert_num(&mut runtime, "#(require('ui.pause_menu').rows())", 7.0)?;
    lua(&mut runtime, "require('ui.pause_menu').select(0)")?;
    press(&mut runtime, KeyCode::Space)?;
    assert_mode(&mut runtime, "Playing")?;

    banner("M20-02d 面板焦点导航：方向键/W-S 走引擎焦点环（背包/任务/暂停/对话/商店）");
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'heal_pill', 1); \
         inv.add(require('core.state').inventory, 'rice', 2); \
         inv.add(require('core.state').inventory, 'timber', 3)",
    )?;
    press(
        &mut runtime,
        KeyCode::from_char('i').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Inventory")?;
    let inv_rows = query_num(&mut runtime, "return #require('ui.inventory_ui').rows()")?;
    if inv_rows < 4.0 {
        return Err(format!("背包行数不足（{inv_rows}），无法验证焦点导航").into());
    }
    assert_num(&mut runtime, "ui.focus_count('bp:focus')", inv_rows)?;
    assert_num(&mut runtime, "ui.focus_index('bp:focus')", 0.0)?;
    // M32 竖版：背包改成**单列** 10 行（横屏是 2 列网格），因此左右方向键不再有意义
    // （单列列表里按右键不应移动选择），逐行下移由 ArrowDown 表达。
    press(&mut runtime, KeyCode::ArrowRight)?;
    assert_num(&mut runtime, "require('ui.inventory_ui').index()", 0.0)?;
    press(&mut runtime, KeyCode::ArrowDown)?;
    assert_num(&mut runtime, "require('ui.inventory_ui').index()", 1.0)?;
    assert_num(&mut runtime, "ui.focus_index('bp:focus')", 1.0)?;
    press(&mut runtime, KeyCode::ArrowDown)?;
    assert_num(&mut runtime, "require('ui.inventory_ui').index()", 2.0)?;
    // W/S 与方向键共用同一焦点环（既有键位仍可用）。
    press(
        &mut runtime,
        KeyCode::from_char('w').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut runtime, "require('ui.inventory_ui').index()", 1.0)?;
    // 末项再下移 → 环绕回**第一项**（单列列表的焦点循环语义：没有"同列"这回事了）。
    let last = inv_rows - 1.0;
    lua(
        &mut runtime,
        &format!("require('ui.inventory_ui').select({last})"),
    )?;
    assert_num(&mut runtime, "require('ui.inventory_ui').index()", last)?;
    press(&mut runtime, KeyCode::ArrowDown)?;
    assert_num(&mut runtime, "require('ui.inventory_ui').index()", 0.0)?;
    let inv_focus_count = query_num(&mut runtime, "return ui.focus_count('bp:focus')")?;
    press(
        &mut runtime,
        KeyCode::from_char('i').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Playing")?;

    press(
        &mut runtime,
        KeyCode::from_char('l').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Quest")?;
    // 任务日志有 8 条（M19-01 订单/主线）> 可视 7 行 → 滚动窗口生效。
    let quest_rows = query_num(&mut runtime, "return #require('ui.quest_ui').rows()")?;
    if quest_rows < 2.0 {
        return Err(format!("任务行数不足（{quest_rows}），无法验证焦点导航").into());
    }
    assert_num(&mut runtime, "require('ui.quest_ui').index()", 0.0)?;
    press(&mut runtime, KeyCode::ArrowDown)?;
    assert_num(&mut runtime, "require('ui.quest_ui').index()", 1.0)?;
    // 单列列表：逐行下移到末项（选中行被滚入视口），再下一次环绕回第一项。
    press_many(&mut runtime, KeyCode::ArrowDown, quest_rows as usize - 2)?;
    assert_num(
        &mut runtime,
        "require('ui.quest_ui').index()",
        quest_rows - 1.0,
    )?;
    // M32 竖版：任务行高 34 → 38（面板从 600×470 变成 592×720）。
    let quest_scroll_max = ((quest_rows - 7.0).max(0.0)) * 38.0;
    assert_num(
        &mut runtime,
        "ui.scroll_max('qst:scroll')",
        quest_scroll_max,
    )?;
    assert_num(
        &mut runtime,
        "ui.scroll_offset('qst:scroll')",
        quest_scroll_max,
    )?;
    assert_str(
        &mut runtime,
        &format!(
            "(function() local rows = require('ui.quest_ui').rows(); \
              local r = rows[{}]; \
              return tostring(r ~= nil and tostring(r.name) ~= '' and tostring(r.name) ~= 'nil') end)()",
            quest_rows as i64
        ),
        "true",
    )?;
    press(&mut runtime, KeyCode::ArrowDown)?;
    assert_num(&mut runtime, "require('ui.quest_ui').index()", 0.0)?;
    assert_num(&mut runtime, "ui.scroll_offset('qst:scroll')", 0.0)?;
    press(
        &mut runtime,
        KeyCode::from_char('q').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Playing")?;

    press(
        &mut runtime,
        KeyCode::from_char('p').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_mode(&mut runtime, "Pause")?;
    press(&mut runtime, KeyCode::ArrowDown)?;
    assert_num(&mut runtime, "require('ui.pause_menu').index()", 1.0)?;
    press(&mut runtime, KeyCode::ArrowDown)?;
    assert_num(&mut runtime, "require('ui.pause_menu').index()", 2.0)?;
    press(&mut runtime, KeyCode::ArrowUp)?;
    assert_num(&mut runtime, "require('ui.pause_menu').index()", 1.0)?;
    press(&mut runtime, KeyCode::Escape)?;
    assert_mode(&mut runtime, "Playing")?;

    assert_str(
        &mut runtime,
        "tostring(require('world.dialogue').start('gu_changfeng_teach'))",
        "true",
    )?;
    assert_mode(&mut runtime, "Dialogue")?;
    assert_num(&mut runtime, "require('world.dialogue').selected()", 1.0)?;
    assert_num(&mut runtime, "ui.focus_index('dlg:focus')", 0.0)?;
    press(&mut runtime, KeyCode::ArrowDown)?;
    assert_num(&mut runtime, "require('world.dialogue').selected()", 2.0)?;
    assert_num(&mut runtime, "ui.focus_index('dlg:focus')", 1.0)?;
    press(&mut runtime, KeyCode::ArrowUp)?;
    assert_num(&mut runtime, "require('world.dialogue').selected()", 1.0)?;
    press(&mut runtime, KeyCode::Escape)?;
    assert_mode(&mut runtime, "Playing")?;

    assert_str(
        &mut runtime,
        "tostring(require('systems.shop').open_ui('zhao_stall'))",
        "true",
    )?;
    assert_mode(&mut runtime, "Shop")?;
    assert_num(&mut runtime, "require('systems.shop').selected()", 1.0)?;
    assert_num(&mut runtime, "ui.focus_index('shp:focus')", 0.0)?;
    press(&mut runtime, KeyCode::ArrowDown)?;
    assert_num(&mut runtime, "require('systems.shop').selected()", 2.0)?;

    banner("M20-02e 列表滚动：库存超过可视槽位时按选中行滚动（ScrollState）");
    // 追加 8 条库存（数据驱动注册表可运行期扩展）→ 共 12 行 > 可视 9 行；
    // 断言完立即还原，避免污染 validate_data 的「重复上架」校验。
    let shop_stock = query_num(&mut runtime, "return #require('systems.shop').rows()")?;
    lua(
        &mut runtime,
        "local def = require('data.shops').zhao_stall; \
         for i = 1, 8 do def.stock[#def.stock + 1] = { item = 'timber', price = 15 + i, limit = 20 } end",
    )?;
    lua(&mut runtime, "require('systems.shop').render()")?;
    assert_num(
        &mut runtime,
        "#(require('systems.shop').rows())",
        shop_stock + 8.0,
    )?;
    assert_num(
        &mut runtime,
        "ui.scroll_max('shp:scroll')",
        (shop_stock + 8.0 - 9.0) * 34.0,
    )?;
    assert_num(&mut runtime, "ui.scroll_offset('shp:scroll')", 0.0)?;
    // 回到第 1 行后方向键下移 11 次 → 第 12 行；窗口滚动到让选中行可见。
    // offset 按选中行算：(12 - 可视 9) * 行高 34 = 102（与初始 stock 数无关；
    // 旧公式 (shop_stock+8-9)*34 只在 stock=4 时巧合正确，Alpha 作物扩展 stock=8 后暴露）。
    lua(&mut runtime, "require('systems.shop').select(1)")?;
    assert_num(&mut runtime, "require('systems.shop').selected()", 1.0)?;
    press_many(&mut runtime, KeyCode::ArrowDown, 11)?;
    assert_num(&mut runtime, "require('systems.shop').selected()", 12.0)?;
    assert_num(&mut runtime, "ui.scroll_offset('shp:scroll')", 102.0)?;
    // （M71 预设化后商店行渲染走 `prefab.set_text(ShopRowN)`，旧 id `shp:row9` 已退役；
    //  选中行与滚动窗口已由上面两条断言覆盖，行文本渲染由 M65 黄金门钉死。）
    let scroll_max = query_num(&mut runtime, "return ui.scroll_max('shp:scroll')")?;
    // 还原库存并收起面板（行数回落到数据文件声明的条数）。
    lua(
        &mut runtime,
        &format!(
            "local def = require('data.shops').zhao_stall; \
             for i = #def.stock, {} + 1, -1 do def.stock[i] = nil end",
            shop_stock as i64
        ),
    )?;
    lua(&mut runtime, "require('systems.shop').select(1)")?;
    assert_num(
        &mut runtime,
        "#(require('systems.shop').rows())",
        shop_stock,
    )?;
    press(&mut runtime, KeyCode::Escape)?;
    assert_mode(&mut runtime, "Playing")?;

    banner("M20-02f 回归锚点：统一校验 + HUD + 主音量 provider 与分组互不干扰");
    assert_str(
        &mut runtime,
        "(function() local report = require('validate_data').check({ strict = true }); \
          return report == '' and 'ok' or report end)()",
        "ok",
    )?;
    assert_str(
        &mut runtime,
        "require('ui.input').update():find('气血', 1, true) ~= nil and 'ok' or 'none'",
        "ok",
    )?;
    // 主音量仍随存档 provider 往返（M19-02 语义不变）。
    assert_str(
        &mut runtime,
        "(function() local a = require('core.audio'); a.set_volume(0.45); \
          local snap = require('core.save_manager').capture(); \
          a.set_volume(1.0); require('core.save_manager').restore(snap); \
          return tostring(math.abs(a.volume() - 0.45) < 1e-6) end)()",
        "true",
    )?;
    // 分组音量不受 provider 影响（以 project.toml 为准）。
    assert_str(
        &mut runtime,
        "tostring(math.abs(require('core.audio').bus_volume('bgm') - 0.3) < 1e-6)",
        "true",
    )?;

    println!(
        "jhyt_m20_audio_ok project={} settings={} buses={} focus_bp={} scroll_max={} pause_rows={} pause_settings_rows={}",
        project.display(),
        settings_path.display(),
        query_str(
            &mut runtime,
            "(function() local a = require('core.audio'); \
              return string.format('%.2f/%.2f/%.2f/%.2f', a.bus_volume('master'), \
                a.bus_volume('bgm'), a.bus_volume('sfx'), a.bus_volume('ui')) end)()"
        )?,
        inv_focus_count,
        scroll_max,
        query_num(&mut runtime, "return #require('ui.pause_menu').rows()")?,
        query_num(
            &mut runtime,
            "return #require('ui.pause_menu').settings_rows()"
        )?,
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers ----------

/// 备份 `<project>/project.toml`，drop 时恢复（原本不存在则删除）。
///
/// M20-02 的 project.toml 是可选工程文件：示例必须写入才能验证「启动应用 +
/// 面板保存」，因此用它保证仓库状态在成功/失败路径下都被还原。
struct SettingsGuard {
    path: PathBuf,
    original: Option<String>,
}

impl SettingsGuard {
    fn capture(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let original = match fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path: path.to_path_buf(),
            original,
        })
    }
}

impl Drop for SettingsGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(text) => {
                let _ = fs::write(&self.path, text);
            }
            None => {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

/// Creates a bare engine runtime pointing at the project scripts, audio assets and save file.
fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // 音频目录固定为 <project>/assets/audio（`rustlua-audio` 的 audio.rs）。
    runtime.set_audio_asset_dir(project.join("assets").join("audio"));
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(1280, 720);
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

fn press_many(
    runtime: &mut LuaRuntime,
    key: KeyCode,
    times: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..times {
        press(runtime, key)?;
    }
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
    let value = query_str(runtime, &format!("return {expr}"))?;
    let got = value
        .parse::<f64>()
        .map_err(|e| format!("{expr} = {value:?} is not a number: {e}"))?;
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
