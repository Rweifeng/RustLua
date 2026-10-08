//! Headless acceptance drive for M30 / 计划 RustLua（阶段五）：
//! 中文排版、缺字与溢出检查——"文字有没有画到面板外面、有没有豆腐块"从此可机器验收。
//!
//! 为什么这一轮值得做：`docs/42` §1 早就写了"中文字形 11px 起可读"、美术交付也按
//! 同一套字号做，但**运行时排版从来没有被检查过**：
//!   * HUD 用"CJK 记 2 单位、其余记 1 单位"的启发式截断（`hud.clip`），
//!     `金币 99999 文`、`WASD…`（宽拉丁字母）这类串会算错宽度；
//!   * 对话正文既未换行也未裁剪，长台词直接画到面板外（外面就是世界，看不见字）；
//!   * 背包/任务/商店/暂停面板的行也只按字符数截断，同样会溢出；
//!   * 字体是 `assetc::package` 用 `char_collect` 收的字符子集裁出来的，
//!     运行时文案里只要有一个字符没被收到，正式包里就是豆腐块（开发机上却看不出来）。
//!
//! 本样例按真实玩法驱动（全部 headless、可重复）：
//! ① 度量口径：`ui.measure_text` 与 `ui.text_fit` 逐字符求和必须一致；CJK 满宽、ASCII 半宽；
//! ② 语料与缺字：从数据表 + 运行时行收集**全部**可能画出的文案，逐条查缺字；
//!    并核对"运行时用到的字符 ⊆ `char_collect` 收集集合"（首包字体子集契约）；
//! ③ 排版审计：逐个打开 HUD / 背包 / 任务 / 暂停（含设置页）/ 商店 / 对话，
//!    遍历 `ui.ui_text` 登记表按真实字宽量宽——零溢出、零缺字、零低于 11px；
//! ④ 最坏数字：金钱 99999、天数 999、数量 999 等压力值下重绘，仍然零溢出；
//! ⑤ 字号下限：作者化载体里的 `Text` 组件（`scenes/*.scene` + M63b 起的
//!    `assets/prefabs/*.prefab`，两者同一套行格式，只数场景会在搬运后静默变瞎）
//!    与 Lua 绘制调用的字号都 ≥ 11（静态扫描 + 运行时读回）；
//!    M59 起还有一路**数据侧**读数：UI 预设（`assets/ui/prefabs/*.ui.json`）把字号从源码
//!    搬进了数据文件，`ui/prefab.lua` 因此是静态扫描的唯一例外（它传 `node.font`，源码里没有
//!    字面量），例外由"读回每个预设 text 节点比同一条下限 + 加载期 10px 负例"补平；
//!    M71 把这一层整体退役（`ui.prefab.NAMES` 清空）后，数据侧那道改判"登记数与节点数
//!    不能一侧非 0"，退役是真退役而不是门瞎了（见 ⑤ 小节注释）；
//! ⑥ 负例：审计必须**有牙齿**（塞进超宽串/emoji/10px 字号必须各报一条），
//!    以及换行规则（标点不落行首、行数上限、省略号收尾）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m30_cjk_text [-- <game_jhyt-root>]

use engine::core::{
    entity::EntityId,
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
    ui::UiText,
};
use std::{env, fs, path::Path, path::PathBuf};

