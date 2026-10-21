//! RLB v1 资源包容器（`docs/67` §4）：一个 bundle = 多个条目 + 块压缩 + 尾部索引。
//!
//! 存在理由与 `pack.rs` 的分工：
//! - [`crate::pack`] 是**逐文件**容器（`RustLua` 头 + gzip + AEAD），服务 loose/开发形态；
//! - 本模块是**多条目**容器，服务发布形态：把上千个小文件收进一个文件，
//!   用块压缩吃掉跨文件冗余，并给随机访问留索引。
//!
//! 压缩分工（wasm 是硬门槛，见 `docs/67` §4.3.1）：
//! - `bundle-write` feature（只在 `assetc` 打开）用 C 版 `zstd` 编码；
//! - 解码用纯 Rust `ruzstd`，因此引擎/`wasm32-unknown-unknown` 里不需要 C 工具链。
//! - 所以 `zstd` 是 **optional** 依赖：任何不写包的 crate（含 wasm 产物）都不会把它编进来。
//!
//! 字节布局：
//!
//! ```text
//! ┌──────────────────── 偏移 0，32 B 定长头 ────────────────────┐
//! │ "RLBU"(4) ver u16 flags u16 block_size u32 index_offset u64 │
//! │ index_len u32 index_raw_len u32 header_fnv u32              │
//! ├──────────────── payload：块 0 │ 块 1 │ … （紧排，无填充）────┤
//! ├──────────────── index：块表 + 条目表（可整体 zstd 压）───────┤
//! └──────────────── 无尾标：索引位置由头部给出 ──────────────────┘
//! ```
//!
//! 不变量（写入端保证、读取端校验）：
//! 1. 条目按**路径字典序**排列，块内顺序与条目表顺序一致（=> 确定性 + 同目录相邻）；
//! 2. 条目**不跨块**；超过 `block_size` 的条目独占自己的块（`raw_len` 可超 `block_size`）；
//! 3. 块偏移由 `stored_len` 前缀和推出，不单独存；
//! 4. 任何块都可以选择"不压"（`flags.bit0 = 0`），此时 `stored_len == raw_len`。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// 容器魔数。`pack::decode` 见到它会明确报错（防止把 bundle 当明文资源读）。
pub const BUNDLE_MAGIC: &[u8; 4] = b"RLBU";
/// 当前格式版本。
pub const BUNDLE_VERSION: u16 = 1;
/// 默认块大小（解压后）。
pub const DEFAULT_BLOCK_SIZE: u32 = 256 * 1024;
/// 头长度（固定）。
pub const HEADER_LEN: usize = 32;
/// 头标志：索引已压缩。
const HEADER_FLAG_INDEX_COMPRESSED: u16 = 1;
/// 块标志：已压缩。
pub const BLOCK_FLAG_COMPRESSED: u8 = 1;
/// 块标志：大像素块（写入端用 L19 档压）。
pub const BLOCK_FLAG_LARGE: u8 = 2;
/// 块标志：已加密（批 F，v1 只保留位）。
pub const BLOCK_FLAG_ENCRYPTED: u8 = 4;
/// 条目标志：`.rltex` 裸像素纹理。
pub const ENTRY_FLAG_RLTEX: u8 = 1;
/// 条目标志：条目自身已压过，容器不必再压（写入端预留）。
pub const ENTRY_FLAG_PRECOMPRESSED: u8 = 2;
/// 索引前缀：zstd 版本号 + salt。
const INDEX_PROLOGUE_LEN: usize = 8;
/// 索引里一条块记录的固定长度：1 标志 + 4 落盘长 + 4 原始长。
const BLOCK_RECORD_LEN: u64 = 9;
/// 索引里一条条目记录的**最小**长度（名字为空时）：2 名字长 + 1 标志 + 4 块号 +
/// 4 偏移 + 4 长度 + 8 校验和。名字本身另外按实际字节算。
const ENTRY_RECORD_MIN_LEN: u64 = 23;
/// 写入端对大块启用 L19 的阈值（解压后字节）。只被 `bundle-write` 用到。
#[cfg(feature = "bundle-write")]
const LARGE_BLOCK_THRESHOLD: u32 = 1024 * 1024;

/// 块级 AEAD 的 salt（`docs/67` §4.4 批 F）。
///
/// **与文档的偏差，必须记下来**：文档写"nonce[0..4] = header.salt（构建期取 index_fnv 低 4 字节）"，
/// 但索引是在块处理**之后**才建出来的——先加密块、后建索引，拿 index_fnv 当 salt 是循环依赖。
/// 这里改成对**条目表**（名字 + raw_len，天然已排序）取 FNV：构建初期就能算、读侧解析完索引
/// 也能重算，且同样"由内容决定、不引入随机"（确定性要求见 §4.5）。
///
/// 只在写端（`bundle-write`）用得到：读端把 salt 当黑盒从索引头读回来。
#[cfg(feature = "bundle-write")]
fn bundle_salt<'a, I>(entries: I) -> u32
where
    I: Iterator<Item = (&'a str, u32)>,
{
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for (name, raw_len) in entries {
        for byte in name.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        for byte in raw_len.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash as u32
}

/// 块 AEAD 的 nonce：`salt(4) ‖ 块号(8)`，小端。
fn block_nonce(salt: u32, number: u32) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[0..4].copy_from_slice(&salt.to_le_bytes());
    nonce[4..12].copy_from_slice(&u64::from(number).to_le_bytes());
    nonce
}

/// 块 AEAD 的 AAD：`ver ‖ 块 flags（不含加密位） ‖ block_size ‖ 块号 ‖ raw_len`。
///
/// 把块号与 raw_len 绑进去，是为了让"换块/换包重放"必然失败：把两个块的密文互换，
/// 各自的 nonce 与 AAD 都对不上。
fn block_aad(flags: u8, block_size: u32, number: u32, raw_len: u32) -> [u8; 20] {
    let mut aad = [0u8; 20];
    aad[0..2].copy_from_slice(&BUNDLE_VERSION.to_le_bytes());
    aad[2] = flags & !BLOCK_FLAG_ENCRYPTED;
    aad[3..7].copy_from_slice(&block_size.to_le_bytes());
    aad[7..15].copy_from_slice(&u64::from(number).to_le_bytes());
    aad[15..19].copy_from_slice(&raw_len.to_le_bytes());
    aad
}
/// 块缓存的常规容量（`docs/67` 复核 P1-3）。
const CACHE_BLOCKS: usize = 4;

/// 资源包错误。
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("资源包头损坏：{0}")]
    Header(String),
    #[error("不支持的资源包版本 {0}（当前支持 {BUNDLE_VERSION}）")]
    UnsupportedVersion(u16),
    #[error("资源包索引损坏：{0}")]
    Index(String),
    #[error("块 {block} 解码失败：{reason}")]
    BlockDecode { block: u32, reason: String },
    #[error("条目 {path} 越界（块 {block} 内 offset={offset} len={len}，块长 {block_len}）")]
    EntryOutOfBlock {
        path: String,
        block: u32,
        offset: u32,
        len: u32,
        block_len: u32,
    },
    #[error("条目 {path} 校验和失配（期望 {expected:016x}，实得 {actual:016x}）")]
    Checksum {
        path: String,
        expected: u64,
        actual: u64,
    },
    #[error("读取资源包数据失败：{0}")]
    Source(String),
    #[error("写入资源包失败：{0}")]
    Write(String),
    #[error("条目路径重复：{0}")]
    DuplicateEntry(String),
    #[error("条目路径非法：{0:?}")]
    InvalidPath(String),
}

/// FNV-1a 64（与 `manifest.txt` 同一算法：清单与条目校验和对得上）。
pub fn checksum(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// `bundle_plan.json`（`docs/67` §5.5）：构建期由 `assetc` 写、运行期由宿主读。
///
/// 放在 infra 而不是 assetc，是因为它必须"两边同一份定义"：`game` 不能依赖 `assetc`
/// （构建工具不进运行器），而计划表的字段一旦两边各写一份就会漂移。
pub mod plan {
    use serde::{Deserialize, Serialize};

    /// 计划文件名（放在包根，与 `manifest.txt` 同级）。
    pub const PLAN_FILE: &str = "bundle_plan.json";
    /// 格式标识（改动字段含义时自增）。
    pub const PLAN_FORMAT: &str = "assetc-bundle-plan-1";

    /// 一个组的计划条目。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct PlannedGroup {
        pub name: String,
        /// 分发单元：`main` / `sub` / `cdn`。
        pub unit: String,
        /// 容器在包内的相对路径（`bundles/<name>-<hash8>.rlb`）。
        pub bundle: String,
        /// 容器内容的 FNV-1a（16 位十六进制）。**增量分发只比这个**：
        /// 文件名里虽然也带 8 位，但那是截断值，而且名字一改就没人记得住当初的语义。
        #[serde(default)]
        pub hash: String,
        pub entries: u64,
        pub raw_bytes: u64,
        pub bundle_bytes: u64,
        /// 组内的工程相对路径（升序）。运行时用它把"未挂载的组"变成可归因的缺件。
        pub paths: Vec<String>,
    }

    impl PlannedGroup {
        /// 路径是否属于本组（`paths` 升序，二分）。
        pub fn contains(&self, path: &str) -> bool {
            self.paths
                .binary_search_by(|item| item.as_str().cmp(path))
                .is_ok()
        }
    }

    /// `bundle_plan.json` 的根结构。
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct BundlePlan {
        pub schema: u32,
        pub format: String,
        pub block_size: u32,
        pub groups: Vec<PlannedGroup>,
        /// 计划表自校验（复核 P2）：对 `groups` 的规范化 JSON 取 FNV-1a 64，十六进制。
        ///
        /// 为什么需要：计划表是**缺件归因的唯一依据**。它被截断、被旧版本覆盖、或与容器不同步时，
        /// 症状是"某些资源突然读不到，而且报不出是哪个包"——比直接报错难查一个量级。
        /// 空串表示旧计划（不校验），保证向后可读。
        #[serde(default)]
        pub checksum: String,
        /// 必须**留在目录里**的工程相对路径（打包期 `--keep` 命中的条目，典型是微信字体）。
        ///
        /// 进这里就等于一条发布期契约，由 `assetc closure` 断言：
        /// - 每个路径在发布根里必须是**真实文件**（宿主 `wx.loadFont` 只吃本地真实路径，
        ///   被塞进容器就加载失败）；
        /// - 且**不得**出现在任何容器索引里（出现了说明 `--keep` 没生效：同一份资源双份占配额）。
        ///
        /// 只记**具体路径**而不是前缀规则：前缀是构建参数，会随版本变；契约必须随产物走。
        ///
        /// 为什么是 `Option` 而不是空 `Vec`：`None` = 这份计划是**本字段之前**的版本生成的
        /// （那时还没有 keep 元数据），验证端据此知道"没有契约可查"，而不是把"旧计划"误读成
        /// "声明了零条 keep"。新版本打出来的计划**总是**带这个键（哪怕是 `[]`）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub keep: Option<Vec<String>>,
    }

    impl BundlePlan {
        /// 解析计划表；格式标识不认就报错（不猜）。
        pub fn parse(bytes: &[u8]) -> Result<Self, String> {
            let plan: Self = serde_json::from_slice(bytes)
                .map_err(|error| format!("bundle_plan 解析失败：{error}"))?;
            if plan.format != PLAN_FORMAT {
                return Err(format!(
                    "bundle_plan 格式不认：{}（期望 {PLAN_FORMAT}）",
                    plan.format
                ));
            }
            // 结构自检**与 checksum 无关**：手改过/旧版本的计划表 checksum 可能是空的，
            // 但"组名重复/容器被两组声明/unit 写错"这类结构错在任何版本里都是歧义——
            // 允许它通过等于让读取端自己猜（猜错的症状是白图，不是报错）。
            if plan.schema != 1 {
                return Err(format!(
                    "bundle_plan schema 不支持：{}（期望 1）",
                    plan.schema
                ));
            }
            if plan.block_size == 0 {
                return Err("bundle_plan 的 block_size 不能为 0".to_owned());
            }
            if plan.groups.is_empty() {
                return Err("bundle_plan 没有 groups".to_owned());
            }
            let mut names = std::collections::BTreeSet::new();
            let mut bundles = std::collections::BTreeSet::new();
            for group in &plan.groups {
                if !names.insert(group.name.as_str()) {
                    return Err(format!("bundle_plan 组名重复：{}", group.name));
                }
                if !group.bundle.starts_with("bundles/") || group.bundle.len() <= "bundles/".len() {
                    return Err(format!(
                        "bundle_plan 组 {} 的容器路径非法：{}（必须在 bundles/ 下）",
                        group.name, group.bundle
                    ));
                }
                if !bundles.insert(group.bundle.as_str()) {
                    return Err(format!("bundle_plan 容器被多个组声明：{}", group.bundle));
                }
                crate::bundle_plan::parse_unit(&group.unit).map_err(|error| {
                    format!("bundle_plan 组 {} 的 unit 非法：{error}", group.name)
                })?;
                // 组内路径：必须是**规范形式 + 严格升序**。读取端用二分查
                // （`PlannedGroup::contains`），乱序或非规范形式不会报错，只会**静默查不到**——
                // 症状是"包里有这个文件，运行期说没有"，即首屏白图。这类错位以前只能在
                // 构建期 `check_plan` ⑤ 挡住，而运行期读的可能是别处传来的计划表。
                let mut previous: Option<&str> = None;
                for path in &group.paths {
                    if crate::paths::normalize_project_path(path).as_deref() != Some(path.as_str())
                    {
                        return Err(format!(
                            "bundle_plan 组 {} 的路径不是规范形式：{path:?}（必须是 `/` 分隔的工程相对路径）",
                            group.name
                        ));
                    }
                    if previous.is_some_and(|prev| prev >= path.as_str()) {
                        return Err(format!(
                            "bundle_plan 组 {} 的路径没有严格升序：{path:?}（乱序/重复会让二分查静默失效）",
                            group.name
                        ));
                    }
                    previous = Some(path.as_str());
                }
            }
            // keep 条目：必须是规范形式、不重复、且不能同时又列在某个组里
            // （同时出现说明 `--keep` 没生效：容器里一份、目录里一份，白占配额）。
            let mut kept = std::collections::BTreeSet::new();
            for path in plan.keep.iter().flatten() {
                if crate::paths::normalize_project_path(path).as_deref() != Some(path.as_str()) {
                    return Err(format!(
                        "bundle_plan 的 keep 路径不是规范形式：{path}（必须是 `/` 分隔的工程相对路径）"
                    ));
                }
                if !kept.insert(path.as_str()) {
                    return Err(format!("bundle_plan 的 keep 路径重复：{path}"));
                }
                if plan.groups.iter().any(|group| group.contains(path)) {
                    return Err(format!(
                        "bundle_plan 的 keep 路径同时属于某个容器组：{path}（keep 与容器只能二选一）"
                    ));
                }
            }
            // 自校验：非空就必须对得上。空串是旧计划（本字段之前生成的），放行。
            if !plan.checksum.is_empty() {
                let expected = plan.compute_checksum();
                if plan.checksum != expected {
                    return Err(format!(
                        "bundle_plan 自校验失败：记录 {}，实际 {}（计划表被截断/篡改，或与容器不是同一版）",
                        plan.checksum, expected
                    ));
                }
            }
            Ok(plan)
        }

        /// 重算自校验（对 `groups` 的规范化 JSON 取 FNV-1a 64）。
        ///
        /// 只覆盖 `groups`：`schema`/`format`/`block_size` 由格式版本本身约束，
        /// 而"计划里到底列了哪些包"才是会静默漂移的那部分。
        pub fn compute_checksum(&self) -> String {
            let canonical = serde_json::to_vec(&self.groups).unwrap_or_default();
            format!("{:016x}", crate::bundle::checksum(&canonical))
        }

        /// 写入自校验并返回自身（构建期用）。
        #[must_use]
        pub fn sealed(mut self) -> Self {
            self.checksum = self.compute_checksum();
            self
        }

        /// 组名 → 组。
        pub fn group(&self, name: &str) -> Option<&PlannedGroup> {
            self.groups.iter().find(|group| group.name == name)
        }

        /// 未挂载组 → 条目名（喂给 [`super::BundleSet::declare_pending`]）。
        ///
        /// `mounted_bundles` 是已挂载容器的**包内相对路径**（与 `PlannedGroup::bundle` 同口径）。
        pub fn pending_for(&self, mounted_bundles: &[String]) -> Vec<(String, Vec<String>)> {
            self.groups
                .iter()
                .filter(|group| {
                    !mounted_bundles
                        .iter()
                        .any(|mounted| mounted == &group.bundle)
                })
                .map(|group| (group.bundle.clone(), group.paths.clone()))
                .collect()
        }

        /// 增量分发：对比"本地已有的计划"与"远端的计划"，算出只下变化的那几个包。
        ///
        /// 语义（三条都必须对，否则要么白下要么缺文件）：
        /// 1. **远端有、本地 hash 相同** → `unchanged`（不下载）；
        /// 2. **远端有、本地没有或 hash 不同** → `download`（含新增组）；
        /// 3. **本地有、远端没有** → `remove`（旧组的残留，留着白占配额）。
        ///
        /// 按**组名**配对而不是按文件名：文件名里带 hash，改名就等于"删旧增新"，
        /// 那样每个版本都会把没变的包也删掉重下（缓存全废）。
        pub fn diff(&self, remote: &BundlePlan) -> PlanDiff {
            let local: std::collections::BTreeMap<&str, &PlannedGroup> = self
                .groups
                .iter()
                .map(|group| (group.name.as_str(), group))
                .collect();
            let remote_names: std::collections::BTreeSet<&str> = remote
                .groups
                .iter()
                .map(|group| group.name.as_str())
                .collect();

            let mut diff = PlanDiff::default();
            for group in &remote.groups {
                match local.get(group.name.as_str()) {
                    Some(existing) if existing.hash == group.hash && !group.hash.is_empty() => {
                        diff.unchanged += 1;
                    }
                    _ => diff.download.push(group.clone()),
                }
            }
            for (name, group) in &local {
                if !remote_names.contains(name) {
                    diff.remove.push((*group).clone());
                }
            }
            diff
        }

        /// 属于某个分发单元的组。
        pub fn groups_in_unit(&self, unit: &str) -> Vec<&PlannedGroup> {
            self.groups
                .iter()
                .filter(|group| group.unit == unit)
                .collect()
        }
    }

    /// 一次增量对比的结果（`docs/67` §9 批 E）。
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct PlanDiff {
        /// 需要下载/替换的组（新增 + hash 变了）。
        pub download: Vec<PlannedGroup>,
        /// 需要清理的旧组（远端已经没有了）。
        pub remove: Vec<PlannedGroup>,
        /// hash 没变、可以直接复用的组数。
        pub unchanged: usize,
    }

    impl PlanDiff {
        /// 本次要下载的字节数（远端口径）。
        pub fn download_bytes(&self) -> u64 {
            self.download.iter().map(|group| group.bundle_bytes).sum()
        }

        /// 是否什么都不用做（内容全等）。
        pub fn is_empty(&self) -> bool {
            self.download.is_empty() && self.remove.is_empty()
        }
    }
}

