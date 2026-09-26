//! M19-03 领域数据面板：把 `examples/game_jhyt/scripts/**` 的 Lua 领域数据接进
//! 编辑器 —— 统一校验入口调用、只读视图（地图出口 / 地点 / NPC / 商店 / 任务 /
//! 对话索引）与 PersistId 注册表。
//!
//! 设计约束（任务书 + M20-01）：
//! - **校验规则只有一个事实来源**：工程提供 `scripts/validate_data.lua`（M20-01）
//!   时，面板执行该入口并调用 `check({ strict = true })`，结果与游戏启动校验、
//!   `game --check` / `editor --check` 完全一致；编辑器**不复刻**任何 NPC/任务/
//!   商店引用规则。没有该入口的老工程回退为「发现 `scripts/data/**` +
//!   定义了 `validate_all` 的 `scripts/*.lua` → 逐个校验」。
//! - 问题文本**原样保留** Lua 校验器给出的文件/对象/字段路径，只额外做模块归属。
//! - 每次校验都使用**新建的 Lua VM**：`require` 有模块缓存，复用 VM 会读到上一次
//!   的文件内容，导致「数据文件已改但校验结果不变」。
//! - 只加载数据模块：以 `validate_data.lua` 自身作为工程脚本执行（建立 `scripts/`
//!   的 require 搜索路径），**不执行 `main.lua`**（游戏入口有副作用）。
//!
//! 纯逻辑（分组、表达式构造、结果解析、PersistId 聚合、源文件定位）与 egui 绘制
//! 分离；测试用临时工程夹具驱动完整链路（LuaRuntime 不需要 GPU）。

use crate::EditorApp;
use eframe::egui;
use engine::core::scene::Scene;
use engine::core::script::LuaRuntime;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// 一个待校验的 Lua 领域模块。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DomainModule {
    /// `require` 名（`data.maps`、`quest`）。
    pub module: String,
    /// 工程相对路径（`scripts/data/maps.lua`）。
    pub relative: String,
}

/// 单模块校验状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DomainModuleStatus {
    /// `validate_all()` 返回 `ok`（或无问题的空列表）。
    Ok,
    /// `validate_all()` 返回 `false`：逐条问题文本（原样保留文件/对象/字段路径）。
    Issues(Vec<String>),
    /// 模块没有 `validate_all` 入口（数据文件/纯数据表），跳过。
    Skipped,
    /// `require` 失败或校验器抛错：保留原始 Lua 消息。
    Error(String),
}

impl DomainModuleStatus {
    pub(crate) fn is_problem(&self) -> bool {
        matches!(self, Self::Issues(_) | Self::Error(_))
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Ok => "通过",
            Self::Issues(_) => "有问题",
            Self::Skipped => "无校验器",
            Self::Error(_) => "加载/校验失败",
        }
    }
}

/// 单模块校验记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DomainModuleReport {
    pub module: String,
    pub relative: String,
    pub status: DomainModuleStatus,
}

impl DomainModuleReport {
    /// 该模块的问题条目（供面板逐条显示）。
    pub(crate) fn issue_lines(&self) -> Vec<String> {
        match &self.status {
            DomainModuleStatus::Issues(issues) => issues.clone(),
            DomainModuleStatus::Error(message) => vec![message.clone()],
            DomainModuleStatus::Ok | DomainModuleStatus::Skipped => Vec::new(),
        }
    }
}

/// 域校验结果来源。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DomainValidationSource {
    /// 工程统一校验入口 `scripts/validate_data.lua`（M20-01）：与游戏启动
    /// （`main.lua`）、`game --check`、`editor --check` 共用同一事实来源。
    #[default]
    UnifiedEntry,
    /// 回退：工程未提供统一入口时，逐模块 `require` + `validate_all()`。
    PerModule,
}

impl DomainValidationSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::UnifiedEntry => {
                "统一校验入口 scripts/validate_data.lua（与 game/editor --check 同一入口）"
            }
            Self::PerModule => "逐模块 validate_all（工程未提供 scripts/validate_data.lua）",
        }
    }
}

/// 一次域校验的完整结果（含只读视图）。
#[derive(Clone, Debug, Default)]
pub(crate) struct DomainDataReport {
    pub project_root: PathBuf,
    pub source: DomainValidationSource,
    /// 校验无法进行（Lua VM/入口加载/入口执行失败）：面板显示该错误并停止。
    pub bootstrap_error: Option<String>,
    /// 问题模块（统一入口只列出有问题的模块；回退模式含通过/跳过的模块）。
    pub modules: Vec<DomainModuleReport>,
    /// 只读视图求值失败。
    pub view_error: Option<String>,
    pub view: DomainView,
}

impl DomainDataReport {
    pub(crate) fn problem_modules(&self) -> usize {
        self.modules
            .iter()
            .filter(|module| module.status.is_problem())
            .count()
    }

    pub(crate) fn issue_count(&self) -> usize {
        self.modules
            .iter()
            .map(|module| module.issue_lines().len())
            .sum()
    }
}

// ---------------------------------------------------------------------------
// 模块发现
// ---------------------------------------------------------------------------

/// 发现待校验模块：
/// 1. `<工程>/scripts/data/**/*.lua`（文件名 → `data.<name>`，目录用 `.` 连接）；
/// 2. `<工程>/scripts/**/*.lua`（`data/` 除外）中**定义了 `validate_all`** 的运行时
///    模块（`schedule`/`quest`/`dialogue`/`shop`/`skills` 等 —— NPC 日程、任务、对话与
///    商店的引用校验器都在这里；`main.lua` 不含该入口，因此不会被加载执行）。
///    运行时模块可放在子目录（`scripts/systems/shop.lua` → `systems.shop`）。
///
/// 结果按模块名排序，保证面板与测试输出稳定。
pub(crate) fn discover_domain_modules(root: &Path) -> Vec<DomainModule> {
    let mut modules = Vec::new();
    let scripts = root.join("scripts");
    collect_data_modules(&scripts.join("data"), &scripts, &mut modules);
    collect_runtime_modules(&scripts, &mut modules);
    modules.sort_by(|a, b| a.module.cmp(&b.module));
    modules.dedup_by(|a, b| a.module == b.module);
    modules
}

/// 递归扫描 `scripts/data/**`，把相对 `scripts/` 的路径映射为 `require` 模块名
/// （`scripts/data/maps.lua` → `data.maps`，`scripts/data/quests/index.lua` →
/// `data.quests.index`）。
fn collect_data_modules(dir: &Path, scripts: &Path, modules: &mut Vec<DomainModule>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_data_modules(&path, scripts, modules);
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("lua") {
            continue;
        }
        let Ok(relative) = path.strip_prefix(scripts) else {
            continue;
        };
        let stem = relative.with_extension("");
        let name = stem
            .components()
            .map(|component| component.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(".");
        modules.push(DomainModule {
            module: name,
            relative: format!("scripts/{}", relative.to_string_lossy().replace('\\', "/")),
        });
    }
}

/// 扫描 `scripts/**/*.lua`（跳过 `data/`，那部分由 [`collect_data_modules`] 负责）：
/// 只有源码中出现 `validate_all` 的运行时模块才会被加载校验。
///
/// 递归是必要的：工程可以把运行时模块按职责放进子目录（`scripts/systems/shop.lua`
/// → `require("systems.shop")`），扁平扫描会让这些模块在回退路径里凭空消失。
/// 目录用 `.` 连接（与 `package.path` 的 `?` 展开一致）。
fn collect_runtime_modules(scripts: &Path, modules: &mut Vec<DomainModule>) {
    let data_dir = scripts.join("data");
    collect_runtime_modules_in(scripts, scripts, &data_dir, modules);
}

fn collect_runtime_modules_in(
    dir: &Path,
    scripts: &Path,
    data_dir: &Path,
    modules: &mut Vec<DomainModule>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        // `data/` 由 [`collect_data_modules`] 负责；子目录递归进去继续找运行时模块。
        if !path.is_dir() {
            if path.extension().and_then(|value| value.to_str()) != Some("lua") {
                continue;
            }
            let Ok(source) = fs::read_to_string(&path) else {
                continue;
            };
            if !source.contains("validate_all") {
                continue;
            }
            let Ok(relative) = path.strip_prefix(scripts) else {
                continue;
            };
            let stem = relative.with_extension("");
            let name = stem
                .components()
                .map(|component| component.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(".");
            modules.push(DomainModule {
                module: name,
                relative: format!("scripts/{}", relative.to_string_lossy().replace('\\', "/")),
            });
            continue;
        }
        if path != data_dir {
            collect_runtime_modules_in(&path, scripts, data_dir, modules);
        }
    }
}

// ---------------------------------------------------------------------------
// Lua 表达式
// ---------------------------------------------------------------------------

/// 引导表达式：把工程 `scripts/` 加入 `package.path`（只影响本 VM 的 require）。
///
/// 注意：引擎的 `execute_project_script` 会额外安装「压缩/加密模块」searcher，
/// 编辑器域校验刻意只支持明文 Lua（见任务文档「已知限制」）。
pub(crate) fn bootstrap_source(root: &Path) -> String {
    let scripts = root.join("scripts").to_string_lossy().replace('\\', "/");
    format!(
        "package.path = \"{}/?.lua;{}/?/init.lua;\" .. package.path",
        escape_lua_string(&scripts),
        escape_lua_string(&scripts)
    )
}