/// 引擎测得的最小可读字号（`docs/42` §1：中文字形 11px 起可读）。
const MIN_FONT_SIZE: f64 = 11.0;
/// 语料规模下限（收集口径见 `scripts/tools/text_audit.lua`；改数据表会增长，不该缩水）。
const MIN_CORPUS_TEXTS: usize = 350;
const MIN_CORPUS_CHARS: usize = 550;
/// 场景 `Text` 组件数量下限（`scenes/*.scene` **加上** `assets/prefabs/*.prefab`；
/// 少一个说明作者侧的 UI 实体被改坏了，或有一份预设搬进了新载体却没算进这道统计——
/// M63b 把门面那 17 个实体从 `main.scene` 搬进预设时，就同时踩到了后者）。
const MIN_SCENE_TEXTS: usize = 21;
/// Lua 侧登记调用下限（`ui_text.draw*` 17 处 + `ui_text.record` 11 处 = 28，2026-10-08 实测）。
/// 只许涨不许跌：少一处登记 = 少一条被宽度与字号审计量到的文案。
/// 原值是 30（那时暂停菜单的 11 行还都画在动态层），M66/B3 把它们换成实体层载体后
/// 动态层只剩 17 处——`record` 补回 11 处，总数仍比迁移前少 2 处（HUD 在 M70 搬进预设时
/// 没有补登记，那批文案今天量不到，见本包 未完成项 与 `tasks/plan/M72-10`）。
const MIN_REGISTERED_TEXT_SITES: usize = 28;

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
    let save_path = fresh_save_path(&project, "jhyt_m30_cjk_text.sav");

    let mut rt = new_runtime(&project, &save_path)?;
    enter_main_menu(&mut rt, &main_script)?;
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;
    frames(&mut rt, 2)?;

    // ---------------- ① 度量口径：与渲染同源 ----------------
    banner("M30-01① ui.measure_text / text_fit：CJK 满宽、ASCII 半宽、逐字符求和一致");
    // 14px 字号 + 1px 字距 → CJK 每字 15px、ASCII 每字约 7.5px（NotoSansSC 的 advance）。
    assert_num(&mut rt, "ui.measure_text('江湖有田', 14).width", 60.0)?;
    assert_num(
        &mut rt,
        "(function() return require('ui.text_fit').width('江湖有田', 14) end)()",
        60.0,
    )?;
    assert_str(
        &mut rt,
        "tostring(ui.measure_text('AB', 14).width < ui.measure_text('江湖', 14).width)",
        "true",
    )?;
    assert_num(&mut rt, "ui.measure_text('江湖\\n有田', 14).width", 30.0)?;
    assert_num(&mut rt, "ui.measure_text('江湖\\n有田', 14).lines", 2.0)?;
    // 两条独立路径必须一致：`text_fit.width` 是逐字符求和（f64），`ui.measure_text` 是引擎
    // 整串度量（f32 累加）——差异只允许是浮点噪声（0.05px = 1/20 像素，肉眼与布局都无感）。
    assert_str(
        &mut rt,
        "(function() local tf = require('ui.text_fit'); \
           local samples = { '江湖有田', 'WASD 移动 · E 交互', '金币 99999 文', \
           '限购剩余 999', '水田（湿润）', 'AB 12/34 【完成】', '背包容量 12/24 · 负重 3.5' }; \
           for _, text in ipairs(samples) do \
             for _, size in ipairs({ 13.0, 15.0, 18.0, 24.0 }) do \
               local a = ui.measure_text(text, size).width; \
               local b = tf.width(text, size); \
               if math.abs(a - b) > tf.EPSILON then \
                 return string.format('mismatch %s@%s: %s vs %s', text, size, a, b) \
               end \
             end \
           end; \
           return 'ok' end)()",
        "ok",
    )?;
    assert_str(&mut rt, "ui.measure_text('江湖🙂', 15).missing", "🙂")?;
    assert_str(
        &mut rt,
        "tostring(ui.measure_text('江湖', 15).missing == '')",
        "true",
    )?;
    assert_str(
        &mut rt,
        "tostring(pcall(function() return ui.measure_text('x', 0) end))",
        "false",
    )?;
    // 读回口径：暂停菜单「帮助」页的小标题行（`PseHRow1` = `操作 · 键鼠` / `操作 · 手柄`）。
    // 操作说明已从 HUD 底部通栏撤下（M33），HUD 上不再有键位文字，帮助页是它的新家。
    lua(
        &mut rt,
        "local pm = require('ui.pause_menu'); pm.open(); pm.open_help()",
    )?;
    assert_str(
        &mut rt,
        "tostring(require('ui.pause_menu').help_active())",
        "true",
    )?;
    let (row1_text, row1_size, row1_drawn) = scene_text(&mut rt, "PseHRow1")?;
    if !row1_drawn {
        return Err(
            "帮助页小标题 PseHRow1 这一帧没画（页容器没显示，或 set_visible 没打开该行）".into(),
        );
    }
    if (row1_size - 18.0).abs() > 1e-6 {
        return Err(format!("PseHRow1 字号 = {row1_size}，want 18").into());
    }
    if !row1_text.contains("操作") {
        return Err(format!("PseHRow1 文字 = {row1_text:?}，不含「操作」").into());
    }
    // 帮助页画完，紧接着 HUD 的引导行也必须能整行读回（曾经被右栏 276px 裁掉半句）。
    let (tutorial_text, _, tutorial_drawn) = scene_text(&mut rt, "HudTutorial")?;
    if !tutorial_drawn {
        return Err("HUD 引导行 HudTutorial 这一帧没画：读回口径没有可断言的东西".into());
    }
    if !tutorial_text.contains("按 E") {
        return Err(format!("HudTutorial 文字 = {tutorial_text:?}，不含「按 E」").into());
    }
    // 换页必须收掉上一页的元素：动态文本跨帧持久，只画新页会让主菜单的标题与
    // 七行菜单叠在帮助页同一块面板上（实机表现为两页文字重影，且没有任何报错）。
    // 预设化后三页互斥走 `prefab.set_visible` 的子树语义，因此牙齿在实体层的节点表里。
    let (_, _, menu_row_drawn) = scene_text(&mut rt, "PseRow1")?;
    if menu_row_drawn {
        return Err("帮助页打开时主菜单的行还在画：两页文字重影".into());
    }
    // 返回主菜单后反向同理：帮助页整页收掉、主菜单重新画出来。
    lua(
        &mut rt,
        "local pm = require('ui.pause_menu'); pm.help_close()",
    )?;
    let (_, _, help_row_drawn) = scene_text(&mut rt, "PseHRow1")?;
    if help_row_drawn {
        return Err("关闭帮助页后帮助页的行还在画：换页没收到上一页".into());
    }
    let (menu_row_text, _, menu_row_drawn) = scene_text(&mut rt, "PseRow1")?;
    if !menu_row_drawn {
        return Err(format!("回到主菜单后 PseRow1 没画（文字 = {menu_row_text:?}）").into());
    }
    lua(&mut rt, "local pm = require('ui.pause_menu'); pm.close()")?;

    // ---------------- ② 语料与首包字符覆盖 ----------------
    banner("M30-02② 全量语料无缺字 + 运行时字符 ⊆ 首包字体子集（char_collect）");
    let corpus_texts = query_num(&mut rt, "require('tools.text_audit').stats().texts")? as usize;
    let corpus_chars = query_num(&mut rt, "require('tools.text_audit').stats().chars")? as usize;
    let corpus_missing =
        query_num(&mut rt, "require('tools.text_audit').stats().missing")? as usize;
    if corpus_texts < MIN_CORPUS_TEXTS || corpus_chars < MIN_CORPUS_CHARS {
        return Err(format!(
            "语料缩水：texts={corpus_texts}（≥{MIN_CORPUS_TEXTS}）chars={corpus_chars}（≥{MIN_CORPUS_CHARS}）"
        )
        .into());
    }
    if corpus_missing != 0 {
        let detail = query_str(&mut rt, "require('tools.text_audit').missing().chars")?;
        return Err(format!("语料缺字 {corpus_missing} 条，缺字符：{detail}").into());
    }
    // 首包字体子集：`assetc::package` 用 `char_collect` 收字符（场景/预制体 Text + Lua 字面量），
    // 运行时用到的每个字符都必须已被收到，否则正式包里就是豆腐块。
    let collected = engine::core::char_collect::collect_project_chars(&project);
    let runtime_chars = query_str(&mut rt, "require('tools.text_audit').unique_chars()")?;
    let mut uncovered: Vec<char> = runtime_chars
        .chars()
        .filter(|ch| !collected.contains(*ch))
        .collect();
    uncovered.sort_unstable();
    uncovered.dedup();
    if !uncovered.is_empty() {
        let list: String = uncovered.iter().collect();
        return Err(format!(
            "运行时文案有 {} 个字符没被首包字体子集覆盖：{list}",
            uncovered.len()
        )
        .into());
    }
    println!(
        "  语料 texts={corpus_texts} chars={corpus_chars} chars_collected={} 覆盖=ok",
        collected.len()
    );

    // ---------------- ③ 排版审计：逐个面板 ----------------
    banner("M30-03③ 逐面板排版审计：真实字宽量宽，零溢出/零缺字/≥11px");
    // HUD（每帧重绘 → `ui.ui_text` 登记表就是"本帧画了什么、各自可用多宽"）。
    let hud_checked = assert_layout_clean(&mut rt, "HUD", 4)?;
    // 桌面侧的平台门（M33）：`app.platform()='desktop'` 时触屏控件必须整个收掉——
    // 登记表里不该有 `vp` 这个 owner，也不该有摇杆/按钮的元素。
    // （反向口径"触屏上必须画得出来"不在这里：native headless 报告不了 webgl，
    // 由 web harness 与 `scripts/wechat-minigame-harness.js` 在真机/浏览器侧守。）
    assert_str(&mut rt, "tostring(app.platform())", "desktop")?;
    assert_str(
        &mut rt,
        "tostring(require('ui.ui_text').recorded().vp == nil)",
        "true",
    )?;
    assert_str(
        &mut rt,
        "tostring(require('ui.virtual_pad').available())",
        "false",
    )?;
    // 背包：物品行 + 详情 + 提示。
    lua(&mut rt, "require('ui.inventory_ui').open()")?;
    let bp_checked = assert_layout_clean(&mut rt, "背包", 3)?;
    lua(&mut rt, "require('ui.inventory_ui').close()")?;
    // 任务日志：行 + 发布者 + 目标行。
    lua(&mut rt, "require('ui.quest_ui').open()")?;
    let qst_checked = assert_layout_clean(&mut rt, "任务日志", 3)?;
    lua(&mut rt, "require('ui.quest_ui').close()")?;
    // 暂停菜单 + 设置页（两页共用 owner="pse"，设置页重绘后登记表是设置页那一轮）。
    lua(&mut rt, "require('ui.pause_menu').open()")?;
    assert_layout_clean(&mut rt, "暂停菜单", 3)?;
    lua(&mut rt, "require('ui.pause_menu').open_settings()")?;
    let pse_checked = assert_layout_clean(&mut rt, "设置页", 3)?;
    lua(&mut rt, "require('ui.pause_menu').settings_close()")?;
    lua(&mut rt, "require('ui.pause_menu').close()")?;
    // 触屏那一整套文案（帮助页分叉 + 三条面板提示）在桌面运行时**一行都不会被画出来**
    // （`app.platform()='desktop'` ⇒ `pause_menu` 走键鼠分支），于是"触屏说明装不装得进
    // 556px 的值列"这种问题门里量不到。`set_touch(true)` 是专门的旁路：这里量的是
    // 微信/浏览器玩家实际看到的那几行，超宽会被判成截断而不是静静画成省略号。
    lua(
        &mut rt,
        "local pm = require('ui.pause_menu'); pm.set_touch(true); pm.open(); pm.open_help()",
    )?;
    let pse_touch = assert_layout_clean(&mut rt, "帮助页(触屏文案)", 8)?;
    lua(&mut rt, "require('ui.pause_menu').help_close()")?;
    assert_layout_clean(&mut rt, "暂停菜单(触屏提示)", 3)?;
    lua(&mut rt, "require('ui.pause_menu').open_settings()")?;
    assert_layout_clean(&mut rt, "设置页(触屏提示)", 3)?;
    lua(
        &mut rt,
        "local pm = require('ui.pause_menu'); pm.settings_close(); pm.close(); pm.set_touch(nil)",
    )?;
    // 商店：标题/金额/数量/行/提示（右列只有 176px，长物品名必须被截断）。
    assert_str(
        &mut rt,
        "tostring(require('systems.shop').open_ui('zhao_stall'))",
        "true",
    )?;
    let shp_checked = assert_layout_clean(&mut rt, "商店", 5)?;
    lua(&mut rt, "require('systems.shop').close_ui()")?;
    // 对话：正文按宽度换行（多行 → 逐行量宽），选项单行截断。
    assert_str(
        &mut rt,
        "tostring(require('world.dialogue').start('lin_xiaoman_rice'))",
        "true",
    )?;
    let dlg_checked = assert_layout_clean(&mut rt, "对话", 3)?;
    lua(&mut rt, "require('world.dialogue').close(true)")?;
    println!(
        "  量宽条数 hud={hud_checked} bp={bp_checked} qst={qst_checked} pse={pse_checked} \
         shp={shp_checked} dlg={dlg_checked} 触屏帮助页={pse_touch}"
    );

    // ---------------- ④ 最坏数字与压力值 ----------------
    banner("M30-04④ 压力值（金钱 99999 / 天数 999 / 数量 999）下仍然零溢出");
    lua(&mut rt, "require('core.state').money = 99999")?;
    assert_num(&mut rt, "require('core.state').money", 99999.0)?;
    frames(&mut rt, 2)?;
    assert_str(
        &mut rt,
        "tostring((function() for _, line in ipairs(require('ui.hud').status_lines()) do \
           if line:find('金钱 99999', 1, true) then return 'found' end end; return 'missing' end)())",
        "found",
    )?;
    let hud_stress = assert_layout_clean(&mut rt, "HUD(压力值)", 4)?;
    // 天数 999：HUD 的日期行是全年最长的文本形态之一（3 位数天数 + 季节 + 时段）。
    lua(&mut rt, "clock.set(999, 720)")?;
    frames(&mut rt, 2)?;
    assert_str(
        &mut rt,
        "tostring((function() for _, line in ipairs(require('ui.hud').status_lines()) do \
           if line:find('第999天', 1, true) then return 'found' end end; return 'missing' end)())",
        "found",
    )?;
    let hud_day999 = assert_layout_clean(&mut rt, "HUD(第999天)", 4)?;
    // 商店：最大金额 + 限购行；背包：满容量文案。
    lua(&mut rt, "require('core.state').money = 999999")?;
    assert_str(
        &mut rt,
        "tostring(require('systems.shop').open_ui('zhao_stall'))",
        "true",
    )?;
    assert_layout_clean(&mut rt, "商店(999999 文)", 5)?;
    lua(&mut rt, "require('systems.shop').close_ui()")?;
    // 截断机制本身：给定预算装不下时必须换成省略号，且宽度不超预算。
    assert_str(
        &mut rt,
        "(function() local tf = require('ui.text_fit'); \
           local fit = tf.fit('一二三四五六七八九十', 15.0, 60.0); \
           local width = tf.width(fit, 15.0); \
           if width > 60.0 + 1e-6 then return 'overflow ' .. width end; \
           if fit:sub(-3) ~= '…' then return 'no_ellipsis ' .. fit end; \
           return 'ok' end)()",
        "ok",
    )?;
    // 真实行的最坏形态：HUD 状态行 + 商店限购行 + 任务行，全部按预算截断后仍放得下。
    assert_str(
        &mut rt,
        "(function() local tf = require('ui.text_fit'); \
           local cases = { \
           { text = '金币 999999 文 · 第 999 天 · 冬 · 23:59', size = 14.0, budget = 252.0 }, \
           { text = '灵芝菌种  999999 文  限购剩余 999', size = 18.0, budget = 592.0 }, \
           { text = '9. 青云山道采药订单【已完成】', size = 17.0, budget = 552.0 }, \
           { text = '容量 24/24 · 负重 999.9', size = 16.0, budget = 276.0 } }; \
           for _, case in ipairs(cases) do \
             local fit = tf.fit(case.text, case.size, case.budget); \
             local width = tf.width(fit, case.size); \
             if width > case.budget + 1e-6 then \
               return string.format('overflow %.1f > %.1f (%s)', width, case.budget, fit) \
             end \
           end; \
           return 'ok' end)()",
        "ok",
    )?;

    // ---------------- ⑤ 字号下限 ----------------
    banner("M30-05⑤ 字号下限：场景/预设 Text ≥ 11px + Lua 登记调用（draw*/record）字号 ≥ 11px");
    let (scene_count, scene_min) = scene_font_sizes(&project)?;
    if scene_count < MIN_SCENE_TEXTS {
        return Err(
            format!("场景与预设的 Text 组件只有 {scene_count} 个（≥{MIN_SCENE_TEXTS}）").into(),
        );
    }
    if scene_min < MIN_FONT_SIZE {
        return Err(format!("场景/预设 Text 存在 {}px 字号（下限 11px）", scene_min).into());
    }
    // Lua 侧：扫描全部 `ui_text` 登记调用（动态层 `draw*` 与实体层 `record`），取各自的
    // 字号实参，必须是数字且 ≥ 11；清单为空是自检项——解析不了就报错，不能"看起来扫过了"。
    // 下限的来龙去脉写在 `MIN_REGISTERED_TEXT_SITES` 的文档里（一句话：只许涨不许跌）。
    let (registered_calls, unresolved, lua_min, lua_min_site) =
        lua_font_sizes(&project.join("scripts"))?;
    if registered_calls < MIN_REGISTERED_TEXT_SITES {
        return Err(format!(
            "只扫到 {registered_calls} 个 ui_text 登记调用（draw*/record，≥{MIN_REGISTERED_TEXT_SITES}）"
        )
        .into());
    }
    if !unresolved.is_empty() {
        return Err(format!(
            "有 {} 个登记调用的字号没能解析（自检失败）：{}",
            unresolved.len(),
            unresolved.join(" | ")
        )
        .into());
    }
    if lua_min < MIN_FONT_SIZE {
        return Err(
            format!("Lua 登记调用存在 {lua_min}px 字号（下限 11px）：{lua_min_site}",).into(),
        );
    }
    println!(
        "  场景/预设 Text {scene_count} 个，最小 {scene_min}px；Lua 登记 {registered_calls} 处，最小 {lua_min}px"
    );
    // **数据侧的同一条下限**（M59）：UI 预设把字号从源码搬进了
    // `assets/ui/prefabs/*.ui.json`，上面那道源码扫描因此看不见它（`prefab.lua` 传的是
    // `node.font`）。这里读的是**加载并校验之后**的每一个 text 节点，比的是同一条 11px——
    // 预设只要存在，下限就自动进门，不需要谁再改一次这道门。
    //
    // **M71 把这一层整体退役了**（`p.NAMES` 清空、`dialogue.ui.json` 改走运行期
    // `.prefab`），所以这里现在量到的是「登记 0 份、text 节点 0 个」。**不把它当成失败**
    // （那等于要求一具尸体还活着），而是把「登记数与 text 节点数必须同时为 0 或同时非 0」
    // 立成断言：往 `NAMES` 里加一份预设却忘了它该过字号门，这里立刻红。
    let prefab_fonts = query_str(
        &mut rt,
        "(function() local p = require('ui.prefab'); \
          local names, count, min = #p.NAMES, 0, math.huge; \
          for _, name in ipairs(p.NAMES) do \
            for _, row in ipairs(p.dump(name)) do \
              if row.type == 'text' then \
                count = count + 1; \
                if type(row.font) ~= 'number' then \
                  return 'not_a_number:' .. name .. ':' .. tostring(row.id) .. '|0|0' \
                end; \
                if row.font < min then min = row.font end \
              end \
            end \
          end; \
          return tostring(names) .. '|' .. tostring(count) .. '|' .. tostring(min) end)()",
    )?;
    let (names_raw, count_raw, min_raw) = match prefab_fonts.split_once('|') {
        Some((head, tail)) => match tail.split_once('|') {
            Some((count, min)) => (head, count, min),
            None => return Err(format!("UI 预设字号读数形状不对: {prefab_fonts}").into()),
        },
        None => return Err(format!("UI 预设字号读数形状不对: {prefab_fonts}").into()),
    };
    let prefab_names: f64 = names_raw
        .parse()
        .map_err(|_| format!("UI 预设字号读数形状不对: {prefab_fonts}"))?;
    let prefab_text_count: f64 = count_raw
        .parse()
        .map_err(|_| format!("UI 预设字号读数形状不对: {prefab_fonts}"))?;
    if prefab_names > 0.0 && prefab_text_count < 1.0 {
        return Err(format!(
            "UI 预设登记了 {prefab_names:.0} 份却一个 text 节点都没扫到（{prefab_fonts}）：例外清单白列"
        )
        .into());
    }
    if prefab_names < 1.0 {
        println!(
            "  UI 预设（JSON 层，M71 退役）登记 0 份、text 节点 0 个；字号下限由实体层扫描 + 加载期负例守"
        );
    } else {
        let prefab_min_font: f64 = min_raw
            .parse()
            .map_err(|_| format!("UI 预设字号读数形状不对: {prefab_fonts}"))?;
        if prefab_min_font < MIN_FONT_SIZE {
            return Err(format!(
                "UI 预设存在 {prefab_min_font}px 字号（下限 11px，{} 个 text 节点）",
                prefab_text_count as usize
            )
            .into());
        }
        println!(
            "  UI 预设 text 节点 {} 个（登记 {prefab_names:.0} 份），最小 {prefab_min_font}px",
            prefab_text_count as usize
        );
    }
    // 常驻负例：加载期必须**拒绝**低于 11px 的字号节点——`lua_font_sizes` 放过
    // `ui/prefab.lua`（它传 `node.font`，源码里没有字面量可解析）只有靠这条才不是洞
    // （没有它，把 `check_node` 的下限删掉两侧都会绿）。JSON 层退役之后这条仍然跑：
    // 它守的是"这条路重新启用时下限还在不在"，而不是一份当前数据。
    assert_str(
        &mut rt,
        "(function() local p = require('ui.prefab'); \
          local bad = { prefab_version = 1, name = 'probe', owner = 'probe', root = { \
            name = 'n', type = 'text', id = 'probe:t', text = '小', font = 10.0, \
            layout = { preset = 'top_left', \
              offsets = { left = 0, top = 0, right = 100, bottom = 20 } } } }; \
          local ok, issues = p.validate('probe', bad); \
          if ok then return 'accepted' end; \
          return issues[1]:find('11px') ~= nil and 'caught' or ('wrong:' .. issues[1]) end)()",
        "caught",
    )?;
    // 架构规则：`scripts/**` 一律不许直接调 `ui.text_at`（唯一出口是 `ui/ui_text.lua`）。
    // 否则"画了但没登记"的文字会从排版检查里漏掉——那正是零报错的坏行为。
    let raw_sites = raw_text_at_sites(&project.join("scripts"))?;
    if raw_sites != 0 {
        return Err(
            format!("有 {raw_sites} 处直接调用 ui.text_at（必须走 ui/ui_text.lua）").into(),
        );
    }

    // ---------------- ⑥ 审计的牙齿与换行规则 ----------------
    banner("M30-06⑥ 负例：溢出/缺字/过小字号/截断必须各报一条；换行遵守标点禁则");
    // 审计必须能报错：塞进一条超宽串、一条含 emoji、一条 10px 字号（owner="probe"），
    // 再加一条**被裁成省略号**的（`draw` 走 `text_fit.fit`，画面上"看着正常"但少了后半句）。
    lua(
        &mut rt,
        "local ui_text = require('ui.ui_text'); \
         ui_text.begin('probe'); \
         ui_text.draw_exact('probe', 'probe:overflow', 0, 0, 15.0, 10.0, '一二三四五六七八九十'); \
         ui_text.draw_exact('probe', 'probe:missing', 0, 0, 15.0, 999.0, '江湖🙂'); \
         ui_text.draw_exact('probe', 'probe:font', 0, 0, 10.0, 999.0, '小'); \
         ui_text.draw('probe', 'probe:clip', 0, 0, 15.0, 60.0, '一二三四五六七八九十')",
    )?;
    let probe_bad = query_num(&mut rt, "#require('tools.text_audit').audit().bad")? as usize;
    if probe_bad != 3 {
        let detail = query_str(
            &mut rt,
            "table.concat(require('tools.text_audit').audit().bad, ' | ')",
        )?;
        return Err(
            format!("审计只报了 {probe_bad} 条（应为溢出/缺字/字号各 1 条）：{detail}").into(),
        );
    }
    assert_str(
        &mut rt,
        "(function() local bad = table.concat(require('tools.text_audit').audit().bad, '|'); \
           for _, kind in ipairs({ 'overflow=', 'missing=', 'font=' }) do \
             if not bad:find(kind, 1, true) then return 'missing:' .. kind end \
           end; \
           return 'ok' end)()",
        "ok",
    )?;
    // 截断这条牙也要能咬：只有 `probe:clip` 一条被裁，且报出的是**原始**文案（不是省略号串）。
    let probe_clip_count =
        query_num(&mut rt, "#require('tools.text_audit').audit().clipped")? as usize;
    let probe_clipped = query_str(
        &mut rt,
        "table.concat(require('tools.text_audit').audit().clipped, ' | ')",
    )?;
    if probe_clip_count != 1 {
        return Err(format!(
            "截断审计应只有 probe:clip 一条（画面上的行都放得下）：{probe_clipped}"
        )
        .into());
    }
    if !probe_clipped.contains("probe:clip") || !probe_clipped.contains("一二三四五六七八九十")
    {
        return Err(format!("截断审计没有咬到 probe:clip：{probe_clipped}").into());
    }
    lua(&mut rt, "require('ui.ui_text').reset('probe')")?;
    assert_layout_clean(&mut rt, "负例清理后", 4)?;
    // 换行：宽度不超、标点不落行首、行数上限 + 省略号收尾。
    assert_str(
        &mut rt,
        "(function() local tf = require('ui.text_fit'); \
           local lines = tf.wrap(\
           '这味药要先晒三天，再配着一钱甘草下锅；火候大了就苦，火候小了不入味。', 18.0, 200.0); \
           if #lines < 2 then return 'not_wrapped' end; \
           local budget = 200.0; \
           for _, line in ipairs(lines) do \
             if tf.width(line, 18.0) > budget + 1e-6 then return 'overflow:' .. line end; \
             local head = line:sub(1, 3); \
             for _, punct in ipairs({ '，', '。', '、', '；', '：', '？', '！', '）', '」' }) do \
               if head == punct then return 'line_start_punct:' .. line end \
             end \
           end; \
           return 'ok' end)()",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "(function() local tf = require('ui.text_fit'); \
           local lines = tf.wrap(\
           '这味药要先晒三天，再配着一钱甘草下锅；火候大了就苦，火候小了不入味。', 18.0, 200.0, 2); \
           if #lines ~= 2 then return 'lines=' .. #lines end; \
           if lines[2]:sub(-3) ~= '…' then return 'no_ellipsis:' .. lines[2] end; \
           return 'ok' end)()",
        "ok",
    )?;
    // 空串 / 零预算 / 单字符边界。
    assert_str(
        &mut rt,
        "(function() return require('ui.text_fit').fit('', 15.0, 100.0) end)()",
        "",
    )?;
    assert_str(
        &mut rt,
        "(function() return require('ui.text_fit').fit('江湖', 15.0, 0.0) end)()",
        "",
    )?;
    assert_str(
        &mut rt,
        "(function() return require('ui.text_fit').fit('江', 15.0, 100.0) end)()",
        "江",
    )?;
    assert_str(
        &mut rt,
        "tostring(#require('ui.text_fit').wrap('', 15.0, 100.0) == 1)",
        "true",
    )?;

    let total_elements = query_num(&mut rt, "require('ui.ui_text').count()")? as usize;
    let _ = fs::remove_file(&save_path);
    println!(
        "jhyt_m30_cjk_text_ok corpus={corpus_texts} chars={corpus_chars} \
         collected={} missing=0 elements={total_elements} \
         hud={hud_checked}/{hud_stress}/{hud_day999} bp={bp_checked} qst={qst_checked} \
         pse={pse_checked} shp={shp_checked} dlg={dlg_checked} \
         scene_text={scene_count}@{scene_min}px lua_text={registered_calls}@{lua_min}px",
        collected.len()
    );
    Ok(())
}

