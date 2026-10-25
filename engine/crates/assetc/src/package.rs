//! 发行包：松散目录复制、压缩/加密基准、RLB 分组打包与包内报告。
//!
//! 从 `lib.rs` 纯搬运而来（M77-05 §2），行为未变。
use anyhow::anyhow;
use rustlua_assets::resource_kind::ResourceKind;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use crate::*;

pub(crate) fn copy_file(source: &Path, destination: &Path) -> anyhow::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination).map_err(|error| {
        anyhow!(
            "复制失败 {} → {}: {error}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn copy_dir(source: &Path, destination: &Path) -> anyhow::Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    let mut files = Vec::new();
    collect_files(source, &mut files)?;
    for (file, _) in files {
        let relative = file
            .strip_prefix(source)
            .expect("file must be inside source dir");
        copy_file(&file, &destination.join(relative))?;
    }
    Ok(())
}

/// FNV-1a 校验和（用于清单，非加密用途）。
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 复制目录；可选对文件进行发布编码（运行时自动解码）。
fn copy_dir_encoded(
    source: &Path,
    destination: &Path,
    compress: bool,
    encrypt: bool,
    should_compress: fn(&Path) -> bool,
    should_encrypt: fn(&Path) -> bool,
) -> anyhow::Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    let mut files = Vec::new();
    collect_files(source, &mut files)?;
    for (file, _) in files {
        let relative = file
            .strip_prefix(source)
            .expect("file must be inside source dir");
        let dest = destination.join(relative);
        let compress_file = compress && should_compress(&file);
        let encrypt_file = encrypt && should_encrypt(&file);
        if compress_file || encrypt_file {
            let bytes = fs::read(&file)?;
            let encoded = rustlua_assets::pack::encode(&bytes, compress_file, encrypt_file);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&dest, &encoded)?;
        } else {
            copy_file(&file, &dest)?;
        }
    }
    Ok(())
}

fn never_encode(_: &Path) -> bool {
    false
}

fn is_encryptable_data(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("lua") | Some("scene"))
}

pub(crate) fn is_compressible_asset(path: &Path) -> bool {
    // mp3 虽然自身是压缩格式，实测工程内 16 个 clips 走 pack 的 gzip 默认档仍从
    // 1,497,215 B 降到 1,441,547 B（-3.7%）：低码率单声道流的帧头与填充字节可压。
    matches!(
        extension(path).as_deref(),
        Some("png")
            | Some("jpg")
            | Some("jpeg")
            | Some("wav")
            | Some("ogg")
            | Some("mp3")
            | Some("ttf")
            | Some("otf")
            | Some("glb")
    )
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// 按资产类型统计压缩前后大小、压缩耗时和解压耗时。
pub fn benchmark_compression(project: &Path) -> anyhow::Result<Vec<CompressionBenchmarkEntry>> {
    ensure_project(project)?;
    let assets = project.join("assets");
    if !assets.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    collect_files(&assets, &mut files)?;
    let mut entries = BTreeMap::<String, CompressionBenchmarkEntry>::new();
    for (file, _) in files {
        if !is_compressible_asset(&file) {
            continue;
        }
        let bytes =
            fs::read(&file).map_err(|error| anyhow!("读取失败 {}: {error}", file.display()))?;
        let kind = ResourceKind::from_path(&file).token().to_owned();
        let started = std::time::Instant::now();
        let compressed = rustlua_assets::pack::encode(&bytes, true, false);
        let compress_time = started.elapsed();
        let started = std::time::Instant::now();
        let decompressed =
            rustlua_assets::pack::decode(&compressed).map_err(|error| anyhow!("{error}"))?;
        let decompress_time = started.elapsed();
        if decompressed != bytes {
            return Err(anyhow!("压缩往返校验失败: {}", file.display()));
        }
        let entry = entries
            .entry(kind.clone())
            .or_insert_with(|| CompressionBenchmarkEntry {
                kind,
                ..CompressionBenchmarkEntry::default()
            });
        entry.files += 1;
        entry.original_bytes += bytes.len() as u64;
        entry.compressed_bytes += compressed.len() as u64;
        entry.compress_time += compress_time;
        entry.decompress_time += decompress_time;
    }
    Ok(entries.into_values().collect())
}

/// 打包时复制到输出目录的项目数据子目录。
const PACKAGE_SOURCE_DIRS: &[&str] = &["scenes", "scripts", "assets"];

/// 校验发布输出目录与项目目录的关系。
///
/// 拒绝三类**真会出事**的组合：
/// 1. `out == project`：直接覆写项目文件；
/// 2. `out` 位于 `scenes/`、`scripts/`、`assets/` 之内：这几个目录正是打包的输入，
///    把产物写进去会让 `collect_files` 一边遍历一边写入（自我复制/无限增长），
///    而且下一次打包会把产物当输入；
/// 3. `project` 位于 `out` 之内：会把项目连同产物一起递归进包。
///
/// **允许** `out` 位于项目内的其它位置（如 `builds/`）：`scan`/`collect_data_files`
/// 只遍历 `project.toml` 与上述三个子目录，因此 `builds/` 下的发布产物不会被当成
/// 项目数据；`scripts/build/package.ps1` 的原子发布正是这样布局的
/// （`<project>/builds/<platform>/.staging-*/pack`）。
fn ensure_distinct_output(project: &Path, out: &Path) -> anyhow::Result<()> {
    let project_abs = canonical_or_absolute(project)?;
    let out_abs = canonical_or_absolute(out)?;
    if out_abs == project_abs {
        return Err(anyhow!("输出目录不能与项目目录相同"));
    }
    for sub in PACKAGE_SOURCE_DIRS {
        let source_abs = canonical_or_absolute(&project_abs.join(sub))?;
        if out_abs.starts_with(&source_abs) {
            return Err(anyhow!(
                "输出目录不能位于项目数据目录内部（{}）：{}",
                sub,
                out_abs.display()
            ));
        }
    }
    if project_abs.starts_with(&out_abs) {
        return Err(anyhow!(
            "项目目录不能位于输出目录内部：{} ⊂ {}",
            project_abs.display(),
            out_abs.display()
        ));
    }
    Ok(())
}

/// 规范化为绝对路径：已存在则用 `canonicalize`，否则规范化其最近存在的祖先后拼回
/// 剩余部分（输出目录通常还不存在）。
pub(crate) fn canonical_or_absolute(path: &Path) -> anyhow::Result<PathBuf> {
    if let Ok(resolved) = fs::canonicalize(path) {
        return Ok(resolved);
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| anyhow!("读取当前目录失败: {error}"))?
            .join(path)
    };
    let mut existing = absolute.clone();
    let mut tail = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut resolved = fs::canonicalize(&existing).unwrap_or(existing);
    for component in tail.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

/// 把 `project.toml` 声明的工程图（`splash_image` / `icon_image`）按声明的相对路径复制进包。
///
/// 为什么不能只靠 `assets/` 那一趟目录复制：启动图是**窗口出现之前就要读**的资源，工程常把
/// 它单独放成顶层目录（`splash/splash.png`，game_jhyt 就这样）。只扫 `assets/` 的话它根本不
/// 进包，运行期 `splash::startup_image` 读不到 ⇒ **静默回落**到编译期内置图，日志里仅有
/// 一条 warn——"换了启动图却没生效"就是这条链。
///
/// 与 `font` 同一条纪律：**声明即随包**（`--drop-undeclared-fonts` 也只保留声明过的字体）。
/// 缺失的文件不在这里报错（那是 `game --check` 的资源校验语境），但越出工程根的路径必须
/// 拒绝：`fs::copy` 会把项目外的文件悄悄塞进发布包。
fn copy_declared_images(project: &Path, out: &Path) -> anyhow::Result<()> {
    for key in ["splash_image", "icon_image"] {
        let Some(value) = project_key_value(project, key) else {
            continue;
        };
        // Windows 工程里两种分隔符都有；包内条目名统一用 `/`（与 manifest/容器同一口径）。
        let relative = value.replace('\\', "/");
        let path = Path::new(&relative);
        if relative.contains(':')
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            return Err(anyhow!(
                "project.toml 的 {key} 必须是工程内相对路径（不得含 ..）：{relative}"
            ));
        }
        let source = project.join(path);
        if !source.is_file() {
            continue;
        }
        let destination = out.join(path);
        if destination.is_file() {
            // 已经在 `assets/` 下：目录复制那一趟已经带过去了，不再覆盖（也不必）。
            continue;
        }
        copy_file(&source, &destination)?;
    }
    Ok(())
}

