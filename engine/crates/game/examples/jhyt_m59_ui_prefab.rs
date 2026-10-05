//! Headless acceptance drive for M59《江湖有田》UI 预设（prefab）地基 + `dialogue` 试点。
//!
//! 这一道门守的是"把版面从 Lua 搬进 JSON 之后，画面一个像素都没变"。
//! 迁移前每个界面自己算矩形（`world/dialogue.lua` 的 `M.render` 一边算
//! `y + OPTION_DY + (i-1) * OPTION_ROW_H` 一边拼文案），迁移后版面来自
//! `assets/ui/prefabs/<name>.ui.json`，由 `scripts/ui/prefab.lua` 解析成同一批
//! `ui.panel/ui.image/ui_text.draw` 调用。玩法回归用例（m19/m20/m24/m30）全部**按 id**
//! 断言文本与位置，所以"id 与解析后的矩形逐值不变"是这次改造唯一可验收的口径。
//!
//! 覆盖：
//! - ① 预设真的**从文件读出来**（`data.load` 走 VFS，不是 Lua 里写死的表）+ 结构校验通过；
//! - ② `prefab.PRESETS` 的 12 个锚点与引擎 `UiLayoutPreset::ALL` **逐值对账**（两边各解释一遍
//!   锚点的表现是"编辑器画一套、运行时画另一套"，而这张表是唯一可能各写一份的地方）；
//! - ③ 版面不变量 `prefab.audit()` 通过，且 `ui/prefab` 已登记进 `validate_data`
//!   （漏登记 = 这份 JSON 从没被任何检查读过，m23 的「登记覆盖」同一口径）；
//! - ④ **黄金对账**：每条对话在开场节点上，逐 id 读回引擎里真正画出来的
//!   矩形/文本/字号，与 `support/jhyt_m59_dialogue_golden.tsv` 逐行相等。
//!   黄金文件的来源是迁移**前**的代码（见 `--capture` 的用法与本文件头注释），
//!   不是新实现自己吐出来的数——否则这道门只会证明"我写的和我写的相符"；
//! - ⑤ 收口：`close()` 之后预设里每一个 id 都必须"不存在或不可见"，
//!   且**选到第 3 项时第 1 项的标记必须消失**（迁移前不会消失，见该小节注释）；
//! - ⑥ 发行可见性：预设文件名不会命中 `assetc::is_dev_artifact` 的三条裁除规则。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m59_ui_prefab [-- <game_jhyt-root>] [--capture]
//!
//! `--capture` 打印 ④ 的读数表后退出（用来**在迁移前的树上**重生成黄金文件）：
//!   git worktree add %TEMP%\jhyt-head HEAD
//!   cargo run -p game --example jhyt_m59_ui_prefab -- %TEMP%\jhyt-head\examples\game_jhyt --capture

use engine::core::{resource_kind::ResourceKind, script::LuaRuntime, ui_layout::UiLayoutPreset};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// ④ 的黄金表：`state \t id \t kind \t 读数`，逐行与运行期读数比较。
const GOLDEN: &str = include_str!("support/jhyt_m59_dialogue_golden.tsv");