/// 断言"当前画面文字零溢出/零缺字/**零截断**/字号达标"，并返回量宽条数（下限 `min_checked`）。
///
/// `clipped`（原始文案被裁成省略号）单独判：`bad` 只抓"画到框外面"，而省略号是另一种
/// 坏味道——文字**看着没溢出**，玩家却读不到被裁掉的后半句（HUD 引导行曾经整条被右栏
/// 276px 预算吃掉一半，画面上一切正常）。这一轮把"零截断"也变成门。
fn assert_layout_clean(
    rt: &mut LuaRuntime,
    label: &str,
    min_checked: usize,
) -> Result<usize, Box<dyn std::error::Error>> {
    let checked = query_num(rt, "require('tools.text_audit').audit().checked")? as usize;
    let bad = query_str(
        rt,
        "table.concat(require('tools.text_audit').audit().bad, ' | ')",
    )?;
    let clipped = query_str(
        rt,
        "table.concat(require('tools.text_audit').audit().clipped, ' | ')",
    )?;
    let worst = query_num(rt, "require('tools.text_audit').audit().worst")?;
    if !bad.is_empty() {
        return Err(format!("{label} 排版审计失败：{bad}").into());
    }
    if !clipped.is_empty() {
        return Err(format!("{label} 有文案被裁成省略号（原始文案超出预算）：{clipped}").into());
    }
    if checked < min_checked {
        return Err(format!("{label} 只量到 {checked} 条文字（≥{min_checked}）").into());
    }
    if worst > 1e-6 {
        return Err(format!("{label} 最大超宽 {worst}").into());
    }
    Ok(checked)
}