/// 一个条目在索引里的样子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub flags: u8,
    pub block: u32,
    pub offset: u32,
    pub len: u32,
    pub checksum: u64,
}

/// 一个块在索引里的样子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub flags: u8,
    pub stored_len: u32,
    pub raw_len: u32,
}

impl Block {
    fn is_compressed(&self) -> bool {
        self.flags & BLOCK_FLAG_COMPRESSED != 0
    }

    /// 该块是否被 AEAD 封过（`docs/67` §4.4）。
    fn is_encrypted(&self) -> bool {
        self.flags & BLOCK_FLAG_ENCRYPTED != 0
    }
}

/// 只读的包索引：由整包字节解析而来，不含块数据本身。
#[derive(Debug, Clone)]
pub struct BundleIndex {
    block_size: u32,
    entries: Vec<Entry>,
    by_path: BTreeMap<String, usize>,
    blocks: Vec<Block>,
    block_offsets: Vec<u64>,
    index_fnv: u32,
    zstd_version: u32,
    salt: u32,
}

impl BundleIndex {
    /// 解析包头与索引；不读块数据。
    pub fn parse(bytes: &[u8]) -> Result<Self, BundleError> {
        if bytes.len() < HEADER_LEN {
            return Err(BundleError::Header(format!(
                "长度 {} 不足 {HEADER_LEN} 字节",
                bytes.len()
            )));
        }
        if &bytes[0..4] != BUNDLE_MAGIC {
            return Err(BundleError::Header("魔数不是 RLBU".to_owned()));
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != BUNDLE_VERSION {
            return Err(BundleError::UnsupportedVersion(version));
        }
        let flags = u16::from_le_bytes([bytes[6], bytes[7]]);
        // v1 只定义"索引是否压缩"这一位。高位只可能来自损坏，或来自"改了格式却没升版本"——
        // 两种都不该被静默当成 0 处理（症状会是"解出来的索引莫名其妙"）。
        if flags & !HEADER_FLAG_INDEX_COMPRESSED != 0 {
            return Err(BundleError::Header(format!(
                "头标志 {flags:#06x} 含 v1 未定义的位（v1 只定义 {HEADER_FLAG_INDEX_COMPRESSED:#06x}）"
            )));
        }
        let block_size = u32::from_le_bytes(bytes[8..12].try_into().expect("4 字节"));
        // 0 会让 AAD 与块缓存判据同时失去意义；写侧最小是 1（见 `BundleBuilder::new`）。
        if block_size == 0 {
            return Err(BundleError::Header("block_size = 0".to_owned()));
        }
        let index_offset = u64::from_le_bytes(bytes[12..20].try_into().expect("8 字节"));
        let index_len = u32::from_le_bytes(bytes[20..24].try_into().expect("4 字节")) as usize;
        let index_raw_len = u32::from_le_bytes(bytes[24..28].try_into().expect("4 字节")) as usize;
        let header_fnv = u32::from_le_bytes(bytes[28..32].try_into().expect("4 字节"));
        let expected = checksum(&bytes[0..28]) as u32;
        if header_fnv != expected {
            return Err(BundleError::Header(format!(
                "header_fnv 失配（存 {header_fnv:08x}，算 {expected:08x}）"
            )));
        }
        let index_end = index_offset
            .checked_add(index_len as u64)
            .ok_or_else(|| BundleError::Header("index 偏移溢出".to_owned()))?;
        if index_end > bytes.len() as u64 {
            return Err(BundleError::Header(format!(
                "index 越界：end={index_end} > 文件长 {}",
                bytes.len()
            )));
        }
        // 写侧总把索引放在**文件末尾**（`build()` 里 `index_offset = HEADER_LEN + payload.len()`）。
        // 尾部多出字节只有两种来源：拼接了半包、或被追加了垃圾。以前这类包照样能解析——
        // 于是"多出来的那段是什么"永远没人问；现在当场点名。
        if index_end != bytes.len() as u64 {
            return Err(BundleError::Header(format!(
                "索引结束于 {index_end}，文件长 {}：尾部多出 {} B（v1 不允许尾部数据）",
                bytes.len(),
                bytes.len() as u64 - index_end
            )));
        }
        let stored_index = &bytes[index_offset as usize..index_end as usize];
        let raw_index: Vec<u8> = if flags & HEADER_FLAG_INDEX_COMPRESSED != 0 {
            zstd_decode(stored_index, index_raw_len)
                .map_err(|error| BundleError::Index(error.to_string()))?
        } else {
            stored_index.to_vec()
        };
        // 以前这里留着 `index_raw_len != 0` 的逃逸口：只要把头里的长度写成 0 就跳过交叉校验。
        // 写侧永远写真实长度（索引至少 `INDEX_PROLOGUE_LEN + 8` B，不可能为 0），
        // 所以 0 只可能是损坏——现在一律比对。
        if raw_index.len() != index_raw_len {
            return Err(BundleError::Index(format!(
                "索引解压长度 {} 与头记录的 {index_raw_len} 不符",
                raw_index.len()
            )));
        }
        let (blocks, entries) = parse_index(&raw_index)?;
        validate_layout(block_size, &blocks, &entries)?;

        let mut block_offsets = Vec::with_capacity(blocks.len());
        let mut cursor = HEADER_LEN as u64;
        for block in &blocks {
            block_offsets.push(cursor);
            cursor += u64::from(block.stored_len);
        }
        if cursor != index_offset {
            return Err(BundleError::Header(format!(
                "块区长度不符：前缀和 {cursor} != index_offset {index_offset}"
            )));
        }
        for entry in &entries {
            let block = blocks.get(entry.block as usize).ok_or_else(|| {
                BundleError::Index(format!(
                    "条目 {} 引用不存在的块 {}",
                    entry.path, entry.block
                ))
            })?;
            let end = u64::from(entry.offset) + u64::from(entry.len);
            if end > u64::from(block.raw_len) {
                return Err(BundleError::EntryOutOfBlock {
                    path: entry.path.clone(),
                    block: entry.block,
                    offset: entry.offset,
                    len: entry.len,
                    block_len: block.raw_len,
                });
            }
        }
        let mut by_path = BTreeMap::new();
        for (index, entry) in entries.iter().enumerate() {
            if by_path.insert(entry.path.clone(), index).is_some() {
                return Err(BundleError::DuplicateEntry(entry.path.clone()));
            }
        }
        let zstd_version = u32::from_le_bytes(raw_index[0..4].try_into().expect("4 字节"));
        let salt = u32::from_le_bytes(raw_index[4..8].try_into().expect("4 字节"));
        Ok(Self {
            block_size,
            entries,
            by_path,
            blocks,
            block_offsets,
            index_fnv: checksum(&raw_index) as u32,
            zstd_version,
            salt,
        })
    }

    /// 块大小（写入端记录的目标值，字节）。
    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    /// 全部条目路径，顺序与写入端一致：默认按名字升序，带优先级（`add_ranked`）时
    /// 按 `(rank, 名字)`——**不是**恒定的字典序，解析侧也不强制排序（只强制唯一）。
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.path.as_str())
    }

    /// 条目表（升序）。
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// 块表。
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// 块区字节数（不含头与索引）。
    pub fn payload_len(&self) -> u64 {
        self.blocks.iter().map(|b| u64::from(b.stored_len)).sum()
    }

    /// 有没有这个条目。
    pub fn contains(&self, path: &str) -> bool {
        self.by_path.contains_key(path)
    }

    /// 取条目元信息。
    pub fn entry(&self, path: &str) -> Option<&Entry> {
        self.by_path.get(path).map(|index| &self.entries[*index])
    }

    /// 条目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空包。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 索引自身的 FNV（低 32 位）。
    pub fn index_fnv(&self) -> u32 {
        self.index_fnv
    }

    /// 写入端记录的 zstd 版本号（仅诊断，不做兼容判定）。
    pub fn zstd_version(&self) -> u32 {
        self.zstd_version
    }

    /// 加密 salt（批 F 用；批 A 只记录）。
    pub fn salt(&self) -> u32 {
        self.salt
    }
}

