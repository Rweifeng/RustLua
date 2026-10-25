//! 依赖图：断链检查（`--check-dependencies`）与发布依赖索引的生成。
//!
//! 从 `lib.rs` 纯搬运而来（M77-05 §2），行为未变。
use anyhow::anyhow;
use rustlua_assets::dependency::{
    DependencyRef, extract_refs, module_path, scan_path_call_args, scene_path,
};
use rustlua_assets::resource_db::{AssetId, ResourceDatabase, ResourceState};
use rustlua_assets::resource_kind::ResourceKind;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

/// 发布检查发现的断链：`consumer` 是直接消费者，`chain` 是
/// `消费者 → … → 根断链资源` 的完整链路（含消费者自身）。
use crate::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrokenLink {
    pub consumer: PathBuf,
    pub chain: Vec<PathBuf>,
    pub reason: String,
}

/// 依赖健康度检查：先注册并传播失效，再对每个 Invalid 消费者沿依赖下探到
/// 断链根，输出完整链路（如 `scenes/main.scene → assets/textures/player.png
/// (缺失)`）。成环时截断并标注循环依赖。健康项目返回空列表。
pub fn check_dependencies(project: &Path) -> anyhow::Result<Vec<BrokenLink>> {
    ensure_project(project)?;
    // 注册 + reconcile + 依赖重算 + 失效传播，让数据库反映最新状态。
    scan(project)?;
    let database = ResourceDatabase::open(project)?;
    let mut links = Vec::new();
    for record in database.iter() {
        if record.status != ResourceState::Invalid {
            continue;
        }
        let Some((chain, reason)) = walk_broken_chain(&database, record.id) else {
            continue;
        };
        let chain_paths = chain
            .iter()
            .map(|id| {
                database
                    .get(*id)
                    .map(|node| node.source_path.clone())
                    .unwrap_or_else(|| PathBuf::from(id.to_string()))
            })
            .collect();
        links.push(BrokenLink {
            consumer: record.source_path.clone(),
            chain: chain_paths,
            reason,
        });
    }
    links.sort_by(|a, b| a.consumer.cmp(&b.consumer));
    Ok(links)
}

/// 断链依赖在链路遍历中的权值（同传播语义：Missing/Orphaned/ImportFailed
/// 折叠为 broken-link，与 Invalid 同级）。
fn break_rank(status: ResourceState) -> u8 {
    match status {
        ResourceState::Invalid
        | ResourceState::Missing
        | ResourceState::Orphaned
        | ResourceState::ImportFailed => 6,
        _ => 0,
    }
}

/// 沿依赖下探到断链根：只沿 Invalid 依赖继续，遇到 Missing/Orphaned/
/// ImportFailed 即叶子；依赖健康却仍 Invalid 时保守返回自身。成环截断并
/// 标注循环依赖，避免无限递归。
fn walk_broken_chain(db: &ResourceDatabase, start: AssetId) -> Option<(Vec<AssetId>, String)> {
    let mut chain = vec![start];
    let mut current = start;
    let mut seen = HashSet::new();
    seen.insert(start);
    loop {
        let record = db.get(current)?;
        let worst = record
            .dependencies
            .iter()
            .filter_map(|dep| {
                let dep_record = db.get(*dep)?;
                (break_rank(dep_record.status) > 0).then_some((*dep, dep_record.status))
            })
            .max_by_key(|(_, status)| break_rank(*status));
        let Some((next, next_status)) = worst else {
            return Some((chain, "无效".to_owned()));
        };
        if !seen.insert(next) {
            chain.push(next);
            return Some((chain, "循环依赖".to_owned()));
        }
        chain.push(next);
        match next_status {
            ResourceState::Invalid => current = next,
            ResourceState::Missing => return Some((chain, "缺失".to_owned())),
            ResourceState::Orphaned => return Some((chain, "已失效".to_owned())),
            ResourceState::ImportFailed => return Some((chain, "导入失败".to_owned())),
            _ => return Some((chain, "无效".to_owned())),
        }
    }
}