/// 作者侧 `Text` 组件的字号统计：返回（数量，最小值）。
///
/// 载体有两个：`scenes/*.scene` 与 `assets/prefabs/*.prefab`（M63b 起门面的 17 个 UI 实体
/// 住在预设里）。两者是同一套 `component\t…\tText\tfont_size=…` 行格式，只认场景的统计会
/// 随搬运**静默变瞎**——实测搬完之后场景侧只剩 14 个 Text（原 21），下限那道断言立刻红，
/// 而字号本身一个都没变。预设与场景同为作者化载体，必须一起数。
fn scene_font_sizes(project: &Path) -> Result<(usize, f64), Box<dyn std::error::Error>> {
    let mut count = 0usize;
    let mut min = f64::MAX;
    let mut files: Vec<PathBuf> = fs::read_dir(project.join("scenes"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "scene"))
        .collect();
    let prefab_dir = project.join("assets").join("prefabs");
    if prefab_dir.is_dir() {
        // 只收一层：预设目录现在是 `assets/prefabs/<name>.prefab` 平铺，将来若要分子目录
        // 再换递归（这里刻意不递归，避免把开发产物也拖进统计）。
        files.extend(
            fs::read_dir(&prefab_dir)?
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| path.extension().is_some_and(|ext| ext == "prefab")),
        );
    }
    files.sort();
    for path in files {
        let is_prefab = path.extension().is_some_and(|ext| ext == "prefab");
        let entities = if is_prefab {
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("{} 读取失败：{error}", path.display()))?;
            engine::core::prefab::Prefab::from_text(&text)
                .map_err(|error| format!("{} 解析失败：{error}", path.display()))?
                .entities
        } else {
            engine::core::scene::Scene::load(&path)
                .map_err(|error| format!("{} 读取失败：{error}", path.display()))?
                .entities
        };
        for entity in entities.values() {
            let Some(value) = entity.components.get("Text") else {
                continue;
            };
            count += 1;
            let size = value
                .split(';')
                .find_map(|field| field.strip_prefix("font_size="))
                .and_then(|raw| raw.trim().parse::<f64>().ok())
                .ok_or_else(|| {
                    format!(
                        "{} 的 Text 组件没有可解析的 font_size：{value}",
                        path.display()
                    )
                })?;
            if size < min {
                min = size;
            }
        }
    }
    Ok((count, if count == 0 { 0.0 } else { min }))
}