fn parse_index(raw: &[u8]) -> Result<(Vec<Block>, Vec<Entry>), BundleError> {
    if raw.len() < INDEX_PROLOGUE_LEN + 8 {
        return Err(BundleError::Index("索引过短".to_owned()));
    }
    let mut cursor = INDEX_PROLOGUE_LEN;
    macro_rules! take {
        ($len:expr) => {{
            let len: usize = $len;
            let end = cursor
                .checked_add(len)
                .ok_or_else(|| BundleError::Index("游标溢出".to_owned()))?;
            if end > raw.len() {
                return Err(BundleError::Index(format!(
                    "索引截断（需要到 {end}，实有 {}）",
                    raw.len()
                )));
            }
            let slice = &raw[cursor..end];
            cursor = end;
            slice
        }};
    }
    let entry_count = u32::from_le_bytes(take!(4).try_into().expect("4 字节")) as usize;
    let block_count = u32::from_le_bytes(take!(4).try_into().expect("4 字节")) as usize;
    // 先用"剩余字节能装下多少条"把声明值兜住，再 `with_capacity`。
    //
    // 为什么：`entry_count` 是 u32，一个 16 B 的坏索引可以声明 40 亿条。直接
    // `Vec::with_capacity(4e9)` 会先向系统要几十 GB 到上百 GB——原生是 OOM/被 kill，
    // wasm 上是 `abort`，两者都**不是**"坏容器报错"。每个块记录固定 9 B、每个条目记录
    // 至少 23 B（2 名字长 + 名字 + 1 标志 + 4 块号 + 4 偏移 + 4 长度 + 8 校验和），
    // 所以这个上界既不误伤合法包，又能让坏包在**分配之前**就失败。
    let needed = BLOCK_RECORD_LEN * block_count as u64 + ENTRY_RECORD_MIN_LEN * entry_count as u64;
    let remaining = (raw.len() - cursor) as u64;
    if needed > remaining {
        return Err(BundleError::Index(format!(
            "声明 {entry_count} 条目 + {block_count} 块至少需要 {needed} B，索引只剩 {remaining} B"
        )));
    }
    let mut blocks = Vec::with_capacity(block_count);
    for number in 0..block_count {
        let flags = take!(1)[0];
        let known = BLOCK_FLAG_COMPRESSED | BLOCK_FLAG_LARGE | BLOCK_FLAG_ENCRYPTED;
        if flags & !known != 0 {
            return Err(BundleError::Index(format!(
                "块 {number} 标志 {flags:#04x} 含 v1 未定义的位（v1 只定义 {known:#04x}）"
            )));
        }
        let stored_len = u32::from_le_bytes(take!(4).try_into().expect("4 字节"));
        let raw_len = u32::from_le_bytes(take!(4).try_into().expect("4 字节"));
        blocks.push(Block {
            flags,
            stored_len,
            raw_len,
        });
    }
    let mut entries = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        let name_len = u16::from_le_bytes(take!(2).try_into().expect("2 字节")) as usize;
        let path = String::from_utf8(take!(name_len).to_vec())
            .map_err(|_| BundleError::Index("条目名不是 UTF-8".to_owned()))?;
        // 读侧与写侧必须**同一份路径口径**（`paths::normalize_project_path`）。写侧一直在
        // 规范化/拒绝，读侧以前只查 UTF-8：于是 `assets\\a.png`、`./a.png`、`a//b.png`
        // 这类名字能进索引，然后在 `BundleSet::key_of` 的规范化键下**永远查不到**——
        // 症状是"包里有这个文件，游戏说没有"，且没有任何日志。现在解析期就点名。
        if crate::paths::normalize_project_path(&path).as_deref() != Some(path.as_str()) {
            return Err(BundleError::InvalidPath(path));
        }
        let flags = take!(1)[0];
        let known = ENTRY_FLAG_RLTEX | ENTRY_FLAG_PRECOMPRESSED;
        if flags & !known != 0 {
            return Err(BundleError::Index(format!(
                "条目 {path} 标志 {flags:#04x} 含 v1 未定义的位（v1 只定义 {known:#04x}）"
            )));
        }
        let block = u32::from_le_bytes(take!(4).try_into().expect("4 字节"));
        let offset = u32::from_le_bytes(take!(4).try_into().expect("4 字节"));
        let len = u32::from_le_bytes(take!(4).try_into().expect("4 字节"));
        let checksum = u64::from_le_bytes(take!(8).try_into().expect("8 字节"));
        entries.push(Entry {
            path,
            flags,
            block,
            offset,
            len,
            checksum,
        });
    }
    // 条目/块记录之后还有字节，说明索引本身被拼接或写坏了。以前这些字节被静默忽略：
    // 头里的 `index_raw_len` 只约束"解出来多长"，不约束"这些字节有没有人消费"。
    // 写侧产出的索引恰好是 prologue + 计数 + 记录，所以 `cursor` 必须落在末尾。
    if cursor != raw.len() {
        return Err(BundleError::Index(format!(
            "索引尾部多出 {} B（声明的 {entry_count} 条目 + {block_count} 块之后还有数据）",
            raw.len() - cursor
        )));
    }
    Ok((blocks, entries))
}

/// 块表与条目表的**联合**不变量：单看一边看不出问题的那些"坏容器"在这里一次挡住。
///
/// 三条都直接来自写侧（`BundleBuilder::build`）的保证，因此不会误伤合法包：
/// 1. 块只能由条目产生（写侧只在有条目时建块），所以"有块没条目"和"某个块没有任何条目
///    引用"都是坏包——后者以前只对**空块**检查，非空块可以白占一段永远读不到的 payload；
/// 2. 非空块的落盘长度必为正（zstd 帧至少 1 B；不压缩时存的就是原字节）；空块只可能是
///    "全部条目都是空文件"那一档，且落盘长度必须是 0；
/// 3. `raw_len > block_size` 只有一种合法来源：单个条目自身就超过块大小、独占一块
///    （`oversized` 分支）。所以这种块必须有一个**完整覆盖它**的条目
///    （`offset == 0 && len == raw_len`）。这一条挡的是"raw_len 撒了 4 GB 的谎"——
///    读侧会照着它解码/判缓存档，而错误直到解压或校验和才暴露，中途还可能先吃掉内存。
fn validate_layout(
    block_size: u32,
    blocks: &[Block],
    entries: &[Entry],
) -> Result<(), BundleError> {
    if blocks.is_empty() && entries.is_empty() {
        return Ok(());
    }
    if entries.is_empty() {
        return Err(BundleError::Index(format!(
            "块表有 {} 块，但条目表为空（写侧不会产出没有被引用的块）",
            blocks.len()
        )));
    }
    // 块号越界由 `parse` 用带路径 + 块号的错误点名；这里先让路，避免"某个块没人引用"
    // 把那条更具体的报错盖掉（两者都是坏包，但后者更容易定位）。
    if entries
        .iter()
        .any(|entry| entry.block as usize >= blocks.len())
    {
        return Ok(());
    }
    let mut owners = vec![0u32; blocks.len()];
    let mut covered = vec![false; blocks.len()];
    for entry in entries {
        let number = entry.block as usize;
        // 块号越界 / 条目越界在 `parse` 里已有更具体的报错（带路径与块号），这里跳过。
        let Some(block) = blocks.get(number) else {
            continue;
        };
        owners[number] += 1;
        if entry.offset == 0 && entry.len == block.raw_len && block.raw_len != 0 {
            covered[number] = true;
        }
    }
    for (number, block) in blocks.iter().enumerate() {
        // 每个块都必须有归属：写侧的 `blocks` 全部由条目切出来，不存在"填充块"或
        // "预热块"。没有归属的块永远读不到，只会让容器白大一段、且没人能解释它是什么。
        if owners[number] == 0 {
            return Err(BundleError::Index(format!(
                "块 {number} 没有任何条目引用（写侧不会产出这种块）"
            )));
        }
        if block.raw_len == 0 {
            if block.stored_len != 0 {
                return Err(BundleError::Index(format!(
                    "空块 {number} 的落盘长度是 {} B（只能是 0）",
                    block.stored_len
                )));
            }
            continue;
        }
        if block.stored_len == 0 {
            return Err(BundleError::Index(format!(
                "块 {number} 的 raw_len 是 {} B 但落盘长度是 0",
                block.raw_len
            )));
        }
        if u64::from(block.raw_len) > u64::from(block_size) && !covered[number] {
            return Err(BundleError::Index(format!(
                "块 {number} 的 raw_len {} 超过 block_size {block_size}，却没有条目完整覆盖它\
                 （写侧只在\"单条目超块\"时这样写）",
                block.raw_len
            )));
        }
    }
    validate_block_spans(blocks, entries)
}

/// 每个块内的字节区间必须**互不重叠、且恰好铺满** `raw_len`。
///
/// 写侧（`BundleBuilder::build`）逐条目顺序切块，所以这两条是构造出来的必然结果；
/// 一旦被破坏，只可能是索引被改过（或打包器回归）：
///
/// - **重叠**：两条路径指向同一段字节。危害是**静默的内容错乱**——条目 FNV 也存在索引里，
///   篡改者把 `a.png` 的 `offset/len` 指到 `b.png` 的字节、同时写上 `b.png` 的校验和，
///   校验和照样通过，游戏读到的是另一张图。这是"包没坏、内容却错"的唯一通道。
/// - **空洞**：块里有一段字节没有任何条目解释。读者永远读不到它，于是它既可以夹带数据，
///   也说明 `raw_len` 或条目表被动过（`raw_len` 撒谎的那一档已有更具体的报错）。
///
/// 零长条目（空文件）不参与区间计算：写侧会把它们都放在块尾，多个空文件共享同一个
/// `offset` 是**合法**的，不能当成重叠。
fn validate_block_spans(blocks: &[Block], entries: &[Entry]) -> Result<(), BundleError> {
    let mut spans: Vec<Vec<(u64, u64, &str)>> = vec![Vec::new(); blocks.len()];
    for entry in entries {
        let Some(slot) = spans.get_mut(entry.block as usize) else {
            continue;
        };
        if entry.len != 0 {
            slot.push((
                u64::from(entry.offset),
                u64::from(entry.offset) + u64::from(entry.len),
                entry.path.as_str(),
            ));
        }
    }
    for (number, block) in blocks.iter().enumerate() {
        let mut list = std::mem::take(&mut spans[number]);
        list.sort_unstable();
        let mut cursor: u64 = 0;
        let mut previous: Option<&str> = None;
        for (start, end, path) in &list {
            if *start < cursor {
                return Err(BundleError::Index(format!(
                    "块 {number} 的条目 {path} 与 {} 重叠（{start}..{end} 落在已覆盖的 0..{cursor} 里）",
                    previous.unwrap_or("<未知>")
                )));
            }
            cursor = *end;
            previous = Some(path);
        }
        // `raw_len == 0` 的块（只装空文件）没有区间，自然铺满。
        if cursor != u64::from(block.raw_len) {
            return Err(BundleError::Index(format!(
                "块 {number} 的条目只覆盖 0..{cursor}，raw_len 是 {}（有空洞：那段字节没有任何条目解释）",
                block.raw_len
            )));
        }
    }
    Ok(())
}

fn zstd_decode(frame: &[u8], limit: usize) -> Result<Vec<u8>, BundleError> {
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(std::io::Cursor::new(frame))
        .map_err(|error| BundleError::Source(format!("zstd 帧头解析失败：{error}")))?;
    // 解压上限来自容器自己声明的长度（索引 `index_raw_len` / 块 `raw_len`），本不该
    // 被突破。以前 `read_to_end` 无上限：一个几 KB 的 zstd 帧可以要求几百 MB 到几 GB
    // 的输出，坏容器于是以 OOM / wasm `abort` 收场，而不是"长度字段撒谎"这条具体错误。
    //
    // 用固定大小缓冲逐段读，而不是 `Vec::with_capacity(limit)` 或 `Read::take`：
    // 后者可能按攻击者声明的上限预分配，仍然把内存交出去。这里 `out` 最多长到 `limit`
    // 多一个读块，超过就立刻报错。
    const CHUNK: usize = 16 * 1024;
    let mut out = Vec::with_capacity(limit.min(CHUNK));
    let mut buffer = [0u8; CHUNK];
    loop {
        let read = decoder
            .read(&mut buffer)
            .map_err(|error| BundleError::Source(format!("zstd 解压失败：{error}")))?;
        if read == 0 {
            break;
        }
        if out.len().saturating_add(read) > limit {
            return Err(BundleError::Source(format!(
                "zstd 解压后超过声明的 {limit} B（坏容器或长度字段撒谎）"
            )));
        }
        out.extend_from_slice(&buffer[..read]);
    }
    Ok(out)
}

/// 块数据来源：桌面是 `File` + 定位读，wasm 是内存切片（批 B 接宿主）。
pub trait BlockSource {
    fn read_range(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, BundleError>;
}

/// 整包驻留内存的来源（批 A 唯一实现；也是 Web 宿主的最终形态）。
#[derive(Debug, Clone)]
pub struct MemSource {
    bytes: std::sync::Arc<[u8]>,
}

impl MemSource {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    pub fn from_shared(bytes: std::sync::Arc<[u8]>) -> Self {
        Self { bytes }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl BlockSource for MemSource {
    fn read_range(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, BundleError> {
        let start = usize::try_from(offset)
            .map_err(|_| BundleError::Source(format!("偏移 {offset} 超出 usize")))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| BundleError::Source("读取区间溢出".to_owned()))?;
        if end > self.bytes.len() {
            return Err(BundleError::Source(format!(
                "读取越界：{start}..{end} > {}",
                self.bytes.len()
            )));
        }
        Ok(self.bytes[start..end].to_vec())
    }
}

/// 读包器：索引 + 数据来源 + 单块缓存。
///
/// 缓存只保留**一个**块：256 KB 块时同一目录的条目基本相邻，一次解压够用；
/// 而图集页这类大块一次就是 8 MB 级，多留几块只会把峰值内存推上去（`docs/67` §4.3）。
pub struct BundleReader<S: BlockSource> {
    index: BundleIndex,
    source: S,
    /// 块缓存（LRU，**上限 4 块**）。复核 P1-3：单块会让"跨组交替读"每次都重解，
    /// 而 4 块 × 256 KB ≈ 1 MB 峰值，对微信低端机也安全（页这类大块不参与多块缓存）。
    cache: RefCell<Vec<(u32, std::sync::Arc<Vec<u8>>)>>,
    decoded_blocks: RefCell<u64>,
}

impl<S: BlockSource> BundleReader<S> {
    pub fn new(index: BundleIndex, source: S) -> Self {
        Self {
            index,
            source,
            cache: RefCell::new(Vec::new()),
            decoded_blocks: RefCell::new(0),
        }
    }

    pub fn index(&self) -> &BundleIndex {
        &self.index
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.index.paths()
    }

    pub fn contains(&self, path: &str) -> bool {
        self.index.contains(path)
    }

    /// 已解压过的块数（诊断/报告用）。
    pub fn decoded_blocks(&self) -> u64 {
        *self.decoded_blocks.borrow()
    }

    /// 读一个条目；不存在返回 `Ok(None)`。默认校验 FNV。
    pub fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>, BundleError> {
        self.read_impl(path, true)
    }

    /// 跳过校验和（批量导出/等价性对比时用）。
    pub fn read_unchecked(&mut self, path: &str) -> Result<Option<Vec<u8>>, BundleError> {
        self.read_impl(path, false)
    }