/// 单模块校验表达式：返回 `{ ok, has_validator, issues }`。
///
/// - `require` 抛错 → `{ loaded = false, error = <原始消息> }`；
/// - 没有 `validate_all` → `{ has_validator = false }`（面板标为「无校验器」）；
/// - `validate_all` 抛错 → `{ error = <原始消息>, issues = { <原始消息> } }`；
/// - 否则 `ok` 为校验器返回的布尔，`issues` 为其返回的问题列表（逐条 `tostring`）。
pub(crate) fn validate_expression(module: &str) -> String {
    let name = escape_lua_string(module);
    format!(
        r#"(function()
  local loaded, module = pcall(require, "{name}")
  if not loaded then
    return {{ loaded = false, error = tostring(module) }}
  end
  if type(module) ~= "table" or type(module.validate_all) ~= "function" then
    return {{ loaded = true, has_validator = false }}
  end
  local called, ok, issues = pcall(module.validate_all)
  if not called then
    local message = tostring(ok)
    return {{ loaded = true, has_validator = true, ok = false, error = message, issues = {{ message }} }}
  end
  local list = {{}}
  if type(issues) == "table" then
    for _, item in ipairs(issues) do
      list[#list + 1] = tostring(item)
    end
  end
  return {{ loaded = true, has_validator = true, ok = ok and true or false, issues = list }}
end)()"#
    )
}

/// 只读视图表达式：一次性取出 maps/places/npcs/shops/任务/对话索引（全部 pcall）。
pub(crate) const DOMAIN_VIEW_EXPRESSION: &str = r#"(function()
  local function safe(name)
    local ok, module = pcall(require, name)
    if not ok or type(module) ~= "table" then
      return nil
    end
    return module
  end

  local view = {}

  local maps = safe("data.maps")
  if maps and type(maps.maps) == "table" then
    local out = {}
    for id, def in pairs(maps.maps) do      if type(def) == "table" then
        local exits = {}
        for _, exit in ipairs(def.exits or {}) do
          if type(exit) == "table" then
            exits[#exits + 1] = {
              id = exit.id,
              cell = exit.cell,
              to = exit.to,
              to_cell = exit.to_cell,
              label = exit.label
            }
          end
        end
        out[#out + 1] = { id = id, display = def.display, spawn = def.spawn, exits = exits }
      end
    end
    table.sort(out, function(a, b) return tostring(a.id) < tostring(b.id) end)
    view.maps = out
    if type(maps.TILE) == "number" then
      view.tile = maps.TILE
    end
  end

  local places = safe("data.places")
  if places and type(places.places) == "table" then
    local out = {}
    for name, place in pairs(places.places) do
      if type(place) == "table" then
        out[#out + 1] = { name = name, map = place.map, cell = place.cell, label = place.label }
      end
    end
    table.sort(out, function(a, b) return tostring(a.name) < tostring(b.name) end)
    view.places = out
  end

  local npcs = safe("data.npcs")
  if npcs and type(npcs.npcs) == "table" then
    local out = {}
    for _, npc in ipairs(npcs.npcs) do
      if type(npc) == "table" then
        out[#out + 1] = {
          name = npc.name,
          persist_id = npc.persist_id,
          character = npc.character,
          home = npc.home,
          schedule = npc.schedule
        }
      end
    end
    view.npcs = out
  end

  local shops = safe("data.shops")
  if shops then
    local out = {}
    for _, def in pairs(shops) do
      if type(def) == "table" and type(def.id) == "string" then
        local stock = {}
        for _, entry in ipairs(def.stock or {}) do
          if type(entry) == "table" then
            stock[#stock + 1] = { item = entry.item, price = entry.price, limit = entry.limit }
          end
        end
        local buys = {}
        for _, item in ipairs(def.buys or {}) do
          buys[#buys + 1] = item
        end
        out[#out + 1] = {
          id = def.id,
          name = def.name,
          keeper = def.keeper,
          place = def.place,
          hours = def.hours,
          stock = stock,
          buys = buys
        }
      end
    end
    table.sort(out, function(a, b) return a.id < b.id end)
    view.shops = out
  end

  local quests = safe("data.quests.index")
  if quests and type(quests.ids) == "table" then
    view.quests = quests.ids
  end

  local dialogues = safe("data.dialogues.index")
  if dialogues and type(dialogues.ids) == "table" then
    view.dialogues = dialogues.ids
  end

  return view
end)()"#;

/// Lua 短字符串转义（模块名/路径可能含 `"` 或 `\`）。
fn escape_lua_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

// ---------------------------------------------------------------------------
// 只读视图（解析后的结构）
// ---------------------------------------------------------------------------

/// 领域数据里的一个格坐标（0-index、y 朝上；与 tilemap/farm 同约定）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DomainCell {
    pub x: i64,
    pub y: i64,
}

impl DomainCell {
    pub(crate) fn label(self) -> String {
        format!("({}, {})", self.x, self.y)
    }
}

/// `data.maps` 的一个出口。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExitDef {
    pub id: String,
    pub cell: Option<DomainCell>,
    pub to: String,
    pub to_cell: Option<DomainCell>,
    pub label: String,
}

/// `data.maps` 的一张地图。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MapDef {
    pub id: String,
    pub display: String,
    pub spawn: Option<DomainCell>,
    pub exits: Vec<ExitDef>,
}

/// `data.places` 的一个地点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlaceDef {
    pub name: String,
    pub map: String,
    pub cell: Option<DomainCell>,
    pub label: String,
}

/// `data.npcs` 的一条 NPC 记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NpcDef {
    pub name: String,
    pub persist_id: String,
    pub character: String,
    pub home: String,
    pub schedule: String,
}

/// `data.shops` 的一条上架记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShopStock {
    pub item: String,
    pub price: Option<i64>,
    pub limit: Option<i64>,
}

/// `data.shops` 的一间商店。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShopDef {
    pub id: String,
    pub name: String,
    pub keeper: String,
    pub place: String,
    pub hours: Option<(i64, i64)>,
    pub stock: Vec<ShopStock>,
    pub buys: Vec<String>,
}

/// 域数据只读视图（面板与 Scene 视图覆盖层共用）。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DomainView {
    pub maps: Vec<MapDef>,
    pub places: Vec<PlaceDef>,
    pub npcs: Vec<NpcDef>,
    pub shops: Vec<ShopDef>,
    pub quests: Vec<String>,
    pub dialogues: Vec<String>,
    /// `data.maps.TILE`：cell → 世界坐标换算用的格子边长（缺失则不画覆盖层）。
    pub tile: Option<f64>,
}

impl DomainView {
    pub(crate) fn is_empty(&self) -> bool {
        self.maps.is_empty() && self.places.is_empty() && self.npcs.is_empty()
    }

    pub(crate) fn map(&self, id: &str) -> Option<&MapDef> {
        self.maps.iter().find(|map| map.id == id)
    }

    pub(crate) fn places_in(&self, map_id: &str) -> Vec<&PlaceDef> {
        self.places
            .iter()
            .filter(|place| place.map == map_id)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// 收集（纯逻辑 + Lua 求值）
// ---------------------------------------------------------------------------

/// 工程统一校验入口（M20-01，相对工程根）。
pub(crate) const UNIFIED_ENTRY_RELATIVE: &str = "scripts/validate_data.lua";

/// 统一入口的校验调用：`check({ strict = true })` 返回 `""` 或换行分隔的问题列表。
pub(crate) const UNIFIED_CHECK_EXPRESSION: &str =
    "return require('validate_data').check({ strict = true })";

/// 收集一次完整域报告。
///
/// 主路径（M20-01）：工程提供 `scripts/validate_data.lua` 时，以该文件作为工程脚本
/// 执行（建立 `scripts/` 的 require 搜索路径，且**不执行 `main.lua`**），再调用
/// `require('validate_data').check({ strict = true })` —— 与游戏启动校验、`game --check`
/// / `editor --check` 用的是同一个事实来源，编辑器不再自带一套模块发现规则。
///
/// 回退路径：工程没有该入口时，回退为「发现 `scripts/data/**` 与定义了 `validate_all`
/// 的 `scripts/*.lua` 模块 → 逐个校验」，保证老工程/新工程骨架仍可用。
///
/// 两条路径都使用**新建的 Lua VM**：`require` 有模块缓存，复用 VM 会读到旧文件内容。
pub(crate) fn collect_domain_report(root: &Path) -> DomainDataReport {
    let report = DomainDataReport {
        project_root: root.to_path_buf(),
        ..DomainDataReport::default()
    };
    let entry = root.join(UNIFIED_ENTRY_RELATIVE);
    if entry.is_file() {
        return collect_via_unified_entry(root, &entry, report);
    }
    collect_via_per_module_scan(root, report)
}

/// 主路径：执行工程统一校验入口。
fn collect_via_unified_entry(
    root: &Path,
    entry: &Path,
    mut report: DomainDataReport,
) -> DomainDataReport {
    report.source = DomainValidationSource::UnifiedEntry;
    let mut runtime = match LuaRuntime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            report.bootstrap_error = Some(format!("Lua VM 初始化失败：{error}"));
            return report;
        }
    };
    runtime.set_project_root(root.to_path_buf());
    if let Err(error) = runtime.execute_project_script(entry) {
        report.bootstrap_error = Some(format!(
            "统一校验入口加载失败（{}）：{error}",
            entry.display()
        ));
        return report;
    }
    match runtime.execute_console(UNIFIED_CHECK_EXPRESSION) {
        Ok(text) => report.modules = group_unified_issues(&text),
        Err(error) => {
            report.bootstrap_error = Some(format!("域数据校验执行失败：{error}"));
        }
    }
    match runtime.eval_json(DOMAIN_VIEW_EXPRESSION) {
        Ok(value) => report.view = parse_view(&value),
        Err(error) => report.view_error = Some(error.to_string()),
    }
    report
}