/// ⑤ 的 Lua 侧读数：（登记调用数，解析不出字号的清单，最小字号，最小字号出处 `文件:行号`）。
/// 起名字而不是把四元组写在签名里，是因为 `clippy::type_complexity` 会把后者判成"复杂类型"，
/// 而这条门恰好是唯一一处要把四个读数一起交回调用方的地方（清单必须能顶掉自检）。
type LuaFontReading = (usize, Vec<String>, f64, String);

/// Lua 登记调用的字号扫描，读数见 [`LuaFontReading`]。
///
/// 扫描对象是 `scripts/**/*.lua` 里所有 `ui_text` 的**登记**调用：
/// `draw` / `draw_wrapped` / `draw_exact`（动态层：画 + 登记，字号是第 5 个实参）与
/// `record`（实体层载体：字由 `prefab.set_text` 写进 `Text` 组件，这里只登记宽度契约，
/// 字号是第 3 个实参）。两种载体共用同一条下限——面板搬进预设之后，如果这里只认
/// `draw*`，"搬家"就等于把 11px 下限静默放开。
/// 清单里带 `文件:行号`，否则"有 2 个没解析出来"这种报错要人回头逐个 grep。
///
/// **唯一的例外**是 `ui/prefab.lua`（M59 的 UI 预设绘制分发器）：它的字号实参是
/// `node.font`，值来自 `assets/ui/prefabs/*.ui.json`，源码里根本没有字面量可解析。
/// 这不是把下限放开——下限改由 ⑤ 的"数据侧扫描 + 加载期负例"两路守（见该小节），
/// 覆盖面比这里少三处调用还严：预设里 `font` 必须是**数值**且 ≥ 11，否则加载就报错。
/// M71 把 JSON 预设层整体退役（`NAMES` 清空），所以这两路今天量到的是空集；数据侧那道
/// 因此不断言"至少一个 text 节点"，而是断言"登记数与节点数不能一侧非 0"——想重新启用
/// 这条路的人依然会被门问住，而不会因为尸体没有呼吸而红。
fn lua_font_sizes(root: &Path) -> Result<LuaFontReading, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    collect_lua_files(root, &mut files)?;
    files.sort();
    // 全局字号常量：`ui/layout.lua` 的版面表（`M.BOX.title_font = 22.0` 等）是**跨文件**
    // 被引用的（各面板 `local BOX = layout.BOX` 后写 `BOX.hint_font`），因此先扫一遍它，
    // 得到一份"全局常量表"，再与每个文件自己的常量合并。
    let mut global_constants = std::collections::BTreeMap::new();
    let layout_module = root.join("ui").join("layout.lua");
    if layout_module.is_file() {
        global_constants = lua_font_constants(&fs::read_to_string(&layout_module)?);
    }
    // `min` 必须从 `f64::MAX` 起：`Default` 给的是 0.0，于是"任何字号都不比它小"，
    // ⑤ 的读数就永远是 0px（这条在 derive(Default) 上踩过一次，注释留在这里防第二次）。
    let mut stat = FontSizeStat {
        min: f64::MAX,
        ..Default::default()
    };
    for path in files {
        if path.file_name().is_some_and(|name| name == "prefab.lua") {
            continue;
        }
        // 注释里也会出现 `ui_text.draw(...)`（本模块的用法说明就是），先剥注释再扫描。
        let source = strip_lua_comments(&fs::read_to_string(&path)?);
        // 模块级字号常量（`local LINE_FONT = 14`）+ 版面表常量：字号允许写成常量，
        // 但必须能解析出来，解析不出来就让 `skipped` 把自检顶掉——不允许"看起来扫过了"。
        let mut constants = global_constants.clone();
        for (name, value) in lua_font_constants(&source) {
            constants.insert(name, value);
        }
        // 逐个 `ui_text.<名>(...)` 调用取字号（名字精确匹配，见 `REGISTERED_FONT_SIZE_ARGS`）。
        scan_registered_calls(&source, &path, &constants, &mut stat)?;
    }
    Ok((
        stat.calls,
        stat.unresolved,
        if stat.calls == 0 { 0.0 } else { stat.min },
        stat.min_site,
    ))
}