    fn read_impl(&mut self, path: &str, verify: bool) -> Result<Option<Vec<u8>>, BundleError> {
        let Some(entry) = self.index.entry(path).cloned() else {
            return Ok(None);
        };
        let block = self.block(entry.block)?;
        // 用 u64 算区间再落到 usize：wasm32 上 `usize` 只有 32 位，`offset + len` 会在
        // `offset`/`len` 都是 u32 时**回绕**成一个小数，`end > block.len()` 于是判不出来，
        // 接着 `block[start..end]` 的 `start > end` 直接 panic。索引已经保证
        // `offset + len <= raw_len`（见 `parse`），这里是第二道：坏索引不该以 panic 收场。
        let start = u64::from(entry.offset);
        let end = start + u64::from(entry.len);
        let block_len = block.len() as u64;
        if end > block_len {
            return Err(BundleError::EntryOutOfBlock {
                path: entry.path.clone(),
                block: entry.block,
                offset: entry.offset,
                len: entry.len,
                block_len: block.len() as u32,
            });
        }
        let (start, end) = (start as usize, end as usize);
        let bytes = block[start..end].to_vec();
        if verify {
            let actual = checksum(&bytes);
            if actual != entry.checksum {
                return Err(BundleError::Checksum {
                    path: entry.path,
                    expected: entry.checksum,
                    actual,
                });
            }
        }
        Ok(Some(bytes))
    }

    /// 逐条目全量校验（`game --check` 用）。
    pub fn verify_all(&mut self) -> Result<(), BundleError> {
        let paths: Vec<String> = self.index.paths().map(str::to_owned).collect();
        for path in paths {
            self.read(&path)?;
        }
        Ok(())
    }

    fn block(&mut self, number: u32) -> Result<std::sync::Arc<Vec<u8>>, BundleError> {
        // 先算命中位置再改缓存：`if let Some(index) = self.cache.borrow()…` 会把不可变借用
        // 一直持有到 `if` 体结束，里面再 `borrow_mut()` 就是 RefCell 双重借用 panic。
        let hit = self.cache.borrow().iter().position(|(n, _)| *n == number);
        if let Some(index) = hit {
            // 命中：提到队尾（LRU），再返回共享引用——不拷贝整块。
            let mut guard = self.cache.borrow_mut();
            let entry = guard.remove(index);
            let cached = std::sync::Arc::clone(&entry.1);
            guard.push(entry);
            return Ok(cached);
        }
        let block = *self
            .index
            .blocks
            .get(number as usize)
            .ok_or_else(|| BundleError::Index(format!("块号 {number} 不存在")))?;
        let offset = self.index.block_offsets[number as usize];
        let stored = self.source.read_range(offset, block.stored_len as usize)?;
        if stored.len() != block.stored_len as usize {
            return Err(BundleError::BlockDecode {
                block: number,
                reason: format!("读到 {} B，索引记录 {} B", stored.len(), block.stored_len),
            });
        }
        // 顺序与写侧相反：先解封、再解压。
        let stored = if block.is_encrypted() {
            let aad = block_aad(block.flags, self.index.block_size, number, block.raw_len);
            crate::pack::open_with_aad(&stored, &block_nonce(self.index.salt(), number), &aad)
                .map_err(|error| BundleError::BlockDecode {
                    block: number,
                    reason: error,
                })?
        } else {
            stored
        };
        let raw = if block.is_compressed() {
            let decoded = zstd_decode(&stored, block.raw_len as usize).map_err(|error| {
                BundleError::BlockDecode {
                    block: number,
                    reason: error.to_string(),
                }
            })?;
            if decoded.len() != block.raw_len as usize {
                return Err(BundleError::BlockDecode {
                    block: number,
                    reason: format!("解压后 {} B，索引记录 {} B", decoded.len(), block.raw_len),
                });
            }
            decoded
        } else {
            stored
        };
        let raw = std::sync::Arc::new(raw);
        {
            let mut guard = self.cache.borrow_mut();
            // 大块（页）只留 1 块：8 MB × 4 会直接把峰值推到 32 MB。
            let is_large = block.raw_len as usize > DEFAULT_BLOCK_SIZE as usize;
            let capacity = if is_large { 1 } else { CACHE_BLOCKS };
            guard.retain(|(n, _)| *n != number);
            guard.push((number, std::sync::Arc::clone(&raw)));
            while guard.len() > capacity {
                guard.remove(0);
            }
        }
        *self.decoded_blocks.borrow_mut() += 1;
        Ok(raw)
    }
}

/// 写包器（只在 `bundle-write` feature 下存在：它需要 C 版 zstd，而同 feature 绝不进 wasm）。
#[cfg(feature = "bundle-write")]
pub use writer::{BundleBuild, BundleBuilder};

/// 估算一段字节进容器块后的落盘长度（构建期选编码形态用）。
///
/// 为什么需要它：图集页的 `.rltex`（裸像素）与 PNG 哪个更省，**不能按原始字节比**。
/// 例：一页稀疏像素的裸 RGBA 有 3.7 MB、PNG 只有 80 KB，但裸像素进 zstd 块后只剩
/// 十几 KB——按原始字节比会选 PNG，反而把包撑大（2026-10-18 实测 art:ui 151 KB → 184 KB）。
/// 这里用与写包相同的 zstd、`level` 取 9（只用于相对比较，不追求与最终块一致）。
#[cfg(feature = "bundle-write")]
pub fn compressed_len_estimate(bytes: &[u8], level: i32) -> u64 {
    zstd::bulk::compress(bytes, level)
        .map(|out| out.len() as u64)
        .unwrap_or(bytes.len() as u64)
}

// ---------------------------------------------------------------------------------------------
// 挂载层（批 B）：多个包 + 未挂载清单 + 缺件归因 + vfs 接线
// ---------------------------------------------------------------------------------------------

impl BlockSource for Box<dyn BlockSource + Send> {
    fn read_range(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, BundleError> {
        (**self).read_range(offset, len)
    }
}

/// 文件来源：打开一次、按需 `seek + read`（桌面）。
///
/// wasm 上没有这个（`std::fs` 恒失败，也不该有）；Web 宿主用 [`MemSource`]。
#[cfg(not(target_arch = "wasm32"))]
pub struct FileSource {
    file: std::fs::File,
    path: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl FileSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BundleError> {
        let path = path.as_ref().to_path_buf();
        let file = std::fs::File::open(&path).map_err(|error| {
            BundleError::Source(format!("打开 {} 失败：{error}", path.display()))
        })?;
        Ok(Self { file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl BlockSource for FileSource {
    fn read_range(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, BundleError> {
        use std::io::{Read, Seek, SeekFrom};
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|error| BundleError::Source(format!("seek {offset} 失败：{error}")))?;
        let mut buffer = vec![0u8; len];
        self.file
            .read_exact(&mut buffer)
            .map_err(|error| BundleError::Source(format!("读 {len} B @ {offset} 失败：{error}")))?;
        Ok(buffer)
    }
}

/// 一次读取的归因：三种状态必须能分开（`docs/67` §7.4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissReason {
    /// 有条目（调用方不该在拿到它之后又问这个）。
    Found,
    /// 不在任何已挂载的包里，也不在任何已知但未挂载的包里 → 与"文件不存在"同义。
    NotInAnyBundle,
    /// 在某个**已知但未挂载**的包里 → 必须先挂载（这是 bundle 化引入的新失效模式）。
    InUnmountedBundle { bundle: String },
}

impl MissReason {
    /// 一句话诊断（错误信息用）。
    pub fn describe(&self) -> String {
        match self {
            Self::Found => "已找到".to_owned(),
            Self::NotInAnyBundle => "不在任何资源包里".to_owned(),
            Self::InUnmountedBundle { bundle } => {
                format!("在资源包 {bundle} 里，但该包未挂载（先 mount 再读）")
            }
        }
    }
}

struct MountedBundle {
    name: String,
    reader: BundleReader<Box<dyn BlockSource + Send>>,
}

/// 已挂载的多个资源包。
///
/// `root` 是工程根：所有 vfs 路径都先 `strip_prefix(root)` 折算成条目名
/// （条目名就是工程相对路径，见 §4.2）。挂载顺序 = 查找顺序（先挂的先命中），
/// 因此"补丁包覆盖基础包"这种玩法在批 E 之前也能表达。
pub struct BundleSet {
    root: PathBuf,
    mounted: Vec<MountedBundle>,
    /// 已知但尚未挂载的条目 → 包名（由 `bundle_plan.json` 在批 C 填充；批 B 只用 API）。
    pending: BTreeMap<String, String>,
}

impl BundleSet {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            mounted: Vec::new(),
            pending: BTreeMap::new(),
        }
    }

    /// 已挂载的包名（按挂载顺序）。
    pub fn mounted_names(&self) -> impl Iterator<Item = &str> {
        self.mounted.iter().map(|m| m.name.as_str())
    }

    /// 挂一个整包驻留内存的容器（Web 宿主）。
    pub fn mount_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<usize, BundleError> {
        let index = BundleIndex::parse(&bytes)?;
        let count = index.len();
        self.mounted.push(MountedBundle {
            name: name.to_owned(),
            reader: BundleReader::new(index, Box::new(MemSource::new(bytes))),
        });
        Ok(count)
    }

    /// 挂一个文件来源的容器（桌面）。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn mount_file(&mut self, path: impl AsRef<Path>) -> Result<usize, BundleError> {
        let path = path.as_ref();
        let source = FileSource::open(path)?;
        let bytes = std::fs::read(path)
            .map_err(|error| BundleError::Source(format!("读 {} 失败：{error}", path.display())))?;
        let index = BundleIndex::parse(&bytes)?;
        let count = index.len();
        // 只解析索引，块数据走 FileSource（不把整包读进内存）。
        drop(bytes);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("bundle")
            .to_owned();
        self.mounted.push(MountedBundle {
            name,
            reader: BundleReader::new(index, Box::new(source)),
        });
        Ok(count)
    }

    /// 登记"某个包里有哪些条目"（包未挂载时也能给出正确归因）。
    pub fn declare_pending(&mut self, bundle: &str, paths: impl IntoIterator<Item = String>) {
        for path in paths {
            self.pending.insert(path, bundle.to_owned());
        }
    }

    /// 路径 → 条目名（相对工程根、`/` 分隔）。不在工程根下或路径非法返回 `None`。
    pub fn key_of(&self, path: &Path) -> Option<String> {
        let relative = path.strip_prefix(&self.root).ok()?;
        let raw = relative.to_string_lossy();
        if raw.is_empty() || raw == "." {
            return Some(String::new());
        }
        crate::paths::normalize_project_path(&raw)
    }

    /// 按条目名读（先挂的先命中）。
    pub fn read(&mut self, key: &str) -> Result<Option<Vec<u8>>, BundleError> {
        for mounted in &mut self.mounted {
            if let Some(bytes) = mounted.reader.read(key)? {
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }

    /// 全部已挂载条目名（升序）。
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        let mut all: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for mounted in &self.mounted {
            all.extend(mounted.reader.paths());
        }
        all.into_iter()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.mounted
            .iter()
            .any(|mounted| mounted.reader.contains(key))
    }

    pub fn entry_count(&self) -> usize {
        self.mounted
            .iter()
            .map(|mounted| mounted.reader.index().len())
            .sum()
    }

    /// 缺件归因：`Found` / `NotInAnyBundle` / `InUnmountedBundle`（§7.4 的三态）。
    pub fn explain_miss(&self, key: &str) -> MissReason {
        if self.contains(key) {
            return MissReason::Found;
        }
        if let Some(bundle) = self.pending.get(key) {
            return MissReason::InUnmountedBundle {
                bundle: bundle.clone(),
            };
        }
        MissReason::NotInAnyBundle
    }

    /// 逐条目全量校验所有已挂载包（`--check` 用）。
    pub fn verify_all(&mut self) -> Result<(), BundleError> {
        for mounted in &mut self.mounted {
            mounted.reader.verify_all()?;
        }
        Ok(())
    }

    /// 目录直接子项（条目名视角）：返回 `(子项名, 是否目录)`。
    ///
    /// `dir_key` 是相对工程根的目录名（`""` = 根）。只返回**直接**子项，排序去重，
    /// 供 `vfs::set_lister` 接到 `vfs::list` 上。
    pub fn list_dir(&self, dir_key: &str) -> Vec<(String, bool)> {
        let prefix = if dir_key.is_empty() {
            String::new()
        } else {
            format!("{}/", dir_key.trim_matches('/'))
        };
        let mut out: std::collections::BTreeMap<String, bool> = std::collections::BTreeMap::new();
        for mounted in &self.mounted {
            for path in mounted.reader.paths() {
                let Some(rest) = path.strip_prefix(&prefix) else {
                    continue;
                };
                if rest.is_empty() {
                    continue;
                }
                match rest.split_once('/') {
                    Some((head, _)) => {
                        out.insert(head.to_owned(), true);
                    }
                    None => {
                        out.insert(rest.to_owned(), false);
                    }
                }
            }
        }
        out.into_iter().collect()
    }
}

/// 组合式查询：命中返回字节，未命中返回 `None`（未挂载时打 `tracing::error!`，数据损坏也打）。
///
/// 宿主若已有自己的 provider（Web 的预载表、微信的适配层读取器），**不要**用 [`install`]——
/// 那条路会占掉进程级唯一的 provider 槽。改用本函数在自己 provider 的第一步里查包，
/// 未命中再回落自己的通道（`docs/67` §7.3）。
pub fn lookup(set: &mut BundleSet, path: &Path) -> Option<Vec<u8>> {
    let key = set.key_of(path)?;
    match set.read(&key) {
        Ok(Some(bytes)) => Some(bytes),
        Ok(None) => {
            if let MissReason::InUnmountedBundle { bundle } = set.explain_miss(&key) {
                tracing::error!(
                    target: "bundle",
                    path = %key,
                    bundle = %bundle,
                    "资源在未挂载的资源包里：先挂载该 bundle 再读（不做静默回落）"
                );
            }
            None
        }
        Err(error) => {
            // 数据损坏必须响亮：provider 的签名只能返回 `Option`，所以这里记 error；
            // `--check --verify-all` 会把同一条错误变成退出码。
            tracing::error!(target: "bundle", path = %key, "读取资源包条目失败：{error}");
            None
        }
    }
}

/// 这条路径是否在已挂载的包里（不读数据、不解码块）。
pub fn contains_path(set: &BundleSet, path: &Path) -> bool {
    set.key_of(path).is_some_and(|key| set.contains(&key))
}

/// 把 `BundleSet` 装成 vfs 的 provider + lister（桌面：`Arc<Mutex<..>>`）。
///
/// 装好之后：
/// 1. `vfs::read` / `vfs::is_file` 先查包，未命中才回落文件系统（[`crate::vfs`] 的既有语义）；
/// 2. `vfs::list` 合并"包内条目"与"磁盘目录"——发布形态下 `scripts/` 只在包里；
/// 3. 命中**未挂载**的包时打 `tracing::error!` 且返回 `None`（不许静默；§7.4 第二种情形）。
///
/// 宿主已有自己的 provider 时改用 [`lookup`]（见其文档）。
#[cfg(not(target_arch = "wasm32"))]
pub fn install(set: std::sync::Arc<std::sync::Mutex<BundleSet>>) -> Result<(), anyhow::Error> {
    let for_list = std::sync::Arc::clone(&set);
    crate::vfs::set_lister(Box::new(move |dir: &Path| {
        let guard = for_list.lock().ok()?;
        let key = guard.key_of(dir)?;
        let entries: Vec<(PathBuf, bool)> = guard
            .list_dir(&key)
            .into_iter()
            .map(|(name, is_dir)| (dir.join(name), is_dir))
            .collect();
        Some(entries)
    }))
    .map_err(|error| anyhow::anyhow!("{error}"))?;

    crate::vfs::set_provider(Box::new(move |path: &Path| {
        let mut guard = set.lock().ok()?;
        lookup(&mut guard, path)
    }))
    .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(())
}

/// wasm 版：单线程，`Rc<RefCell<..>>`。
#[cfg(target_arch = "wasm32")]
pub fn install(set: std::rc::Rc<std::cell::RefCell<BundleSet>>) -> Result<(), anyhow::Error> {
    let for_list = std::rc::Rc::clone(&set);
    crate::vfs::set_lister(move |dir: &Path| {
        let guard = for_list.borrow();
        let key = guard.key_of(dir)?;
        let entries: Vec<(PathBuf, bool)> = guard
            .list_dir(&key)
            .into_iter()
            .map(|(name, is_dir)| (dir.join(name), is_dir))
            .collect();
        Some(entries)
    })
    .map_err(|error| anyhow::anyhow!("{error}"))?;

    crate::vfs::set_provider(move |path: &Path| {
        let mut guard = set.borrow_mut();
        lookup(&mut guard, path)
    })
    .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(())
}

#[cfg(feature = "bundle-write")]
mod writer {
    use super::*;

