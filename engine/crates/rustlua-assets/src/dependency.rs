//! Dependency-graph extraction and resolution for the resource database.
//!
//! Pure logic (no I/O) shared by the editor, assetc, and game's runtime
//! verification: it scans scene/prefab component strings, `.atlas` texture
//! references and Lua source for references to other assets, resolves them to
//! [`crate::resource_db::AssetId`]s through the database, and (in `propagate` /
//! `detect_cycles`) propagates a dependency's bad state onto its consumers.

use crate::resource_db::AssetId;
use crate::resource_db::AssetRecord;
use crate::resource_db::ResourceDatabase;
use crate::resource_db::ResourceState;
use crate::resource_kind::ResourceKind;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A single extracted reference in its raw form (project-relative, forward
/// slashes). [`DependencyRef::Dynamic`] marks references that cannot be
/// resolved statically; they are recorded but never turned into edges.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum DependencyRef {
    /// Literal asset path, e.g. `assets/textures/player.png`.
    Asset(PathBuf),
    /// Lua `require("name")` sibling module → `scripts/<name>.lua`.
    LuaModule(String),
    /// Scene-load literal → `scenes/<name>.scene`.
    SceneName(String),
    /// Reference that cannot be resolved statically (dynamic variable).
    Dynamic(String),
}

/// A reference that has no matching registered record (file missing, outside
/// the scanned roots, or misspelled). Callers treat it as a broken link.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnresolvedRef {
    pub path: PathBuf,
    pub raw: String,
}

/// Extracts raw references from scene/prefab text or Lua source.
///
/// Pure function, quote-aware: Lua scanning only matches inside string literals
/// so comments and docstrings never produce references. Scene/prefab text is
/// tab-separated with unquoted component fields, so its scan stays "any
/// position" exactly like the runtime `verify_project` already does.
pub fn extract_refs(text: &str, kind: ResourceKind) -> Vec<DependencyRef> {
    match kind {
        ResourceKind::Scene | ResourceKind::Prefab => extract_scene_assets(text),
        ResourceKind::Atlas => extract_atlas_texture(text),
        ResourceKind::LuaScript => extract_lua_refs(text),
        _ => Vec::new(),
    }
}

/// Resolves raw references to registered asset IDs, reporting any that have no
/// matching record. `self_id` is used to drop self-references (an asset naming
/// itself would otherwise create a self-loop). Missing-case references fall
/// back to a case-insensitive lookup for Windows authoring differences.
pub fn resolve_refs(
    refs: &[DependencyRef],
    db: &ResourceDatabase,
    self_id: AssetId,
) -> (Vec<AssetId>, Vec<UnresolvedRef>) {
    let mut ids = Vec::new();
    let mut unresolved = Vec::new();
    let mut seen = HashSet::new();
    for reference in refs {
        let (candidate, raw) = match reference {
            DependencyRef::Asset(path) => (path.clone(), path.to_string_lossy().into_owned()),
            DependencyRef::LuaModule(name) => {
                let path = PathBuf::from(format!("scripts/{name}.lua"));
                (path, format!("require(\"{name}\")"))
            }
            DependencyRef::SceneName(name) => {
                let path = PathBuf::from(format!("scenes/{name}.scene"));
                (path, format!("scene.load(\"{name}\")"))
            }
            DependencyRef::Dynamic(_) => continue,
        };
        let id = db
            .find_by_path(&candidate)
            .ok()
            .flatten()
            .map(|record| record.id)
            .or_else(|| db.find_by_path_ci(&candidate));
        let Some(id) = id else {
            unresolved.push(UnresolvedRef {
                path: candidate,
                raw,
            });
            continue;
        };
        if id == self_id {
            continue;
        }
        if seen.insert(id) {
            ids.push(id);
        }
    }
    (ids, unresolved)
}

/// Recomputes dependency edges for every container asset (Scene/Prefab/Atlas/LuaScript),
/// overwriting the previous edges each time so the operation is idempotent.
/// `read_source` reads a project-relative source file; returning `None` means
/// the file is gone and yields no edges. Unresolved references are returned so
/// callers can surface them as broken links.
pub fn recompute_edges(
    db: &mut ResourceDatabase,
    read_source: &dyn Fn(&Path) -> Option<String>,
) -> Vec<UnresolvedRef> {
    let containers: Vec<(AssetId, ResourceKind, PathBuf)> = db
        .iter()
        .filter(|record| {
            matches!(
                record.kind,
                ResourceKind::Scene
                    | ResourceKind::Prefab
                    | ResourceKind::Atlas
                    | ResourceKind::LuaScript
            )
        })
        .map(|record| (record.id, record.kind, record.source_path.clone()))
        .collect();
    let refs_per_id: Vec<(AssetId, Vec<DependencyRef>)> = containers
        .into_iter()
        .map(|(id, kind, path)| {
            let refs = read_source(&path)
                .map(|text| extract_refs(&text, kind))
                .unwrap_or_default();
            (id, refs)
        })
        .collect();
    let mut unresolved = Vec::new();
    // resolve 用不可变借用，collect 完成后借用结束，再应用可变 set_dependencies。
    let resolved: Vec<(AssetId, Vec<AssetId>, Vec<UnresolvedRef>)> = refs_per_id
        .iter()
        .map(|(id, refs)| {
            let (ids, un_resolved) = resolve_refs(refs, db, *id);
            (*id, ids, un_resolved)
        })
        .collect();
    for (id, ids, un_resolved) in resolved {
        db.set_dependencies(id, ids)
            .expect("container still present");
        unresolved.extend(un_resolved);
    }
    unresolved
}