/// `ui_text` 的登记函数 → 字号实参在第几个（0 起）。
///
/// `draw` / `draw_wrapped` / `draw_exact` 画在**动态层**（`owner, id, x, y, size, …`），
/// 字号在第 5 位；`record` 的载体是**实体层**（`.prefab` 的 `Text` 组件由
/// `prefab.set_text` 写），位置由预设里的 `UiLayout` 决定、脚本里没有 x/y 可传，
/// 所以字号在第 3 位。
///
/// 名字必须**整段相等**：`ui_text.recorded()`（登记表的只读访问器）含 `record` 这个前缀，
/// 按前缀匹配会把它当成一处登记调用、然后在第 3 个实参上报解析不出字号（实踩过）。
const REGISTERED_FONT_SIZE_ARGS: &[(&str, usize)] = &[
    ("draw", 4),
    ("draw_wrapped", 4),
    ("draw_exact", 4),
    ("record", 2),
];

/// ⑤ 的累计量：登记调用数、解析不出字号的清单、最小字号（带 `文件:行号`，
/// 否则"最小 0px"这种读数要人回头逐个 grep）。
#[derive(Default)]
struct FontSizeStat {
    calls: usize,
    unresolved: Vec<String>,
    min: f64,
    min_site: String,
}

/// 扫一份 Lua 源码里的每一个登记调用：顶层第 `size_arg`（0 起）个实参必须能解析出字号，
/// 解析不出就带 `文件:行号` 进清单——不允许"看起来扫过了"。
fn scan_registered_calls(
    source: &str,
    path: &Path,
    constants: &std::collections::BTreeMap<String, f64>,
    stat: &mut FontSizeStat,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = source.as_bytes();
    let is_ident_byte = |ch: u8| ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'.';
    let mut cursor = 0usize;
    while let Some(found) = source[cursor..].find("ui_text.") {
        let start = cursor + found;
        let name_start = start + "ui_text.".len();
        let mut name_end = name_start;
        while name_end < bytes.len() && is_ident_byte(bytes[name_end]) {
            name_end += 1;
        }
        // `ui_text.` 后面可能再跟一层（`ui_text.audit`），也允许空白后才是左括号。
        let name = source[name_start..name_end].trim_end();
        let Some(&(_, size_arg)) = REGISTERED_FONT_SIZE_ARGS.iter().find(|(n, _)| *n == name)
        else {
            cursor = name_end;
            continue;
        };
        let Some(open) = source[name_end..].find('(').map(|offset| name_end + offset) else {
            cursor = name_end;
            continue;
        };
        // 调用后面第一个左括号必须属于这次调用本身：中间若换了行/出现别的标记就跳过
        // （`is_ident_byte` 已把 `recorded`、`draw_x` 这类长名字整段吃掉，这里只管空白）。
        if source[name_end..open].chars().any(|ch| !ch.is_whitespace()) {
            cursor = name_end;
            continue;
        }
        let source_line = source[..start].lines().count();
        // 找到与之匹配的右括号（跳过字符串字面量里的括号）。
        let mut depth = 0usize;
        let mut index = open;
        let mut in_string: Option<u8> = None;
        while index < bytes.len() {
            let ch = bytes[index];
            match in_string {
                Some(quote) => {
                    if ch == b'\\' {
                        index += 1;
                    } else if ch == quote {
                        in_string = None;
                    }
                }
                None => match ch {
                    b'\'' | b'"' => in_string = Some(ch),
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                },
            }
            index += 1;
        }
        if index >= bytes.len() {
            stat.unresolved.push(format!(
                "{}:{} 调用的括号没有闭合",
                path.display(),
                source_line
            ));
            break;
        }
        let args = split_top_level(&source[open + 1..index]);
        stat.calls += 1;
        let size = args.get(size_arg).map(|arg| arg.trim().to_string());
        let resolved = size.as_deref().and_then(|raw| {
            raw.parse::<f64>()
                .ok()
                .or_else(|| constants.get(raw).copied())
        });
        match resolved {
            Some(size) => {
                if size < stat.min {
                    stat.min = size;
                    stat.min_site = format!("{}:{}", path.display(), source_line);
                }
            }
            None => stat.unresolved.push(format!(
                "{}:{} 第 {} 个实参解析不出字号：{:?}（已知常量 {} 个）",
                path.display(),
                source_line,
                size_arg + 1,
                size,
                constants.len()
            )),
        }
        cursor = index + 1;
    }
    Ok(())
}