/// 生成发布资源包：复制 `project.toml`、`scenes/`、`scripts/`、`assets/`
/// 到 `out`，并写入 `manifest.txt`（路径、大小、校验和）。
/// 此外还按 `project.toml` 的 `splash_image` / `icon_image` 把**声明的**工程图带进包
/// （它们可以不在 `assets/` 下，见 `copy_declared_images`）。
/// `compress` 对 `assets/` 下支持压缩的资产生效；`encrypt` 仅对 Lua 与场景文件生效。
/// `drop_undeclared_fonts`（微信形态，docs/52 配额门）：删掉 `assets/fonts/` 下**没被
/// `project.toml` 声明**（`font`/`font_fallback`）的字体二进制——它们是开发树对照物，
/// 宿主只 `wx.loadFont` 声明那一份，随包发就是把配额门压在不被读取的死重上。
/// 剔除发生在 `manifest.txt` 生成**之前**，清单与磁盘天然一致。
pub fn package(
    project: &Path,
    out: &Path,
    compress: bool,
    encrypt: bool,
    drop_undeclared_fonts: bool,
) -> anyhow::Result<PackageManifest> {
    ensure_project(project)?;
    ensure_distinct_output(project, out)?;
    copy_file(&project.join("project.toml"), &out.join("project.toml"))?;
    copy_dir_encoded(
        &project.join("scenes"),
        &out.join("scenes"),
        false,
        encrypt,
        never_encode,
        is_encryptable_data,
    )?;
    copy_dir_encoded(
        &project.join("scripts"),
        &out.join("scripts"),
        false,
        encrypt,
        never_encode,
        is_encryptable_data,
    )?;
    copy_dir_encoded(
        &project.join("assets"),
        &out.join("assets"),
        compress,
        false,
        is_compressible_asset,
        never_encode,
    )?;
    copy_declared_images(project, out)?;
    // 首包字体：收集项目用到的字符清单，运行时预生成字形（首帧文字不卡）。
    let chars = rustlua_assets_import::char_collect::collect_project_chars(project);
    fs::write(out.join("chars.txt"), &chars)?;
    // 发布子集化：把主字体裁剪到用到的字符，缩小体积（原 `subset-font.py` 的 Rust 版）。
    // 子集化失败（如 CFF 字体）时保留完整字体。
    if let Some(font_path) = project_font_key(project) {
        let src = project.join(&font_path);
        let dst = out.join(&font_path);
        if src.is_file()
            && dst.is_file()
            && let Ok(bytes) = fs::read(&src)
            && let Ok(subset) = rustlua_render::font_subset::subset_ttf(&bytes, &chars)
        {
            fs::write(&dst, subset)?;
        }
    }
    if drop_undeclared_fonts {
        let declared = project_font_declarations(project);
        let fonts_dir = out.join("assets").join("fonts");
        if fonts_dir.is_dir() {
            for entry in fs::read_dir(&fonts_dir)? {
                let path = entry?.path();
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !matches!(ext.as_str(), "ttf" | "otf" | "woff" | "woff2") {
                    continue;
                }
                let relative = path
                    .strip_prefix(out)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                if declared.contains(&relative) {
                    continue;
                }
                fs::remove_file(&path)?;
            }
        }
    }
    write_dependency_index(project, &out.join("dependencies.json"))?;

    let mut files = Vec::new();
    collect_files(out, &mut files)?;
    files.retain(|(path, _)| path != &out.join("manifest.txt"));
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut manifest = BTreeMap::new();
    let mut lines = vec![format!("assetc_manifest=1\t{PACKAGE_VERSION}")];
    for (path, _) in files {
        // 这里曾经是 `expect`：`collect_files(out)` 的结果理论上一定在 out 下，
        // 但打包中途 panic 会留下半个发布目录。改成可定位的错误。
        let relative = path
            .strip_prefix(out)
            .map_err(|_| {
                anyhow!(
                    "清单文件不在输出目录内：{} (out={})",
                    path.display(),
                    out.display()
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&path)?;
        let entry = FileEntry {
            size: bytes.len() as u64,
            checksum: fnv1a(&bytes),
        };
        lines.push(format!(
            "{relative}\t{}\t{:016x}",
            entry.size, entry.checksum
        ));
        manifest.insert(relative, entry);
    }
    fs::write(out.join("manifest.txt"), lines.join("\n"))?;
    Ok(PackageManifest { entries: manifest })
}

/// 打包形态（`docs/67` 批 A/批 C）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundleMode {
    /// 目录形态：一文件一资源（今天的 `assetc package`）。
    Loose,
    /// 单包形态：全部资源进一个 RLB 容器，输出根只留骨架。
    Single,
    /// 分组形态：按 `bundles.json`（缺省用内置内容域规则）切成多个容器 + `bundle_plan.json`。
    Grouped,
}

impl BundleMode {
    /// 命令行/日志用的短名。
    pub fn token(self) -> &'static str {
        match self {
            Self::Loose => "none",
            Self::Single => "single",
            Self::Grouped => "grouped",
        }
    }

    /// `--bundle <值>` 解析。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" | "loose" => Some(Self::Loose),
            "single" => Some(Self::Single),
            "grouped" | "groups" => Some(Self::Grouped),
            _ => None,
        }
    }
}