    /// 写入结果与统计（体积报告直接用它）。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct BundleBuild {
        pub bytes: Vec<u8>,
        /// 条目原始字节合计。
        pub raw_len: u64,
        /// 块区落盘字节合计（不含文件头与索引）。
        pub stored_len: u64,
        pub blocks: u32,
        pub entries: u32,
        /// 索引原始长度（压缩前）。
        pub index_raw_len: u32,
        /// 索引落盘长度。
        pub index_len: u32,
        /// 整包 FNV-1a（`manifest.txt` 用它）。
        pub checksum: u64,
    }

    impl BundleBuild {
        /// 压缩比（整包落盘 / 原始）。
        pub fn ratio(&self) -> f64 {
            if self.raw_len == 0 {
                1.0
            } else {
                self.bytes.len() as f64 / self.raw_len as f64
            }
        }

        /// 文件字节数。
        pub fn len(&self) -> usize {
            self.bytes.len()
        }

        pub fn is_empty(&self) -> bool {
            self.bytes.is_empty()
        }
    }

    /// 逐条目搭包，最后一次性 [`build`](BundleBuilder::build)。
    pub struct BundleBuilder {
        block_size: u32,
        level: i32,
        /// 块级 AEAD 开关（`docs/67` §4.4 批 F）：`--encrypt` 时为 true。
        encrypt: bool,
        /// 条目优先级（0 = 最热）。同一份输入仍然产出逐字节相同的容器：
        /// 排序键是 `(rank, path)`，两者都由内容决定。
        ranks: std::collections::BTreeMap<String, u8>,
        entries: Vec<(String, Vec<u8>)>,
    }

    impl BundleBuilder {
        pub fn new(block_size: u32) -> Self {
            Self {
                encrypt: false,
                ranks: std::collections::BTreeMap::new(),
                block_size: if block_size == 0 {
                    DEFAULT_BLOCK_SIZE
                } else {
                    block_size
                },
                level: 9,
                entries: Vec::new(),
            }
        }

        /// 覆盖默认压缩档（默认 9；`--fast` 用 3）。
        pub fn with_level(mut self, level: i32) -> Self {
            self.level = level.clamp(1, 19);
            self
        }

        /// 带优先级加入（0 = 最热）。
        ///
        /// 为什么需要它：容器按 256 KB 顺序切块，而 `build()` 内部会按名字重排条目——
        /// 于是"先在 assetc 侧排好序"是无效的（实测 blocks 分布毫无变化）。
        /// 把"谁该排在前面"作为构造期的一等信息交给 builder，排序才真正生效。
        pub fn add_ranked(
            &mut self,
            path: &str,
            bytes: Vec<u8>,
            rank: u8,
        ) -> Result<(), BundleError> {
            let normalized = crate::paths::normalize_project_path(path)
                .ok_or_else(|| BundleError::InvalidPath(path.to_owned()))?;
            self.ranks.insert(normalized.clone(), rank);
            self.add(&normalized, bytes)
        }

        /// 打开块级 AEAD（`--encrypt`）：每块独立 ChaCha20Poly1305，nonce/AAD 见
        /// `block_nonce` / `block_aad`。
        ///
        /// 不做整包 AEAD 的原因：整包要求"先全量解密才能读第一个字节"，随机访问、按组挂载、
        /// CDN 分段缓存全部作废；块级则把代价限制在"读哪个块解哪个块"。
        #[must_use]
        pub fn with_encryption(mut self, encrypt: bool) -> Self {
            self.encrypt = encrypt;
            self
        }

        pub fn add(&mut self, path: &str, bytes: Vec<u8>) -> Result<(), BundleError> {
            let path = crate::paths::normalize_project_path(path)
                .ok_or_else(|| BundleError::InvalidPath(path.to_owned()))?;
            if path.len() > u16::MAX as usize {
                return Err(BundleError::InvalidPath(format!(
                    "{path}（名字 {} B 超过 u16 上限）",
                    path.len()
                )));
            }
            if self.entries.iter().any(|(existing, _)| existing == &path) {
                return Err(BundleError::DuplicateEntry(path));
            }
            self.entries.push((path, bytes));
            Ok(())
        }

        pub fn build(mut self) -> Result<BundleBuild, BundleError> {
            // 排序键 = (优先级, 名字)。名字这一档保证确定性；优先级那档让"启动闭包"
            // 落到容器的前几个块（见 `add_ranked`）。
            self.entries.sort_by(|a, b| {
                let rank_a = self.ranks.get(&a.0).copied().unwrap_or(u8::MAX);
                let rank_b = self.ranks.get(&b.0).copied().unwrap_or(u8::MAX);
                rank_a.cmp(&rank_b).then_with(|| a.0.cmp(&b.0))
            });

            // 1) 切块：条目不出块；大条目独占一个块。
            struct PendingBlock {
                buf: Vec<u8>,
                entries: Vec<Entry>,
            }
            fn empty_block() -> PendingBlock {
                PendingBlock {
                    buf: Vec::new(),
                    entries: Vec::new(),
                }
            }
            let mut blocks: Vec<PendingBlock> = Vec::new();
            let mut current = empty_block();
            let mut raw_len = 0u64;
            for (path, bytes) in &self.entries {
                raw_len += bytes.len() as u64;
                let oversized = bytes.len() > self.block_size as usize;
                if oversized {
                    // 先把当前块收尾，大条目再独占一块。
                    //
                    // 判据看的是**条目**而不只是字节：空文件不会往 `buf` 里写任何字节，
                    // 只看 `buf` 会把"只装着空文件的那个块"连同块里的条目一起丢掉
                    // （症状：容器里这几条路径彻底消失，`read` 返回 `None`、零报错）。
                    if !current.buf.is_empty() || !current.entries.is_empty() {
                        blocks.push(std::mem::replace(&mut current, empty_block()));
                    }
                    let block_number = blocks.len() as u32;
                    blocks.push(PendingBlock {
                        entries: vec![Entry {
                            path: path.clone(),
                            flags: entry_flags(path),
                            block: block_number,
                            offset: 0,
                            len: bytes.len() as u32,
                            checksum: checksum(bytes),
                        }],
                        buf: bytes.clone(),
                    });
                    continue;
                }
                if !current.buf.is_empty()
                    && current.buf.len() + bytes.len() > self.block_size as usize
                {
                    blocks.push(std::mem::replace(&mut current, empty_block()));
                }
                let block_number = blocks.len() as u32;
                let offset = current.buf.len() as u32;
                current.entries.push(Entry {
                    path: path.clone(),
                    flags: entry_flags(path),
                    block: block_number,
                    offset,
                    len: bytes.len() as u32,
                    checksum: checksum(bytes),
                });
                current.buf.extend_from_slice(bytes);
            }
            // 同上：有条目就得收尾（全空文件的容器否则会产出"零条目"的包）。
            if !current.buf.is_empty() || !current.entries.is_empty() {
                blocks.push(current);
            }

            // 2) 压块（逐块判定 store / compress）。
            let mut payload: Vec<u8> = Vec::new();
            let mut block_table: Vec<Block> = Vec::with_capacity(blocks.len());
            let mut entries: Vec<Entry> = Vec::new();
            // salt 在建块**之前**就能定：它只依赖条目表（名字 + raw_len）。
            let salt = bundle_salt(
                self.entries
                    .iter()
                    .map(|(path, bytes)| (path.as_str(), bytes.len() as u32)),
            );
            for (block_number, block) in blocks.into_iter().enumerate() {
                let raw = block.buf;
                let raw_len_block = raw.len() as u32;
                // 大像素块用 L19：判据是"块大"**或**"块里有 `.rltex`"（复核 P1-2）。
                // 只看块大小会漏掉小页：1024×512 的裸像素块只有 2 MB、512×512 只到 1 MB 边，
                // 退到 L9 后同一张页可能从 14.7 KB 变 30 KB 量级（`docs/67` §1.3）。
                let has_rltex = block
                    .entries
                    .iter()
                    .any(|entry| entry.path.ends_with(".rltex"));
                let large = raw_len_block >= LARGE_BLOCK_THRESHOLD || has_rltex;
                let level = if large { 19 } else { self.level };
                let compressed = zstd_compress(&raw, level)?;
                // 压不动就原样存：不给已压数据白付解码成本。
                let (stored, flags) = if compressed.len() < raw.len() {
                    let mut flags = BLOCK_FLAG_COMPRESSED;
                    if large {
                        flags |= BLOCK_FLAG_LARGE;
                    }
                    (compressed, flags)
                } else {
                    (raw, 0u8)
                };
                // 加密**在压缩之后**：先压再封，密文就是最终落盘字节（不可再压）。
                let (stored, flags) = if self.encrypt {
                    let number = block_number as u32;
                    let aad = block_aad(flags, self.block_size, number, raw_len_block);
                    let sealed =
                        crate::pack::seal_with_aad(&stored, &block_nonce(salt, number), &aad);
                    (sealed, flags | BLOCK_FLAG_ENCRYPTED)
                } else {
                    (stored, flags)
                };
                block_table.push(Block {
                    flags,
                    stored_len: stored.len() as u32,
                    raw_len: raw_len_block,
                });
                payload.extend_from_slice(&stored);
                entries.extend(block.entries);
            }
            let stored_len = payload.len() as u64;

            // 3) 索引（可整体压）。
            let index_raw = build_index(&entries, &block_table, &payload);
            let index_compressed = zstd_compress(&index_raw, self.level)?;
            let (index_bytes, header_flags) = if index_compressed.len() < index_raw.len() {
                (index_compressed, HEADER_FLAG_INDEX_COMPRESSED)
            } else {
                (index_raw.clone(), 0u16)
            };

            // 4) 头 + 装配。
            let index_offset = (HEADER_LEN + payload.len()) as u64;
            let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len() + index_bytes.len());
            bytes.extend_from_slice(BUNDLE_MAGIC);
            bytes.extend_from_slice(&BUNDLE_VERSION.to_le_bytes());
            bytes.extend_from_slice(&header_flags.to_le_bytes());
            bytes.extend_from_slice(&self.block_size.to_le_bytes());
            bytes.extend_from_slice(&index_offset.to_le_bytes());
            bytes.extend_from_slice(&(index_bytes.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&(index_raw.len() as u32).to_le_bytes());
            debug_assert_eq!(bytes.len(), 28);
            let header_fnv = checksum(&bytes) as u32;
            bytes.extend_from_slice(&header_fnv.to_le_bytes());
            debug_assert_eq!(bytes.len(), HEADER_LEN);
            bytes.extend_from_slice(&payload);
            bytes.extend_from_slice(&index_bytes);

            Ok(BundleBuild {
                checksum: checksum(&bytes),
                raw_len,
                stored_len,
                blocks: block_table.len() as u32,
                entries: entries.len() as u32,
                index_raw_len: index_raw.len() as u32,
                index_len: index_bytes.len() as u32,
                bytes,
            })
        }
    }

    fn entry_flags(path: &str) -> u8 {
        if path.ends_with(".rltex") {
            ENTRY_FLAG_RLTEX
        } else {
            0
        }
    }

    fn build_index(entries: &[Entry], blocks: &[Block], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + blocks.len() * 9 + entries.len() * 48);
        out.extend_from_slice(&zstd_version().to_le_bytes());
        // salt：**由条目表（名字 + raw_len）决定**的确定性值，批 F 的块级 AEAD 用它当 nonce 前缀。
        //
        // 原来这里是 `checksum(payload)`，但块区在加密时已经被封过——nonce 需要 salt、
        // salt 又需要密文，是循环依赖。改成条目表之后：写侧建块前就能算，读侧解析完索引
        // 也能重算，且同样零随机（确定性要求见 §4.5）。
        let salt = bundle_salt(entries.iter().map(|entry| (entry.path.as_str(), entry.len)));
        let _ = payload;
        out.extend_from_slice(&salt.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
        for block in blocks {
            out.push(block.flags);
            out.extend_from_slice(&block.stored_len.to_le_bytes());
            out.extend_from_slice(&block.raw_len.to_le_bytes());
        }
        for entry in entries {
            out.extend_from_slice(&(entry.path.len() as u16).to_le_bytes());
            out.extend_from_slice(entry.path.as_bytes());
            out.push(entry.flags);
            out.extend_from_slice(&entry.block.to_le_bytes());
            out.extend_from_slice(&entry.offset.to_le_bytes());
            out.extend_from_slice(&entry.len.to_le_bytes());
            out.extend_from_slice(&entry.checksum.to_le_bytes());
        }
        out
    }