/// `scripts/**` 里直接调用 `ui.text_at` 的处数（`ui/ui_text.lua` 自己除外）。
fn raw_text_at_sites(root: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    collect_lua_files(root, &mut files)?;
    files.sort();
    let mut total = 0usize;
    for path in files {
        if path.file_name().is_some_and(|name| name == "ui_text.lua") {
            continue;
        }
        let source = strip_lua_comments(&fs::read_to_string(&path)?);
        let hits = source.matches("ui.text_at(").count();
        if hits > 0 {
            eprintln!("  {} 直接调用 ui.text_at {hits} 处", path.display());
            total += hits;
        }
    }
    Ok(total)
}

/// 收集模块级数字常量：`local NAME = 14` / `local NAME = 18.0`（单赋值一行）。
fn lua_number_constants(source: &str) -> std::collections::BTreeMap<String, f64> {
    let mut map = std::collections::BTreeMap::new();
    for line in source.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("local ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty()
            || !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            || name.chars().next().is_some_and(|ch| ch.is_ascii_digit())
        {
            continue;
        }
        let value = value.trim().trim_end_matches(',');
        if let Ok(number) = value.parse::<f64>() {
            map.insert(name.to_string(), number);
        }
    }
    map
}

/// **字号**常量解析：在 [`lua_number_constants`] 之上，额外接受来自版面表的点号引用。
///
/// 为什么需要（M32 竖版）：版面几何集中在 `scripts/ui/layout.lua`，字号也跟着进去了
/// （`M.BOX.title_font = 22.0`、`M.BOX.hint_font = 14.0`）。各面板写成
/// `ui_text.draw(..., BOX.hint_font, ...)`（`local BOX = layout.BOX`）或
/// `ui_text.draw(..., layout.BOX.hint_font, ...)`。
/// 扫描器原先只认"裸数字 / 模块级数字常量"，于是这些调用会落进 `skipped` 把自检顶掉——
/// 而自检的本意是"不允许看起来扫过了"，不是"字号只许写字面量"。
///
/// 做法（纯文本，不引入 Lua 求值器）：把 `M.BOX = { ... }` 块里的 `key = <数字>`
/// 同时登记成三种写法 `key` / `BOX.key` / `layout.BOX.key`；面板里的 `BOX` 别名因此
/// 自然命中，不需要再去追别名链。解析不出来的引用仍计入 `skipped`（自检保持有牙齿）。
fn lua_font_constants(source: &str) -> std::collections::BTreeMap<String, f64> {
    let mut map = lua_number_constants(source);
    // 字号还允许写成"可读下限 + 随几何缩放"：`local cfont = math.max(11, c_radius * 0.62)`
    // （触屏按钮里的字随圆半径变，但永不低于 11px）。这里登记那个**下限**——它正是这道门
    // 要保证的最小值；不认这种写法的话，动态字号就只能从检查里漏掉（漏掉=零报错的坏行为）。
    for line in source.lines() {
        let Some(rest) = line.trim().strip_prefix("local ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty()
            || !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            continue;
        }
        let Some(inner) = value
            .trim()
            .strip_prefix("math.max(")
            .and_then(|s| s.strip_suffix(')'))
        else {
            continue;
        };
        let Some(floor) = inner
            .split(',')
            .next()
            .and_then(|first| first.trim().parse::<f64>().ok())
        else {
            continue;
        };
        map.insert(name.to_string(), floor);
    }
    // 截取 `M.BOX = {` 起始的整块（从 `{` 到配对的 `}`）。
    let Some(anchor) = source.find("M.BOX = {") else {
        return map;
    };
    let rest = &source[anchor..];
    let Some(open) = rest.find('{') else {
        return map;
    };
    let mut depth = 0usize;
    let mut end = rest.len();
    for (offset, ch) in rest[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = open + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    for line in rest[open..end].lines() {
        let line = line.trim().trim_end_matches(',').trim_end_matches('}');
        // 去掉行尾注释后取 `key = value`。
        let line = line.split("--").next().unwrap_or("").trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim().trim_end_matches(','));
        if key.is_empty()
            || !key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            continue;
        }
        if let Ok(number) = value.parse::<f64>() {
            map.insert(key.to_string(), number);
            map.insert(format!("BOX.{key}"), number);
            map.insert(format!("layout.BOX.{key}"), number);
        }
    }
    map
}