/// 打包体积报告（写进包内 `package-report.json`，也用于命令行摘要）。
///
/// **必须确定性**：只含由输入决定的值，不含耗时/时间戳/进程信息——否则同一份工程两次打包
/// 产物不同，`manifest` 与 golden 门都会抖（`docs/67` §4.5）。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PackageReport {
    /// 报告 schema，改动字段含义时自增。
    pub schema: u32,
    /// `none` / `single`。
    pub mode: String,
    /// 输出目录里的文件数（含清单与报告自身）。
    pub files: u64,
    /// 包内条目数（目录形态下等于文件数减清单）。
    pub entries: u64,
    /// 块数（目录形态恒 0）。
    pub blocks: u64,
    /// 条目原始字节合计。
    pub raw_bytes: u64,
    /// 容器落盘字节（目录形态 = `raw_bytes`）。
    pub bundle_bytes: u64,
    /// 索引落盘 / 解压后字节。
    pub index_bytes: u64,
    pub index_raw_bytes: u64,
    /// 压缩比 × 1000（整数，避免浮点在报告里抖动）。
    pub ratio_milli: u64,
    /// 容器文件的包内相对路径（目录形态为空；分组形态为第一个组，其余见 `groups`）。
    pub bundle_path: String,
    /// 分组形态的逐组结果（`single` / 目录形态为空）。
    #[serde(default)]
    pub groups: Vec<GroupReport>,
}

/// 报告里的单组结果。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GroupReport {
    pub name: String,
    /// `main` / `sub` / `cdn`。
    pub unit: String,
    pub entries: u64,
    pub raw_bytes: u64,
    pub bundle_bytes: u64,
    /// 容器在包内的相对路径。
    pub bundle: String,
}

impl PackageReport {
    fn ratio_milli(raw: u64, stored: u64) -> u64 {
        if raw == 0 {
            1000
        } else {
            stored.saturating_mul(1000).checked_div(raw).unwrap_or(1000)
        }
    }

    /// 压缩比（浮点，仅供打印/测试）。
    pub fn ratio(&self) -> f64 {
        self.ratio_milli as f64 / 1000.0
    }
}

/// 容器形态下**故意留在目录里**的路径前缀。
pub type KeepPrefixes<'a> = &'a [String];

/// 判断一个条目是否要留在目录形态里（不进容器）。
///
/// 为什么需要这个口子：有些消费者**按真实文件路径**读资源，容器里的条目它们够不着。
/// 目前只有一个实例，但很硬：微信 `wx.loadFont(path)` 只吃本地文件，字体一旦进容器就加载不了
/// （实测：`package.ps1` 的"字体必须在 pack/ 里"断言当场拦下）。
/// 把这类资源按前缀排除之后，容器与目录两种形态可以并存——它们仍是 manifest 条目、
/// 仍参与完整性校验，只是不被打进 `.rlb`。
fn is_kept(relative: &str, keep: KeepPrefixes<'_>) -> bool {
    keep.iter()
        .any(|prefix| relative.starts_with(prefix.as_str()))
}

/// 启动闭包优先级：小的先排（0 = 启动就读，1 = 启动后按需）。
///
/// 判据只列**确实在 boot 期间被读**的东西（见 `game` / `game_web` 的启动序列）：
/// `project.toml`、启动场景、主脚本、图集重定向表。字体不算——容器形态下它被 `--keep`
/// 留在目录里，根本不进块。
pub(crate) fn boot_priority(relative: &str) -> u8 {
    // 0 = 引擎**确定**在 boot 期读的四个文件：project.toml、图集重定向表、启动场景、主脚本。
    //     它们合计约 75 KB，排在一起就能塞进同一个块。
    if relative == "project.toml"
        || relative == "atlas_redirect.json"
        || relative == "scenes/main.scene"
        || relative == "scripts/main.lua"
    {
        return 0;
    }
    // 1 = 其余 Lua 脚本：`require` 同步，boot 期间会连带读到（1600 KB / 155 个文件，约 7 块）。
    //     紧跟锚点，等于把"启动会摸到的块"压到容器最前面。
    if relative.starts_with("scripts/") {
        return 1;
    }
    // 2 = 其余（场景、预制体…）：启动只读其中一个场景，其余按需。
    2
}