/// 遍历的 id 顺序 = 读数表的顺序（面板框 → 立绘 → 文本 → 选项行 → 提示 → 继续箭头）。
/// `text`/`image`/`panel` 决定用哪个读回 API；`ui.*_get` 对不存在的 id 返回 nil，
/// 于是"该画的没画"和"不该画的没清"都会落进同一张表里。
const PROBE_IDS: &[(&str, &str)] = &[
    ("dlg:box", "panel"),
    ("dlg:portrait_frame", "image"),
    ("dlg:portrait", "image"),
    ("dlg:speaker", "text"),
    ("dlg:text", "text"),
    ("dlg:opt1", "text"),
    ("dlg:opt2", "text"),
    ("dlg:opt3", "text"),
    ("dlg:opt4", "text"),
    ("dlg:opt5", "text"),
    ("dlg:marker1", "image"),
    ("dlg:marker2", "image"),
    ("dlg:marker3", "image"),
    ("dlg:marker4", "image"),
    ("dlg:marker5", "image"),
    ("dlg:hint", "text"),
    ("dlg:continue", "image"),
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let capture = args.iter().any(|a| a == "--capture");
    let project = args
        .iter()
        .find(|a| !a.starts_with("--"))
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
    let prefab_file = project
        .join("assets")
        .join("ui")
        .join("prefabs")
        .join("dialogue.ui.json");

    let save_path = fresh_save_path(&project, "jhyt_m59_ui_prefab.sav");
    let mut runtime = new_runtime(&project, &save_path)?;
    enter_playable(&mut runtime, &main_script)?;

    banner("④ 黄金对账：逐条对话开场节点的 id→矩形/文本/字号读数");
    let dump = collect_dump(&mut runtime)?;
    if capture {
        // 迁移前的树没有 `ui/prefab.lua`，`--capture` 就是给那棵树用的：只打印读数、不做比较。
        println!("{dump}");
        let _ = fs::remove_file(&save_path);
        return Ok(());
    }
    compare_golden(&dump)?;
    println!("golden_lines={}", dump.lines().count());

    banner("① 预设来自文件：data.load 读 `<name>.ui.json` + 结构校验");
    // 这三条合起来证明"版面确实在数据文件里"：改盘上的 JSON 会改变解析结果，
    // 而 Lua 侧没有任何一份写死的对话版面。
    assert_str(
        &mut runtime,
        "tostring(require('ui.prefab').DIR == 'assets/ui/prefabs/')",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "require('ui.prefab').load('dialogue').name",
        "dialogue",
    )?;
    let tree_size = query_num(
        &mut runtime,
        "#(require('ui.prefab').load('dialogue').root.children)",
    )?;
    if tree_size < 1.0 {
        return Err("预设 root.children 为空：版面没有在 JSON 里".into());
    }
    println!("prefab_root_children={tree_size}");
    // 结构非法必须**报错**而不是静默少画：`skin.icon` 对未知键是 no-op，
    // "贴图键拼错"的旧形态就是屏上少一张图、日志一行没有。
    assert_str(
        &mut runtime,
        "(function() local p = require('ui.prefab'); \
          local bad = { prefab_version = 1, name = 'x', owner = 'x', root = { \
            name = 'n', type = 'image', id = 'x:a', texture = 'no_such_skin_key', \
            layout = { preset = 'top_left', offsets = { left = 0, top = 0, right = 8, bottom = 8 } } } }; \
          local ok, issues = p.validate('x', bad); \
          if ok then return 'accepted' end; \
          return issues[1]:find('未在 ui/skin.lua 注册') ~= nil and 'caught' or ('wrong:' .. issues[1]) end)()",
        "caught",
    )?;
    // 缺槽位也要响：静默当 0 的表现是"控件叠在父节点原点上"。
    assert_str(
        &mut runtime,
        "(function() local p = require('ui.prefab'); \
          local tree = p.load('dialogue'); \
          local ok = pcall(p.render, 'dialogue', { speaker = 'a', body = 'b', hint = 'c', options = {} }); \
          return tostring(ok) end)()",
        "false",
    )?;

    banner("② 锚点预设表与引擎 UiLayoutPreset::ALL 逐值对账");
    let mut checked = 0;
    for preset in UiLayoutPreset::ALL {
        let name = preset.label().to_ascii_lowercase().replace(' ', "_");
        let anchors = preset.anchors();
        let expected = format!(
            "{},{},{},{}",
            anchors[0], anchors[1], anchors[2], anchors[3]
        );
        let got = query_str(
            &mut runtime,
            &format!(
                "(function() local a = require('ui.prefab').PRESETS['{name}']; \
                  if not a then return 'missing' end; \
                  return string.format('%g,%g,%g,%g', a[1], a[2], a[3], a[4]) end)()"
            ),
        )?;
        if got != expected {
            return Err(format!("锚点预设 {name} 与引擎不一致：Lua={got} 引擎={expected}").into());
        }
        checked += 1;
    }
    println!("anchor_presets={checked}");

    banner("③ 版面不变量 + 启动门登记");
    assert_str(
        &mut runtime,
        "(function() local ok, issues = require('ui.prefab').audit(); \
          return ok and 'ok' or table.concat(issues, ' | ') end)()",
        "ok",
    )?;
    assert_str(
        &mut runtime,
        "(function() for _, e in ipairs(require('validate_data').registered()) do \
           if e.path == 'ui.prefab' then return 'registered' end end; return 'missing' end)()",
        "registered",
    )?;
    // 与 `game --check` / 编辑器同一个门：预设里那份 `layout.PANEL.dialogue` 副本一旦分叉，
    // 这里必须读到非空报告（`audit()` ② 的定义处只有一个，检查点必须也只有一个）。
    assert_str(
        &mut runtime,
        "(function() local report = require('validate_data').check({ strict = true }); \
          return report == '' and 'ok' or report end)()",
        "ok",
    )?;
    // 预设的 root 矩形 = `layout.PANEL.dialogue`（迁移期矩形唯一定义处仍是 layout.lua）。
    assert_str(
        &mut runtime,
        "(function() local p = require('ui.prefab').load('dialogue'); \
          local panel = require('ui.layout').PANEL.dialogue; \
          local off = p.root.layout.offsets; \
          return string.format('%.0f,%.0f,%.0f,%.0f', panel.x, panel.y, panel.w, panel.h) \
            .. '|' .. string.format('%.0f,%.0f,%.0f,%.0f', \
              off.left, off.top, off.right - off.left, off.bottom - off.top) end)()",
        "240,450,800,240|240,450,800,240",
    )?;
    // `options.max` 必须等于玩法侧的 `OPTION_LIMIT`（数字直选 1..5 与 UI 槽位同源）：
    // 两边不一致的表现是"按 6 号键没反应"或"第 6 条选项永远看不见"。
    assert_str(
        &mut runtime,
        "(function() local list; \
          for _, child in ipairs(require('ui.prefab').load('dialogue').root.children) do \
            if child.type == 'list' then list = child end end; \
          return tostring(list.max) .. '|' .. tostring(require('world.dialogue').option_limit()) end)()",
        "5|5",
    )?;
    // 提示行字号仍取 `layout.BOX.hint_font`（预设里写的是同一数值，这条守"数值没各走各的"）。
    assert_str(
        &mut runtime,
        "(function() local layout = require('ui.layout'); \
          for _, child in ipairs(require('ui.prefab').load('dialogue').root.children) do \
            if child.id == 'dlg:hint' then \
              return child.font == layout.BOX.hint_font and 'equal' or 'diff' end end; \
          return 'missing' end)()",
        "equal",
    )?;

    banner("⑤ 收口：关面板与移动选中后，不该留下的那条都不留");
    // `clear_ui()` 迁移前是 14 行手写 `ui.clear`/`ui_text.hide`，漏一条就是"上一段对话的
    // 选项还挂在屏上"。现在整棵树的 id 由预设遍历收口，所以这里改成**遍历断言**：
    // 预设里的每个 id 都必须"不存在或不可见"。
    assert_str(
        &mut runtime,
        "(function() local dlg = require('world.dialogue'); \
          dlg.start('gu_changfeng_teach'); dlg.close(true); \
          local left = {}; \
          for _, row in ipairs(require('ui.prefab').dump('dialogue')) do \
            if row.id then \
              local t = ui.text_get(row.id); local im = ui.image_get(row.id); \
              local pa = ui.panel_get(row.id); \
              if (t and t.visible ~= false) or (im and im.visible ~= false and im.x ~= nil) \
                  or (pa and pa.visible ~= false) then left[#left + 1] = row.id end \
            end \
          end; \
          return #left == 0 and 'ok' or ('leftover:' .. table.concat(left, ',')) end)()",
        "ok",
    )?;
    // 迁移修掉的一条真缺陷：旧 `M.render` 只给选中行画 8×8 标记，**从不收掉上一次选中的那条**
    // （引擎的图片记录是 retained 的），所以 W/S 走一遍之后每行都留着一个箭头。
    // 这条断言在迁移前的代码上是红的——黄金表因此只取"开场、selected=1"的状态，
    // 而这个状态单独钉在这里，作为"新旧不完全等价"的那一处**有意**差异。
    assert_str(
        &mut runtime,
        "(function() local dlg = require('world.dialogue'); \
          dlg.close(true); dlg.start('gu_changfeng_teach'); \
          for _, row in ipairs({ 2, 3 }) do dlg.select(row); dlg.render() end; \
          local stale = {}; \
          for i = 1, 5 do \
            local im = ui.image_get('dlg:marker' .. i); \
            local drawn = im ~= nil and im.visible ~= false; \
            if drawn and i ~= 3 then stale[#stale + 1] = i end \
          end; \
          return #stale == 0 and 'ok' or ('stale:' .. table.concat(stale, ',')) end)()",
        "ok",
    )?;
    // 整屏隐藏（等价 Unity 的 SetActive(false)）走同一条收口。
    assert_str(
        &mut runtime,
        "(function() local p = require('ui.prefab'); \
          p.render('dialogue', { speaker = '顾长风', body = '台词', hint = '提示', \
            options = {}, has_portrait = false }, { hidden = true }); \
          local pa = ui.panel_get('dlg:box'); local t = ui.text_get('dlg:speaker'); \
          return tostring(pa == nil or pa.visible == false) .. '|' .. \
            tostring(t == nil or t.visible == false) end)()",
        "true|true",
    )?;

    banner("⑥ 发行可见性：预设文件名不会被打包裁掉");
    // `assetc::is_dev_artifact` 只裁三条：文件名**恰为** `manifest.json`、
    // 含 `preview` 的 `*.png`、以 `_` 开头的名字。`dialogue.ui.json` 三条都不命中。
    let stem = prefab_file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !prefab_file.is_file() {
        return Err(format!("预设文件不存在: {}", prefab_file.display()).into());
    }
    if stem == "manifest.json"
        || stem.starts_with('_')
        || (stem.ends_with(".png") && stem.contains("preview"))
    {
        return Err(format!("{stem} 会命中 assetc 的开发产物裁除规则").into());
    }
    // 编辑器侧的既有事实（不是断言，是给 M59 后续批次的锚点）：`*.ui.json` 的扩展名是
    // `.json`，而 `ResourceKind::from_path` 现在把它归为 **Scene**——所以"在编辑器里做 UI 预设"
    // 必须先给它一条独立归类，否则资产面板会把预设当场景导入。
    let kind = ResourceKind::from_path(&prefab_file);
    println!("prefab_file={stem} resource_kind={kind:?}");

    println!(
        "jhyt_m59_ui_prefab_ok project={} golden_lines={} anchors={}",
        project.display(),
        dump.lines().count(),
        checked
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers ----------

/// ④ 的驱动：对**每条已注册对话**开场（`selected` 恒为 1），逐 id 读回真实画面记录。
///
/// 只走开场状态： cond 依赖任务/变量，`choose` 会改这些状态，黄金表因此必须建立在
/// "新档、未改状态"这一确定起点上（同一条对话在两个树里的读数才可比）。
fn collect_dump(runtime: &mut LuaRuntime) -> Result<String, Box<dyn std::error::Error>> {
    // 拼一张 Lua 侧的 id 清单（与 `PROBE_IDS` 同序，顺序本身由 Rust 侧决定）。
    let ids_lua = PROBE_IDS
        .iter()
        .map(|(id, kind)| format!("{{ '{id}', '{kind}' }}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = String::new();
    let ids = query_str(
        runtime,
        "return table.concat(require('world.dialogue').ids(), ',')",
    )?;
    for dialogue_id in ids.split(',').filter(|s| !s.is_empty()) {
        let block = format!(
            "(function() local dlg = require('world.dialogue'); \
              dlg.close(true); \
              if not dlg.start('{dialogue_id}') then return 'start_failed' end; \
              local rows = {{}}; \
              for _, probe in ipairs({{ {ids_lua} }}) do \
                local id, kind = probe[1], probe[2]; \
                local line = '{dialogue_id}' .. '\\t' .. id .. '\\t' .. kind; \
                if kind == 'text' then \
                  local t = ui.text_get(id); \
                  if not t then line = line .. '\\tmissing' \
                  elseif t.visible == false then line = line .. '\\thidden' \
                  else \
                    local text = tostring(t.text):gsub('\\n', '\\\\n'); \
                    line = line .. string.format('\\t%.2f,%.2f,%g,%s', t.x, t.y, t.size, text) \
                  end \
                elseif kind == 'image' then \
                  local im = ui.image_get(id); \
                  if not im then line = line .. '\\tmissing' \
                  elseif im.visible == false then line = line .. '\\thidden' \
                  else \
                    line = line .. string.format('\\t%.2f,%.2f,%.2f,%.2f,%s', \
                      im.x, im.y, im.width, im.height, tostring(im.texture)) \
                  end \
                else \
                  local pa = ui.panel_get(id); \
                  if not pa then line = line .. '\\tmissing' \
                  elseif pa.visible == false then line = line .. '\\thidden' \
                  else \
                    line = line .. string.format('\\t%.2f,%.2f,%.2f,%.2f,%.2f', \
                      pa.x, pa.y, pa.width, pa.height, pa.a) \
                  end \
                end; \
                rows[#rows + 1] = line \
              end; \
              dlg.close(true); \
              return table.concat(rows, '\\n') end)()"
        );
        let dump = query_str(runtime, &block)?;
        if dump == "start_failed" {
            return Err(format!("对话 {dialogue_id} 开场失败（黄金表的驱动前提不成立）").into());
        }
        out.push_str(&dump);
        out.push('\n');
    }
    Ok(out)
}

/// 逐行与黄金表比较（第一处差异就报错，并把两边那一行都打出来）。
fn compare_golden(dump: &str) -> Result<(), Box<dyn std::error::Error>> {
    let golden: Vec<&str> = GOLDEN.lines().filter(|l| !l.is_empty()).collect();
    let fresh: Vec<&str> = dump.lines().filter(|l| !l.is_empty()).collect();
    if golden.len() != fresh.len() {
        return Err(format!(
            "黄金表 {} 行，实得 {} 行（id 集合或对话清单变了）",
            golden.len(),
            fresh.len()
        )
        .into());
    }
    for (index, (want, got)) in golden.iter().zip(fresh.iter()).enumerate() {
        if want != got {
            return Err(format!(
                "黄金对账第 {} 行不一致：\n  期望 {want}\n  实得 {got}",
                index + 1
            )
            .into());
        }
    }
    Ok(())
}

/// Creates a bare engine runtime pointing at the project scripts, audio assets and save file.
fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    runtime.set_audio_asset_dir(project.join("assets").join("audio"));
    // 动态 UI 是屏幕像素空间，视口与设计分辨率一致（与 jhyt_m19_ui 同口径）。
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
}

fn enter_playable(
    runtime: &mut LuaRuntime,
    main_script: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_project_script(main_script)?;
    runtime.execute_console("scene.load('main')")?;
    runtime.process_scene_transition()?;
    lua(runtime, "scene.load('manor')")?;
    runtime.process_scene_transition()?;
    let mode = query_str(runtime, "return require('core.state').mode")?;
    if mode != "Playing" {
        return Err(format!("进入玩法态失败: {mode}").into());
    }
    // 天气钉成晴天：HUD 的天气图标与对话无关，但读数要可跨树比较（M29 起天气按抽签）。
    lua(runtime, "require('world.weather').set('sunny')")?;
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