    fn zstd_compress(bytes: &[u8], level: i32) -> Result<Vec<u8>, BundleError> {
        zstd::bulk::compress(bytes, level).map_err(|error| BundleError::Write(error.to_string()))
    }

    fn zstd_version() -> u32 {
        zstd::zstd_safe::version_number()
    }
}

#[cfg(all(test, feature = "bundle-write"))]
mod tests {
    use super::*;

    /// 复核 P2：计划表自校验——正常计划可解析，被改过的必须当场拒绝。
    #[test]
    fn plan_checksum_catches_tampering() {
        use crate::bundle::plan::{BundlePlan, PLAN_FORMAT, PlannedGroup};

        let plan = BundlePlan {
            schema: 1,
            format: PLAN_FORMAT.to_owned(),
            block_size: DEFAULT_BLOCK_SIZE,
            groups: vec![PlannedGroup {
                name: "core".to_owned(),
                unit: "main".to_owned(),
                bundle: "bundles/core-1a2b3c4d.rlb".to_owned(),
                hash: "0011223344556677".to_owned(),
                entries: 3,
                raw_bytes: 100,
                bundle_bytes: 60,
                paths: vec!["scripts/main.lua".to_owned()],
            }],
            checksum: String::new(),
            keep: None,
        }
        .sealed();
        let bytes = serde_json::to_vec_pretty(&plan).unwrap();
        assert_eq!(BundlePlan::parse(&bytes).unwrap(), plan);

        // 改一个条目路径：JSON 仍合法、字段仍齐全，只有自校验能发现
        let tampered = String::from_utf8(bytes.clone())
            .unwrap()
            .replace("scripts/main.lua", "scripts/evil.lua")
            .into_bytes();
        let error = BundlePlan::parse(&tampered).expect_err("改过内容必须被拒绝");
        assert!(error.contains("自校验失败"), "{error}");

        // 旧计划（没有 checksum 字段）仍然可读：向后兼容不能为了自校验牺牲
        let legacy = br#"{"schema":1,"format":"assetc-bundle-plan-1","block_size":262144,
            "groups":[{"name":"core","unit":"main","bundle":"bundles/core-1a2b3c4d.rlb",
            "hash":"","entries":1,"raw_bytes":1,"bundle_bytes":1,"paths":["project.toml"]}]}"#;
        assert!(
            BundlePlan::parse(legacy).is_ok(),
            "旧计划（无 checksum，但结构合法）必须还能解析"
        );
    }

    /// 结构自检与 checksum 无关：手改过/无自校验的计划表也要拒绝**有歧义**的结构，
    /// 否则读取端只能自己猜，猜错的表现是白图而不是报错。
    #[test]
    fn plan_structure_is_validated_even_without_checksum() {
        use crate::bundle::plan::{BundlePlan, PLAN_FORMAT, PlannedGroup};

        let group = |name: &str, unit: &str, bundle: &str| PlannedGroup {
            name: name.to_owned(),
            unit: unit.to_owned(),
            bundle: bundle.to_owned(),
            hash: String::new(),
            entries: 1,
            raw_bytes: 1,
            bundle_bytes: 1,
            paths: vec!["project.toml".to_owned()],
        };
        let plan = |groups: Vec<PlannedGroup>| BundlePlan {
            schema: 1,
            format: PLAN_FORMAT.to_owned(),
            block_size: DEFAULT_BLOCK_SIZE,
            groups,
            checksum: String::new(),
            keep: None,
        };
        let encode = |groups: Vec<PlannedGroup>| serde_json::to_vec(&plan(groups)).unwrap();

        assert!(
            BundlePlan::parse(&encode(vec![group("core", "main", "bundles/core.rlb")])).is_ok()
        );
        // 组名重复
        let error = BundlePlan::parse(&encode(vec![
            group("core", "main", "bundles/a.rlb"),
            group("core", "sub", "bundles/b.rlb"),
        ]))
        .expect_err("组名重复必须被拒绝");
        assert!(error.contains("组名重复"), "{error}");
        // 一个容器被两个组声明
        let error = BundlePlan::parse(&encode(vec![
            group("core", "main", "bundles/a.rlb"),
            group("art", "sub", "bundles/a.rlb"),
        ]))
        .expect_err("容器共用必须被拒绝");
        assert!(error.contains("被多个组声明"), "{error}");
        // unit 写错 / 容器不在 bundles/ 下 / block_size = 0 / schema 不认识
        assert!(
            BundlePlan::parse(&encode(vec![group("core", "bundle", "bundles/a.rlb")]))
                .expect_err("未知 unit 必须被拒绝")
                .contains("unit 非法")
        );
        assert!(
            BundlePlan::parse(&encode(vec![group("core", "main", "core.rlb")]))
                .expect_err("容器必须在 bundles/ 下")
                .contains("容器路径非法")
        );
        let mut zero = plan(vec![group("core", "main", "bundles/a.rlb")]);
        zero.block_size = 0;
        assert!(
            BundlePlan::parse(&serde_json::to_vec(&zero).unwrap())
                .expect_err("block_size = 0 必须被拒绝")
                .contains("block_size")
        );
        let mut future = plan(vec![group("core", "main", "bundles/a.rlb")]);
        future.schema = 2;
        assert!(
            BundlePlan::parse(&serde_json::to_vec(&future).unwrap())
                .expect_err("未知 schema 必须被拒绝")
                .contains("schema")
        );
    }

    /// 批 F：块级 AEAD——加密后不可直接读、正确密钥能读、错块/坏字节必须响亮。
    #[test]
    fn block_aead_roundtrip_and_tamper_detection() {
        let payload =
            b"unit-test payload for block aead, long enough to exceed one block? no".to_vec();
        let build = |encrypt: bool| {
            let mut builder = BundleBuilder::new(256);
            for index in 0..6u32 {
                builder
                    .add(&format!("scripts/file_{index}.lua"), payload.clone())
                    .unwrap();
            }
            builder.with_encryption(encrypt).build().unwrap()
        };

        let plain = build(false);
        let sealed = build(true);
        // 1) 加密确实改变了落盘字节，且块标志带上了加密位
        assert_ne!(plain.bytes, sealed.bytes);
        let index = BundleIndex::parse(&sealed.bytes).unwrap();
        assert!(
            index
                .blocks
                .iter()
                .all(|block| block.flags & BLOCK_FLAG_ENCRYPTED != 0)
        );
        // 2) 正确密钥能读回原字节
        let mut reader = BundleReader::new(index, MemSource::new(sealed.bytes.clone()));
        for index in 0..6u32 {
            let path = format!("scripts/file_{index}.lua");
            assert_eq!(reader.read(&path).unwrap().unwrap(), payload, "{path}");
        }
        reader.verify_all().unwrap();
        // 3) 密文里不该出现明文字节（否则"加密"只是换个头部）
        assert!(
            !sealed
                .bytes
                .windows(payload.len())
                .any(|window| window == payload.as_slice()),
            "密文里出现了明文"
        );
        // 4) 坏一个字节 → 必须报 BlockDecode，而不是给出错字节。
        //    要改的是**该条目所在块**里的字节：改别处（比如索引区）会先炸在解析上，
        //    验不到"块级 AEAD 认得出密文被改"这件事。
        let mut corrupted = build(true).bytes;
        let parsed = BundleIndex::parse(&corrupted).unwrap();
        let block = parsed.entry("scripts/file_0.lua").unwrap().block as usize;
        let target = parsed.block_offsets[block] as usize + 1;
        corrupted[target] ^= 0xff;
        let mut reader = BundleReader::new(
            BundleIndex::parse(&corrupted).unwrap(),
            MemSource::new(corrupted),
        );
        let error = reader
            .read("scripts/file_0.lua")
            .expect_err("坏字节必须报错");
        assert!(
            matches!(
                error,
                BundleError::BlockDecode { .. } | BundleError::Checksum { .. }
            ),
            "得到 {error:?}"
        );
    }

    /// 批 F：换块重放必须失败（nonce 与 AAD 都绑了块号）。
    #[test]
    fn block_aead_rejects_swapped_blocks() {
        let mut builder = BundleBuilder::new(128);
        // 两个块各自装不同的内容，块号绑进 AAD/nonce
        for index in 0..4u32 {
            builder
                .add(&format!("assets/blob_{index}.bin"), vec![index as u8; 200])
                .unwrap();
        }
        let built = builder.with_encryption(true).build().unwrap();
        let index = BundleIndex::parse(&built.bytes).unwrap();
        assert!(index.blocks.len() >= 2, "至少要两个块才谈得上换块");

        // 把前两个块的密文互换，再解析索引（索引记录的 stored_len 不变，长度对得上）
        let first = index.block_offsets[0] as usize;
        let second = index.block_offsets[1] as usize;
        let len0 = index.blocks[0].stored_len as usize;
        let len1 = index.blocks[1].stored_len as usize;
        let mut bytes = built.bytes.clone();
        if len0 == len1 {
            let head: Vec<u8> = bytes[first..first + len0].to_vec();
            let tail: Vec<u8> = bytes[second..second + len1].to_vec();
            bytes[first..first + len0].copy_from_slice(&tail);
            bytes[second..second + len1].copy_from_slice(&head);
            let mut reader =
                BundleReader::new(BundleIndex::parse(&bytes).unwrap(), MemSource::new(bytes));
            // 任一被换过的块读出来都必须报错（AAD 里的块号对不上）
            let error = reader
                .read("assets/blob_0.bin")
                .or_else(|_| reader.read("assets/blob_1.bin"))
                .expect_err("换块后必须解析失败");
            assert!(
                matches!(error, BundleError::BlockDecode { .. }),
                "得到 {error:?}"
            );
        }
    }

    /// 批 E：增量对比的三条语义（相同→复用、变了→下载、没了→清理）。
    #[test]
    fn plan_diff_compares_by_group_name_and_hash() {
        use crate::bundle::plan::{BundlePlan, PLAN_FORMAT, PlannedGroup};

        let group = |name: &str, hash: &str, bytes: u64| PlannedGroup {
            name: name.to_owned(),
            unit: "sub".to_owned(),
            bundle: format!("bundles/{name}-{hash}.rlb"),
            hash: hash.to_owned(),
            entries: 1,
            raw_bytes: bytes,
            bundle_bytes: bytes,
            paths: vec![format!("{name}/a.bin")],
        };
        let plan = |groups: Vec<PlannedGroup>| BundlePlan {
            schema: 1,
            format: PLAN_FORMAT.to_owned(),
            block_size: DEFAULT_BLOCK_SIZE,
            groups,
            checksum: String::new(),
            keep: None,
        };

        // 本地有 core(v1)+audio(v1)；远端 core(v1)+audio(v2)+新增 art
        let local = plan(vec![group("core", "aaaa", 100), group("audio", "bbbb", 50)]);
        let remote = plan(vec![
            group("core", "aaaa", 100),
            group("audio", "cccc", 60),
            group("art", "dddd", 70),
        ]);
        let diff = local.diff(&remote);
        assert_eq!(diff.unchanged, 1, "core 没变，必须复用");
        let download: Vec<&str> = diff.download.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(download, vec!["audio", "art"], "hash 变了 + 新增都要下载");
        assert!(diff.remove.is_empty());
        assert_eq!(diff.download_bytes(), 130);

        // 远端删掉 audio：本地该组的残留要清理
        let shrunk = plan(vec![group("core", "aaaa", 100)]);
        let diff = local.diff(&shrunk);
        assert_eq!(diff.remove.len(), 1);
        assert_eq!(diff.remove[0].name, "audio");
        assert!(diff.download.is_empty());
        assert!(!diff.is_empty());

        // 完全一致 → 什么都不用做
        assert!(local.diff(&local).is_empty());
        assert_eq!(local.diff(&local).unchanged, 2);

        // 空 hash（旧计划没有这个字段）不能被当成"相同"
        let legacy = plan(vec![PlannedGroup {
            hash: String::new(),
            ..group("core", "aaaa", 100)
        }]);
        assert_eq!(
            local.diff(&legacy).download.len(),
            1,
            "空 hash 必须下载而不是猜相同"
        );
    }

    fn build(entries: &[(&str, &[u8])]) -> BundleBuild {
        let mut builder = BundleBuilder::new(DEFAULT_BLOCK_SIZE);
        for (path, bytes) in entries {
            builder.add(path, bytes.to_vec()).unwrap();
        }
        builder.build().unwrap()
    }

    fn reader(built: &BundleBuild) -> BundleReader<MemSource> {
        let index = BundleIndex::parse(&built.bytes).unwrap();
        BundleReader::new(index, MemSource::new(built.bytes.clone()))
    }

    /// 64 KB 高熵数据：zstd 压不动，用来构造 store 块。
    fn noise(len: usize, seed: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        let mut state = seed | 1;
        while out.len() < len {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            out.push((state & 0xff) as u8);
        }
        out
    }

    /// 手工装配容器：**写侧永远不会产出的字节组合，必须由读侧挡住**，所以坏包得自己拼。
    ///
    /// 只拼未压缩索引（`header_flags = 0`）：这样用例不依赖 zstd 的编码端，"索引被压"
    /// 那条路径已由写侧既有用例覆盖。头部 FNV 按拼出来的 28 B 现算，所以改头字段
    /// （标志位/块大小/索引长度）不会先撞上"包头损坏"。
    #[derive(Clone)]
    struct Craft {
        block_size: u32,
        header_flags: u16,
        payload: Vec<u8>,
        /// `(flags, stored_len, raw_len)`
        blocks: Vec<(u8, u32, u32)>,
        entries: Vec<CraftEntry>,
        /// `None` = 写真实长度；`Some(n)` 用来伪造头里的记录值。
        index_raw_len: Option<u32>,
        /// 追加在（未压缩）索引记录之后的多余字节：`index_raw_len` 会把它算进去。
        index_trailing: Vec<u8>,
        trailing: Vec<u8>,
    }

