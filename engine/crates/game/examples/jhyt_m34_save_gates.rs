//! M34-00 存档门的负向用例：让"版本门"和"整批回滚"两条口径真的会红。
//!
//! `core/save_manager.lua` 早就实现了失败收集 + 回滚点，`jhyt_m19_soak` 也跑过
//! 30 天长跑与 100 次磁盘往返——但那些门只断言"成功路径没炸"，没有任何一条用例
//! **注入失败**。因此版本门（本批 `building/crafting/gather` 新补）与回滚分支
//! 都是"写了但没人踩"的代码。本用例专踩它们：
//!
//! - A map.state 版本门（三家 provider 各一负一正）：版本不认识的 map.state
//!   必须**整块不读入**，把版本改回当前值又必须照常读入——证明红的是版本，
//!   不是别的偶然因素。
//! - B 快照版本门：把 capture 出来的快照里 `building.version`（注册名是单数）改成 99 →
//!   `save_manager.restore` 返回 false 且 reason 带 `bad_version`；内存里
//!   的建筑一座不少；磁盘存档文件字节不变（拒绝读档不得顺手重写存档）。
//! - C 半恢复负向：注册一个会 `error` 的 provider → restore 失败后，排在它
//!   前面已经写进内存的 `state.money` 必须回到 restore 前的值（不留半恢复）；
//!   同一个快照去掉 error 后必须整批恢复成功。
//! - D 读档循环逐字段相等：100 次 `save()` → `load()`，每次的规范化快照
//!   （递归排序 key 的字符串）与第 1 次**完全一致**——soak 的 C2 只比了
//!   `day>=30` 与地图名，这里比的是全部字段。
//!
//! Usage:
//!   cargo run -q -p game --example jhyt_m34_save_gates [-- <game_jhyt-root>]
//!
//! 这条门确实会红（2026-10-03 变异实测）：把 `systems/gather.lua` 的 `version_ok` 首行
//! 临时改成 `if true then return true end`（即取消版本判定），A 章立刻
//! `Error: "#(require('systems.gather').gathered_ids()) = 1, want 0"`；还原后全绿。

use rustlua_infra::games_dir;
use rustlua_scripting::script::LuaRuntime;
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Instant,
};