/// Emit the static resource graph used by synchronous runtimes to avoid
/// loading unrelated package entries. Any dynamic/unsupported resource access
/// marks its source opaque so consumers can conservatively preload everything.
pub(crate) fn write_dependency_index(project: &Path, destination: &Path) -> anyhow::Result<()> {
    let mut files = Vec::new();
    for root in ["scenes", "scripts", "assets"] {
        let dir = project.join(root);
        if dir.is_dir() {
            collect_files(&dir, &mut files)?;
        }
    }
    let mut known = BTreeMap::<String, String>::new();
    let mut sources = Vec::new();
    for (path, _) in files {
        let relative = path
            .strip_prefix(project)
            .map_err(|_| anyhow!("依赖索引源文件不在项目目录内：{}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let folded = relative.to_ascii_lowercase();
        known.insert(folded, relative.clone());
        sources.push((relative, path));
    }

    let mut edges = BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    let mut opaque = std::collections::BTreeSet::new();
    for (source, path) in sources {
        let kind = ResourceKind::from_path(&path);
        if !matches!(
            kind,
            ResourceKind::Scene
                | ResourceKind::Prefab
                | ResourceKind::Atlas
                | ResourceKind::LuaScript
        ) {
            continue;
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => {
                opaque.insert(source);
                continue;
            }
        };
        // These APIs consume runtime paths synchronously. Most call sites pass a literal
        // (often through a same-file constant), which is provable; only the ones that stay
        // unresolvable after that keep the conservative full-package fallback.
        //
        // `audio.*` 曾经也在这张表里，但那条判据是反的：音频在运行期由宿主自己按名字取回
        // （浏览器 `pack/assets/audio/<name>.mp3`、微信走适配层路由），**不读**同步 vfs 预载表。
        // 把它算进"必须证明完整"的同步读取里，代价是每个调用 `audio.sfx(...)` 的脚本都被标
        // opaque——本工程 155 个 Lua 里有 88 个，全量预载的触发点正是这里。
        let unresolved_calls = if kind == ResourceKind::LuaScript {
            scan_path_call_args(&text)
        } else {
            Vec::new()
        };
        // 字面量截断（`"assets/ui/prefabs/"`、`"assets/audio/"`）本身不能定位文件，但当它有
        // 包内命中时，那些命中就是可证的边——否则一个目录级常量会把它所在文件整片标成 opaque。
        let prefix_refs = if kind == ResourceKind::LuaScript {
            asset_prefix_refs(&text)
        } else {
            Vec::new()
        };
        // 同文件字面量常量表：`local PRESET_PATH = "assets/prefabs/dialogue.prefab"` 这类写法
        // 让"变量实参"其实完全可证。非 Lua 源没有调用实参，表留空即可。
        let constants = if kind == ResourceKind::LuaScript {
            assigned_literals(&text)
        } else {
            BTreeMap::new()
        };
        let mut refs = extract_refs(&text, kind);
        let resolved = if kind == ResourceKind::LuaScript {
            let mut paths = Vec::new();
            for argument in &unresolved_calls {
                if let Some(path) = resolve_static_path(argument, &constants, &known) {
                    paths.push(path);
                }
            }
            for prefix in &prefix_refs {
                paths.extend(expand_asset_prefix(prefix, &known));
            }
            paths
        } else {
            Vec::new()
        };
        for path in resolved {
            let reference = DependencyRef::Asset(std::path::PathBuf::from(path));
            if !refs.contains(&reference) {
                refs.push(reference);
            }
        }
        let mut targets = std::collections::BTreeSet::new();
        for reference in refs {
            let candidate = match reference {
                DependencyRef::Asset(path) => path.to_string_lossy().replace('\\', "/"),
                DependencyRef::LuaModule(name) => module_path(&name),
                DependencyRef::SceneName(name) => scene_path(&name),
                DependencyRef::Dynamic(_) => {
                    opaque.insert(source.clone());
                    continue;
                }
            };
            if let Some(target) = known
                .get(&candidate.to_ascii_lowercase())
                .or_else(|| known.get(&candidate))
            {
                if target != &source {
                    targets.insert(target.clone());
                }
            } else if candidate.contains("assets/")
                || candidate == "project.toml"
                || candidate == "chars.txt"
            {
                // 根级工程文件的字面量引用是合法的（本索引只扫 scenes/scripts/assets，
                // 它们在包内但不在 known 里）；而 `assets/` 路径不是"引用不存在的东西"，
                // 只是这个文件不随包分发（例：M71 退役后已删除的 `assets/ui/prefabs/*.ui.json`），
                // 运行期同样读不到——两种情况都不需要把整包拖成全量预载。
                continue;
            } else {
                opaque.insert(source.clone());
            }
        }
        if targetless_asset_args(&unresolved_calls, &constants, &known) {
            // 实参确实是 `assets/` 路径却谁也解析不出来：索引无法证明闭包完整，回退全量。
            opaque.insert(source.clone());
        }
        if !targets.is_empty() {
            edges.insert(source, targets);
        }
    }

    let index = DependencyIndex {
        format: 1,
        edges: edges
            .into_iter()
            .map(|(source, targets)| (source, targets.into_iter().collect()))
            .collect(),
        opaque: opaque.into_iter().collect(),
    };
    let bytes =
        serde_json::to_vec(&index).map_err(|error| anyhow!("序列化依赖索引失败：{error}"))?;
    fs::write(destination, bytes)?;
    Ok(())
}

/// 同文件字面量常量表：`local PRESET_PATH = "assets/prefabs/dialogue.prefab"` → `PRESET_PATH`。
///
/// 只做这一种形态（标识符 = 纯字符串字面量），不做表达式求值：拼接、表字段、函数返回值
/// 一律留在"证不出来"那一侧，由调用方标 opaque 走全量兜底。这样静态分析的能力边界是
/// 一句话能说完的，而不是"基本能猜"。
fn assigned_literals(text: &str) -> BTreeMap<String, String> {
    // `==` 的左侧不是赋值：不排掉的话 `if M.DIR == "assets/ui/"` 会造出一条假常量。
    let text = text.replace("==", "!=");
    let chars: Vec<char> = text.chars().collect();
    let mut literals = BTreeMap::new();
    for quote in ['"', '\''] {
        let mut search = 0;
        while let Some(offset) = text[search..].find(quote) {
            let start = search + offset;
            search = start + 1;
            let Some(body) = literal_at(&chars, start) else {
                continue;
            };
            if let Some(name) = assignment_name(&chars, start) {
                literals.insert(name, body);
            }
        }
    }
    literals
}

/// 位置 `start` 处是一个字符串字面量时返回去引号的正文（含转义原样保留）。
fn literal_at(chars: &[char], start: usize) -> Option<String> {
    let quote = *chars.get(start)?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut index = start + 1;
    let mut body = String::new();
    while index < chars.len() {
        let c = chars[index];
        if c == '\\' {
            if let Some(&escaped) = chars.get(index + 1) {
                body.push(escaped);
                index += 2;
                continue;
            }
            break;
        }
        if c == quote {
            return Some(body);
        }
        body.push(c);
        index += 1;
    }
    None
}

/// 字面量之前紧邻 `=`（且不是 `==`）时，反向读出被赋值的名字。
fn assignment_name(chars: &[char], quote_index: usize) -> Option<String> {
    let mut index = quote_index;
    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }
    if index == 0 || chars[index - 1] != '=' {
        return None;
    }
    index -= 1;
    if index > 0 && chars[index - 1] == '=' {
        return None;
    }
    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }
    let end = index;
    while index > 0 && (chars[index - 1].is_alphanumeric() || matches!(chars[index - 1], '_' | '.'))
    {
        index -= 1;
    }
    (index < end).then(|| chars[index..end].iter().collect())
}