/// 单包形态：先按目录形态产出（复用字体子集/依赖索引/安全边界检查），
/// 再把整棵树收进 `bundles/all-<hash8>.rlb`，输出根只留骨架
/// （`project.toml` + `manifest.txt` + 容器 + 报告）。
///
/// 为什么"先生成再收"而不是直接流式写包：这样"包内条目 == 目录形态的产物"是**构造性成立**的，
/// 等价性门（`bundle_package_matches_loose_output`）不需要再维护一份拷贝逻辑。
pub fn package_bundled(
    project: &Path,
    out: &Path,
    drop_undeclared_fonts: bool,
    fast: bool,
    encrypt: bool,
    keep: KeepPrefixes<'_>,
) -> anyhow::Result<PackageReport> {
    package_bundled_with(project, out, drop_undeclared_fonts, fast, encrypt, keep, 0)
}

/// [`package_bundled`] 的可调块大小版本（`block_size = 0` 表示默认 256 KB）。
pub fn package_bundled_with(
    project: &Path,
    out: &Path,
    drop_undeclared_fonts: bool,
    fast: bool,
    encrypt: bool,
    keep: KeepPrefixes<'_>,
    block_size: u32,
) -> anyhow::Result<PackageReport> {
    let block_size = if block_size == 0 {
        rustlua_infra::bundle::DEFAULT_BLOCK_SIZE
    } else {
        block_size
    };
    use rustlua_infra::bundle::BundleBuilder;

    // 1) 目录形态打底 + （可选）图集 cook。cook 读的是 `out/assets/**`、写完页与重定向表后
    //    源图就不在了，所以后面的条目收集天然拿到"页 + 定义"而不是散图。
    package(project, out, false, false, drop_undeclared_fonts)?;
    if let Some(config) = atlas_cook::AtlasConfig::load(&project.join("atlas.json"))? {
        let redirect = atlas_cook::cook(project, out, &config)?;
        fs::write(
            out.join(atlas_cook::REDIRECT_FILE),
            serde_json::to_vec_pretty(&redirect)
                .map_err(|error| anyhow!("写重定向表失败：{error}"))?,
        )?;
    }

    // 2) 收集条目（排除稍后要重写的清单与报告）。
    let mut files = Vec::new();
    collect_files(out, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let manifest_path = out.join("manifest.txt");
    let report_path = out.join(PACKAGE_REPORT_FILE);
    let mut builder = BundleBuilder::new(block_size);
    // 块级 AEAD（docs/67 §4.4）：--encrypt 直接落进容器，贴图/音频/字体一起进加密域。
    if encrypt {
        builder = builder.with_encryption(true);
    }
    if fast {
        builder = builder.with_level(3);
    }
    let mut entries = 0u64;
    for (path, _) in &files {
        if path == &manifest_path || path == &report_path {
            continue;
        }
        let relative = path
            .strip_prefix(out)
            .map_err(|_| anyhow!("打包条目不在输出目录内：{}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        if is_kept(&relative, keep) {
            continue; // 留在目录里：宿主按真实路径读它（微信 `wx.loadFont`）
        }
        let bytes = fs::read(path)?;
        builder
            .add(&relative, bytes)
            .map_err(|error| anyhow!("{error}"))?;
        entries += 1;
    }
    if entries == 0 {
        return Err(anyhow!("没有可打包的资源（输出目录为空）"));
    }

    // 3) 建包。
    let built = builder.build().map_err(|error| anyhow!("{error}"))?;
    let bundle_name = format!("all-{:08x}.rlb", built.checksum as u32);
    let bundle_rel = format!("{BUNDLE_DIR}/{bundle_name}");
    let bundle_path = out.join(BUNDLE_DIR).join(&bundle_name);
    if let Some(parent) = bundle_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&bundle_path, &built.bytes)?;

    // 4) 收尾：删松散文件 → 写报告 → 重写清单。
    let mut report = PackageReport {
        schema: 2,
        mode: BundleMode::Single.token().to_owned(),
        files: 0,
        entries,
        blocks: u64::from(built.blocks),
        raw_bytes: built.raw_len,
        bundle_bytes: built.bytes.len() as u64,
        index_bytes: u64::from(built.index_len),
        index_raw_bytes: u64::from(built.index_raw_len),
        ratio_milli: PackageReport::ratio_milli(built.raw_len, built.bytes.len() as u64),
        bundle_path: bundle_rel,
        groups: Vec::new(),
    };
    finalize_container_pack(out, &files, &[bundle_path], keep, &mut report)?;
    Ok(report)
}

/// 分组形态（`docs/67` §5，批 C）：按 `bundles.json`（缺省内置内容域规则）切成多个容器，
/// 产出 `bundle_plan.json`，并在构建期跑三条硬门（覆盖完整 / 不跨分发单元 / core 自足）。
pub fn package_grouped(
    project: &Path,
    out: &Path,
    drop_undeclared_fonts: bool,
    fast: bool,
) -> anyhow::Result<PackageReport> {
    package_grouped_keep(project, out, drop_undeclared_fonts, fast, false, &[])
}

/// [`package_grouped`] 的带"留在目录里"前缀的版本（微信字体走这条路，见 `is_kept`）。
pub fn package_grouped_keep(
    project: &Path,
    out: &Path,
    drop_undeclared_fonts: bool,
    fast: bool,
    encrypt: bool,
    keep: KeepPrefixes<'_>,
) -> anyhow::Result<PackageReport> {
    package_grouped_keep_with(
        project,
        out,
        drop_undeclared_fonts,
        fast,
        encrypt,
        keep,
        0,
        false,
    )
}

/// [`package_grouped_keep`] 的可调块大小版本（`0` = 默认 256 KB）。
///
/// 块大小的权衡（`docs/67` §9 的性能项）：块越大压缩率略好、同一次解压覆盖更多条目，
/// 但单次解压更久、惰性读的粒度更粗。默认 256 KB 是拍的，这个参数就是为了量它。
///
/// `exclude_cdn`：本地包（微信分包/整包）不携带 `unit = "cdn"` 的组——它们本来就该由
/// CDN 单独分发；带进 `pack/` 会变成"主包/分包里一份、CDN 上再一份"的死重
/// （`splash/` 品牌图就是这种：发布器已把它压成 `branding/splash.jpg`，资源组里那份是纯重复）。
///
/// 参数是平铺的打包选项标量（与 `main.rs` / `tests/bundle_mount.rs` 三个调用点保持一致），
/// 保持扁平比拆成配置结构更易读。
#[allow(clippy::too_many_arguments)]
pub fn package_grouped_keep_with(
    project: &Path,
    out: &Path,
    drop_undeclared_fonts: bool,
    fast: bool,
    encrypt: bool,
    keep: KeepPrefixes<'_>,
    block_size: u32,
    exclude_cdn: bool,
) -> anyhow::Result<PackageReport> {
    let block_size = if block_size == 0 {
        rustlua_infra::bundle::DEFAULT_BLOCK_SIZE
    } else {
        block_size
    };
    use bundle_plan::{
        BundleConfig, MAX_BUNDLE_BYTES, PLAN_FILE, build_plan, plan_to_json, validate_plan,
    };
    use rustlua_infra::bundle::{BundleBuilder, DEFAULT_BLOCK_SIZE};

    // 1) 目录形态打底（字体子集/依赖索引/安全边界都在这一层，与单包形态同源）+（可选）图集 cook。
    package(project, out, false, false, drop_undeclared_fonts)?;
    if let Some(config) = atlas_cook::AtlasConfig::load(&project.join("atlas.json"))? {
        let redirect = atlas_cook::cook(project, out, &config)?;
        fs::write(
            out.join(atlas_cook::REDIRECT_FILE),
            serde_json::to_vec_pretty(&redirect)
                .map_err(|error| anyhow!("写重定向表失败：{error}"))?,
        )?;
    }

    let manifest_path = out.join("manifest.txt");
    let report_path = out.join(PACKAGE_REPORT_FILE);
    let plan_path = out.join(PLAN_FILE);

    // 2) 收集条目（排除稍后要重写的清单/报告/计划）。
    //
    // 注意 `collect_files` 的第二个字段是**修改时间（纳秒）**，不是字节数
    // （见 `collect_files` 的文档与 `modified_nanos`）。分组要按字节切，所以这里单独取长度；
    // 拿 mtime 当字节数会把几个 20 位数字加爆 u64（批 C 第一次跑就踩了）。
    let mut files = Vec::new();
    collect_files(out, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut entries: Vec<(String, u64, PathBuf)> = Vec::new();
    // `--keep` 命中的**具体路径**要写进计划表：发布期 `assetc closure` 靠它断言"这些文件
    // 必须以真实文件留在发布根里，且不得再出现在任何容器索引里"。只把前缀传下去是不够的——
    // 前缀是规则，规则会在下一版改；契约必须随产物走。
    let mut kept_paths: Vec<String> = Vec::new();
    for (path, _mtime) in &files {
        if path == &manifest_path || path == &report_path || path == &plan_path {
            continue;
        }
        let relative = path
            .strip_prefix(out)
            .map_err(|_| anyhow!("打包条目不在输出目录内：{}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        if is_kept(&relative, keep) {
            kept_paths.push(relative);
            continue; // 留在目录里：宿主按真实路径读它（微信 `wx.loadFont`）
        }
        let size = fs::metadata(path)
            .map_err(|error| anyhow!("读 {} 的长度失败：{error}", path.display()))?
            .len();
        entries.push((relative, size, path.clone()));
    }
    if entries.is_empty() {
        return Err(anyhow!("没有可打包的资源（输出目录为空）"));
    }

    // 3) 分组 + 三条硬门。
    let config = BundleConfig::load_or_default(&project.join("bundles.json"))?;
    let plan_entries: Vec<(String, u64)> = entries
        .iter()
        .map(|(path, size, _)| (path.clone(), *size))
        .collect();
    let mut plan = build_plan(&config, &plan_entries, DEFAULT_BLOCK_SIZE)?;
    if exclude_cdn {
        // 先把 cdn 组的路径从打包条目里剔掉，再把组从计划里删掉并重封自校验。
        // 顺序很重要：先 build_plan 才能知道哪些路径属于 cdn（归组规则只在 plan 里落地），
        // 再按路径过滤 entries，最后 sealed() 重算 groups 的 FNV。
        let cdn_paths: std::collections::BTreeSet<String> = plan
            .groups
            .iter()
            .filter(|group| group.unit == "cdn")
            .flat_map(|group| group.paths.iter().cloned())
            .collect();
        if !cdn_paths.is_empty() {
            entries.retain(|(path, _, _)| !cdn_paths.contains(path));
            plan.groups.retain(|group| group.unit != "cdn");
            plan = plan.sealed();
            if entries.is_empty() {
                return Err(anyhow!("排除 cdn 后没有可打包的资源"));
            }
        }
    }
    // 被 `--keep` 排除出容器的条目（微信字体）不在计划里，自然也不该参与"core 自足"断言：
    // 它们本来就是**目录里的真实文件**，按路径读得到，这正是 keep 的目的。
    let bundled: std::collections::BTreeSet<&str> =
        entries.iter().map(|(path, _, _)| path.as_str()).collect();
    let keys = bundle_plan::ProjectKeys {
        main_script: project_key_value(project, "main_script"),
        startup_scene: project_key_value(project, "startup_scene"),
        fonts: project_font_declarations(project)
            .into_iter()
            .filter(|font| bundled.contains(font.as_str()))
            .collect(),
    };
    validate_plan(
        &plan,
        entries.len() as u64,
        &keys,
        &config,
        MAX_BUNDLE_BYTES,
    )?;

    // 4) 逐组建包。
    let by_path: BTreeMap<&str, &PathBuf> = entries
        .iter()
        .map(|(relative, _, path)| (relative.as_str(), path))
        .collect();
    let mut bundle_paths: Vec<PathBuf> = Vec::new();
    let mut report_groups: Vec<GroupReport> = Vec::with_capacity(plan.groups.len());
    let (mut total_raw, mut total_bytes, mut total_blocks) = (0u64, 0u64, 0u64);
    let (mut total_index, mut total_index_raw) = (0u64, 0u64);
    for group in &mut plan.groups {
        let mut builder = BundleBuilder::new(block_size);
        // 块级 AEAD（docs/67 §4.4）：--encrypt 直接落进容器，贴图/音频/字体一起进加密域。
        if encrypt {
            builder = builder.with_encryption(true);
        }
        if fast {
            builder = builder.with_level(3);
        }
        // 组内装入次序 = **启动闭包优先**，其余按路径升序（`docs/67` §9 的性能项）。
        //
        // 为什么次序能省时间：容器按 256 KB 顺序切块，同块的条目共享一次解压。
        // 启动只读极少数条目（project.toml / 启动场景 / 主脚本 / 重定向表），
        // 它们在**哪个块**决定启动要解几次——散落在三个块里就要解三次。
        // 把它们排到组内最前面，启动就落在第 0 块（必要时第 1 块）。
        // 排序仍完全确定（同一输入 → 同一份字节），不吃掉 §4.5 的确定性要求。
        for relative in &group.paths {
            let path = by_path
                .get(relative.as_str())
                .ok_or_else(|| anyhow!("计划里的条目在产物里找不到：{relative}"))?;
            // 排序交给 builder：它内部会按 (优先级, 名字) 重排，在 assetc 侧先排是无效的（实测）。
            builder
                .add_ranked(relative, fs::read(path)?, boot_priority(relative))
                .map_err(|error| anyhow!("{error}"))?;
        }
        let built = builder
            .build()
            .map_err(|error| anyhow!("构建组 {} 失败：{error}", group.name))?;
        let name = format!(
            "{}-{:08x}.rlb",
            bundle_plan::sanitize(&group.name),
            built.checksum as u32
        );
        let relative = format!("{BUNDLE_DIR}/{name}");
        let path = out.join(BUNDLE_DIR).join(&name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, &built.bytes)?;
        // 复核 P0-1：单包上限按**落盘**字节判（微信分包口径），不是原始字节。
        if group.unit != "cdn" && built.bytes.len() as u64 > MAX_BUNDLE_BYTES {
            return Err(anyhow!(
                "组 {} 落盘 {} B 超过单包上限 {} B（微信分包口径）：请拆细规则或改用 unit = \"cdn\"",
                group.name,
                built.bytes.len(),
                MAX_BUNDLE_BYTES
            ));
        }
        total_raw += built.raw_len;
        total_bytes += built.bytes.len() as u64;
        total_blocks += u64::from(built.blocks);
        total_index += u64::from(built.index_len);
        total_index_raw += u64::from(built.index_raw_len);
        group.bundle = relative.clone();
        group.bundle_bytes = built.bytes.len() as u64;
        // 增量分发的比较键：整包 FNV（与文件名里的 8 位同源，这里给全 16 位）。
        group.hash = format!("{:016x}", built.checksum);
        report_groups.push(GroupReport {
            name: group.name.clone(),
            unit: group.unit.clone(),
            entries: group.entries,
            raw_bytes: built.raw_len,
            bundle_bytes: built.bytes.len() as u64,
            bundle: relative,
        });
        bundle_paths.push(path);
    }

    // 5) 计划表（先写：清单要覆盖它）。
    // 计划表自校验（复核 P2）：**写盘前**签，因为 `bundle`/`bundle_bytes`/`hash` 在逐组建包时
    // 才被回填；早签必然对不上。计划表是缺件归因的唯一依据，截断/错版必须当场发现。
    plan.keep = Some(kept_paths);
    plan.checksum = plan.compute_checksum();
    fs::write(&plan_path, plan_to_json(&plan)?)?;
    bundle_paths.push(plan_path.clone());

    // 6) 收尾。
    let mut report = PackageReport {
        schema: 2,
        mode: BundleMode::Grouped.token().to_owned(),
        files: 0,
        entries: entries.len() as u64,
        blocks: total_blocks,
        raw_bytes: total_raw,
        bundle_bytes: total_bytes,
        index_bytes: total_index,
        index_raw_bytes: total_index_raw,
        ratio_milli: PackageReport::ratio_milli(total_raw, total_bytes),
        bundle_path: report_groups
            .first()
            .map(|group| group.bundle.clone())
            .unwrap_or_default(),
        groups: report_groups,
    };
    finalize_container_pack(out, &files, &bundle_paths, keep, &mut report)?;
    Ok(report)
}

/// 容器形态的收尾：删掉已进容器的松散文件（保留 `project.toml` 与 `keep`）→ 写报告 → 重写清单。
///
/// `files` 是第 2 步收集到的**松散产物全集**（含清单与报告），`keep` 是必须留在磁盘上的
/// 已生成文件（容器、计划表）。
fn finalize_container_pack(
    out: &Path,
    files: &[(PathBuf, u64)],
    keep: &[PathBuf],
    prefixes: KeepPrefixes<'_>,
    report: &mut PackageReport,
) -> anyhow::Result<()> {
    let manifest_path = out.join("manifest.txt");
    let report_path = out.join(PACKAGE_REPORT_FILE);
    let project_toml = out.join("project.toml");

    for (path, _) in files {
        if path == &manifest_path || path == &report_path || path == &project_toml {
            continue;
        }
        if keep.iter().any(|kept| kept == path) {
            continue;
        }
        // `--keep` 排出去的文件（微信字体）本来就不在容器里，绝不能在这里被当"中间产物"删掉：
        // 删了之后容器读不到它（它不在容器里）、磁盘也没有，`wx.loadFont` 直接失效。
        let relative = path
            .strip_prefix(out)
            .map(|tail| tail.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if is_kept(&relative, prefixes) {
            continue;
        }
        if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    for dir in ["scenes", "scripts", "assets"] {
        remove_dir_if_empty(&out.join(dir))?;
    }
    prune_empty_dirs(out, 0)?;

    let files_after = {
        let mut list = Vec::new();
        collect_files(out, &mut list)?;
        list.len() as u64
    };
    // 此刻磁盘上是"骨架 + 容器(+计划) + 第一轮留下的清单"，再加 1 份待写的报告。
    report.files = files_after + 1;
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(report).map_err(|error| anyhow!("写报告失败：{error}"))?,
    )?;

    let mut listed = Vec::new();
    collect_files(out, &mut listed)?;
    listed.retain(|(path, _)| path != &manifest_path);
    listed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut lines = vec![format!("assetc_manifest=1\t{PACKAGE_VERSION}")];
    for (path, _) in listed {
        let relative = path
            .strip_prefix(out)
            .map_err(|_| anyhow!("清单文件不在输出目录内：{}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&path)?;
        lines.push(format!(
            "{relative}\t{}\t{:016x}",
            bytes.len(),
            fnv1a(&bytes)
        ));
    }
    fs::write(&manifest_path, lines.join("\n"))?;
    Ok(())
}

/// 读 `project.toml` 里的一个键（与 `project_font_key` 同一条逐行扫描口径）。
fn project_key_value(project: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(project.join("project.toml")).ok()?;
    text.lines().find_map(|line| {
        let line = line.trim().trim_start_matches('\u{feff}');
        line.strip_prefix(&format!("{key} ="))
            .map(|rest| rest.trim().trim_matches('"').to_owned())
            .filter(|value| !value.is_empty())
    })
}

/// `assetc inspect <包目录|.rlb>`：打印容器索引摘要与**启动闭包落在第几个块**。
///
/// 为什么需要它：`game --check` 只说"1,371 个条目都读得出来"，看不出"启动要解几个块"——
/// 而那正是块排序优化的唯一指标。索引本身是 zstd 压缩的（`HEADER_FLAG_INDEX_COMPRESSED`），
/// 外部脚本解不了，所以这个读数只能由 Rust 侧给。
///
/// 输出刻意保持"一行一条 + 可 grep 的键值"：它同时是运维排查工具，
/// `assetc inspect <pack\bundles> | Select-String boot_blocks` 就是一条门。
/// 同 [`distribution_layout_report`]，但输出**机器可读 JSON**（发布脚本/适配层消费）。
///
/// 结构固定：`{ schema, entries, total_bytes, roots: [{ unit, root, bundles, entries, bytes }] }`。
/// 与文本版共用同一次 `distribution_layout` 调用，所以两者不可能给出不同的分组。
pub fn distribution_layout_json(target: &Path) -> anyhow::Result<String> {
    let plan_path = if target.is_dir() {
        target.join(bundle_plan::PLAN_FILE)
    } else {
        target.to_path_buf()
    };
    let bytes = fs::read(&plan_path)
        .map_err(|error| anyhow!("读 {} 失败：{error}", plan_path.display()))?;
    let plan = bundle_plan::BundlePlan::parse(&bytes).map_err(anyhow::Error::msg)?;
    let roots = bundle_plan::distribution_layout(&plan)?;
    let total: u64 = roots.iter().map(|root| root.bytes).sum();
    let entries: u64 = roots.iter().map(|root| root.entries).sum();
    let value = serde_json::json!({
        "schema": 1,
        "entries": entries,
        "total_bytes": total,
        "roots": roots
            .iter()
            .map(|root| serde_json::json!({
                "unit": root.unit,
                "root": root.root,
                "bundles": root.bundles,
                "entries": root.entries,
                "bytes": root.bytes,
            }))
            .collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&value).map_err(|error| anyhow!("序列化布局失败：{error}"))
}

/// 为任意工程生成一份 `bundles.json` 草稿（引擎级通用规则），可选 `force` 覆盖。
///
/// 用途：新工程接入分包时的**零猜测起点**——不绑定任何具体游戏路径，
/// 之后由编辑器面板或手改细化。
pub fn init_bundles(project: &Path, force: bool) -> anyhow::Result<String> {
    let path = project.join("bundles.json");
    if path.exists() && !force {
        anyhow::bail!(
            "{} 已存在：用 --force 覆盖，或直接在编辑器「资源 ▸ 分包」里编辑",
            path.display()
        );
    }
    let config = rustlua_infra::bundle_plan::BundleConfig::generic_domains();
    config.validate()?;
    std::fs::write(&path, config.to_json()?)?;
    Ok(format!(
        "assetc_bundles_init_ok path={} groups={}",
        path.display(),
        config.groups.len()
    ))
}

/// 读 `bundle_plan.json` 并打印分发布局（单元 → 包内根），供发布脚本消费。
pub fn distribution_layout_report(target: &Path) -> anyhow::Result<String> {
    let plan_path = if target.is_dir() {
        target.join(bundle_plan::PLAN_FILE)
    } else {
        target.to_path_buf()
    };
    let bytes = fs::read(&plan_path)
        .map_err(|error| anyhow!("读 {} 失败：{error}", plan_path.display()))?;
    let plan = bundle_plan::BundlePlan::parse(&bytes).map_err(anyhow::Error::msg)?;
    let roots = bundle_plan::distribution_layout(&plan)?;
    let mut out = String::from("assetc_layout");
    let mut total = 0u64;
    for root in &roots {
        total += root.bytes;
        out.push_str(&format!(
            "\n{} unit={} bundles={} entries={} kb={}",
            root.root,
            root.unit,
            root.bundles.len(),
            root.entries,
            root.bytes / 1024
        ));
    }
    let entries: u64 = plan.groups.iter().map(|group| group.entries).sum();
    out.push_str(&format!(
        "\nassetc_layout_ok roots={} entries={entries} kb={}",
        roots.len(),
        total / 1024
    ));
    Ok(out)
}

pub fn inspect_bundles(target: &Path) -> anyhow::Result<String> {
    use rustlua_infra::bundle::BundleIndex;

    // 启动闭包：boot 期间确实会读的条目（与 `boot_priority` 同口径）。
    let is_boot = |name: &str| {
        name == "project.toml"
            || name == "atlas_redirect.json"
            // 只认**启动场景**：把所有 scenes/* 都算进来会得到 3 个块的假读数
            // （那些场景启动时根本不读，只在切场景时才用）。
            || name == "scenes/main.scene"
            || name == "scripts/main.lua"
    };

    let mut files: Vec<PathBuf> = Vec::new();
    if target.is_file() {
        files.push(target.to_path_buf());
    } else {
        let nested = target.join(BUNDLE_DIR);
        let dir = if nested.is_dir() {
            nested
        } else {
            target.to_path_buf()
        };
        for entry in fs::read_dir(&dir)
            .map_err(|error| anyhow!("读 {} 失败：{error}", dir.display()))?
            .flatten()
        {
            let path = entry.path();
            if path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("rlb"))
            {
                files.push(path);
            }
        }
        files.sort();
    }
    if files.is_empty() {
        return Err(anyhow!("没有找到 .rlb 容器：{}", target.display()));
    }

    let mut out = String::new();
    let mut total_blocks = 0usize;
    let mut boot_places: Vec<(String, u32, String)> = Vec::new();
    for file in &files {
        let bytes =
            fs::read(file).map_err(|error| anyhow!("读 {} 失败：{error}", file.display()))?;
        let index =
            BundleIndex::parse(&bytes).map_err(|error| anyhow!("{}: {error}", file.display()))?;
        let name = file
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        let index_kb = index
            .blocks()
            .iter()
            .map(|block| block.stored_len as u64)
            .sum::<u64>()
            / 1024;
        out.push_str(&format!(
            "bundle {name} blocks={} entries={} payload_kb={index_kb} block_size={}\n",
            index.blocks().len(),
            index.len(),
            index.block_size()
        ));
        total_blocks += index.blocks().len();
        let mut per_block = vec![0usize; index.blocks().len()];
        for entry in index.entries() {
            if let Some(slot) = per_block.get_mut(entry.block as usize) {
                *slot += 1;
            }
        }
        for (number, block) in index.blocks().iter().enumerate() {
            out.push_str(&format!(
                "  block {number} flags={} stored={} raw={} entries={}\n",
                block.flags, block.stored_len, block.raw_len, per_block[number]
            ));
        }
        for entry in index.entries() {
            if is_boot(&entry.path) {
                boot_places.push((name.clone(), entry.block, entry.path.clone()));
            }
        }
    }
    boot_places.sort_by(|a, b| a.2.cmp(&b.2));
    for (bundle, block, name) in &boot_places {
        out.push_str(&format!("boot {bundle} block={block} {name}\n"));
    }
    let distinct: std::collections::BTreeSet<(String, u32)> = boot_places
        .iter()
        .map(|(bundle, block, _)| (bundle.clone(), *block))
        .collect();
    out.push_str(&format!(
        "inspect_ok bundles={} boot_entries={} boot_blocks={} total_blocks={total_blocks}",
        files.len(),
        boot_places.len(),
        distinct.len()
    ));
    Ok(out)
}

/// 等价性校验（`docs/67` 复核 P0-2）：容器形态的**每个条目**都必须与目录形态逐字节相等。
///
/// 为什么要有它：`game --check` 里的逐条目 FNV 只证明"容器内部自洽"，
/// 它证明不了"容器里的内容 == 目录形态的内容"。两者是不同的断言，发布前要都成立。
/// 这条入口同时也是发布脚本可以常跑的门（不是一次性测试）。
pub fn verify_bundle_equivalence(loose: &Path, container: &Path) -> anyhow::Result<usize> {
    use rustlua_infra::bundle::BundleSet;
    let bundles_dir = container.join(BUNDLE_DIR);
    let mut files: Vec<PathBuf> = fs::read_dir(&bundles_dir)
        .map_err(|error| anyhow!("读 {} 失败：{error}", bundles_dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("rlb"))
        })
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(anyhow!("{} 下没有 .rlb 容器", bundles_dir.display()));
    }
    let mut set = BundleSet::new(container);
    for file in &files {
        set.mount_file(file)
            .map_err(|error| anyhow!("挂载 {} 失败：{error}", file.display()))?;
    }
    let mut compared = 0usize;
    let mut first_mismatch: Option<String> = None;
    for path in set.paths().map(str::to_owned).collect::<Vec<_>>() {
        let expected = fs::read(loose.join(&path))
            .map_err(|error| anyhow!("目录形态缺少条目 {path}（容器里有）：{error}"))?;
        let actual = set
            .read(&path)?
            .ok_or_else(|| anyhow!("容器读不出条目 {path}"))?;
        compared += 1;
        if actual != expected && first_mismatch.is_none() {
            first_mismatch = Some(format!(
                "{path}（容器 {} B / 目录 {} B）",
                actual.len(),
                expected.len()
            ));
        }
    }
    if let Some(mismatch) = first_mismatch {
        return Err(anyhow!("条目字节不一致：{mismatch}"));
    }
    Ok(compared)
}

/// 删空目录（非递归、失败忽略）：`assets/` 下的小目录要一起收掉，否则目录形态的残骸还在。
fn remove_dir_if_empty(dir: &Path) -> anyhow::Result<()> {
    if dir.is_dir() && fs::read_dir(dir)?.next().is_none() {
        fs::remove_dir(dir)?;
    }
    Ok(())
}

/// 自底向上删掉所有空目录（限制深度，防误伤）。
fn prune_empty_dirs(root: &Path, depth: usize) -> anyhow::Result<()> {
    if depth > 16 {
        return Ok(());
    }
    let mut children = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            prune_empty_dirs(&path, depth + 1)?;
            children.push(path);
        }
    }
    for path in children {
        remove_dir_if_empty(&path)?;
    }
    Ok(())
}