/// 回退路径：逐模块 require + `validate_all()`（工程未提供统一入口）。
fn collect_via_per_module_scan(root: &Path, mut report: DomainDataReport) -> DomainDataReport {
    report.source = DomainValidationSource::PerModule;
    let mut runtime = match LuaRuntime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            report.bootstrap_error = Some(format!("Lua VM 初始化失败：{error}"));
            return report;
        }
    };
    if let Err(error) = runtime.execute_console(&bootstrap_source(root)) {
        report.bootstrap_error = Some(format!(
            "工程 Lua 搜索路径引导失败（工程根目录：{}）：{error}",
            root.display()
        ));
        return report;
    }
    for module in discover_domain_modules(root) {
        let status = match runtime.eval_json(&validate_expression(&module.module)) {
            Ok(value) => parse_validate_result(&value),
            Err(error) => DomainModuleStatus::Error(error.to_string()),
        };
        report.modules.push(DomainModuleReport {
            module: module.module,
            relative: module.relative,
            status,
        });
    }
    match runtime.eval_json(DOMAIN_VIEW_EXPRESSION) {
        Ok(value) => report.view = parse_view(&value),
        Err(error) => report.view_error = Some(error.to_string()),
    }
    report
}

/// 把统一入口的换行分隔问题列表按 `[模块]` 前缀分组为问题模块行。
///
/// 每条形如 `[maps] data/maps.lua:maps.x.spawn: out of map bounds`：模块归属取
/// 前缀，问题文本（含文件/对象/字段路径）**原样保留**；无前缀的行归入「未归类」。
pub(crate) fn group_unified_issues(text: &str) -> Vec<DomainModuleReport> {
    let mut grouped: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (module, issue) = split_module_prefix(line);
        grouped.entry(module).or_default().push(issue.to_owned());
    }
    grouped
        .into_iter()
        .map(|(module, issues)| {
            let relative = issue_source(&issues[0])
                .map(|source| {
                    if source.starts_with("scripts/") {
                        source
                    } else {
                        format!("scripts/{source}")
                    }
                })
                .unwrap_or_else(|| UNIFIED_ENTRY_RELATIVE.to_owned());
            DomainModuleReport {
                module,
                relative,
                status: DomainModuleStatus::Issues(issues),
            }
        })
        .collect()
}

/// 拆分 `[module] rest`：无前缀时模块名为「未归类」。
fn split_module_prefix(line: &str) -> (String, &str) {
    let Some(rest) = line.strip_prefix('[') else {
        return ("未归类".to_owned(), line);
    };
    let Some((module, tail)) = rest.split_once(']') else {
        return ("未归类".to_owned(), line);
    };
    (module.trim().to_owned(), tail.trim())
}

/// 从问题文本中提取首个 `.lua` 源文件路径（用于「定位」显示）。
pub(crate) fn issue_source(issue: &str) -> Option<String> {
    issue
        .split(|character: char| character.is_whitespace() || character == ':')
        .find(|token| !token.is_empty() && token.ends_with(".lua"))
        .map(str::to_owned)
}

/// 解析单模块校验表达式返回值。
fn parse_validate_result(value: &Value) -> DomainModuleStatus {
    if value.get("loaded").and_then(Value::as_bool) == Some(false) {
        return DomainModuleStatus::Error(
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("require failed")
                .to_owned(),
        );
    }
    if value.get("has_validator").and_then(Value::as_bool) != Some(true) {
        return DomainModuleStatus::Skipped;
    }
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return DomainModuleStatus::Error(error.to_owned());
    }
    let issues = string_list(value.get("issues"));
    if value.get("ok").and_then(Value::as_bool) == Some(true) && issues.is_empty() {
        DomainModuleStatus::Ok
    } else {
        DomainModuleStatus::Issues(issues)
    }
}

/// 解析只读视图。
pub(crate) fn parse_view(value: &Value) -> DomainView {
    let mut view = DomainView::default();
    for map in array(value.get("maps")) {
        let mut exits = Vec::new();
        for exit in array(map.get("exits")) {
            exits.push(ExitDef {
                id: string_field(exit, "id"),
                cell: cell_field(exit, "cell"),
                to: string_field(exit, "to"),
                to_cell: cell_field(exit, "to_cell"),
                label: string_field(exit, "label"),
            });
        }
        view.maps.push(MapDef {
            id: string_field(map, "id"),
            display: string_field(map, "display"),
            spawn: cell_field(map, "spawn"),
            exits,
        });
    }
    for place in array(value.get("places")) {
        view.places.push(PlaceDef {
            name: string_field(place, "name"),
            map: string_field(place, "map"),
            cell: cell_field(place, "cell"),
            label: string_field(place, "label"),
        });
    }
    for npc in array(value.get("npcs")) {
        view.npcs.push(NpcDef {
            name: string_field(npc, "name"),
            persist_id: string_field(npc, "persist_id"),
            character: string_field(npc, "character"),
            home: string_field(npc, "home"),
            schedule: string_field(npc, "schedule"),
        });
    }
    for shop in array(value.get("shops")) {
        let mut stock = Vec::new();
        for entry in array(shop.get("stock")) {
            stock.push(ShopStock {
                item: string_field(entry, "item"),
                price: entry.get("price").and_then(Value::as_i64),
                limit: entry.get("limit").and_then(Value::as_i64),
            });
        }
        let hours = shop.get("hours").and_then(|hours| {
            let open = hours.get("open").and_then(Value::as_i64)?;
            let close = hours.get("close").and_then(Value::as_i64)?;
            Some((open, close))
        });
        view.shops.push(ShopDef {
            id: string_field(shop, "id"),
            name: string_field(shop, "name"),
            keeper: string_field(shop, "keeper"),
            place: string_field(shop, "place"),
            hours,
            stock,
            buys: string_list(shop.get("buys")),
        });
    }
    view.quests = string_list(value.get("quests"));
    view.dialogues = string_list(value.get("dialogues"));
    view.tile = value.get("tile").and_then(Value::as_f64);
    view
}

fn array(value: Option<&Value>) -> impl Iterator<Item = &Value> {
    value
        .and_then(Value::as_array)
        .map(|values| values.iter())
        .into_iter()
        .flatten()
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn cell_field(value: &Value, key: &str) -> Option<DomainCell> {
    let cell = value.get(key)?;
    Some(DomainCell {
        x: cell.get("x").and_then(Value::as_i64)?,
        y: cell.get("y").and_then(Value::as_i64)?,
    })
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .map(|item| match item {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// PersistId 注册表
// ---------------------------------------------------------------------------

/// 一条 PersistId 定义来源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersistSource {
    pub id: String,
    /// 人类可读来源（`scenes/village.scene#JianghuPlayer`、`data/npcs.lua: 沈青萝`）。
    pub source: String,
}

/// 一条 PersistId 引用（如商店店主的 NPC persist_id）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersistReference {
    pub id: String,
    pub source: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PersistIssueKind {
    /// 同一 id 有多处定义。
    Duplicate,
    /// id 同时出现在场景 `entity_meta persist_id` 与 `data/npcs.persist_id`：
    /// 运行期 Lua 会用同一 id 注册实体，场景实体先占位会被引擎拒绝。
    NpcSceneConflict,
    /// 被引用（商店店主等）但没有任何定义。
    UndefinedReference,
}

impl PersistIssueKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Duplicate => "重复定义",
            Self::NpcSceneConflict => "场景/NPC 冲突",
            Self::UndefinedReference => "引用未定义",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersistIssue {
    pub kind: PersistIssueKind,
    pub id: String,
    pub detail: String,
}

/// PersistId 注册表视图：定义汇总 + 问题列表 + 备注。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PersistRegistry {
    pub entries: Vec<PersistSource>,
    pub issues: Vec<PersistIssue>,
    /// 非问题备注（如场景里声明但领域数据未提及的 id —— 可能是农田/建筑/容器）。
    pub notes: Vec<String>,
}

impl PersistRegistry {
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.issues.is_empty()
    }

    pub(crate) fn issue_count(&self) -> usize {
        self.issues.len()
    }
}

/// 扫描工程所有场景的 `entity_meta persist_id`（用场景文本解析器，兼容转义）。
/// 返回 (定义列表, 读取错误说明)。
pub(crate) fn scan_scene_persist_ids(root: &Path) -> (Vec<PersistSource>, Vec<String>) {
    let mut sources = Vec::new();
    let mut errors = Vec::new();
    let scenes = root.join("scenes");
    let Ok(entries) = fs::read_dir(&scenes) else {
        return (sources, errors);
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("scene"))
        .collect();
    paths.sort();
    for path in paths {
        let relative = path
            .strip_prefix(root)
            .map(|value| value.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().to_string());
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                errors.push(format!("{relative}: 读取失败：{error}"));
                continue;
            }
        };
        match Scene::from_text(&text) {
            Ok(scene) => {
                for entity in scene.entities.values() {
                    if let Some(id) = entity
                        .metadata
                        .get("persist_id")
                        .filter(|value| !value.trim().is_empty())
                    {
                        sources.push(PersistSource {
                            id: id.clone(),
                            source: format!("{relative}#{}", entity.name),
                        });
                    }
                }
            }
            Err(error) => errors.push(format!("{relative}: 解析失败：{error}")),
        }
    }
    sources.sort_by(|a, b| a.id.cmp(&b.id).then(a.source.cmp(&b.source)));
    (sources, errors)
}