    #[derive(Clone)]
    struct CraftEntry {
        path: String,
        flags: u8,
        block: u32,
        offset: u32,
        len: u32,
        checksum: u64,
    }

    impl Craft {
        fn new() -> Self {
            Self {
                block_size: DEFAULT_BLOCK_SIZE,
                header_flags: 0,
                payload: Vec::new(),
                blocks: Vec::new(),
                entries: Vec::new(),
                index_raw_len: None,
                index_trailing: Vec::new(),
                trailing: Vec::new(),
            }
        }

        /// 追加"一个块 + 一个完整覆盖它的条目"（最常见的那种合法形态）。
        fn single(mut self, path: &str, bytes: &[u8]) -> Self {
            let block = self.blocks.len() as u32;
            self.blocks
                .push((0, bytes.len() as u32, bytes.len() as u32));
            self.payload.extend_from_slice(bytes);
            self.entries.push(CraftEntry {
                path: path.to_owned(),
                flags: 0,
                block,
                offset: 0,
                len: bytes.len() as u32,
                checksum: checksum(bytes),
            });
            self
        }

        fn bytes(&self) -> Vec<u8> {
            let mut index_raw = Vec::new();
            index_raw.extend_from_slice(&0u32.to_le_bytes()); // zstd_version
            index_raw.extend_from_slice(&0u32.to_le_bytes()); // salt
            index_raw.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
            index_raw.extend_from_slice(&(self.blocks.len() as u32).to_le_bytes());
            for (flags, stored_len, raw_len) in &self.blocks {
                index_raw.push(*flags);
                index_raw.extend_from_slice(&stored_len.to_le_bytes());
                index_raw.extend_from_slice(&raw_len.to_le_bytes());
            }
            for entry in &self.entries {
                index_raw.extend_from_slice(&(entry.path.len() as u16).to_le_bytes());
                index_raw.extend_from_slice(entry.path.as_bytes());
                index_raw.push(entry.flags);
                index_raw.extend_from_slice(&entry.block.to_le_bytes());
                index_raw.extend_from_slice(&entry.offset.to_le_bytes());
                index_raw.extend_from_slice(&entry.len.to_le_bytes());
                index_raw.extend_from_slice(&entry.checksum.to_le_bytes());
            }
            index_raw.extend_from_slice(&self.index_trailing);
            let index_offset = (HEADER_LEN + self.payload.len()) as u64;
            let index_raw_len = self.index_raw_len.unwrap_or(index_raw.len() as u32);
            let mut bytes = Vec::new();
            bytes.extend_from_slice(BUNDLE_MAGIC);
            bytes.extend_from_slice(&BUNDLE_VERSION.to_le_bytes());
            bytes.extend_from_slice(&self.header_flags.to_le_bytes());
            bytes.extend_from_slice(&self.block_size.to_le_bytes());
            bytes.extend_from_slice(&index_offset.to_le_bytes());
            bytes.extend_from_slice(&(index_raw.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&index_raw_len.to_le_bytes());
            let header_fnv = checksum(&bytes) as u32;
            bytes.extend_from_slice(&header_fnv.to_le_bytes());
            bytes.extend_from_slice(&self.payload);
            bytes.extend_from_slice(&index_raw);
            bytes.extend_from_slice(&self.trailing);
            bytes
        }
    }

    #[test]
    fn roundtrip_preserves_bytes() {
        let text = "function update() return 'ok' end".repeat(200);
        let built = build(&[
            ("scripts/main.lua", text.as_bytes()),
            ("assets/a.png", b"\x89PNG\r\n\x1a\nfake"),
            ("scenes/main.scene", b"scene_version=1\n"),
        ]);
        let mut r = reader(&built);
        assert_eq!(
            r.read("scripts/main.lua").unwrap().unwrap(),
            text.as_bytes()
        );
        assert_eq!(
            r.read("assets/a.png").unwrap().unwrap(),
            b"\x89PNG\r\n\x1a\nfake"
        );
        assert_eq!(
            r.read("scenes/main.scene").unwrap().unwrap(),
            b"scene_version=1\n"
        );
        assert!(r.read("nope.txt").unwrap().is_none());
        assert_eq!(r.index().len(), 3);
        r.verify_all().unwrap();
    }

    #[test]
    fn deterministic_bytes() {
        let a = build(&[("b.txt", b"bbb".as_slice()), ("a.txt", b"aaa")]);
        let b = build(&[("a.txt", b"aaa".as_slice()), ("b.txt", b"bbb")]);
        assert_eq!(a.bytes, b.bytes, "条目顺序不应影响产物");
        assert_eq!(a.checksum, b.checksum);
    }

    #[test]
    fn entries_are_sorted_and_neighbours_share_a_block() {
        let built = build(&[
            ("z/last.txt", b"z".as_slice()),
            ("a/first.txt", b"a"),
            ("m/mid.txt", b"m"),
        ]);
        let index = BundleIndex::parse(&built.bytes).unwrap();
        let paths: Vec<&str> = index.paths().collect();
        assert_eq!(paths, vec!["a/first.txt", "m/mid.txt", "z/last.txt"]);
        assert_eq!(built.blocks, 1);
    }

    #[test]
    fn incompressible_blocks_are_stored() {
        let payload = noise(64 * 1024, 0x1234_5678_9abc_def0);
        let built = build(&[("assets/noise.bin", payload.as_slice())]);
        let index = BundleIndex::parse(&built.bytes).unwrap();
        assert_eq!(index.blocks().len(), 1);
        assert_eq!(
            index.blocks()[0].flags & BLOCK_FLAG_COMPRESSED,
            0,
            "压不动的块必须原样存"
        );
        assert_eq!(index.blocks()[0].stored_len, index.blocks()[0].raw_len);
        let mut r = reader(&built);
        assert_eq!(r.read("assets/noise.bin").unwrap().unwrap(), payload);
    }

    #[test]
    fn oversized_entry_gets_its_own_block() {
        let big = vec![7u8; DEFAULT_BLOCK_SIZE as usize + 4096];
        let built = build(&[("big.bin", big.as_slice()), ("small.txt", b"x")]);
        let index = BundleIndex::parse(&built.bytes).unwrap();
        assert_eq!(index.blocks().len(), 2, "大条目必须独占块");
        let mut r = reader(&built);
        assert_eq!(r.read("big.bin").unwrap().unwrap(), big);
        assert_eq!(r.read("small.txt").unwrap().unwrap(), b"x");
        r.verify_all().unwrap();
    }

    #[test]
    fn corrupt_header_is_rejected() {
        let built = build(&[("a.txt", b"aaa".as_slice())]);

        // ① 改块大小字段：header_fnv 失配 → Header
        let mut touched = built.bytes.clone();
        touched[8] ^= 0xff;
        let error = BundleIndex::parse(&touched).unwrap_err();
        assert!(matches!(error, BundleError::Header(_)), "得到 {error:?}");

        // ② 改版本号：先命中版本门 → UnsupportedVersion
        let mut bumped = built.bytes.clone();
        bumped[4] = 9;
        let error = BundleIndex::parse(&bumped).unwrap_err();
        assert!(
            matches!(error, BundleError::UnsupportedVersion(9)),
            "得到 {error:?}"
        );

        // ③ 魔数不对：直接拒绝
        let mut magic = built.bytes.clone();
        magic[0] = b'X';
        let error = BundleIndex::parse(&magic).unwrap_err();
        assert!(matches!(error, BundleError::Header(_)), "得到 {error:?}");
    }

    #[test]
    fn checksum_mismatch_is_loud() {
        // 用 store 块，保证改一个字节不会破坏 zstd 帧结构，从而命中校验和分支。
        let payload = noise(4096, 0xdead_beef_1234_5678);
        let mut built = build(&[("assets/noise.bin", payload.as_slice())]);
        built.bytes[HEADER_LEN] ^= 0xff;
        let index = BundleIndex::parse(&built.bytes).unwrap();
        let mut r = BundleReader::new(index, MemSource::new(built.bytes.clone()));
        let error = r.read("assets/noise.bin").unwrap_err();
        assert!(
            matches!(error, BundleError::Checksum { .. }),
            "得到 {error:?}"
        );
        // 跳过校验仍能读出（导出/对比路径）
        let mut r = BundleReader::new(
            BundleIndex::parse(&built.bytes).unwrap(),
            MemSource::new(built.bytes.clone()),
        );
        let got = r.read_unchecked("assets/noise.bin").unwrap().unwrap();
        assert_eq!(got.len(), payload.len());
        assert_ne!(got, payload);
    }

    #[test]
    fn block_cache_decodes_once() {
        let built = build(&[
            ("a.txt", "aaaa".repeat(1000).as_bytes()),
            ("b.txt", "bbbb".repeat(1000).as_bytes()),
        ]);
        let mut r = reader(&built);
        r.read("a.txt").unwrap();
        r.read("b.txt").unwrap();
        r.read("a.txt").unwrap();
        assert_eq!(r.decoded_blocks(), 1, "同块复用不应重复解压");
    }

    #[test]
    fn index_is_compressed_and_records_salt() {
        let mut builder = BundleBuilder::new(DEFAULT_BLOCK_SIZE);
        for i in 0..64 {
            builder
                .add(&format!("assets/art/frame_{i:02}.png"), vec![i as u8; 512])
                .unwrap();
        }
        let built = builder.build().unwrap();
        let index = BundleIndex::parse(&built.bytes).unwrap();
        assert_eq!(index.len(), 64);
        assert!(
            built.index_len < built.index_raw_len,
            "索引应当被压过（{} vs {}）",
            built.index_len,
            built.index_raw_len
        );
        assert_ne!(index.salt(), 0);
        assert_eq!(index.zstd_version(), zstd::zstd_safe::version_number());
        assert!(built.len() < 64 * 512 + 4096, "整包应显著小于原始");
    }

    #[test]
    fn duplicate_and_bad_paths_are_rejected() {
        let mut builder = BundleBuilder::new(DEFAULT_BLOCK_SIZE);
        builder.add("a.txt", b"1".to_vec()).unwrap();
        assert!(matches!(
            builder.add("a.txt", b"2".to_vec()).unwrap_err(),
            BundleError::DuplicateEntry(_)
        ));
        assert!(matches!(
            builder.add("", b"3".to_vec()).unwrap_err(),
            BundleError::InvalidPath(_)
        ));
    }

    /// 构建端必须与读取端同一口径：非规范化路径要么被规范化，要么当场拒绝。
    /// 否则会出现"包建成功了，运行期 `BundleSet::key_of` 永远查不到"的静默缺件。
    #[test]
    fn builder_normalizes_or_rejects_non_canonical_paths() {
        let mut builder = BundleBuilder::new(DEFAULT_BLOCK_SIZE);
        builder.add(r"assets\art\\a.png", b"1".to_vec()).unwrap();
        builder.add("./assets/art/b.png", b"2".to_vec()).unwrap();
        // 规范化之后与前一条同名 → 重复条目（而不是两个查不到的键）。
        assert!(matches!(
            builder
                .add(r"assets\\art/a.png", b"3".to_vec())
                .unwrap_err(),
            BundleError::DuplicateEntry(_)
        ));
        for bad in [
            "../escape.png",
            "/abs.png",
            r"C:\win.png",
            "a/../../b.png",
            ".",
        ] {
            assert!(
                matches!(
                    builder.add(bad, b"x".to_vec()).unwrap_err(),
                    BundleError::InvalidPath(_)
                ),
                "{bad:?} 必须被拒绝"
            );
        }
        let built = builder.build().unwrap();
        let index = BundleIndex::parse(&built.bytes).unwrap();
        let paths: Vec<&str> = index.paths().collect();
        assert_eq!(paths, vec!["assets/art/a.png", "assets/art/b.png"]);
    }

    #[test]
    fn key_of_uses_canonical_project_relative_paths() {
        let set = BundleSet::new("/project");
        assert_eq!(
            set.key_of(Path::new("/project/scripts/./data//items.lua")),
            Some("scripts/data/items.lua".to_owned())
        );
        assert_eq!(set.key_of(Path::new("/project/../outside.lua")), None);
        assert_eq!(set.key_of(Path::new("/project2/scripts/main.lua")), None);
    }

    #[test]
    fn truncated_bundle_is_rejected_not_panicking() {
        let built = build(&[("a.txt", b"aaa".as_slice())]);
        for cut in [0, 1, 8, HEADER_LEN - 1, HEADER_LEN] {
            let error = BundleIndex::parse(&built.bytes[..cut]).unwrap_err();
            assert!(matches!(error, BundleError::Header(_)), "cut={cut}");
        }
    }

    /// 正对照：手工装配的"合法"容器必须能解析并读回。
    ///
    /// 没有这一条，下面那批负向用例可能只是"手拼的字节本来就坏"，挡住的不是不变量。
    #[test]
    fn crafted_container_is_valid_until_an_invariant_is_broken() {
        let bytes = Craft::new().single("a.txt", b"aaa").bytes();
        let index = BundleIndex::parse(&bytes).unwrap();
        let mut r = BundleReader::new(index, MemSource::new(bytes));
        assert_eq!(r.read("a.txt").unwrap().unwrap(), b"aaa");
    }

    /// 头里的 `index_raw_len` 以前有 `!= 0` 的逃逸口：写成 0 就跳过交叉校验。
    /// 写侧永远写真实长度（索引至少 16 B），所以 0 只能是损坏。
    #[test]
    fn recorded_index_len_is_always_cross_checked() {
        let mut craft = Craft::new().single("a.txt", b"aaa");
        craft.index_raw_len = Some(0);
        let error = BundleIndex::parse(&craft.bytes()).unwrap_err();
        assert!(matches!(error, BundleError::Index(_)), "得到 {error:?}");

        // 少报一个字节同样要红（不只看 0 这档）。
        let mut craft = Craft::new().single("a.txt", b"aaa");
        craft.index_raw_len = Some(1);
        let error = BundleIndex::parse(&craft.bytes()).unwrap_err();
        assert!(matches!(error, BundleError::Index(_)), "得到 {error:?}");
    }