/// Propagates dependency invalidation over a snapshot of records.
///
/// Pure function, idempotent and reversible: a consumer's state is the rank-max
/// of its own physical state and the worst state among its dependencies, so a
/// broken dependency surfaces on every consumer that transitively depends on it
/// (`Missing → Invalid → Invalid`), and restoring the dependency revokes the
/// `Invalid` back to a consumable state. Callers must run it with statuses
/// freshly reflecting the physical state (right after `reconcile` or an import
/// batch).
pub fn propagate(
    records: &HashMap<AssetId, AssetRecord>,
) -> HashMap<AssetId, (ResourceState, Option<String>)> {
    let mut current: HashMap<AssetId, ResourceState> = records
        .iter()
        .map(|(id, record)| (*id, record.status))
        .collect();
    // 循环依赖 error 先写入；传播只在状态变化时覆盖，保持 Ready 的环内节点保留循环文案。
    let mut errors: HashMap<AssetId, Option<String>> =
        records.keys().map(|id| (*id, None)).collect();
    for cycle in detect_cycles(records) {
        let text = cycle_error_text(&cycle);
        for id in cycle {
            errors.insert(id, Some(text.clone()));
        }
    }
    // rank 单调不减，至多 records.len() 轮必收敛。
    let rounds = records.len().max(1);
    for _ in 0..rounds {
        let mut changed = false;
        for (id, record) in records {
            if !record.status.is_durable() {
                continue;
            }
            if physical_rank(record) == 7 {
                continue; // 自身源缺失：保持 Missing/Orphaned，不因依赖改变。
            }
            let (dep_worst, worst_dep) = worst_dependency(records, &current, *id);
            let final_rank = physical_rank(record).max(dep_worst);
            if let Some(target) = state_for_rank(final_rank) {
                if current[id] != target {
                    current.insert(*id, target);
                    changed = true;
                }
                // 只有依赖才是拖后腿因子（比自身物理态更差）时才写依赖 error；
                // 自身差而依赖健康时保留原 error（如导入失败文案）或循环依赖文案。
                if let Some((dep, dep_state)) = worst_dep
                    && dep_worst > physical_rank(record)
                {
                    let text = dependency_error(dep, dep_state, record, records);
                    if errors[id].as_deref() != Some(text.as_str()) {
                        errors.insert(*id, Some(text));
                    }
                }
            } else if current[id] == ResourceState::Invalid {
                // 依赖全部健康：撤销 Invalid 回到可消费状态。
                let restored = if record.content_hash.is_empty() {
                    ResourceState::Discovered
                } else {
                    ResourceState::Ready
                };
                current.insert(*id, restored);
                errors.insert(*id, None);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    current
        .into_iter()
        .map(|(id, state)| (id, (state, errors.remove(&id).flatten())))
        .collect()
}

/// Detects dependency cycles with a three-color DFS and an explicit path stack.
/// Returns each cycle as a node list (deduplicated after rotating to a stable
/// starting node). A cycle is a graph-health warning, not itself a broken link.
pub fn detect_cycles(records: &HashMap<AssetId, AssetRecord>) -> Vec<Vec<AssetId>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }
    let mut color: HashMap<AssetId, Color> = HashMap::new();
    let mut stack: Vec<AssetId> = Vec::new();
    let mut cycles: Vec<Vec<AssetId>> = Vec::new();
    let mut seen: HashSet<Vec<AssetId>> = HashSet::new();
    fn dfs(
        v: AssetId,
        records: &HashMap<AssetId, AssetRecord>,
        color: &mut HashMap<AssetId, Color>,
        stack: &mut Vec<AssetId>,
        cycles: &mut Vec<Vec<AssetId>>,
        seen: &mut HashSet<Vec<AssetId>>,
    ) {
        color.insert(v, Color::Gray);
        stack.push(v);
        if let Some(record) = records.get(&v) {
            for dep in &record.dependencies {
                match color.get(dep).copied().unwrap_or(Color::White) {
                    Color::White => dfs(*dep, records, color, stack, cycles, seen),
                    Color::Gray => {
                        if let Some(index) = stack.iter().position(|x| x == dep) {
                            let cycle: Vec<AssetId> = stack[index..].to_vec();
                            let mut normalized = cycle.clone();
                            if let Some(min_index) = normalized
                                .iter()
                                .enumerate()
                                .min_by_key(|(_, id)| id.to_string())
                                .map(|(index, _)| index)
                            {
                                normalized.rotate_left(min_index);
                            }
                            if seen.insert(normalized) {
                                cycles.push(cycle);
                            }
                        }
                    }
                    Color::Black => {}
                }
            }
        }
        stack.pop();
        color.insert(v, Color::Black);
    }
    for id in records.keys() {
        if color.get(id).copied().unwrap_or(Color::White) == Color::White {
            dfs(*id, records, &mut color, &mut stack, &mut cycles, &mut seen);
        }
    }
    cycles
}

/// Worst state among `id`'s dependencies: `None`-dependent records (deleted)
/// count as broken links, and a dependency's own `Missing`/`Orphaned` is folded
/// into the broken-link class rather than being forwarded verbatim.
fn worst_dependency(
    records: &HashMap<AssetId, AssetRecord>,
    current: &HashMap<AssetId, ResourceState>,
    id: AssetId,
) -> (u8, Option<(AssetId, ResourceState)>) {
    let Some(record) = records.get(&id) else {
        return (2, None);
    };
    let mut worst = 2;
    let mut worst_dep = None;
    for dep in &record.dependencies {
        let (rank, state) = match records.get(dep) {
            None => (6, ResourceState::Missing), // 指向已删除记录 → 断链。
            Some(dep_record) => {
                let state = current.get(dep).copied().unwrap_or(dep_record.status);
                // 依赖侧 Missing/Orphaned/ImportFailed 都是断链类：消费者重导自己
                // 也无济于事，所以归到 Invalid(6) 而不是转发自身等级。
                let rank = if matches!(
                    state,
                    ResourceState::Missing | ResourceState::Orphaned | ResourceState::ImportFailed
                ) {
                    6
                } else {
                    state_rank(state)
                };
                (rank, state)
            }
        };
        if rank > worst {
            worst = rank;
            worst_dep = Some((*dep, state));
        }
    }
    (worst, worst_dep)
}

/// Consumer's own physical-state rank. `Invalid` is not physical: it is treated
/// as the Ready baseline so the dependency graph decides whether it holds or is
/// revoked.
fn physical_rank(record: &AssetRecord) -> u8 {
    match record.status {
        ResourceState::Missing | ResourceState::Orphaned => 7,
        ResourceState::ImportFailed => 5,
        ResourceState::Stale => 4,
        ResourceState::Discovered => 3,
        ResourceState::Invalid
        | ResourceState::Ready
        | ResourceState::ImportQueued
        | ResourceState::Importing
        | ResourceState::Removed => 2,
    }
}

/// Rank of a dependency's current state as seen by its consumers.
fn state_rank(state: ResourceState) -> u8 {
    match state {
        ResourceState::Invalid => 6,
        ResourceState::ImportFailed => 5,
        ResourceState::Stale => 4,
        ResourceState::Discovered => 3,
        ResourceState::Ready
        | ResourceState::Missing
        | ResourceState::Orphaned
        | ResourceState::ImportQueued
        | ResourceState::Importing
        | ResourceState::Removed => 2,
    }
}

fn state_for_rank(rank: u8) -> Option<ResourceState> {
    match rank {
        6 => Some(ResourceState::Invalid),
        5 => Some(ResourceState::ImportFailed),
        4 => Some(ResourceState::Stale),
        3 => Some(ResourceState::Discovered),
        _ => None,
    }
}

/// Error copy describing why a consumer is invalid, prefixed with `自身已变更；`
/// when the consumer's own physical state is not Ready.
fn dependency_error(
    dep: AssetId,
    dep_state: ResourceState,
    consumer: &AssetRecord,
    records: &HashMap<AssetId, AssetRecord>,
) -> String {
    let label = records
        .get(&dep)
        .map(|record| record.source_path.display().to_string())
        .unwrap_or_else(|| format!("记录 {dep}"));
    let cause = match dep_state {
        ResourceState::Missing | ResourceState::Orphaned => format!("依赖 {label} 缺失"),
        ResourceState::Invalid => format!("依赖 {label} 无效"),
        ResourceState::ImportFailed => format!("依赖 {label} 导入失败"),
        ResourceState::Stale => format!("依赖 {label} 已变更"),
        ResourceState::Discovered => format!("依赖 {label} 未导入"),
        _ => format!("依赖 {label} 状态异常"),
    };
    let self_bad = matches!(
        consumer.status,
        ResourceState::Stale | ResourceState::Discovered | ResourceState::ImportFailed
    );
    if self_bad {
        format!("自身已变更；{cause}")
    } else {
        cause
    }
}

fn cycle_error_text(cycle: &[AssetId]) -> String {
    let text = cycle
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" → ");
    format!("循环依赖: {text}")
}