/// 调用实参 → 包内路径。`"assets/…"` 直接归一化；标识符查同文件常量表；查到的值再归一化一次。
fn resolve_static_path(
    argument: &str,
    constants: &BTreeMap<String, String>,
    known: &BTreeMap<String, String>,
) -> Option<String> {
    let trimmed = argument.trim();
    if let Some(value) = literal_body(trimmed) {
        return normalize_asset_path(&value, known);
    }
    let name = trimmed.trim_end_matches([')', ',', ';']);
    let value = constants.get(name)?;
    normalize_asset_path(value, known)
}

/// 去引号（`trim_matches` 与扫描器同口径：首尾成对的引号，中间不再含引号）。
fn literal_body(trimmed: &str) -> Option<String> {
    if trimmed.len() < 2 || !trimmed.starts_with(['"', '\'']) {
        return None;
    }
    let body = trimmed.trim_matches(['"', '\'']);
    (body != trimmed || trimmed.len() == 2).then(|| body.to_owned())
}

/// 绝对化的**包内**资源路径才算边：包外路径（用户目录、`/tmp/...`）运行期同样取不到，
/// 不算"引用不存在的东西"。大小写回落到项目里的真实写法（索引与清单按原样比对）。
fn normalize_asset_path(value: &str, known: &BTreeMap<String, String>) -> Option<String> {
    let trimmed = value.trim();
    let index = trimmed.find("assets/")?;
    let candidate = trimmed[index..].trim_end_matches([';', ':', '\n', '\r']);
    known
        .get(&candidate.to_ascii_lowercase())
        .or_else(|| known.get(candidate))
        .cloned()
}