/// building/crafting/gather 三家的当前状态版本（与 Lua 侧 STATE_VERSION 一致）。
const STATE_VERSION: f64 = 1.0;
/// 故意不认识的版本号：高于任何已发布 schema。
const FUTURE_VERSION: f64 = 99.0;
/// D 章的磁盘往返次数。
const DISK_CYCLES: u32 = 100;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| games_dir().join("examples").join("game_jhyt"));
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let main_script = project.join("scripts/main.lua");
    if !main_script.is_file() {
        return Err(format!("missing {}", main_script.display()).into());
    }
    let save_path = fresh_save_path(&project, "jhyt_m34_save_gates.sav");

    let mut runtime = new_runtime(&project, &save_path)?;
    bootstrap_manor(&mut runtime, &main_script)?;
    let anchor = legal_anchor(&mut runtime, "herb_hut")?;

    banner("M34-00a map.state 版本门：building / crafting / gather 各一负一正");
    gate_building(&mut runtime, &anchor)?;
    gate_crafting(&mut runtime, &anchor)?;
    gate_gather(&mut runtime)?;

    banner("M34-00b 快照版本门：拒绝读档、内存不动、磁盘文件字节不变");
    let building_before = query_num(&mut runtime, "#(require('systems.building').list())")?;
    assert!(
        building_before > 0.0,
        "chapter B needs at least one placed building"
    );
    assert_str(&mut runtime, "require('core.save_manager').save()", "true")?;
    let file_before = fs::read(&save_path)?;
    lua(
        &mut runtime,
        &format!(
            "__gate_snap = require('core.save_manager').capture(); \
             __gate_snap.providers.building.version = {}; \
             __gate_snap.building.version = {}",
            FUTURE_VERSION, FUTURE_VERSION
        ),
    )?;
    let reason = query_str(
        &mut runtime,
        "return tostring(select(2, require('core.save_manager').restore(__gate_snap)))",
    )?;
    if !reason.contains("bad_version") {
        return Err(format!("snapshot version gate did not report bad_version: {reason}").into());
    }
    assert_num(
        &mut runtime,
        "#(require('systems.building').list())",
        building_before,
    )?;
    let file_after = fs::read(&save_path)?;
    if file_before != file_after {
        return Err("rejected restore rewrote the save file on disk".into());
    }
    println!(
        "snapshot gate: rejected, {}-byte save untouched, buildings still {}",
        file_before.len(),
        building_before as i64
    );

    banner("M34-00c 半恢复负向：provider 中途 error 必须整批回滚");
    probe_rollback(&mut runtime)?;

    banner("M34-00d 读档循环：100 次磁盘往返的规范化快照逐字段相等");
    let dump_bytes = disk_cycle_drift(&mut runtime)?;

    println!(
        "jhyt_m34_save_gates_ok project={} map_state_gates=3 snapshot_reject=true rollback=true cycles={} dump_bytes={}",
        project.display(),
        DISK_CYCLES,
        dump_bytes
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- 章节 ----------

/// building：真实放置一座药庐 → 把 map.state 的版本号改成不认识的（list 原样保留）
/// → 重进图后内存建筑必须清零；版本改回当前值 → 同一份 list 必须照常读回。
fn gate_building(runtime: &mut LuaRuntime, anchor: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (ax, ay) = anchor.split_once(',').ok_or("bad anchor")?;
    lua(
        runtime,
        &format!(
            "require('systems.building').place('herb_hut', {}, {})",
            ax, ay
        ),
    )?;
    assert_num(runtime, "#(require('systems.building').list())", 1.0)?;
    assert_num(runtime, "map.state('buildings').version", STATE_VERSION)?;
    lua(
        runtime,
        &format!(
            "(function() local s = map.state('buildings'); s.version = {}; \
             map.state('buildings', s); require('systems.building').on_enter() end)()",
            FUTURE_VERSION
        ),
    )?;
    assert_num(runtime, "#(require('systems.building').list())", 0.0)?;
    // 反向：同一份 list、版本改回当前值 → 必须读回那 1 座（红只可能来自版本判定）。
    lua(
        runtime,
        &format!(
            "(function() local s = map.state('buildings'); s.version = {}; \
             map.state('buildings', s); require('systems.building').on_enter() end)()",
            STATE_VERSION
        ),
    )?;
    assert_num(runtime, "#(require('systems.building').list())", 1.0)?;
    println!("building gate: version=99 => 0 records, version=1 => same list reads back");
    Ok(())
}

/// crafting：往 map.state 塞一条进行中的 job，版本不认识 → `has_job` 必须为假；
/// 版本对 → 同一条 job 必须读回。
fn gate_crafting(runtime: &mut LuaRuntime, anchor: &str) -> Result<(), Box<dyn std::error::Error>> {
    let job = format!(
        "['{anchor}'] = {{ recipe = 'pill_refine', building = 'herb_hut', start_abs = 0, \
         done_abs = 999, output_item = 'heal_pill', output_count = 1 }}"
    );
    for (version, expect) in [(FUTURE_VERSION, "false"), (STATE_VERSION, "true")] {
        lua(
            runtime,
            &format!(
                "map.state('crafting', {{ version = {}, jobs = {{ {} }} }}); \
                 local c = require('systems.crafting'); \
                 c.on_enter('__m34_probe_map__'); c.on_enter('manor')",
                version, job
            ),
        )?;
        let probe = query_str(
            runtime,
            "(function() local s = map.state('crafting'); \
               return tostring(s and s.version) .. '|' .. tostring(type(s and s.jobs)) .. '|' .. \
                 tostring(s and s.jobs and next(s.jobs) ~= nil) end)()",
        )?;
        println!("crafting probe: map.state version/type/non-empty = {probe}");
        assert_str(
            runtime,
            &format!("tostring(require('systems.crafting').has_job('{anchor}'))"),
            expect,
        )?;
    }

    lua(runtime, "require('systems.crafting').reset()")?;
    println!("crafting gate: bad version drops the job table, current version reads it back");
    Ok(())
}

/// gather：`reset()` 先把 loaded_map 放开，再写一份带合成节点 id 的 map.state；
/// 版本不认识 → 已采列表为空；版本对 → 同一条记录读回。
fn gate_gather(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    for (version, expect) in [(FUTURE_VERSION, 0.0), (STATE_VERSION, 1.0)] {
        lua(
            runtime,
            &format!(
                "require('systems.gather').reset(); \
                 map.state('gather', {{ state_version = {}, day = 1, \
                   nodes = {{ ['m34_gate_node'] = {{ day = 0 }} }} }}); \
                 require('systems.gather').ensure_loaded()",
                version
            ),
        )?;
        assert_num(
            runtime,
            "#(require('systems.gather').gathered_ids())",
            expect,
        )?;
    }
    assert_str(
        runtime,
        "table.concat(require('systems.gather').gathered_ids(), ',')",
        "m34_gate_node",
    )?;
    lua(runtime, "require('systems.gather').reset()")?;
    println!("gather gate: state_version gate holds on both directions");
    Ok(())
}

/// 注册一个会中途 `error` 的 provider，验证 `save_manager.restore` 的整批回滚。
fn probe_rollback(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
    lua(
        runtime,
        "__m34_probe = { value = 1, fail = false }; \
         require('core.save_manager').register('m34_probe', \
           function() return { value = __m34_probe.value } end, \
           function(data) \
             if __m34_probe.fail then error('m34 probe restore blew up') end \
             __m34_probe.value = tonumber(data.value) or 0 \
           end)",
    )?;
    lua(
        runtime,
        "__m34_snap = require('core.save_manager').capture()",
    )?;
    let snap_money = query_num(runtime, "return __m34_snap.providers.state.money")?;
    // 把内存改脏：money=777、probe=2，并让 probe 的 restore 抛错。
    lua(
        runtime,
        "require('core.state').money = 777; __m34_probe.value = 2; __m34_probe.fail = true",
    )?;
    let reason = query_str(
        runtime,
        "return tostring(select(2, require('core.save_manager').restore(__m34_snap)))",
    )?;
    if !reason.contains("m34 probe restore blew up") {
        return Err(format!("failing provider was not reported: {reason}").into());
    }
    // state provider 排在 probe 之前，已经写进内存的钱必须被回滚回 777。
    assert_num(runtime, "require('core.state').money", 777.0)?;
    assert_num(runtime, "__m34_probe.value", 2.0)?;
    // 去掉 error：同一份快照必须整批恢复成功（上一步的红只来自失败分支）。
    lua(runtime, "__m34_probe.fail = false")?;
    assert_str(
        runtime,
        "tostring(require('core.save_manager').restore(__m34_snap))",
        "true",
    )?;
    assert_num(runtime, "require('core.state').money", snap_money)?;
    assert_num(runtime, "__m34_probe.value", 1.0)?;
    println!("rollback: failed restore kept money=777/probe=2, retry without error restored both");
    Ok(())
}

/// 100 次 `save()` → `load()` 磁盘往返；每次把 capture 的快照规范化成排序字符串，
/// 在 Lua 侧与第 1 次逐字符比较（不把百份大字符串搬回 Rust）。
fn disk_cycle_drift(runtime: &mut LuaRuntime) -> Result<usize, Box<dyn std::error::Error>> {
    lua(
        runtime,
        "__m34_dump = function(value) \
           if type(value) ~= 'table' then return tostring(value) end; \
           local names = {}; local by_name = {}; \
           for k, v in pairs(value) do local nk = tostring(k); \
             names[#names + 1] = nk; by_name[nk] = v end; \
           table.sort(names); \
           local parts = {}; \
           for i = 1, #names do parts[i] = names[i] .. '=' .. __m34_dump(by_name[names[i]]) end; \
           return '{' .. table.concat(parts, ',') .. '}' \
         end; \
         __m34_base = nil; __m34_drift = 0; \
         __m34_first_diff = ''",
    )?;
    let started = Instant::now();
    for _ in 1..=DISK_CYCLES {
        assert_str(runtime, "require('core.save_manager').save()", "true")?;
        assert_str(runtime, "require('core.save_manager').load()", "true")?;
        lua(
            runtime,
            "(function() local d = __m34_dump(require('core.save_manager').capture()); \
               if __m34_base == nil then __m34_base = d; return end; \
               if d ~= __m34_base then __m34_drift = __m34_drift + 1; \
                 if __m34_first_diff == '' then \
                   local i = 1; \
                   while d:sub(i, i) == __m34_base:sub(i, i) and i <= #d do i = i + 1 end; \
                   __m34_first_diff = 'offset ' .. i .. ' base=' .. __m34_base:sub(i, i + 60) .. \
                     ' now=' .. d:sub(i, i + 60); \
                 end \
               end end)()",
        )?;
    }
    let drift = query_num(runtime, "return __m34_drift")?;
    if drift > 0.0 {
        let detail = query_str(runtime, "return __m34_first_diff")?;
        return Err(format!(
            "{drift} of {DISK_CYCLES} save/load cycles drifted from cycle 1: {detail}"
        )
        .into());
    }
    let bytes = query_num(runtime, "return #__m34_base")? as usize;
    println!(
        "disk cycles: {DISK_CYCLES} identical snapshots ({bytes} bytes each, {:.1}s)",
        started.elapsed().as_secs_f64()
    );
    Ok(bytes)
}

// ---------- helpers ----------

fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry())?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
}