/// 纯函数：由「场景定义 + 领域定义 + 引用」聚合出 PersistId 注册表。
///
/// - `scene_sources`：来自 `scenes/*.scene` 的 `entity_meta persist_id`；
/// - `view`：Lua 域数据只读视图（`data.npcs.persist_id`、`data.shops.keeper`）；
/// - `scan_errors`：场景扫描错误（作为备注保留）。
pub(crate) fn build_persist_registry(
    scene_sources: &[PersistSource],
    view: &DomainView,
    scan_errors: &[String],
) -> PersistRegistry {
    let mut registry = PersistRegistry::default();
    let mut domain_sources: Vec<PersistSource> = Vec::new();
    for npc in &view.npcs {
        if !npc.persist_id.trim().is_empty() {
            domain_sources.push(PersistSource {
                id: npc.persist_id.clone(),
                source: format!("data/npcs.lua: {}", npc.name),
            });
        }
    }
    registry.entries.extend(scene_sources.iter().cloned());
    registry.entries.extend(domain_sources.iter().cloned());
    registry
        .entries
        .sort_by(|a, b| a.id.cmp(&b.id).then(a.source.cmp(&b.source)));

    // 重复定义：同一 id 出现在 ≥2 个来源。
    let mut by_id: std::collections::BTreeMap<&str, Vec<&PersistSource>> =
        std::collections::BTreeMap::new();
    for entry in &registry.entries {
        by_id.entry(entry.id.as_str()).or_default().push(entry);
    }
    for (id, entries) in &by_id {
        if entries.len() >= 2 {
            registry.issues.push(PersistIssue {
                kind: PersistIssueKind::Duplicate,
                id: (*id).to_owned(),
                detail: entries
                    .iter()
                    .map(|entry| entry.source.clone())
                    .collect::<Vec<_>>()
                    .join(" / "),
            });
        }
    }

    // 场景 meta 与 data.npcs 冲突。
    let npc_ids: std::collections::BTreeSet<&str> = domain_sources
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    for entry in scene_sources {
        if npc_ids.contains(entry.id.as_str()) {
            registry.issues.push(PersistIssue {
                kind: PersistIssueKind::NpcSceneConflict,
                id: entry.id.clone(),
                detail: format!(
                    "{} 与 data.npcs.persist_id 同名：运行期 Lua 注册同名实体会被引擎拒绝（persist_id already in use）",
                    entry.source
                ),
            });
        }
    }

    // 引用完整性：商店店主必须存在于 data.npcs。
    let mut references: Vec<PersistReference> = Vec::new();
    for shop in &view.shops {
        if !shop.keeper.trim().is_empty() {
            references.push(PersistReference {
                id: shop.keeper.clone(),
                source: format!("data/shops.lua: {}.keeper", shop.id),
            });
        }
    }
    let defined: std::collections::BTreeSet<&str> = domain_sources
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    for reference in references {
        if !defined.contains(reference.id.as_str()) {
            registry.issues.push(PersistIssue {
                kind: PersistIssueKind::UndefinedReference,
                id: reference.id.clone(),
                detail: format!("{} 引用了未定义的 persist_id", reference.source),
            });
        }
    }

    for entry in scene_sources {
        if !defined.contains(entry.id.as_str()) {
            registry.notes.push(format!(
                "{} 声明的 persist_id 未出现在 data.npcs（可能是农田/建筑/容器 id）",
                entry.source
            ));
        }
    }
    for error in scan_errors {
        registry.notes.push(error.clone());
    }
    registry
}

/// 在 Lua 源文本中定位锚点所在行（1-based）。用于「定位到源文件」。
pub(crate) fn locate_line(source: &str, anchor: &str) -> Option<usize> {
    if anchor.is_empty() {
        return None;
    }
    source
        .lines()
        .position(|line| line.contains(anchor))
        .map(|index| index + 1)
}

/// 定位出口/地点在 `data/maps.lua` / `data/places.lua` 里的锚点。
pub(crate) fn exit_anchor(exit_id: &str) -> String {
    format!("id = \"{exit_id}\"")
}

pub(crate) fn place_anchor(place_name: &str) -> String {
    format!("{place_name} = ")
}

// ---------------------------------------------------------------------------
// 面板绘制（egui）
// ---------------------------------------------------------------------------