/// Scans tab-separated scene/prefab text for `assets/` references, cutting at
/// the same terminator set the runtime uses (`;`/`:`/newline). Empty results
/// (a bare `assets/` before a terminator) are dropped; order is preserved with
/// duplicates removed.
///
/// **`#` 行首的注释不参与扫描**，判定与两个真正的解析器逐字同源：
/// `Scene::from_text`（`rustlua-scene/src/scene.rs:185`）与 prefab 解析器
/// （`prefab.rs:227`）都跳过 `line.starts_with('#')`。注释不是数据——此前这里
/// 扫全文，门面场景的一句中文注释（`examples/game_jhyt/scenes/main.scene:8`
/// 把 `assets/prefabs/title_menu.prefab` 写进反引号）被读成引用，截出的路径
/// 带着反引号与全角逗号，`game --check` 于是报"资源缺失"整条判红。图集那条
/// 路径早就按同一原则处理（见 `atlas_texture_field_is_the_only_edge`）。
fn extract_scene_assets(text: &str) -> Vec<DependencyRef> {
    let mut refs = Vec::new();
    let mut seen = HashSet::new();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if i + 7 <= chars.len() && chars[i..i + 7].iter().collect::<String>() == "assets/" {
                let mut end = i;
                while end < chars.len() && !matches!(chars[end], ';' | ':' | '\n' | '\r') {
                    end += 1;
                }
                let path: String = chars[i..end].iter().collect();
                let path = strip_region_suffix(&path);
                if path != "assets/" && seen.insert(path.to_owned()) {
                    refs.push(DependencyRef::Asset(PathBuf::from(path)));
                }
                i = end;
            } else {
                i += 1;
            }
        }
    }
    refs
}

/// `assets/atlases/hero.atlas#walk` 这类图集区域引用：图的消费者是图集文件本身，
/// `#区域名` 只在装配期解析，所以边指向去掉后缀的图集路径。
fn strip_region_suffix(raw: &str) -> &str {
    raw.split('#').next().unwrap_or(raw)
}

/// 图集描述文件只依赖它引用的那张合并纹理（`texture=` 行）。
/// 格式约定见 `docs/11`：一条图集记录只有一个纹理，缺字段由 importer 校验报错，
/// 这里只负责成边。
fn extract_atlas_texture(text: &str) -> Vec<DependencyRef> {
    let mut refs = Vec::new();
    let mut seen = HashSet::new();
    for line in text.lines() {
        let line = line.trim_start();
        if line.starts_with('#') {
            continue;
        }
        let Some(value) = line.strip_prefix("texture=") else {
            continue;
        };
        let value = value.trim();
        if !value.is_empty() && seen.insert(value.to_owned()) {
            refs.push(DependencyRef::Asset(PathBuf::from(value)));
        }
    }
    refs
}

/// `scene.load` 的 ASCII 字符序列，用于 chars 切片前缀比较（避免把字符索引
/// 当字节索引切片）。
const SCENE_LOAD: [char; 10] = ['s', 'c', 'e', 'n', 'e', '.', 'l', 'o', 'a', 'd'];

/// `prefab.spawn` / `data.load` / `data.reload` / `asset.load` 的**实参就是资源路径**，
/// 而这个路径在游戏脚本里常常经由同文件常量（`local PRESET_PATH = "assets/…"`）传入。
/// 这种写法静态上其实完全可证：本函数只在**实参自己就是字符串字面量**时产出边，变量实参
/// 交给 `write_dependency_index` 的同文件常量化处理；两者都证不出来时才返回名字，
/// 由调用方标 opaque（全量预载兜底）。
///
/// 为什么值得单独扫一遍：本仓八个界面都用 `prefab.spawn(PRESET_PATH)`，而那些
/// `PRESET_PATH` 就是字面量——只按"文件里出现过 `prefab.spawn`"就判 opaque，
/// 等于把已经证过的依赖再标一次"无法证明"。
pub fn scan_path_call_args(text: &str) -> Vec<String> {
    const CALLS: [&str; 4] = ["prefab.spawn", "data.load", "data.reload", "asset.load"];
    let mut unresolved = Vec::new();
    for call in CALLS {
        let mut search = 0;
        while let Some(offset) = text[search..].find(call) {
            let start = search + offset + call.len();
            search = start;
            let argument = match call_argument(text, start) {
                Some(argument) => argument,
                None => continue,
            };
            if quote_trimmed(argument).is_some() {
                // 字面量实参：路径引用由 `lua_string_refs` 的 `assets/` 分支产出
                // （本文件的 `extract_refs` 已包含它），这里不再重复产出一条。
                continue;
            }
            if !unresolved.iter().any(|seen| seen == argument) {
                unresolved.push(argument.to_owned());
            }
        }
    }
    unresolved
}