/// 文件里出现的、以 `assets/` 开头但**在引用处就截断**的字符串前缀（`"assets/ui/prefabs/"`）。
///
/// 只收以 `/` 结尾的整串字面量：完整路径由 `lua_string_refs` 的 `assets/` 分支产出，
/// 拼接中段（`"assets/" .. kind .. "/x.png"`）也在这里产出，两者都可能不是完整路径，
/// 交给 `expand_asset_prefix` 按包内命中判定；命中不了就丢弃（同"不存在"口径）。
pub(crate) fn asset_prefix_refs(text: &str) -> Vec<String> {
    let mut prefixes = Vec::new();
    for literal in string_literals(text) {
        if literal.starts_with("assets/")
            && literal.ends_with('/')
            // 仅 `"assets/"`（无下钻）不算目录引用：它是**校验用的字面量**
            // （`string.find(entry.pattern, "assets/", 1, true)` 这类"路径确实是工程相对资产路径吗"
            // 的判据），展开它等于把整包挂成这个文件闭包的一部分。
            // 2026-10-17 实测：`scripts/data/season_art.lua:114` 的这一个字面量让它的边涨到
            // 1,207 条（整棵 assets 树、含 17.7 MB 的已停用字体），运行期的闭包选择因此永远
            // 走不到"少量首屏资源"那条路。目录引用至少给到一级子目录才有信息量。
            && literal.trim_end_matches('/').len() > "assets".len()
            && !prefixes.contains(&literal)
        {
            prefixes.push(literal);
        }
    }
    prefixes
}

/// 收集全部字符串字面量的正文（引号感知，`--` 注释直接跳过，行为与扫描器同源）。
fn string_literals(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut literals = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c == '-' && chars.get(index + 1) == Some(&'-') {
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if c == '"' || c == '\'' {
            if let Some(body) = literal_at(&chars, index) {
                literals.push(body);
            }
            index += 1;
            continue;
        }
        index += 1;
    }
    literals
}

/// 前缀 → 包内命中全集（`"assets/ui/prefabs/"` → `assets/ui/prefabs/*.ui.json`）。
///
/// 目录常量 + 变量名拼出的路径**只可能**落在该目录下，所以这些命中是完整闭包而不是猜测；
/// 命中集为空时返回空（该前缀指的是包内不存在的东西）。
fn expand_asset_prefix(prefix: &str, known: &BTreeMap<String, String>) -> Vec<String> {
    let lowered = prefix.to_ascii_lowercase();
    known
        .iter()
        .filter(|(folded, _)| folded.starts_with(&lowered))
        .map(|(_, actual)| actual.clone())
        .collect()
}

/// 实参已能证明是 `assets/` 路径、却既不完整也不指向任何包内文件：
/// 这才是真的"索引证明不了闭包完整"，必须回退全量。
fn targetless_asset_args(
    unresolved: &[String],
    constants: &BTreeMap<String, String>,
    known: &BTreeMap<String, String>,
) -> bool {
    unresolved.iter().any(|argument| {
        let trimmed = argument.trim();
        let text = match literal_body(trimmed) {
            Some(value) => value,
            None => match constants.get(trimmed.trim_end_matches([')', ',', ';'])) {
                Some(value) => value.clone(),
                None => return false,
            },
        };
        // 绝对路径（`/…`）不属于包内资源，运行期同样读不到，不算索引的缺口。
        text.contains("assets/")
            && !text.starts_with('/')
            && normalize_asset_path(&text, known).is_none()
    })
}