impl EditorApp {
    /// 领域数据窗口：模块校验状态 + 逐条问题 + 只读视图 + PersistId 注册表。
    pub(crate) fn draw_domain_data_panel(&mut self, context: &egui::Context) {
        if !self.show_domain_data_panel {
            return;
        }
        if self.domain_report.is_none() {
            self.refresh_domain_report();
        }
        let mut open = self.show_domain_data_panel;
        let mut revalidate = false;
        let mut located: Option<String> = None;
        egui::Window::new("领域数据（Lua 校验）")
            .open(&mut open)
            .default_size([640.0, 460.0])
            .resizable(true)
            .show(context, |ui| {
                let Some(report) = self.domain_report.as_ref() else {
                    ui.label("尚未校验。");
                    return;
                };
                ui.horizontal(|ui| {
                    if ui.button("重新校验").clicked() {
                        revalidate = true;
                    }
                    let proj_name = report
                        .project_root
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("项目");
                    ui.colored_label(
                        ui.visuals().weak_text_color(),
                        format!("工程：{proj_name}"),
                    );
                    // 右侧：领域数据统计 chips。
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let weak = ui.visuals().weak_text_color();
                        let chip = |ui: &mut egui::Ui, label: &str, n: usize| {
                            ui.colored_label(weak, format!("{label} {n}"));
                        };
                        chip(ui, "对话", report.view.dialogues.len());
                        chip(ui, "任务", report.view.quests.len());
                        chip(ui, "商店", report.view.shops.len());
                        chip(ui, "NPC", report.view.npcs.len());
                        chip(ui, "地点", report.view.places.len());
                        let exits: usize = report.view.maps.iter().map(|m| m.exits.len()).sum();
                        chip(ui, "出口", exits);
                        chip(ui, "地图", report.view.maps.len());
                    });
                });
                if let Some(error) = &report.bootstrap_error {
                    ui.colored_label(egui::Color32::RED, format!("校验失败：{error}"));
                    return;
                }
                if report.issue_count() == 0 {
                    ui.colored_label(
                        egui::Color32::from_rgb(120, 200, 130),
                        egui::RichText::new("✓ 全部通过").strong(),
                    );
                } else {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 170, 60),
                        format!("⚠ 问题 {} 条（模块 {} 个）", report.issue_count(), report.problem_modules()),
                    );
                }
                ui.colored_label(
                    ui.visuals().weak_text_color(),
                    egui::RichText::new(report.source.label()).small(),
                );
                if let Some(error) = &report.view_error {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 150, 90),
                        format!("只读视图失败：{error}"),
                    );
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .hscroll(false)
                    .show(ui, |ui| {
                        for module in &report.modules {
                            let color = match module.status {
                                DomainModuleStatus::Ok => egui::Color32::from_rgb(120, 200, 130),
                                DomainModuleStatus::Skipped => ui.visuals().weak_text_color(),
                                DomainModuleStatus::Issues(_) => {
                                    egui::Color32::from_rgb(230, 170, 60)
                                }
                                DomainModuleStatus::Error(_) => egui::Color32::RED,
                            };
                            ui.horizontal(|ui| {
                                ui.colored_label(color, module.status.label());
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{}  ←  {}",
                                        module.module, module.relative
                                    ))
                                    .monospace(),
                                );
                            });
                            for issue in module.issue_lines() {
                                ui.horizontal_wrapped(|ui| {
                                    ui.add_space(16.0);
                                    // 问题文本原样保留文件/对象/字段路径，便于定位。
                                    ui.label(egui::RichText::new(issue).monospace());
                                });
                            }
                        }
                        ui.separator();
                        ui.collapsing("PersistId 注册表", |ui| {
                            self.draw_persist_registry_section(ui);
                        });
                        ui.collapsing("只读视图（出口 / 地点 / NPC / 商店）", |ui| {
                            self.draw_domain_view_section(ui, &mut located);
                        });
                    });
            });
        self.show_domain_data_panel = open;
        if revalidate {
            self.refresh_domain_report();
        }
        if let Some(message) = located {
            self.status = message;
        }
    }

    /// 只读视图摘要行（供面板标题与状态栏复用）。
    pub(crate) fn domain_view_summary(&self) -> String {
        let Some(view) = self.domain_report.as_ref().map(|report| &report.view) else {
            return "领域数据不可用".to_owned();
        };
        format!(
            "地图 {} · 出口 {} · 地点 {} · NPC {} · 商店 {} · 任务 {} · 对话 {}",
            view.maps.len(),
            view.maps.iter().map(|map| map.exits.len()).sum::<usize>(),
            view.places.len(),
            view.npcs.len(),
            view.shops.len(),
            view.quests.len(),
            view.dialogues.len()
        )
    }

    fn draw_persist_registry_section(&self, ui: &mut egui::Ui) {
        let Some(registry) = self.persist_registry.as_ref() else {
            ui.label("尚未收集。");
            return;
        };
        if registry.is_empty() {
            ui.label("工程内没有 persist_id 定义（场景 entity_meta / data.npcs）。");
            return;
        }
        ui.label(format!(
            "定义 {} 条 · 问题 {} 条",
            registry.entries.len(),
            registry.issue_count()
        ));
        let weak = ui.visuals().weak_text_color();
        for issue in &registry.issues {
            ui.colored_label(
                egui::Color32::from_rgb(235, 120, 110),
                egui::RichText::new(format!(
                    "[{}] {}：{}",
                    issue.kind.label(),
                    issue.id,
                    issue.detail
                ))
                .monospace(),
            );
        }
        for note in &registry.notes {
            ui.colored_label(weak, egui::RichText::new(note));
        }
        egui::ScrollArea::vertical()
            .id_salt("persist_entries")
            .max_height(160.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for entry in &registry.entries {
                    ui.label(
                        egui::RichText::new(format!("{}  ←  {}", entry.id, entry.source))
                            .monospace(),
                    );
                }
            });
    }

    /// 只读视图列表：出口 / 地点 / NPC 日程 / 商店引用，并给出「定位」按钮把
    /// `文件:行` 复制到剪贴板（编辑器不做 Lua 源文本外科改动，见任务文档限制）。
    fn draw_domain_view_section(&self, ui: &mut egui::Ui, located: &mut Option<String>) {
        let Some(view) = self.domain_report.as_ref().map(|report| &report.view) else {
            return;
        };
        ui.label(format!("任务索引：{}", join_or_none(&view.quests)));
        ui.label(format!("对话索引：{}", join_or_none(&view.dialogues)));
        ui.label("地图出口：");
        for map in &view.maps {
            for exit in &map.exits {
                ui.horizontal(|ui| {
                    let cell = exit
                        .cell
                        .map(DomainCell::label)
                        .unwrap_or_else(|| "(缺 cell)".to_owned());
                    let to_cell = exit
                        .to_cell
                        .map(DomainCell::label)
                        .unwrap_or_else(|| "(缺 to_cell)".to_owned());
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {} cell={} → {} {}  「{}」",
                            map.id, exit.id, cell, exit.to, to_cell, exit.label
                        ))
                        .monospace(),
                    );
                    if ui.small_button("定位").clicked() {
                        *located = Some(locate_in_editor(
                            ui,
                            &self.project.root,
                            "scripts/data/maps.lua",
                            &exit_anchor(&exit.id),
                        ));
                    }
                });
            }
        }
        ui.label("NPC 地点：");
        for place in &view.places {
            ui.horizontal(|ui| {
                let cell = place
                    .cell
                    .map(DomainCell::label)
                    .unwrap_or_else(|| "(缺 cell)".to_owned());
                ui.label(
                    egui::RichText::new(format!(
                        "{} · map={} cell={}  「{}」",
                        place.name, place.map, cell, place.label
                    ))
                    .monospace(),
                );
                if ui.small_button("定位").clicked() {
                    *located = Some(locate_in_editor(
                        ui,
                        &self.project.root,
                        "scripts/data/places.lua",
                        &place_anchor(&place.name),
                    ));
                }
            });
        }
        ui.label("NPC 日程：");
        for npc in &view.npcs {
            ui.label(
                egui::RichText::new(format!(
                    "{}（{}，{}） home={} schedule={}",
                    npc.persist_id, npc.name, npc.character, npc.home, npc.schedule
                ))
                .monospace(),
            );
        }
        ui.label("商店：");
        for shop in &view.shops {
            let stock = shop
                .stock
                .iter()
                .map(|entry| {
                    format!(
                        "{}[price={},limit={}]",
                        entry.item,
                        entry
                            .price
                            .map_or_else(|| "-".to_owned(), |price| price.to_string()),
                        entry
                            .limit
                            .map_or_else(|| "-".to_owned(), |limit| limit.to_string())
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            ui.label(
                egui::RichText::new(format!(
                    "{} keeper={} place={} stock=[{}] buys=[{}]",
                    shop.id,
                    shop.keeper,
                    shop.place,
                    stock,
                    shop.buys.join(",")
                ))
                .monospace(),
            );
        }
    }

    /// 重新收集域报告与 PersistId 注册表：**新建 Lua VM**（丢弃模块缓存）后读取
    /// 磁盘最新内容，再扫描工程场景的 `entity_meta persist_id` 做交叉校验。
    pub(crate) fn refresh_domain_report(&mut self) {
        let root = self.project.root.clone();
        let report = collect_domain_report(&root);
        let (scene_sources, scan_errors) = scan_scene_persist_ids(&root);
        let registry = build_persist_registry(&scene_sources, &report.view, &scan_errors);
        self.status = if let Some(error) = &report.bootstrap_error {
            format!("域数据校验失败：{error}")
        } else {
            format!(
                "域数据校验：模块 {} 个 / 问题 {} 条；PersistId 定义 {} 条 / 问题 {} 条",
                report.modules.len(),
                report.issue_count(),
                registry.entries.len(),
                registry.issue_count()
            )
        };
        self.domain_report = Some(report);
        self.persist_registry = Some(registry);
    }

    /// 2D Scene 视图的**只读**领域覆盖层：当前场景（= 地图 id）在 `data.maps`
    /// 里的出口格、`data.places` 里的地点格与地图默认出生点。
    ///
    /// 坐标换算复用 Scene 视图的世界→面板映射（`pan`/`zoom`/`pixels_per_point`），
    /// 与 `tilemap_cell_screen_rect`/`entity_2d_screen_box` 完全一致：
    /// `cell` 覆盖 `[c*TILE, (c+1)*TILE)`、中心 `(c+0.5)*TILE`（与 `data/maps.lua`
    /// 的 `cell_to_world` 一致）。
    pub(crate) fn draw_domain_overlay(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        pixels_per_point: f32,
    ) {
        if !self.show_domain_overlay {
            return;
        }
        let Some(view) = self.domain_report.as_ref().map(|report| &report.view) else {
            return;
        };
        let Some(tile) = view.tile else {
            return;
        };
        if view.is_empty() {
            return;
        }
        let map_id = self.scene_name();
        let ppp = pixels_per_point.max(1.0e-3);
        let zoom = self.scene_zoom;
        let pan = self.scene_state.pan;
        let to_panel = |world_x: f64, world_y: f64| -> egui::Pos2 {
            egui::pos2(
                rect.center().x + (pan.x + world_x as f32 * zoom) / ppp,
                rect.center().y + (-pan.y - world_y as f32 * zoom) / ppp,
            )
        };
        let cell_rect = |cell: DomainCell| -> egui::Rect {
            egui::Rect::from_min_max(
                to_panel(cell.x as f64 * tile, (cell.y as f64 + 1.0) * tile),
                to_panel((cell.x as f64 + 1.0) * tile, cell.y as f64 * tile),
            )
        };
        let exit_fill = egui::Color32::from_rgba_unmultiplied(80, 200, 120, 55);
        let exit_stroke = egui::Color32::from_rgb(110, 230, 150);
        let place_fill = egui::Color32::from_rgba_unmultiplied(90, 180, 230, 45);
        let place_stroke = egui::Color32::from_rgb(120, 200, 240);
        let spawn_stroke = egui::Color32::from_rgb(240, 190, 90);
        let label_font = egui::FontId::proportional(11.0);
        let mut hover: Option<(egui::Rect, String)> = None;
        let hover_pos = painter.ctx().pointer_hover_pos();

        let places = view.places_in(&map_id);
        let has_ground_map =
            scene_has_ground_map(self.current_scene().map(|document| &document.scene));
        match view.map(&map_id) {
            Some(map) => {
                for exit in &map.exits {
                    let Some(cell) = exit.cell else {
                        continue;
                    };
                    let marker = cell_rect(cell);
                    painter.rect_filled(marker.shrink(1.0), 0.0, exit_fill);
                    painter.rect_stroke(
                        marker,
                        0.0,
                        egui::Stroke::new(1.5_f32, exit_stroke),
                        egui::StrokeKind::Inside,
                    );
                    painter.text(
                        marker.center_top() + egui::vec2(0.0, -2.0),
                        egui::Align2::CENTER_BOTTOM,
                        format!("exit {}", exit.id),
                        label_font.clone(),
                        exit_stroke,
                    );
                    set_hover(
                        &mut hover,
                        hover_pos,
                        marker,
                        format!(
                            "出口 {}\ncell {}\n→ {} {}\n{}",
                            exit.id,
                            cell.label(),
                            exit.to,
                            exit.to_cell
                                .map(DomainCell::label)
                                .unwrap_or_else(|| "(缺 to_cell)".to_owned()),
                            exit.label
                        ),
                    );
                }
                if let Some(spawn) = map.spawn {
                    let marker = cell_rect(spawn);
                    painter.rect_stroke(
                        marker,
                        0.0,
                        egui::Stroke::new(1.0_f32, spawn_stroke),
                        egui::StrokeKind::Inside,
                    );
                    painter.text(
                        marker.center(),
                        egui::Align2::CENTER_CENTER,
                        "spawn",
                        label_font.clone(),
                        spawn_stroke,
                    );
                    set_hover(
                        &mut hover,
                        hover_pos,
                        marker,
                        format!("出生格 {} · 地图 {}", spawn.label(), map.id),
                    );
                }
            }
            None => {
                // 有 `Tilemap` 却没注册 = 真的漏登记（橙色告警）；纯 UI 场景
                // （主入口门面）按设计不入 `data/maps`，只给中性说明、不着色告警。
                let (notice, warn) = unregistered_scene_notice(has_ground_map, &map_id);
                painter.text(
                    rect.left_bottom() + egui::vec2(12.0, -28.0),
                    egui::Align2::LEFT_BOTTOM,
                    notice,
                    label_font.clone(),
                    if warn {
                        egui::Color32::from_rgb(235, 150, 90)
                    } else {
                        painter.ctx().style().visuals.weak_text_color()
                    },
                );
            }
        }
        for place in &places {
            let Some(cell) = place.cell else {
                continue;
            };
            let marker = cell_rect(cell);
            painter.rect_filled(marker.shrink(2.0), 0.0, place_fill);
            painter.rect_stroke(
                marker,
                0.0,
                egui::Stroke::new(1.0_f32, place_stroke),
                egui::StrokeKind::Inside,
            );
            painter.circle_filled(marker.center(), 2.0, place_stroke);
            set_hover(
                &mut hover,
                hover_pos,
                marker,
                format!(
                    "地点 {} · {}\nmap {}\ncell {}",
                    place.name,
                    place.label,
                    place.map,
                    cell.label()
                ),
            );
        }
        // 未注册场景的提示已由上面的 notice 承担；摘要只在真取到地图时给出，
        // 避免"未注册"下面再挂一行无意义的「出口 0 · 地点 0」。
        if let Some(map) = view.map(&map_id) {
            painter.text(
                rect.left_bottom() + egui::vec2(12.0, -12.0),
                egui::Align2::LEFT_BOTTOM,
                format!(
                    "领域覆盖（只读）：{map_id} · 出口 {} · 地点 {}",
                    map.exits.len(),
                    places.len()
                ),
                label_font,
                painter.ctx().style().visuals.weak_text_color(),
            );
        }
        if let Some((marker, detail)) = hover {
            let anchor = marker.right_top() + egui::vec2(8.0, -8.0);
            let galley =
                painter.layout_no_wrap(detail, egui::FontId::monospace(11.0), egui::Color32::WHITE);
            let box_rect = egui::Rect::from_min_size(anchor, galley.size() + egui::vec2(10.0, 6.0));
            painter.rect_filled(
                box_rect,
                3.0,
                egui::Color32::from_rgba_unmultiplied(20, 20, 24, 235),
            );
            painter.rect_stroke(
                box_rect,
                3.0,
                egui::Stroke::new(1.0_f32, egui::Color32::GRAY),
                egui::StrokeKind::Inside,
            );
            painter.galley(anchor + egui::vec2(5.0, 3.0), galley, egui::Color32::WHITE);
        }
    }
}

/// 指针落在 marker 上时记录悬停详情（后写覆盖先写：地点优先于出口）。
fn set_hover(
    hover: &mut Option<(egui::Rect, String)>,
    hover_pos: Option<egui::Pos2>,
    marker: egui::Rect,
    detail: String,
) {
    if hover_pos.is_some_and(|pos| marker.contains(pos)) {
        *hover = Some((marker, detail));
    }
}

/// 场景是否带 `Tilemap`（地编地图的判据）。UI 门面场景（如 `main.scene`）
/// 没有任何 `Tilemap`，因此"未在 `data.maps` 注册"对它是正常状态而非漏登记。
fn scene_has_ground_map(scene: Option<&Scene>) -> bool {
    scene.is_some_and(|scene| {
        scene
            .entities
            .values()
            .any(|entity| entity.components.contains_key("Tilemap"))
    })
}

/// 场景名不在 `data.maps` 时的左下角提示：`(文案, 是否橙色告警)`。
///
/// - 带 `Tilemap` 的玩法场景没注册 = 漏登记 → 橙色告警（原行为）。
/// - 没有地编的纯 UI 场景（主入口门面 `scenes/main.scene`）按设计不进
///   `data/maps.lua`（见 `examples/game_jhyt/docs/44`，`main.lua` 用
///   `maps.get() == nil` 分流到门面分支）→ 中性说明，不报错。
fn unregistered_scene_notice(has_ground_map: bool, map_id: &str) -> (String, bool) {
    if has_ground_map {
        (format!("域覆盖：场景「{map_id}」未在 data.maps 注册"), true)
    } else {
        (
            format!(
                "域覆盖：场景「{map_id}」非地编场景（无 GroundMap），未在 data.maps 注册属预期"
            ),
            false,
        )
    }
}

/// 在工程相对路径的 Lua 源文件里定位锚点行，并把 `路径:行` 复制到剪贴板。
fn locate_in_editor(ui: &egui::Ui, root: &Path, relative: &str, anchor: &str) -> String {
    let path = root.join(relative);
    let Ok(source) = fs::read_to_string(&path) else {
        return format!("无法读取 {relative}（工程根目录：{}）", root.display());
    };
    match locate_line(&source, anchor) {
        Some(line) => {
            let text = format!("{relative}:{line}");
            ui.ctx().copy_text(text.clone());
            format!("已复制定位：{text}")
        }
        None => format!("{relative} 中未找到锚点 {anchor}"),
    }
}

fn join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "(空)".to_owned()
    } else {
        values.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::core::scene::{SceneEntity, SceneEntityId};
    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 删除临时夹具目录。Windows 上 `remove_dir_all` 可能在 Lua 运行时还没释放文件
    /// 句柄时返回「另一个程序正在使用此文件」（错误 32）——清理失败只是留下一个带纳秒
    /// 后缀的临时目录，绝不该把测试判失败（此前这里是 `expect("cleanup")`，在全量并行
    /// 跑测试时会偶发失败）。
    fn cleanup(root: PathBuf) {
        for _ in 0..20 {
            if fs::remove_dir_all(&root).is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("p2-editor-m1903-{label}-{suffix}"));
        fs::create_dir_all(root.join("scripts/data/nested")).expect("fixture dirs");
        fs::create_dir_all(root.join("scripts/systems")).expect("runtime module dir");
        fs::create_dir_all(root.join("scenes")).expect("scenes dir");
        root
    }

    /// 临时工程夹具：一个合法模块、一个非法模块、一个无校验器模块、一个加载即
    /// 失败的模块，外加一个顶层运行时模块与 `main.lua`（不得被执行）。
    fn fixture_project(label: &str) -> PathBuf {
        let root = temp_root(label);
        let data = root.join("scripts/data");
        fs::write(
            data.join("good.lua"),
            "local M = {}\nM.version = 1\nfunction M.validate_all() return true, {} end\nreturn M\n",
        )
        .expect("good module");
        fs::write(
            data.join("bad.lua"),
            "local M = {}\nM.version = 1\nfunction M.validate_all()\n  return false, { 'data/bad.lua:things[1].x: required number' }\nend\nreturn M\n",
        )
        .expect("bad module");
        fs::write(
            data.join("nested/index.lua"),
            "return { version = 1, ids = { 'a', 'b' } }\n",
        )
        .expect("nested module");
        fs::write(
            data.join("broken.lua"),
            "error('boom at load')\nreturn {}\n",
        )
        .expect("broken module");
        fs::write(
            root.join("scripts/runtime_check.lua"),
            "local M = {}\nfunction M.validate_all() return false, { 'runtime_check.lua: config: missing field' } end\nreturn M\n",
        )
        .expect("runtime module");
        // 运行时模块放进子目录也必须被发现（工程按职责分目录，如
        // `scripts/systems/shop.lua` → `require("systems.shop")`）。
        fs::write(
            root.join("scripts/systems/nested_check.lua"),
            "local M = {}\nfunction M.validate_all() return true, {} end\nreturn M\n",
        )
        .expect("nested runtime module");
        fs::write(root.join("scripts/plain.lua"), "local M = {}\nreturn M\n")
            .expect("plain module");
        fs::write(
            root.join("scripts/main.lua"),
            "error('main.lua must never be executed by the editor')\nreturn {}\n",
        )
        .expect("main module");
        root
    }

    /// 夹具：提供 M20-01 统一校验入口的工程。`main.lua` 故意写成加载即 error，
    /// 用来证明编辑器只执行校验入口、不会执行游戏入口。
    fn unified_fixture_project(label: &str, check_body: &str) -> PathBuf {
        let root = temp_root(label);
        fs::write(
            root.join("scripts/validate_data.lua"),
            format!("local M = {{}}\nfunction M.check(options)\n{check_body}\nend\nreturn M\n"),
        )
        .expect("validate_data");
        fs::write(
            root.join("scripts/main.lua"),
            "error('main.lua must never be executed by the editor')\nreturn {}\n",
        )
        .expect("main");
        fs::write(
            root.join("scripts/data/maps.lua"),
            "local M = {}\nM.TILE = 32\nM.maps = { village = { display = 'v', spawn = { x = 1, y = 1 }, exits = {} } }\nreturn M\n",
        )
        .expect("maps");
        root
    }

    /// M20-01：统一入口返回问题列表时，按 `[模块]` 前缀归属且问题文本原样保留，
    /// 同时只读视图仍可取出（maps 出口/TILE）。
    #[test]
    fn unified_entry_groups_issues_and_still_reads_the_view() {
        let root = unified_fixture_project(
            "unified-issues",
            "  return \"[maps] data/maps.lua:maps.village.spawn: out of map bounds\\n[npcs] data/npcs.lua:npcs[3].schedule: unknown schedule ghost\"",
        );
        let report = collect_domain_report(&root);
        assert_eq!(report.source, DomainValidationSource::UnifiedEntry);
        assert!(
            report.bootstrap_error.is_none(),
            "{:?}",
            report.bootstrap_error
        );
        let modules: Vec<&str> = report.modules.iter().map(|m| m.module.as_str()).collect();
        assert_eq!(modules, vec!["maps", "npcs"]);
        assert_eq!(report.issue_count(), 2);
        assert_eq!(report.problem_modules(), 2);
        assert_eq!(
            report.modules[0].status,
            DomainModuleStatus::Issues(vec![
                "data/maps.lua:maps.village.spawn: out of map bounds".to_owned()
            ])
        );
        assert_eq!(report.modules[0].relative, "scripts/data/maps.lua");
        assert_eq!(report.modules[1].relative, "scripts/data/npcs.lua");
        assert!(report.view.map("village").is_some());
        assert_eq!(report.view.tile, Some(32.0));
        cleanup(root);
    }

    /// M20-01：统一入口返回空串 = 全部通过（0 条问题，无问题模块）。
    #[test]
    fn unified_entry_empty_report_means_all_clear() {
        let root = unified_fixture_project("unified-clear", "  return \"\"");
        let report = collect_domain_report(&root);
        assert_eq!(report.source, DomainValidationSource::UnifiedEntry);
        assert!(
            report.bootstrap_error.is_none(),
            "{:?}",
            report.bootstrap_error
        );
        assert!(report.modules.is_empty());
        assert_eq!(report.issue_count(), 0);
        assert_eq!(report.problem_modules(), 0);
        cleanup(root);
    }

    /// 统一入口加载/执行失败时报告硬错误（不 panic）。
    #[test]
    fn unified_entry_load_failure_is_reported() {
        let root = temp_root("unified-broken");
        fs::write(
            root.join("scripts/validate_data.lua"),
            "error('validate_data boom')\nreturn {}\n",
        )
        .expect("validate_data");
        let report = collect_domain_report(&root);
        let error = report.bootstrap_error.expect("bootstrap error");
        assert!(error.contains("validate_data boom"), "{error}");
        cleanup(root);
    }

    #[test]
    fn group_unified_issues_keeps_paths_and_falls_back_to_uncategorized() {
        let modules = group_unified_issues(
            "[maps] data/maps.lua:maps.x.spawn: out of map bounds\nno prefix line\n\n",
        );
        assert_eq!(modules.len(), 2);
        assert_eq!(modules[0].module, "maps");
        assert_eq!(modules[0].relative, "scripts/data/maps.lua");
        assert_eq!(
            modules[0].issue_lines(),
            vec!["data/maps.lua:maps.x.spawn: out of map bounds".to_owned()]
        );
        assert_eq!(modules[1].module, "未归类");
        assert_eq!(modules[1].relative, UNIFIED_ENTRY_RELATIVE);
        assert_eq!(
            issue_source("[maps] data/maps.lua:maps.x: bad"),
            Some("data/maps.lua".to_owned())
        );
        assert_eq!(issue_source("no path here"), None);
        assert_eq!(split_module_prefix("[a] b]").0, "a");
    }

    /// 真实工程冒烟：`examples/game_jhyt` 提供统一校验入口时，编辑器域面板路径
    /// 必须能加载并取出只读视图（地图/出口/地点/NPC/商店/TILE）。
    ///
    /// 刻意**不断言问题条数为 0**：该工程由并行会话持续修改，域数据可能临时带有
    /// 问题（这属于面板要显示的内容）；本测试只锁定「入口可加载 + 视图可用 +
    /// 错误定位可用」，不锁定内容数值。
    #[test]
    fn real_game_project_loads_unified_entry_and_reads_view() {
        let root = engine::core::games_dir().join("examples").join("game_jhyt");
        if !root.join(UNIFIED_ENTRY_RELATIVE).is_file() {
            // 仓库状态不含统一入口时跳过（该能力由 M20-01 提供）。
            return;
        }
        let report = collect_domain_report(&root);
        assert_eq!(report.source, DomainValidationSource::UnifiedEntry);
        assert!(
            report.bootstrap_error.is_none(),
            "统一校验入口应可加载：{:?}",
            report.bootstrap_error
        );
        assert!(
            report.view_error.is_none(),
            "只读视图应可读取：{:?}",
            report.view_error
        );
        assert!(!report.view.maps.is_empty(), "应至少读出一张地图");
        assert!(!report.view.places.is_empty(), "应至少读出一个地点");
        assert!(!report.view.npcs.is_empty(), "应至少读出一条 NPC 记录");
        assert!(report.view.tile.is_some(), "data.maps.TILE 应可读出");
        assert!(
            report
                .view
                .maps
                .iter()
                .flat_map(|map| map.exits.iter())
                .any(|exit| exit.cell.is_some() && exit.to_cell.is_some()),
            "应至少有一个出口同时带 cell 与 to_cell"
        );
        // 场景名即地图 id：当前工程的场景文件应能在 data.maps 中找到。
        let (scene_sources, scan_errors) = scan_scene_persist_ids(&root);
        assert!(scan_errors.is_empty(), "场景不应解析失败：{scan_errors:?}");
        let registry = build_persist_registry(&scene_sources, &report.view, &scan_errors);
        assert!(
            !registry.entries.is_empty(),
            "至少应汇总出 data.npcs.persist_id 定义"
        );
    }

    #[test]
    fn discovery_maps_files_to_modules_and_skips_main_lua() {
        let root = fixture_project("discover");
        let modules = discover_domain_modules(&root);
        let names: Vec<&str> = modules.iter().map(|m| m.module.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "data.bad",
                "data.broken",
                "data.good",
                "data.nested.index",
                "runtime_check",
                "systems.nested_check",
            ],
            "模块名应按文件名映射并稳定排序；main.lua/plain.lua 不含 validate_all 不参与"
        );
        let nested = modules
            .iter()
            .find(|m| m.module == "data.nested.index")
            .expect("nested module");
        assert_eq!(nested.relative, "scripts/data/nested/index.lua");
        let nested_runtime = modules
            .iter()
            .find(|m| m.module == "systems.nested_check")
            .expect("nested runtime module");
        assert_eq!(nested_runtime.relative, "scripts/systems/nested_check.lua");
        cleanup(root);
    }

    #[test]
    fn collect_reports_ok_issues_skip_and_load_error_with_module_attribution() {
        let root = fixture_project("collect");
        let report = collect_domain_report(&root);
        assert!(
            report.bootstrap_error.is_none(),
            "{:?}",
            report.bootstrap_error
        );
        assert!(report.view_error.is_none(), "{:?}", report.view_error);

        let good = report
            .modules
            .iter()
            .find(|m| m.module == "data.good")
            .expect("good report");
        assert_eq!(good.status, DomainModuleStatus::Ok);

        let bad = report
            .modules
            .iter()
            .find(|m| m.module == "data.bad")
            .expect("bad report");
        assert_eq!(bad.relative, "scripts/data/bad.lua");
        assert_eq!(
            bad.issue_lines(),
            vec!["data/bad.lua:things[1].x: required number".to_owned()],
            "问题文本必须原样保留 Lua 校验器的文件/对象/字段路径"
        );

        let runtime = report
            .modules
            .iter()
            .find(|m| m.module == "runtime_check")
            .expect("runtime report");
        assert_eq!(runtime.relative, "scripts/runtime_check.lua");
        assert_eq!(report.problem_modules(), 3, "bad + broken + runtime_check");
        assert_eq!(report.issue_count(), 3);

        let nested = report
            .modules
            .iter()
            .find(|m| m.module == "data.nested.index")
            .expect("nested report");
        assert_eq!(nested.status, DomainModuleStatus::Skipped);

        let broken = report
            .modules
            .iter()
            .find(|m| m.module == "data.broken")
            .expect("broken report");
        match &broken.status {
            DomainModuleStatus::Error(message) => {
                assert!(
                    message.contains("boom at load"),
                    "原始消息应保留：{message}"
                );
            }
            other => panic!("应报告加载失败，实际 {other:?}"),
        }
        cleanup(root);
    }

    #[test]
    fn collect_without_scripts_directory_reports_bootstrap_and_empty_view() {
        let root = temp_root("empty");
        fs::remove_dir_all(root.join("scripts")).expect("remove scripts");
        let report = collect_domain_report(&root);
        // 引导本身不报错（只是 package.path 多了一条无效前缀），视图为空。
        assert!(report.bootstrap_error.is_none());
        assert!(report.modules.is_empty());
        assert!(report.view.is_empty());
        cleanup(root);
    }

    #[test]
    fn validate_expression_escapes_module_names() {
        let expression = validate_expression("data.we\"ird");
        assert!(expression.contains(r#"pcall(require, "data.we\"ird")"#));
    }

    #[test]
    fn parse_view_reads_maps_places_npcs_and_shops() {
        let value = serde_json::json!({
            "maps": [{
                "id": "village",
                "display": "云泽村",
                "spawn": { "x": 19, "y": 2 },
                "exits": [{
                    "id": "village_to_main",
                    "cell": { "x": 19, "y": 1 },
                    "to": "manor",
                    "to_cell": { "x": 16, "y": 3 },
                    "label": "返回山庄"
                }]
            }],
            "places": [{ "name": "village_gate", "map": "village", "cell": { "x": 19, "y": 2 }, "label": "南市街口" }],
            "npcs": [{ "name": "沈青萝", "persist_id": "npc_shen_qingluo", "character": "shen_qingluo", "home": "village_home_west", "schedule": "shen_qingluo" }],
            "shops": [{
                "id": "zhao_stall",
                "name": "货摊",
                "keeper": "npc_zhao_tiesuanpan",
                "place": "village_stall",
                "hours": { "open": 480, "close": 1080 },
                "stock": [{ "item": "seed_rice", "price": 12, "limit": 10 }],
                "buys": ["rice"]
            }],
            "quests": ["quest_herb"],
            "dialogues": ["gu_changfeng_teach"]
        });
        let view = parse_view(&value);
        assert_eq!(view.maps.len(), 1);
        assert_eq!(
            view.map("village").expect("map").exits[0].cell,
            Some(DomainCell { x: 19, y: 1 })
        );
        assert_eq!(view.places_in("village").len(), 1);
        assert_eq!(view.places_in("manor").len(), 0);
        assert_eq!(view.npcs[0].persist_id, "npc_shen_qingluo");
        assert_eq!(view.shops[0].hours, Some((480, 1080)));
        assert_eq!(view.shops[0].stock[0].item, "seed_rice");
        assert_eq!(view.shops[0].buys, vec!["rice".to_owned()]);
        assert_eq!(view.quests, vec!["quest_herb".to_owned()]);
        assert_eq!(view.dialogues, vec!["gu_changfeng_teach".to_owned()]);
        // 缺失字段按空值降级，不 panic。
        let empty = parse_view(&serde_json::json!({ "maps": "nope" }));
        assert!(empty.is_empty());
    }

    #[test]
    fn persist_registry_reports_duplicates_conflicts_and_undefined_references() {
        let scene_sources = vec![
            PersistSource {
                id: "npc_shen_qingluo".to_owned(),
                source: "scenes/village.scene#Shen".to_owned(),
            },
            PersistSource {
                id: "village_crate".to_owned(),
                source: "scenes/village.scene#Crate".to_owned(),
            },
            PersistSource {
                id: "village_crate".to_owned(),
                source: "scenes/manor.scene#Crate".to_owned(),
            },
        ];
        let view = DomainView {
            npcs: vec![NpcDef {
                name: "沈青萝".to_owned(),
                persist_id: "npc_shen_qingluo".to_owned(),
                character: "shen_qingluo".to_owned(),
                home: "village_home_west".to_owned(),
                schedule: "shen_qingluo".to_owned(),
            }],
            shops: vec![
                ShopDef {
                    id: "zhao_stall".to_owned(),
                    name: "货摊".to_owned(),
                    keeper: "npc_zhao_tiesuanpan".to_owned(),
                    place: "village_stall".to_owned(),
                    hours: None,
                    stock: Vec::new(),
                    buys: Vec::new(),
                },
                ShopDef {
                    id: "good_stall".to_owned(),
                    name: "好摊".to_owned(),
                    keeper: "npc_shen_qingluo".to_owned(),
                    place: "village_stall".to_owned(),
                    hours: None,
                    stock: Vec::new(),
                    buys: Vec::new(),
                },
            ],
            ..DomainView::default()
        };
        let registry = build_persist_registry(
            &scene_sources,
            &view,
            &["scenes/x.scene: 解析失败".to_owned()],
        );
        assert_eq!(registry.entries.len(), 4, "3 场景 meta + 1 NPC 定义");
        let kinds: Vec<PersistIssueKind> = registry.issues.iter().map(|issue| issue.kind).collect();
        assert!(kinds.contains(&PersistIssueKind::Duplicate));
        assert!(kinds.contains(&PersistIssueKind::NpcSceneConflict));
        assert!(kinds.contains(&PersistIssueKind::UndefinedReference));
        let duplicate = registry
            .issues
            .iter()
            .find(|issue| issue.kind == PersistIssueKind::Duplicate && issue.id == "village_crate")
            .expect("duplicate");
        assert_eq!(duplicate.id, "village_crate");
        assert!(duplicate.detail.contains("scenes/village.scene#Crate"));
        assert!(duplicate.detail.contains("scenes/manor.scene#Crate"));
        let conflict = registry
            .issues
            .iter()
            .find(|issue| issue.kind == PersistIssueKind::NpcSceneConflict)
            .expect("conflict");
        assert_eq!(conflict.id, "npc_shen_qingluo");
        let undefined = registry
            .issues
            .iter()
            .find(|issue| issue.kind == PersistIssueKind::UndefinedReference)
            .expect("undefined");
        assert_eq!(undefined.id, "npc_zhao_tiesuanpan");
        assert_eq!(
            registry.notes.len(),
            3,
            "village_crate 两条备注 + 1 条场景扫描错误"
        );
        // 干净输入不报问题。
        let clean = build_persist_registry(&[], &DomainView::default(), &[]);
        assert!(clean.is_empty());
    }

    #[test]
    fn scene_scan_reads_entity_meta_persist_id() {
        let root = temp_root("scan");
        fs::write(
            root.join("scenes/village.scene"),
            "scene_version=2\nname=village\nentity\t1\tPlayer\ncomponent\t1\tTransform\tx=0.0;y=0.0\nentity_meta\t1\tpersist_id\tplayer_main\nentity\t2\tCrate\nentity_meta\t2\tpersist_id\tvillage_crate\nentity_meta\t2\trole\tprop\n",
        )
        .expect("scene");
        fs::write(
            root.join("scenes/broken.scene"),
            "this is not a scene file\n",
        )
        .expect("scene");
        let (sources, errors) = scan_scene_persist_ids(&root);
        assert_eq!(
            sources,
            vec![
                PersistSource {
                    id: "player_main".to_owned(),
                    source: "scenes/village.scene#Player".to_owned(),
                },
                PersistSource {
                    id: "village_crate".to_owned(),
                    source: "scenes/village.scene#Crate".to_owned(),
                },
            ]
        );
        // 缺少必需字段的场景文件被记录为备注而不是 panic。
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("scenes/broken.scene"));
        cleanup(root);
    }

    #[test]
    fn locate_line_finds_anchors_for_exits_and_places() {
        let maps = "M.maps = {\n  manor = {\n    exits = {\n      {\n        id = \"main_to_village\",\n        cell = { x = 16, y = 1 },\n";
        assert_eq!(locate_line(maps, &exit_anchor("main_to_village")), Some(5));
        assert_eq!(locate_line(maps, &exit_anchor("missing")), None);
        let places = "M.places = {\n  manor_gate = { map = \"manor\" },\n";
        assert_eq!(locate_line(places, &place_anchor("manor_gate")), Some(2));
        assert_eq!(locate_line(places, ""), None);
    }

    /// 地编判据：有 `Tilemap` 的场景才算地图；纯 UI 门面场景没有。
    #[test]
    fn scene_has_ground_map_detects_tilemap_component() {
        fn scene_with(components: &[&str]) -> Scene {
            let mut scene = Scene::new("fixture");
            let mut entity = SceneEntity {
                id: SceneEntityId(1),
                name: "Ground".to_owned(),
                parent: None,
                children: Vec::new(),
                components: BTreeMap::new(),
                resources: BTreeMap::new(),
                metadata: BTreeMap::new(),
            };
            for component in components {
                entity
                    .components
                    .insert((*component).to_owned(), "tiles=1x1".to_owned());
            }
            scene.entities.insert(entity.id, entity);
            scene
        }

        assert!(scene_has_ground_map(Some(&scene_with(&[
            "Transform",
            "Tilemap",
        ]))));
        assert!(!scene_has_ground_map(Some(&scene_with(&[
            "Transform",
            "UiLayout",
            "Image",
        ]))));
        assert!(!scene_has_ground_map(Some(&Scene::new("empty"))));
        assert!(!scene_has_ground_map(None), "没有打开场景时不算地编");
    }

    /// 注册缺失提示分两类：地编场景漏登记 → 橙色告警；纯 UI 场景 → 中性说明。
    #[test]
    fn unregistered_scene_notice_only_warns_for_ground_maps() {
        let (warn_text, warn) = unregistered_scene_notice(true, "main");
        assert!(warn, "带 Tilemap 的场景未注册必须仍然告警");
        assert_eq!(warn_text, "域覆盖：场景「main」未在 data.maps 注册");

        let (info_text, warn) = unregistered_scene_notice(false, "main");
        assert!(!warn, "主入口门面这类纯 UI 场景不告警");
        assert!(info_text.contains("main"), "{info_text}");
        assert!(
            info_text.contains("非地编场景") && info_text.contains("预期"),
            "中性说明要点明这是预期状态：{info_text}"
        );
    }
}