/// `prefab.spawn` 这类调用的实参文本：`prefab.spawn(x)` → `x`，`prefab.spawn "x"` → `"x"`，
/// 无实参或无函数调用括号时返回 `None`（`f` 指向调用名末尾）。
fn call_argument(text: &str, f: usize) -> Option<&str> {
    let rest = text.get(f..)?;
    let trimmed = rest.trim_start();
    let after = match trimmed.strip_prefix('(') {
        Some(after) => after,
        // 无括号形态只接字符串字面量（Lua 的 `f "x"`），其他一律不猜。
        None => return trimmed.starts_with(['"', '\'']).then_some(trimmed),
    };
    let mut depth = 1usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (index, c) in after.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if let Some(open) = quote {
            if c == '\\' {
                escaped = true;
            } else if c == open {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(after[..index].trim());
                }
            }
            _ => {}
        }
    }
    None
}

/// 整串就是一个字符串字面量时返回去掉引号的正文，否则 `None`
/// （拼接、table 字段、变量一律不算字面量）。
fn quote_trimmed(argument: &str) -> Option<&str> {
    let trimmed = argument.trim();
    let mut chars = trimmed.chars();
    let quote = match chars.next() {
        Some(quote @ ('"' | '\'')) => quote,
        _ => return None,
    };
    if !trimmed.ends_with(quote) || trimmed.len() < 2 {
        return None;
    }
    let body = &trimmed[1..trimmed.len() - 1];
    (!body.contains(quote)).then_some(body)
}

/// Scans Lua source with a quote-aware state machine: `--` comments are skipped
/// to end of line, and references are only read from string literals (or from
/// `scene.load(<variable>)`, which is recorded as [`DependencyRef::Dynamic`]).
fn extract_lua_refs(text: &str) -> Vec<DependencyRef> {
    let chars: Vec<char> = text.chars().collect();
    let mut refs = Vec::new();
    let mut seen = HashSet::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // `--` 注释到行尾：字符串内的 `--` 由字符串分支处理，不会走到这里。
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // `scene.load(<变量>)` 动态引用（字面量参数由字符串扫描处理）。
        // 用 chars 切片比较而非 `text[i..]`（i 是字符索引，字节切片会因
        // 中文等非 ASCII 字符错位 panic）。
        if chars[i..].starts_with(&SCENE_LOAD) {
            if let Some(reference) = scene_load_dynamic(&chars, i)
                && seen.insert(reference.clone())
            {
                refs.push(reference);
            }
            i += "scene.load".len();
            continue;
        }
        if c != '"' && c != '\'' {
            i += 1;
            continue;
        }
        // 字符串字面量：收集到匹配引号，`\` 转义跳过下一字符。
        let quote = c;
        let context = lua_context(&chars, i);
        let mut content = String::new();
        i += 1;
        let mut closed = false;
        while i < chars.len() {
            if chars[i] == '\\' {
                content.push(chars[i]);
                if let Some(&escaped) = chars.get(i + 1) {
                    content.push(escaped);
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            if chars[i] == quote {
                i += 1;
                closed = true;
                break;
            }
            content.push(chars[i]);
            i += 1;
        }
        if closed {
            for reference in lua_string_refs(&context, &content) {
                if seen.insert(reference.clone()) {
                    refs.push(reference);
                }
            }
        }
    }
    refs
}

/// Reverse-scan context token for the string starting at `quote_index`: skips
/// whitespace and an optional `(` so both `require("x")` and `require "x"`
/// yield `require`, and `scene.load("x")` yields `scene.load`.
fn lua_context(chars: &[char], quote_index: usize) -> String {
    let mut j = quote_index as isize - 1;
    let mut token: Vec<char> = Vec::new();
    while j >= 0 {
        let c = chars[j as usize];
        if c.is_whitespace() {
            if !token.is_empty() {
                break;
            }
            j -= 1;
        } else if c == '(' {
            j -= 1;
        } else if is_ident_char(c) {
            token.push(c);
            j -= 1;
        } else {
            break;
        }
    }
    token.reverse();
    token.into_iter().collect()
}

/// Detects `scene.load(<variable>)` and records the argument as [`DependencyRef::Dynamic`]
/// so the graph acknowledges the reference without resolving it.
///
/// 例外：实参区域里出现字面量场景名时（游戏里最常见的路由写法就是
/// `scene.load(maps.get(target) and target or "manor")`——查表命中用表里的名字，
/// 未命中用兜底字面量），那个名字是**可证的真依赖**，因此产出场景名而不是 opaque。
/// 主脚本因此不必为一句"查表 + 兜底"把整包拖成全量预载。
fn scene_load_dynamic(chars: &[char], start: usize) -> Option<DependencyRef> {
    let mut j = start + "scene.load".len();
    while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
    }
    let mut arg_start = j;
    let open = j < chars.len() && chars[j] == '(';
    if open {
        j += 1;
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        arg_start = j;
    }
    if j < chars.len() && matches!(chars[j], '"' | '\'') {
        return None; // 字面量场景引用由字符串扫描处理。
    }
    let mut end = j;
    while end < chars.len() && is_ident_char(chars[end]) {
        end += 1;
    }
    if end <= arg_start {
        return None;
    }
    let name: String = chars[arg_start..end].iter().collect();
    let name = name.trim_end_matches([')', ',', ';']).to_owned();
    if open && let Some(scene) = scene_literal_in_call(chars, start) {
        return Some(DependencyRef::SceneName(scene));
    }
    (!name.is_empty()).then_some(DependencyRef::Dynamic(name))
}