/// 从标题进入山庄并完成 boot（新档）——与 jhyt_m19_soak 的真实路径一致。
fn bootstrap_manor(
    runtime: &mut LuaRuntime,
    main_script: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_project_script(main_script)?;
    runtime.execute_console("scene.load('main')")?;
    runtime.process_scene_transition()?;
    runtime.execute_console("on_click('BtnStart')")?;
    runtime.process_scene_transition()?;
    let scene = query_str(runtime, "scene.reload()")?;
    if scene != "manor" {
        return Err(format!("scene = {scene:?}, want manor").into());
    }
    Ok(())
}

/// 山庄上一个可放置锚点（对场景几何不敏感的确定性扫描）。
fn legal_anchor(
    runtime: &mut LuaRuntime,
    building_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let anchor = query_str(
        runtime,
        &format!(
            "(function() local b = require('systems.building') \
               for y = 0, 21 do for x = 0, 39 do \
                 if b.can_place('{}', x, y) then return x .. ',' .. y end end end \
               return '' end)()",
            building_id
        ),
    )?;
    if anchor.is_empty() || !anchor.contains(',') {
        return Err(format!("no legal {building_id} anchor on manor: {anchor:?}").into());
    }
    println!("legal anchor: {anchor}");
    Ok(anchor)
}

fn lua(runtime: &mut LuaRuntime, code: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(code)?;
    Ok(())
}

fn query_str(runtime: &mut LuaRuntime, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(runtime.execute_console(expr)?.trim().to_owned())
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
    let got = query_num(runtime, expr)?;
    if (got - expected).abs() > 1e-6 {
        return Err(format!("{expr} = {got}, want {expected}").into());
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