    #[test]
    fn block_size_zero_is_rejected() {
        let mut craft = Craft::new().single("a.txt", b"aaa");
        craft.block_size = 0;
        let error = BundleIndex::parse(&craft.bytes()).unwrap_err();
        assert!(matches!(error, BundleError::Header(_)), "得到 {error:?}");
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut craft = Craft::new().single("a.txt", b"aaa");
        craft.trailing = b"junk-from-a-bad-copy".to_vec();
        let error = BundleIndex::parse(&craft.bytes()).unwrap_err();
        assert!(
            matches!(&error, BundleError::Header(text) if text.contains("尾部")),
            "得到 {error:?}"
        );
    }

    /// 索引**自己**尾部多出字节（不是文件尾部，而是声明记录之后）以前被静默忽略：
    /// `index_raw_len` 只约束解出来多长，不约束这些字节有没有人消费。
    #[test]
    fn trailing_bytes_inside_index_are_rejected() {
        let mut craft = Craft::new().single("a.txt", b"aaa");
        craft.index_trailing = b"extra-index-bytes".to_vec();
        let error = BundleIndex::parse(&craft.bytes()).unwrap_err();
        assert!(
            matches!(&error, BundleError::Index(text) if text.contains("索引尾部")),
            "得到 {error:?}"
        );
    }

    /// 解压上限取自容器声明的长度：坏容器不能用几 KB 的 zstd 帧要求几百 MB 的输出。
    #[test]
    fn zstd_decode_respects_declared_limit() {
        let frame = zstd::bulk::compress(&vec![7u8; 4096], 3).unwrap();
        let error = zstd_decode(&frame, 64).expect_err("超过声明长度必须报错");
        assert!(
            matches!(&error, BundleError::Source(text) if text.contains("超过声明")),
            "得到 {error:?}"
        );
        // 正对照：恰好等于声明长度必须通过（防"上限写成 < 而不是 >"）。
        assert_eq!(zstd_decode(&frame, 4096).unwrap(), vec![7u8; 4096]);
    }

    /// 非空块也必须被至少一个条目引用。以前只检查空块，于是"多塞一段 payload 再让
    /// 块表里有个没人引用的块"可以解析成功——那段字节永远读不到、也永远没人解释。
    #[test]
    fn unreferenced_non_empty_block_is_rejected() {
        let mut craft = Craft::new().single("a.txt", b"aaaa");
        craft.blocks.push((0, 4, 4));
        craft.payload.extend_from_slice(b"bbbb");
        let error = BundleIndex::parse(&craft.bytes()).unwrap_err();
        assert!(
            matches!(&error, BundleError::Index(text) if text.contains("没有任何条目引用")),
            "得到 {error:?}"
        );
    }

    /// 同一块里两条路径指向重叠的字节段：**静默的内容错乱**通道。
    ///
    /// 条目 FNV 也存在索引里，所以"改 offset 指到邻居的字节 + 抄邻居的校验和"能过所有
    /// 校验和检查，游戏读到的是另一张图。写侧逐条目顺序切块、不可能重叠，故解析期拒绝。
    #[test]
    fn overlapping_entries_in_one_block_are_rejected() {
        let mut craft = Craft::new().single("a.txt", b"aaaabbbb");
        craft.entries.push(CraftEntry {
            path: "b.txt".to_owned(),
            flags: 0,
            block: 0,
            offset: 4,
            len: 4,
            checksum: checksum(b"bbbb"),
        });
        let error = BundleIndex::parse(&craft.bytes()).unwrap_err();
        assert!(
            matches!(&error, BundleError::Index(text) if text.contains("重叠")),
            "得到 {error:?}"
        );

        // 完全同一段（不同路径）同样是重叠，不能被"相同区间"混过去。
        let mut alias = Craft::new().single("a.txt", b"aaaa");
        alias.entries.push(CraftEntry {
            path: "copy.txt".to_owned(),
            flags: 0,
            block: 0,
            offset: 0,
            len: 4,
            checksum: checksum(b"aaaa"),
        });
        let error = BundleIndex::parse(&alias.bytes()).unwrap_err();
        assert!(
            matches!(&error, BundleError::Index(text) if text.contains("重叠")),
            "得到 {error:?}"
        );
    }

    /// 相邻不重叠的区间必须照常通过（负向用例不能顺手挡掉正常索引）。
    #[test]
    fn adjacent_entries_and_shared_empty_offsets_are_accepted() {
        let mut craft = Craft::new().single("a.txt", b"aaaabbbb");
        craft.entries.push(CraftEntry {
            path: "b.txt".to_owned(),
            flags: 0,
            block: 0,
            offset: 4,
            len: 4,
            checksum: checksum(b"bbbb"),
        });
        craft.entries[0].len = 4;
        craft.entries[0].checksum = checksum(b"aaaa");
        let index = BundleIndex::parse(&craft.bytes()).expect("相邻区间合法");
        assert!(index.contains("a.txt") && index.contains("b.txt"));

        // 多个空文件共享块尾的同一个 offset 是写侧的常态，不能算重叠。
        let mut empties = Craft::new().single("a.txt", b"aaaa");
        for path in ["e1.txt", "e2.txt"] {
            empties.entries.push(CraftEntry {
                path: path.to_owned(),
                flags: 0,
                block: 0,
                offset: 4,
                len: 0,
                checksum: checksum(b""),
            });
        }
        let index = BundleIndex::parse(&empties.bytes()).expect("共享 offset 的零长条目合法");
        assert_eq!(index.len(), 3);
        assert!(index.entry("e2.txt").is_some());
    }

    /// 块里有一段没有任何条目解释的字节（空洞）：读者永远读不到它，
    /// 所以它只可能是"夹带的数据"或"索引被动过"。
    #[test]
    fn gaps_inside_a_block_are_rejected() {
        let mut craft = Craft::new().single("a.txt", b"aaaabbbb");
        // 条目只覆盖前 4 B，块却声明 8 B。
        craft.entries[0].len = 4;
        craft.entries[0].checksum = checksum(b"aaaa");
        let error = BundleIndex::parse(&craft.bytes()).unwrap_err();
        assert!(
            matches!(&error, BundleError::Index(text) if text.contains("空洞")),
            "得到 {error:?}"
        );
    }

    /// 计划表里的组内路径必须是规范形式且严格升序：读取端用二分查（`PlannedGroup::contains`），
    /// 乱序不会报错，只会**静默查不到**——"包里有这个文件，运行期说没有"。
    #[test]
    fn plan_paths_must_be_canonical_and_ascending() {
        use crate::bundle::plan::{BundlePlan, PLAN_FORMAT, PlannedGroup};
        let plan = |paths: Vec<&str>| {
            BundlePlan {
                schema: 1,
                format: PLAN_FORMAT.to_owned(),
                block_size: 640,
                groups: vec![PlannedGroup {
                    name: "core".to_owned(),
                    unit: "main".to_owned(),
                    bundle: "bundles/core.rlb".to_owned(),
                    hash: String::new(),
                    entries: paths.len() as u64,
                    raw_bytes: 0,
                    bundle_bytes: 0,
                    paths: paths.into_iter().map(str::to_owned).collect(),
                }],
                checksum: String::new(),
                keep: None,
            }
            .sealed()
        };
        for bad in [
            vec!["scripts/main.lua", "scenes/main.scene"], // 乱序
            vec!["a.txt", "a.txt"],                        // 重复（等于不升序）
            vec!["assets\\a.png"],                         // 反斜杠
            vec!["./a.png"],                               // 非规范
            vec!["/abs.png"],                              // 绝对路径
        ] {
            let bytes = serde_json::to_vec(&plan(bad.clone())).unwrap();
            let error = BundlePlan::parse(&bytes).unwrap_err();
            assert!(
                error.contains("bundle_plan 组 core"),
                "{bad:?} 必须被拒绝，得到 {error}"
            );
        }
        // 正对照：升序 + 规范形式照常通过（不能顺手把正常计划挡掉）。
        let bytes = serde_json::to_vec(&plan(vec!["assets/a.png", "scripts/main.lua"])).unwrap();
        assert!(BundlePlan::parse(&bytes).is_ok());
    }

    /// 一个 16 B 的坏索引可以声明 40 亿条目。以前会先 `with_capacity` 再报"索引截断"——
    /// 原生是 OOM、wasm 是 abort，都不是"坏容器报错"。现在必须在**分配之前**失败。
    #[test]
    fn declared_table_counts_cannot_exhaust_memory() {
        let mut craft = Craft::new().single("a.txt", b"aaa");
        let bytes = craft.clone().bytes();
        // 索引里前 8 B 是 prologue，接着是 entry_count(4) / block_count(4)。
        let index_offset = HEADER_LEN + craft.payload.len();
        craft.payload.shrink_to_fit();
        for field in 0..2 {
            let at = index_offset + 8 + field * 4;
            let mut bytes = bytes.clone();
            bytes[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            let error = BundleIndex::parse(&bytes).unwrap_err();
            assert!(
                matches!(&error, BundleError::Index(text) if text.contains("至少需要")),
                "field={field} 得到 {error:?}"
            );
        }
    }

    /// 未定义标志位 = 损坏或"改了格式没升版本"，两种情况都该点名，不能当 0 用。
    #[test]
    fn unknown_flag_bits_are_rejected() {
        let mut flat = Craft::new().single("a.txt", b"aaa");
        flat.header_flags = 0b10;
        let error = BundleIndex::parse(&flat.bytes()).unwrap_err();
        assert!(
            matches!(&error, BundleError::Header(text) if text.contains("未定义的位")),
            "头标志：{error:?}"
        );

        let mut block = Craft::new().single("a.txt", b"aaa");
        block.blocks[0].0 = 0b1000_0000;
        let error = BundleIndex::parse(&block.bytes()).unwrap_err();
        assert!(
            matches!(&error, BundleError::Index(text) if text.contains("块 0 标志")),
            "块标志：{error:?}"
        );

        let mut entry = Craft::new().single("a.txt", b"aaa");
        entry.entries[0].flags = 0b1000_0000;
        let error = BundleIndex::parse(&entry.bytes()).unwrap_err();
        assert!(
            matches!(&error, BundleError::Index(text) if text.contains("条目 a.txt 标志")),
            "条目标志：{error:?}"
        );
    }

    /// 索引里的路径必须已经是**规范化**形态，否则运行期 `BundleSet::key_of` 的规范化键
    /// 永远匹配不上——"包里有这个文件，游戏说没有"，且没有任何日志。
    #[test]
    fn non_canonical_entry_paths_are_rejected() {
        for bad in [
            r"assets\art\a.png",
            "./assets/a.png",
            "assets//a.png",
            "/absolute.png",
            "../escape.png",
            "assets/./a.png",
            "",
        ] {
            let error = BundleIndex::parse(&Craft::new().single(bad, b"x").bytes()).unwrap_err();
            assert!(
                matches!(&error, BundleError::InvalidPath(path) if path == bad),
                "{bad:?} 必须被拒绝，得到 {error:?}"
            );
        }
        // 规范化形态照样通过（负向用例不能顺手把正常名字也挡了）。
        let index =
            BundleIndex::parse(&Craft::new().single("assets/art/a.png", b"x").bytes()).unwrap();
        assert!(index.contains("assets/art/a.png"));
    }

    /// `raw_len` 超过 `block_size` 只有"单条目自身超块、独占一块"这一种合法来源。
    /// 没有这条，"raw_len 撒 4 GB 的谎"会被解析期放行，读侧照着它解压/判缓存档。
    #[test]
    fn oversized_block_must_be_covered_by_one_entry() {
        let mut lying = Craft::new().single("a.txt", b"aaaa");
        lying.block_size = 1024;
        lying.blocks[0].2 = 4096; // raw_len 说谎，条目只覆盖 4 B
        let error = BundleIndex::parse(&lying.bytes()).unwrap_err();
        assert!(
            matches!(&error, BundleError::Index(text) if text.contains("超过 block_size")),
            "得到 {error:?}"
        );

        // 同一块，但有一个条目完整覆盖它 ⇒ 结构合法（内容违法由 `block()` 在读时报）。
        let mut covered = Craft::new().single("a.txt", b"aaaa");
        covered.block_size = 1024;
        covered.blocks[0].2 = 4096;
        covered.entries[0].len = 4096;
        covered.entries[0].checksum = checksum(b"aaaa");
        assert!(BundleIndex::parse(&covered.bytes()).is_ok());
    }

    #[test]
    fn block_table_without_entries_is_rejected() {
        let mut craft = Craft::new();
        craft.blocks.push((0, 4, 4));
        craft.payload.extend_from_slice(b"aaaa");
        let error = BundleIndex::parse(&craft.bytes()).unwrap_err();
        assert!(
            matches!(&error, BundleError::Index(text) if text.contains("块表有 1 块")),
            "得到 {error:?}"
        );
    }

    /// 写侧曾经把"只装空文件的那个块"连同条目一起丢掉：容器建成功、`read` 返回 `None`、
    /// 零报错。空文件必须能原样往返（并且只产出 1 个空块）。
    #[test]
    fn empty_entries_survive_the_roundtrip() {
        let built = build(&[("empty.txt", b"".as_slice())]);
        let index = BundleIndex::parse(&built.bytes).unwrap();
        assert_eq!(index.len(), 1, "空文件条目不能消失");
        assert_eq!(index.blocks().len(), 1);
        assert_eq!(index.blocks()[0].raw_len, 0);
        let mut r = reader(&built);
        assert_eq!(r.read("empty.txt").unwrap().unwrap(), Vec::<u8>::new());
        r.verify_all().unwrap();

        // 与非空条目同包时，空条目落在同一块里、偏移等于块尾（顺序由写侧按名字排，
        // 所以这里按"块尾"断言而不是写死数字）。
        let built = build(&[
            ("a.txt", b"aaa".as_slice()),
            ("empty.txt", b""),
            ("b.txt", b"bb"),
        ]);
        let index = BundleIndex::parse(&built.bytes).unwrap();
        assert_eq!(index.len(), 3);
        let empty = index.entry("empty.txt").unwrap();
        assert_eq!(empty.len, 0);
        assert_eq!(
            empty.offset,
            index.blocks()[empty.block as usize].raw_len,
            "空条目应正好落在块尾"
        );
        let mut r = reader(&built);
        assert_eq!(r.read("empty.txt").unwrap().unwrap(), Vec::<u8>::new());
        r.verify_all().unwrap();
    }
}