/// `scene.load(` 之后的整段实参文本里出现的**第一个**字符串字面量（配平括号截取）。
///
/// 只取一个是有意的保守：多分支字面量里挑任意一个都不能证明"全部"可达，但
/// `a and b or "literal"` 这个最普遍形态的兜底分支恰好就是唯一的那个字面量，
/// 取它既不多算也不少算；真正的多分支路由仍会因为主分支是变量而落回 opaque。
fn scene_literal_in_call(chars: &[char], start: usize) -> Option<String> {
    let mut j = start + "scene.load".len();
    while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
    }
    if j >= chars.len() || chars[j] != '(' {
        return None;
    }
    j += 1;
    let mut depth = 1usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut literal = String::new();
    while j < chars.len() {
        let c = chars[j];
        if escaped {
            escaped = false;
            if quote.is_some() {
                literal.push(c);
            }
            j += 1;
            continue;
        }
        if let Some(open) = quote {
            match c {
                '\\' => escaped = true,
                c if c == open => {
                    return (!literal.trim().is_empty()).then(|| literal.trim().to_owned());
                }
                _ => literal.push(c),
            }
            j += 1;
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return None;
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// Turns a Lua string literal plus its call context into references: `require`
/// and `scene.load` literals are module/scene references; any literal containing
/// `assets/` is an asset path reference. Other strings are ignored.
fn lua_string_refs(context: &str, content: &str) -> Vec<DependencyRef> {
    let mut refs = Vec::new();
    let trimmed = content.trim();
    if context.ends_with("require") && !trimmed.is_empty() {
        refs.push(DependencyRef::LuaModule(trimmed.to_owned()));
    }
    if context.ends_with("scene.load") && !trimmed.is_empty() {
        refs.push(DependencyRef::SceneName(trimmed.to_owned()));
    }
    if let Some(index) = content.find("assets/") {
        let path = strip_region_suffix(&content[index..]).trim_end_matches([';', ':', '\n', '\r']);
        refs.push(DependencyRef::Asset(PathBuf::from(path)));
    }
    refs
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

/// 模块名 → 包内脚本路径。**点号是路径分隔符，不是文件名的一部分**：
/// `require("data.buildings")` 指的是 `scripts/data/buildings.lua`。
///
/// 这个换算以前只存在于宿主侧（`web-runtime.js` 装载 `package.preload` 前把
/// `scripts/a/b.lua` 记成 `a.b`），索引侧却写成 `format!("scripts/{name}.lua")`——
/// 于是 `data.buildings` 被拼成 `scripts/data.buildings.lua`，一个包内不存在的路径，
/// **`require` 的边一条都连不上**：155 个 Lua 里 55 个被 require 的文件全被标 opaque，
/// 主脚本一出发就回退全量预载。换算放在这里，两侧共用一个定义。
pub fn module_path(name: &str) -> String {
    format!("scripts/{}.lua", name.replace('.', "/"))
}

/// 场景名 → 包内场景路径（与 `scene.load("manor")` 的取值口径一致）。
pub fn scene_path(name: &str) -> String {
    format!("scenes/{name}.scene")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 提取 Asset 引用的路径列表（用于断言）。
    fn asset_paths(refs: &[DependencyRef]) -> Vec<String> {
        refs.iter()
            .filter_map(|reference| match reference {
                DependencyRef::Asset(path) => Some(path.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect()
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustlua-m12-02-{label}-{}", std::process::id()))
    }

    #[test]
    fn scene_refs_sprite_image_animation_tilemap_mesh() {
        // 用 examples 真实组件行：Sprite 结构化 texture=、Animation 冒号分隔、
        // Tilemap texture=、MeshRenderer mesh=。
        let scene = "scene_version=2\nname=main\n\
            component 1 Animation clip:idle:assets/textures/hero-idle.png:4:4:true;clip:move:assets/textures/hero-move.png:4:8:true\n\
            component 1 Sprite texture=assets/textures/player.png;size=32x32\n\
            component 2 Tilemap texture=assets/textures/tileset.png;tiles=1111111\n\
            component 4 MeshRenderer mesh=assets/models/m8-test-triangle.glb\n\
            component 6 Animation3D mesh=assets/models/kaykit-warrior.glb;clip=Idle;looped=true\n";
        let refs = extract_refs(scene, ResourceKind::Scene);
        let assets = asset_paths(&refs);
        for expected in [
            "assets/textures/hero-idle.png",
            "assets/textures/hero-move.png",
            "assets/textures/player.png",
            "assets/textures/tileset.png",
            "assets/models/m8-test-triangle.glb",
            "assets/models/kaykit-warrior.glb",
        ] {
            assert!(
                assets.contains(&expected.to_owned()),
                "missing {expected}: {assets:?}"
            );
        }
        assert_eq!(assets.len(), 6, "dedup + exact extraction: {assets:?}");
    }

    #[test]
    fn scene_comment_lines_do_not_produce_refs() {
        // 回归：`examples/game_jhyt/scenes/main.scene:8` 是一句中文注释，把预设路径
        // 写进了反引号并以全角逗号收尾。旧实现扫全文，截出的"引用"带着 ` 与 ，，
        // 让 `game --check` 报"场景引用的资源缺失"整条判红（HEAD 实测）。
        // 注释行必须整行跳过，且同文件里的真实引用一个不能少。
        let scene = "scene_version=2\nname=main\n\
            # 与它们的 `Text` 标签 / 卷轴贴士 / 版本号）整批搬进 `assets/prefabs/title_menu.prefab`，\n\
            component 1 Sprite texture=assets/textures/player.png;size=32x32\n";
        assert_eq!(
            asset_paths(&extract_refs(scene, ResourceKind::Scene)),
            vec!["assets/textures/player.png".to_owned()],
            "注释里的反引号路径不得成边，数据行的引用必须保留"
        );

        // 预制走同一套扫描：注释行同样跳过。
        let prefab = "name=demo\n\
            # 与 `assets/textures/ghost.png` 同一约定\n\
            component 1 Sprite texture=assets/textures/player.png;size=32x32\n";
        assert_eq!(
            asset_paths(&extract_refs(prefab, ResourceKind::Prefab)),
            vec!["assets/textures/player.png".to_owned()]
        );
    }

    #[test]
    fn comment_only_when_hash_leads_the_line() {
        // `#` 只在行首才是注释（与 `Scene::from_text` 的 `line.starts_with('#')` 同源）：
        // 行内的 `#` 是图集区域后缀，不改变"这条引用有效"的判定。
        let scene = "component 1 Sprite texture=assets/atlases/hero.atlas#walk;size=32x16\n";
        assert_eq!(
            asset_paths(&extract_refs(scene, ResourceKind::Scene)),
            vec!["assets/atlases/hero.atlas".to_owned()]
        );
    }

    #[test]
    fn prefab_uses_same_format() {
        let prefab =
            "name=demo\ncomponent 1 Sprite texture=assets/textures/player.png;size=32x32\n";
        let refs = extract_refs(prefab, ResourceKind::Prefab);
        assert_eq!(
            asset_paths(&refs),
            vec!["assets/textures/player.png".to_owned()]
        );
    }

    #[test]
    fn atlas_texture_field_is_the_only_edge() {
        // 图集的依赖是它引用的那张合并纹理；region 行不产生边。
        let atlas = "atlas_version=1\ntexture=assets/atlases/hero-0.png\n\
            # 注释里的 texture=assets/x.png 不算\n\
            region\twalk\t2\t2\t32\t16\n";
        assert_eq!(
            asset_paths(&extract_refs(atlas, ResourceKind::Atlas)),
            vec!["assets/atlases/hero-0.png".to_owned()]
        );
    }

    #[test]
    fn region_suffix_is_dropped_when_building_edges() {
        // `图集路径#区域名`：边指向图集文件本身，`#walk` 只在使用点解析。
        let scene = "component 1 Sprite texture=assets/atlases/hero.atlas#walk;size=32x16\n";
        assert_eq!(
            asset_paths(&extract_refs(scene, ResourceKind::Scene)),
            vec!["assets/atlases/hero.atlas".to_owned()]
        );
        let lua = "sprite.set(id, 'texture=assets/atlases/hero.atlas#walk;size=32x16')\n";
        assert_eq!(
            asset_paths(&extract_refs(lua, ResourceKind::LuaScript)),
            vec!["assets/atlases/hero.atlas".to_owned()]
        );
    }

    #[test]
    fn lua_require_modules() {
        let lua = "local game = require(\"game\")\nlocal demo3d = require('demo3d')\nlocal hud = require(\"ui/hud\")\n";
        let refs = extract_refs(lua, ResourceKind::LuaScript);
        let modules: Vec<_> = refs
            .iter()
            .filter_map(|reference| match reference {
                DependencyRef::LuaModule(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(modules, vec!["game", "demo3d", "ui/hud"]);
    }

    #[test]
    fn lua_scene_load_literal_only() {
        let lua = "scene.load(\"main\")\nscene.load('arena')\nscene.load(state.scene_name)\n";
        let refs = extract_refs(lua, ResourceKind::LuaScript);
        let scenes: Vec<_> = refs
            .iter()
            .filter_map(|reference| match reference {
                DependencyRef::SceneName(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(scenes, vec!["main", "arena"]);
        let dynamic: Vec<_> = refs
            .iter()
            .filter_map(|reference| match reference {
                DependencyRef::Dynamic(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(dynamic, vec!["state.scene_name"], "变量参数记录为动态");
    }

    #[test]
    fn lua_scene_load_fallback_literal_is_a_real_edge() {
        // 游戏里最常见的路由写法：查表命中用表里的名字，未命中用兜底字面量。
        // 兜底那支是可证的真依赖；整句都算 Dynamic 会让主脚本被标 opaque。
        let lua = "return scene.load(maps.get(target) and target or \"manor\")\n";
        let refs = extract_refs(lua, ResourceKind::LuaScript);
        let scenes: Vec<_> = refs
            .iter()
            .filter_map(|reference| match reference {
                DependencyRef::SceneName(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(scenes, vec!["manor"], "{refs:?}");
        assert!(
            !refs
                .iter()
                .any(|reference| matches!(reference, DependencyRef::Dynamic(_))),
            "兜底字面量可证时不再记 Dynamic：{refs:?}"
        );

        // 纯变量、且实参里没有任何字面量场景名：仍然记 Dynamic（保守回退）。
        let lua = "scene.load(maps.route_of(target))\n";
        let refs = extract_refs(lua, ResourceKind::LuaScript);
        assert!(
            refs.iter()
                .any(|reference| matches!(reference, DependencyRef::Dynamic(_))),
            "{refs:?}"
        );
    }

    #[test]
    fn path_call_args_are_literal_or_unresolved() {
        // 字面量实参（含同文件常量那种）由 assets 字面量分支产边，这里不重复报；
        // 只有真证不出来的实参才返回给调用方，由它决定 opaque。
        let lua = "local P = \"assets/prefabs/hud.prefab\"\n\
            prefab.spawn(P)\n\
            prefab.spawn(\"assets/prefabs/shop.prefab\")\n\
            prefab.spawn(preset_name)\n\
            data.load(DIR .. name)\n";
        assert_eq!(
            scan_path_call_args(lua),
            vec![
                // 常量实参也原样返回：常量解析在依赖索引那一侧（它才知道包内有哪些文件），
                // 这一层只负责"实参文本不是字面量"这一件事。
                "P".to_owned(),
                "preset_name".to_owned(),
                "DIR .. name".to_owned(),
            ]
        );
        // 无括号的字面量形态同样是"已解析"，不进 unresolved。
        assert!(scan_path_call_args("prefab.spawn \"assets/x.prefab\"\n").is_empty());
        // 嵌套表参不越界：外层括号配平后只取到最外层实参文本。
        assert_eq!(
            scan_path_call_args("data.load(table.concat({ DIR, name }))\n"),
            vec!["table.concat({ DIR, name })".to_owned()]
        );
    }

    #[test]
    fn module_and_scene_names_become_package_paths() {
        // 点号是分隔符：这是 `require` 边能不能连上的唯一依据，写错一个字符
        // 就会让整条 require 链静默消失（表现为索引里全是 opaque）。
        assert_eq!(module_path("data.buildings"), "scripts/data/buildings.lua");
        assert_eq!(module_path("core.state"), "scripts/core/state.lua");
        assert_eq!(module_path("validate_data"), "scripts/validate_data.lua");
        assert_eq!(scene_path("manor"), "scenes/manor.scene");
    }

    #[test]
    fn lua_asset_paths() {
        let lua = "mesh.set(id, \"assets/models/m8-test-triangle.glb\")\n";
        let refs = extract_refs(lua, ResourceKind::LuaScript);
        assert_eq!(
            asset_paths(&refs),
            vec!["assets/models/m8-test-triangle.glb".to_owned()]
        );
    }

    #[test]
    fn excludes_ground_plane() {
        let scene = "component 1 MeshRenderer mesh=__ground_plane__\n\
            component 2 MeshRenderer mesh=assets/models/kaykit-warrior.glb\n";
        let refs = extract_refs(scene, ResourceKind::Scene);
        let assets = asset_paths(&refs);
        assert_eq!(assets, vec!["assets/models/kaykit-warrior.glb".to_owned()]);
    }

    #[test]
    fn comment_aware_scan() {
        // 注释里的 require / assets 引用不产出。
        let lua = "-- local x = require(\"ghost\")\n\
            -- mesh.set(id, \"assets/textures/ghost.png\")\n\
            local ok = require(\"ok\")\n";
        let refs = extract_refs(lua, ResourceKind::LuaScript);
        assert!(refs.iter().all(|reference| match reference {
            DependencyRef::LuaModule(name) => name == "ok",
            _ => false,
        }));
    }

    #[test]
    fn dedupe_preserves_order() {
        let scene = "Sprite texture=assets/textures/a.png\nSprite texture=assets/textures/a.png\nSprite texture=assets/textures/b.png\n";
        let refs = extract_refs(scene, ResourceKind::Scene);
        assert_eq!(
            asset_paths(&refs),
            vec![
                "assets/textures/a.png".to_owned(),
                "assets/textures/b.png".to_owned()
            ]
        );
    }

    #[test]
    fn resolve_self_cycle_excluded() {
        let root = temp_root("self");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "scene-1")
            .unwrap();
        // 场景文本里 scene.load("main") 指向自己 → 排除。
        let refs = vec![DependencyRef::SceneName("main".to_owned())];
        let (ids, unresolved) = resolve_refs(&refs, &db, scene);
        assert!(ids.is_empty(), "self-loop dropped");
        assert!(unresolved.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_asset_and_report_unresolved() {
        let root = temp_root("resolve");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let tex = db
            .register(
                "assets/textures/player.png",
                ResourceKind::Texture,
                "texture-1",
            )
            .unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "scene-1")
            .unwrap();
        let refs = vec![
            DependencyRef::Asset(PathBuf::from("assets/textures/player.png")),
            DependencyRef::Asset(PathBuf::from("assets/textures/ghost.png")),
            DependencyRef::Asset(PathBuf::from("assets/textures/PLAYER.PNG")), // CI 兜底
        ];
        let (ids, unresolved) = resolve_refs(&refs, &db, scene);
        assert_eq!(ids, vec![tex], "asset resolved; CI fallback also resolves");
        assert_eq!(unresolved.len(), 1, "only ghost.png is broken");
        assert_eq!(
            unresolved[0].path,
            PathBuf::from("assets/textures/ghost.png")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recompute_edges_reads_sources_and_writes_graph() {
        let root = temp_root("edges");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let tex = db
            .register(
                "assets/textures/player.png",
                ResourceKind::Texture,
                "texture-1",
            )
            .unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "scene-1")
            .unwrap();
        let lua = db
            .register("scripts/main.lua", ResourceKind::LuaScript, "script-1")
            .unwrap();
        // require("game") 指向的模块：真实项目中会随扫描注册，测试需同步注册。
        let game = db
            .register("scripts/game.lua", ResourceKind::LuaScript, "script-1")
            .unwrap();
        let sources = [
            (
                PathBuf::from("scenes/main.scene"),
                "component 1 Sprite texture=assets/textures/player.png;size=32x32\n".to_owned(),
            ),
            (
                PathBuf::from("scripts/main.lua"),
                "local game = require(\"game\")\n".to_owned(),
            ),
            (
                PathBuf::from("scripts/game.lua"),
                "function update() end\n".to_owned(),
            ),
        ];
        let read = |path: &Path| -> Option<String> {
            sources
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, text)| text.clone())
        };
        let unresolved = recompute_edges(&mut db, &read);
        assert!(unresolved.is_empty(), "all references resolvable");
        assert_eq!(db.get(scene).unwrap().dependencies, vec![tex]);
        // Lua 引用 scripts/game.lua。
        assert_eq!(db.get(lua).unwrap().dependencies, vec![game]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recompute_edges_chains_scene_atlas_texture() {
        let root = temp_root("edges-atlas");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let sheet = db
            .register(
                "assets/atlases/hero-0.png",
                ResourceKind::Texture,
                "texture-1",
            )
            .unwrap();
        let atlas = db
            .register("assets/atlases/hero.atlas", ResourceKind::Atlas, "atlas-1")
            .unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "scene-1")
            .unwrap();
        let sources = [
            (
                PathBuf::from("assets/atlases/hero.atlas"),
                "atlas_version=1\ntexture=assets/atlases/hero-0.png\nregion\twalk\t2\t2\t32\t16\n"
                    .to_owned(),
            ),
            (
                PathBuf::from("scenes/main.scene"),
                "component 1 Sprite texture=assets/atlases/hero.atlas#walk;size=32x16\n".to_owned(),
            ),
        ];
        let read = |path: &Path| -> Option<String> {
            sources
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, text)| text.clone())
        };
        assert!(recompute_edges(&mut db, &read).is_empty());
        assert_eq!(db.get(atlas).unwrap().dependencies, vec![sheet]);
        assert_eq!(db.get(scene).unwrap().dependencies, vec![atlas]);
        // 纹理变更沿两级边传导：sheet 缺失 → 图集 Invalid → 场景 Invalid。
        db.get_mut_for_test(sheet).status = ResourceState::Missing;
        db.get_mut_for_test(atlas).status = ResourceState::Ready;
        db.get_mut_for_test(scene).status = ResourceState::Ready;
        let outcome = propagate(&record_snapshot(&db));
        assert_eq!(applied_state(&outcome, atlas), ResourceState::Invalid);
        assert_eq!(applied_state(&outcome, scene), ResourceState::Invalid);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recompute_edges_reports_unresolved() {
        let root = temp_root("edges-unresolved");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "scene-1")
            .unwrap();
        let read = |_path: &Path| {
            Some("component 1 Sprite texture=assets/textures/player.png;size=32x32\n".to_owned())
        };
        let unresolved = recompute_edges(&mut db, &read);
        assert_eq!(unresolved.len(), 1);
        assert_eq!(
            unresolved[0].path,
            PathBuf::from("assets/textures/player.png")
        );
        assert_eq!(db.get(scene).unwrap().dependencies, Vec::<AssetId>::new());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// DB 记录 → 传播算法用的快照 HashMap。
    fn record_snapshot(db: &ResourceDatabase) -> HashMap<AssetId, AssetRecord> {
        db.iter()
            .map(|record| (record.id, record.clone()))
            .collect()
    }

    fn applied_state(
        outcome: &HashMap<AssetId, (ResourceState, Option<String>)>,
        id: AssetId,
    ) -> ResourceState {
        outcome
            .get(&id)
            .map(|(state, _)| *state)
            .expect("record in outcome")
    }

    #[test]
    fn propagate_missing_dep_invalidates_consumer() {
        let root = temp_root("prop-missing");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let tex = db
            .register("assets/textures/player.png", ResourceKind::Texture, "t1")
            .unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "s1")
            .unwrap();
        db.set_dependencies(scene, vec![tex]).unwrap();
        db.get_mut_for_test(tex).status = ResourceState::Missing;
        db.get_mut_for_test(scene).status = ResourceState::Ready;
        let outcome = propagate(&record_snapshot(&db));
        assert_eq!(
            applied_state(&outcome, scene),
            ResourceState::Invalid,
            "依赖源缺失 → 消费者 Invalid"
        );
        let error = outcome[&scene].1.as_deref().unwrap();
        assert!(error.contains("缺失"), "{error}");
        assert_eq!(applied_state(&outcome, tex), ResourceState::Missing);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn propagate_import_failed_dep_invalidates() {
        let root = temp_root("prop-import-failed");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let tex = db
            .register("assets/textures/player.png", ResourceKind::Texture, "t1")
            .unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "s1")
            .unwrap();
        db.set_dependencies(scene, vec![tex]).unwrap();
        db.get_mut_for_test(tex).status = ResourceState::ImportFailed;
        db.get_mut_for_test(scene).status = ResourceState::Ready;
        let outcome = propagate(&record_snapshot(&db));
        assert_eq!(applied_state(&outcome, scene), ResourceState::Invalid);
        let error = outcome[&scene].1.as_deref().unwrap();
        assert!(error.contains("导入失败"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn propagate_stale_dep_marks_stale() {
        let root = temp_root("prop-stale");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let tex = db
            .register("assets/textures/player.png", ResourceKind::Texture, "t1")
            .unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "s1")
            .unwrap();
        db.set_dependencies(scene, vec![tex]).unwrap();
        db.get_mut_for_test(tex).status = ResourceState::Stale;
        db.get_mut_for_test(scene).status = ResourceState::Ready;
        let outcome = propagate(&record_snapshot(&db));
        assert_eq!(applied_state(&outcome, scene), ResourceState::Stale);
        let error = outcome[&scene].1.as_deref().unwrap();
        assert!(error.contains("已变更"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn propagate_restore_revokes_invalid() {
        let root = temp_root("prop-restore");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let tex = db
            .register("assets/textures/player.png", ResourceKind::Texture, "t1")
            .unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "s1")
            .unwrap();
        db.set_dependencies(scene, vec![tex]).unwrap();
        db.get_mut_for_test(scene).status = ResourceState::Ready;
        db.get_mut_for_test(scene).content_hash = "cafe".into();
        db.get_mut_for_test(tex).status = ResourceState::Missing;
        let first = propagate(&record_snapshot(&db));
        assert_eq!(applied_state(&first, scene), ResourceState::Invalid);
        // 模拟 propagate_invalidation 写回后，依赖文件恢复（discovered→ready）。
        db.get_mut_for_test(scene).status = ResourceState::Invalid;
        db.get_mut_for_test(tex).status = ResourceState::Ready;
        let second = propagate(&record_snapshot(&db));
        assert_eq!(
            applied_state(&second, scene),
            ResourceState::Ready,
            "依赖恢复后撤销 Invalid"
        );
        assert_eq!(applied_state(&second, tex), ResourceState::Ready);
        assert!(second[&scene].1.is_none(), "撤销时清空 error");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn propagate_self_missing_keeps_missing() {
        let root = temp_root("prop-self-missing");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let tex = db
            .register("assets/textures/player.png", ResourceKind::Texture, "t1")
            .unwrap();
        let scene = db
            .register("scenes/main.scene", ResourceKind::Scene, "s1")
            .unwrap();
        db.set_dependencies(scene, vec![tex]).unwrap();
        // scene 自身源也缺失（rank 7 最强），不因依赖缺失变 Invalid。
        db.get_mut_for_test(tex).status = ResourceState::Missing;
        db.get_mut_for_test(scene).status = ResourceState::Missing;
        let outcome = propagate(&record_snapshot(&db));
        assert_eq!(
            applied_state(&outcome, scene),
            ResourceState::Missing,
            "自身缺失优先级高于依赖"
        );
        assert_eq!(applied_state(&outcome, tex), ResourceState::Missing);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn propagate_chain_a_to_c() {
        let root = temp_root("prop-chain");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let a = db
            .register("assets/textures/player.png", ResourceKind::Texture, "t1")
            .unwrap();
        let b = db
            .register("assets/atlases/ui.atlas", ResourceKind::Atlas, "t1")
            .unwrap();
        let c = db
            .register("scenes/main.scene", ResourceKind::Scene, "s1")
            .unwrap();
        db.set_dependencies(b, vec![a]).unwrap();
        db.set_dependencies(c, vec![b]).unwrap();
        db.get_mut_for_test(a).status = ResourceState::Missing;
        db.get_mut_for_test(b).status = ResourceState::Ready;
        db.get_mut_for_test(c).status = ResourceState::Ready;
        let outcome = propagate(&record_snapshot(&db));
        assert_eq!(applied_state(&outcome, b), ResourceState::Invalid);
        assert_eq!(
            applied_state(&outcome, c),
            ResourceState::Invalid,
            "B 失效级联到 C"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn propagate_cycle_terminates() {
        let root = temp_root("prop-cycle");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let a = db
            .register("scenes/a.scene", ResourceKind::Scene, "s1")
            .unwrap();
        let b = db
            .register("scenes/b.scene", ResourceKind::Scene, "s1")
            .unwrap();
        db.set_dependencies(a, vec![b]).unwrap();
        db.set_dependencies(b, vec![a]).unwrap();
        db.get_mut_for_test(a).status = ResourceState::Ready;
        db.get_mut_for_test(b).status = ResourceState::Ready;
        let outcome = propagate(&record_snapshot(&db));
        assert_eq!(applied_state(&outcome, a), ResourceState::Ready);
        assert_eq!(applied_state(&outcome, b), ResourceState::Ready);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cycle_detection_reports_chain() {
        let root = temp_root("cycle-detect");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let a = db
            .register("scenes/a.scene", ResourceKind::Scene, "s1")
            .unwrap();
        let b = db
            .register("scenes/b.scene", ResourceKind::Scene, "s1")
            .unwrap();
        db.set_dependencies(a, vec![b]).unwrap();
        db.set_dependencies(b, vec![a]).unwrap();
        let cycles = detect_cycles(&record_snapshot(&db));
        assert_eq!(cycles.len(), 1, "恰好一个环: {cycles:?}");
        assert_eq!(cycles[0].len(), 2);
        assert!(
            cycles[0].contains(&a) && cycles[0].contains(&b),
            "{:?}",
            cycles[0]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cycle_error_preserved_when_ready() {
        let root = temp_root("cycle-error");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let a = db
            .register("scenes/a.scene", ResourceKind::Scene, "s1")
            .unwrap();
        let b = db
            .register("scenes/b.scene", ResourceKind::Scene, "s1")
            .unwrap();
        db.set_dependencies(a, vec![b]).unwrap();
        db.set_dependencies(b, vec![a]).unwrap();
        db.get_mut_for_test(a).status = ResourceState::Ready;
        db.get_mut_for_test(b).status = ResourceState::Ready;
        let outcome = propagate(&record_snapshot(&db));
        let error = outcome[&a].1.as_deref().unwrap();
        assert!(error.contains("循环依赖"), "{error}");
        assert!(outcome[&b].1.as_deref().unwrap().contains("循环依赖"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