/// 剥掉 Lua 注释（`--` 到行尾、`--[[ ... ]]` 与 `--[==[ ... ]==]` 长注释），保留字符串。
/// 字号扫描只看"真的会被执行的绘制调用"，注释里讨论用法不算调用。
fn strip_lua_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'-' && bytes.get(index + 1) == Some(&b'-') {
            let cursor = index + 2;
            if bytes.get(cursor) == Some(&b'[') {
                let mut equals = 0;
                while bytes.get(cursor + 1 + equals) == Some(&b'=') {
                    equals += 1;
                }
                if bytes.get(cursor + 1 + equals) == Some(&b'[') {
                    let close = format!("]{}]", "=".repeat(equals));
                    let rest = &text[cursor + 2 + equals..];
                    index = match rest.find(&close) {
                        Some(offset) => cursor + 2 + equals + offset + close.len(),
                        None => text.len(),
                    };
                    continue;
                }
            }
            index = match text[index..].find('\n') {
                Some(offset) => index + offset,
                None => text.len(),
            };
            continue;
        }
        let ch = text[index..].chars().next().unwrap_or('\0');
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

/// 顶层逗号切分（忽略括号与字符串里的逗号）。
fn split_top_level(source: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        match in_string {
            Some(quote) => {
                current.push(ch);
                if ch == '\\' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else if ch == quote {
                    in_string = None;
                }
            }
            None => match ch {
                '\'' | '"' => {
                    in_string = Some(ch);
                    current.push(ch);
                }
                '(' | '{' | '[' => {
                    depth += 1;
                    current.push(ch);
                }
                ')' | '}' | ']' => {
                    depth = depth.saturating_sub(1);
                    current.push(ch);
                }
                ',' if depth == 0 => {
                    parts.push(current.trim().to_string());
                    current.clear();
                }
                _ => current.push(ch),
            },
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

fn collect_lua_files(
    root: &Path,
    out: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_lua_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "lua") {
            out.push(path);
        }
    }
    Ok(())
}

fn fresh_save_path(project: &Path, file: &str) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(file);
    let _ = fs::remove_file(&path);
    path
}

/// 读回**实体层**（`.prefab` 实例）里一个 `Text` 节点：`(文字, 字号, 这一帧画不画)`。
///
/// 为什么不读 `ui.text_get`（这条门原本的做法）：M66/B3 把暂停菜单、M70 把 HUD 搬进预设
/// 之后，画面上的行由 `prefab.set_text` 写进场景的 `Text` 组件，动态层那些 id 只剩
/// `ui_text.hide` 留下的占位——`ui.hide` 走 `entry(id).or_insert(默认)`，于是它的字号恒为
/// 基准 12、可见性恒为 false，`ui.text_get` 再也断言不到画面上的任何东西（`hud:tutorial`
/// 那个 id 干脆已经不存在，读回是 nil）。继续留着这些断言，门会绿得没有牙齿。
///
/// "画不画"看的是 `prefab_ui_nodes`：`prefab.set_visible(name, false)` 藏掉的整棵子树
/// **一个布局节点都不建**（`scene_spawn.rs` 的口径——拿不到矩形就既不画也不产点击目标），
/// 所以节点在表里 = 这一帧真在画，与"看不见的按钮仍能被点中"互斥。
fn scene_text(
    runtime: &mut LuaRuntime,
    name: &str,
) -> Result<(String, f64, bool), Box<dyn std::error::Error>> {
    let raw = query_str(runtime, &format!("return tostring(entity.find('{name}'))"))?;
    let Ok(raw) = raw.parse::<f64>() else {
        return Err(format!("实体层找不到节点 {name:?}（entity.find 返回 {raw:?}）").into());
    };
    let raw = raw as u64;
    let id = EntityId::from_raw((raw >> 32) as u32, raw as u32);
    let drawn = runtime
        .prefab_ui_nodes()
        .contains_key(&u64::from(id.index()));
    let text = runtime.with_world(|world| {
        world
            .get::<UiText>(id)
            .ok()
            .flatten()
            .map(|component| (component.text.clone(), f64::from(component.font_size)))
    });
    let Some((text, size)) = text else {
        return Err(format!("节点 {name:?} 没有 Text 组件").into());
    };
    Ok((text, size, drawn))
}

fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
}

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

fn frames(runtime: &mut LuaRuntime, n: usize) -> Result<(), Box<dyn std::error::Error>> {
    runtime.set_input(InputSnapshot::default());
    for _ in 0..n {
        runtime.call_update()?;
    }
    Ok(())
}

#[allow(dead_code)]
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
    let value = query_str(runtime, &format!("return tostring({expr})"))?;
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
    let got = query_num(runtime, expr)?;
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

fn banner(title: &str) {
    println!("== {title}");
}
