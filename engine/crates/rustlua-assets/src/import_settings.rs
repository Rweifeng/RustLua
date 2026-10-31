//! 资源库的**可提交**元数据段：逐资源的导入设置。
//!
//! 四族资源各有一张表，共用一份文件（工程根的 `resources.meta`）：
//! - **贴图**：采样滤镜 / 寻址模式；
//! - **模型**：统一缩放倍数（画的时候乘进模型矩阵，不重导网格）；
//! - **音频**：音量倍数（与总线音量相乘）；
//! - **字体**：字重覆盖（不登记 = 沿用工程级字重）。
//!
//! 为什么另开一个文件，而不是往 `.cache/resources.db` 里加字段：那份资源库落在
//! `<root>/.cache/`，而 `.cache/` 在 `.gitignore` 里（它是**可重建缓存**——指纹、状态、
//! 缓存产物路径）。导入设置是"作者意图"，换一台机器必须还在，所以它不能只活在那份缓存里。
//! 本模块就是资源库的**持久那一半**：一个入库的文本文件，与 `resources.db` 同一套
//! 无依赖行格式（不引序列化运行时，理由与 [`crate::resource_db`] 一致）。
//!
//! 键是**工程相对路径**（正斜杠），不是 `AssetId`：`AssetId::from_seed` 的种子含
//! `importer_version`，将来导入器升版会让同一张图换 UUID，设置不能跟着漂。

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// 入库文件名（工程根下）。与 `atlas.json` 同族：作者写、运行时读、必须可 review。
pub const RESOURCE_META_FILE: &str = "resources.meta";
const RESOURCE_META_HEADER: &str = "# rustlua-resource-meta-v1";

/// 放大时怎么取色。默认 [`TextureFilter::Nearest`]——与引擎今天的全局 2D 采样器
/// （`rustlua-gpu` 的 `rustlua-texture-sampler`：Nearest/Nearest + ClampToEdge）逐字节一致，
/// 所以"没设过设置的贴图"渲染结果不许变，这条由门钉住。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub enum TextureFilter {
    /// 最近邻：像素美术要的样子（放大不出糊边）。
    #[default]
    Nearest,
    /// 线性：照片级贴图/渐变图放大更顺。
    Linear,
}

/// 超出 UV `[0,1]` 时怎么取。默认 [`TextureWrap::ClampToEdge`]，同上。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub enum TextureWrap {
    /// 夹到边：绝大多数情况（精灵/UI 不用重复采样）。
    #[default]
    ClampToEdge,
    /// 平铺：背景、地面 tile。
    Repeat,
    /// 镜像平铺。
    MirroredRepeat,
}

impl TextureFilter {
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Self::Nearest => "nearest",
            Self::Linear => "linear",
        }
    }

    /// 采样器小表里的下标高位（渲染器按 `滤镜 × 寻址` 预建，见 `rustlua-gpu`）。
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Nearest => 0,
            Self::Linear => 1,
        }
    }

    /// 滤镜档数（渲染器建表用）。
    pub const COUNT: usize = 2;

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "nearest" => Some(Self::Nearest),
            "linear" => Some(Self::Linear),
            _ => None,
        }
    }
}

impl TextureWrap {
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Self::ClampToEdge => "clamp",
            Self::Repeat => "repeat",
            Self::MirroredRepeat => "mirror",
        }
    }

    /// 采样器小表里的下标低位。
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::ClampToEdge => 0,
            Self::Repeat => 1,
            Self::MirroredRepeat => 2,
        }
    }

    /// 寻址档数（渲染器建表用）。
    pub const COUNT: usize = 3;

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "clamp" => Some(Self::ClampToEdge),
            "repeat" => Some(Self::Repeat),
            "mirror" => Some(Self::MirroredRepeat),
            _ => None,
        }
    }
}

/// 一张纹理的导入设置。本批只有采样两件事；将来加 mipmap / 预乘 alpha 时在这里扩，
/// 并**那时**才需要把设置并进 `ImportCache` 的指纹（那两类会改产物字节，这两类不改）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub struct TextureImportSettings {
    pub filter: TextureFilter,
    pub wrap: TextureWrap,
}

impl TextureImportSettings {
    /// `const fn`：让门与面板能把某一档写成 `const` 而不是每次现拼（这一层没有运行时逻辑，
    /// 做成 const 也让"默认档 == 引擎今天的行为"这类断言可以进编译期检查）。
    #[must_use]
    pub const fn new(filter: TextureFilter, wrap: TextureWrap) -> Self {
        Self { filter, wrap }
    }

    /// 与引擎默认一致 ⇒ 不必写进入库文件（保持那份文件接近空）。
    #[must_use]
    pub fn is_default(self) -> bool {
        self == Self::default()
    }
}

/// 模型导入设置：统一缩放倍数。
///
/// 默认 1.0 = 不改大小（与引擎今天的行为一致）。这个值在**画的时候**乘进模型矩阵，
/// 所以它不需要重导网格：改完立刻在 Scene View / Play 里生效。
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct ModelImportSettings {
    pub scale: f32,
}

impl Default for ModelImportSettings {
    fn default() -> Self {
        Self { scale: 1.0 }
    }
}

impl ModelImportSettings {
    #[must_use]
    pub const fn new(scale: f32) -> Self {
        Self { scale }
    }

    /// 与引擎默认一致 ⇒ 不必写进入库文件。
    #[must_use]
    pub fn is_default(self) -> bool {
        self.scale == 1.0
    }
}

/// 音频导入设置：音量倍数（0 = 静音，1 = 原样，>1 = 放大）。
///
/// 与总线音量（`project.toml` 的分组音量）**相乘**：总线管"这一类声音多大"，
/// 导入设置管"这一个文件多大"。两者互不覆盖。
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct AudioImportSettings {
    pub gain: f32,
}

impl Default for AudioImportSettings {
    fn default() -> Self {
        Self { gain: 1.0 }
    }
}

impl AudioImportSettings {
    #[must_use]
    pub const fn new(gain: f32) -> Self {
        Self { gain }
    }

    #[must_use]
    pub fn is_default(self) -> bool {
        self.gain == 1.0
    }
}

/// 字体导入设置：**字重覆盖**。
///
/// `None` = 不覆盖，沿用工程级字重（`project.toml` 的 `font_weight`）。
/// 用 `Option` 而不是"默认 400"：工程级字重可以变，"没登记"与"登记成 400"是两件事。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub struct FontImportSettings {
    pub weight: Option<u16>,
}

impl FontImportSettings {
    #[must_use]
    pub const fn new(weight: Option<u16>) -> Self {
        Self { weight }
    }

    #[must_use]
    pub fn is_default(self) -> bool {
        self.weight.is_none()
    }
}

/// 模型缩放的可写范围（厘米/米这类单位差在 0.01×~100× 之间）。
pub const MODEL_SCALE_MIN: f32 = 0.001;
/// 模型缩放的可写范围上界。
pub const MODEL_SCALE_MAX: f32 = 1000.0;
/// 音量的可写范围下界（0 = 静音）。
pub const AUDIO_GAIN_MIN: f32 = 0.0;
/// 音量的可写范围上界（再大只会削波）。
pub const AUDIO_GAIN_MAX: f32 = 8.0;
/// 字重的可写范围（1~1000，覆盖可变字体的连续字重）。
pub const FONT_WEIGHT_MIN: u16 = 1;
/// 字重的可写范围上界。
pub const FONT_WEIGHT_MAX: u16 = 1000;

/// 工程根下那份可提交的元数据段。
///
/// 四张表按**资源家族**分开（贴图 / 模型 / 音频 / 字体）：同一份文件里混着不同家族的
/// 行，但每张表只认自己的键——贴图那张表的键不会去模型那张表里找。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResourceMeta {
    root: PathBuf,
    /// 工程相对路径（正斜杠）→ 设置。用 `BTreeMap`：保存出来是稳定序，diff 不抖。
    textures: BTreeMap<String, TextureImportSettings>,
    models: BTreeMap<String, ModelImportSettings>,
    audio: BTreeMap<String, AudioImportSettings>,
    fonts: BTreeMap<String, FontImportSettings>,
}

#[derive(Debug, thiserror::Error)]
pub enum ResourceMetaError {
    #[error("资源元数据读写失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("资源元数据表头不认：{0}")]
    UnsupportedSchema(String),
    #[error("资源元数据第 {line} 行不合法")]
    Malformed { line: usize },
    #[error("未知的采样设置取值：{0}")]
    UnknownToken(String),
    #[error("资源元数据的{field}取值越界：{value}")]
    OutOfRange { field: &'static str, value: String },
    #[error("资源元数据的键必须是工程相对路径：{0}")]
    InvalidKey(String),
    #[error("资源元数据解不开：{0}")]
    Decode(String),
}

/// 规范化成表里用的键：绝对路径 → 相对工程根；反斜杠 → 正斜杠。
fn normalize_key(root: &Path, path: &Path) -> Result<String, ResourceMetaError> {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let key = relative.to_string_lossy().replace('\\', "/");
    if key.is_empty()
        || key == "."
        || key.starts_with("../")
        || key.contains('\t')
        || key.contains('\n')
    {
        return Err(ResourceMetaError::InvalidKey(path.display().to_string()));
    }
    Ok(key)
}

impl ResourceMeta {
    /// 读工程根下的元数据段；文件不存在 = 空表（首次使用是常态，不是错误）。
    ///
    /// "在不在"也走 `vfs`：发布形态下这份文件在容器/宿主资源表里，`Path::exists()`
    /// 会说不存在，于是**整份设置被跳过**——这比读错更糟，因为它看起来像"没登记过"。
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ResourceMetaError> {
        let root = root.as_ref().to_path_buf();
        let mut meta = Self {
            root,
            ..Self::default()
        };
        if rustlua_infra::vfs::is_file(&meta.path()) {
            meta.load()?;
        }
        Ok(meta)
    }

    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.root.join(RESOURCE_META_FILE)
    }

    /// 查这张图的设置；没登记过就是默认（= 引擎今天的行为）。
    #[must_use]
    pub fn texture_settings(&self, path: impl AsRef<Path>) -> TextureImportSettings {
        let key = path.as_ref().to_string_lossy().replace('\\', "/");
        self.textures.get(&key).copied().unwrap_or_default()
    }

    /// 查这个模型的缩放；没登记过就是 1.0（不改大小）。
    #[must_use]
    pub fn model_settings(&self, path: impl AsRef<Path>) -> ModelImportSettings {
        let key = path.as_ref().to_string_lossy().replace('\\', "/");
        self.models.get(&key).copied().unwrap_or_default()
    }

    /// 查这个音频的音量；没登记过就是 1.0（原样）。
    #[must_use]
    pub fn audio_settings(&self, path: impl AsRef<Path>) -> AudioImportSettings {
        let key = path.as_ref().to_string_lossy().replace('\\', "/");
        self.audio.get(&key).copied().unwrap_or_default()
    }

    /// 查这个字体的字重覆盖；没登记过 = `None`（沿用工程级字重）。
    #[must_use]
    pub fn font_settings(&self, path: impl AsRef<Path>) -> FontImportSettings {
        let key = path.as_ref().to_string_lossy().replace('\\', "/");
        self.fonts.get(&key).copied().unwrap_or_default()
    }

    /// 登记/改一张图的设置。写回默认值 = 从表里删掉这一行（默认不该占 diff）。
    ///
    /// 返回是否**改变**了表内容——面板据此决定要不要标脏、要不要重传纹理。
    pub fn set_texture_settings(
        &mut self,
        path: impl AsRef<Path>,
        settings: TextureImportSettings,
    ) -> Result<bool, ResourceMetaError> {
        let key = normalize_key(&self.root, path.as_ref())?;
        let changed = if settings.is_default() {
            self.textures.remove(&key).is_some()
        } else {
            match self.textures.get(&key) {
                Some(&existing) if existing == settings => false,
                _ => {
                    self.textures.insert(key, settings);
                    true
                }
            }
        };
        Ok(changed)
    }

    /// 登记/改一个模型的缩放。写回 1.0 = 删掉这一行。
    ///
    /// 越界的值**拒绝**而不是夹到范围里：夹了就是在骗人（面板写 10000，运行期按 1000 跑）。
    pub fn set_model_settings(
        &mut self,
        path: impl AsRef<Path>,
        settings: ModelImportSettings,
    ) -> Result<bool, ResourceMetaError> {
        let key = normalize_key(&self.root, path.as_ref())?;
        if !settings.scale.is_finite()
            || settings.scale < MODEL_SCALE_MIN
            || settings.scale > MODEL_SCALE_MAX
        {
            return Err(ResourceMetaError::OutOfRange {
                field: "模型缩放",
                value: format!("{:?}", settings.scale),
            });
        }
        let changed = if settings.is_default() {
            self.models.remove(&key).is_some()
        } else {
            match self.models.get(&key) {
                Some(&existing) if existing == settings => false,
                _ => {
                    self.models.insert(key, settings);
                    true
                }
            }
        };
        Ok(changed)
    }

    /// 登记/改一个音频的音量。写回 1.0 = 删掉这一行。
    pub fn set_audio_settings(
        &mut self,
        path: impl AsRef<Path>,
        settings: AudioImportSettings,
    ) -> Result<bool, ResourceMetaError> {
        let key = normalize_key(&self.root, path.as_ref())?;
        if !settings.gain.is_finite()
            || settings.gain < AUDIO_GAIN_MIN
            || settings.gain > AUDIO_GAIN_MAX
        {
            return Err(ResourceMetaError::OutOfRange {
                field: "音量",
                value: format!("{:?}", settings.gain),
            });
        }
        let changed = if settings.is_default() {
            self.audio.remove(&key).is_some()
        } else {
            match self.audio.get(&key) {
                Some(&existing) if existing == settings => false,
                _ => {
                    self.audio.insert(key, settings);
                    true
                }
            }
        };
        Ok(changed)
    }

    /// 登记/改一个字体的字重覆盖。写回 `None` = 删掉这一行。
    pub fn set_font_settings(
        &mut self,
        path: impl AsRef<Path>,
        settings: FontImportSettings,
    ) -> Result<bool, ResourceMetaError> {
        let key = normalize_key(&self.root, path.as_ref())?;
        if let Some(weight) = settings.weight
            && !(FONT_WEIGHT_MIN..=FONT_WEIGHT_MAX).contains(&weight)
        {
            return Err(ResourceMetaError::OutOfRange {
                field: "字重",
                value: weight.to_string(),
            });
        }
        let changed = if settings.is_default() {
            self.fonts.remove(&key).is_some()
        } else {
            match self.fonts.get(&key) {
                Some(&existing) if existing == settings => false,
                _ => {
                    self.fonts.insert(key, settings);
                    true
                }
            }
        };
        Ok(changed)
    }

    /// 表里登记了多少条（四个家族加起来）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.textures.len() + self.models.len() + self.audio.len() + self.fonts.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 已登记的贴图条目（工程相对路径 → 设置），按路径序。
    /// 不加 `#[must_use]`：返回的 `impl Iterator` 自带，`-D clippy::double_must_use` 会判红。
    pub fn texture_entries(&self) -> impl Iterator<Item = (&str, TextureImportSettings)> {
        self.textures
            .iter()
            .map(|(path, settings)| (path.as_str(), *settings))
    }

    /// 已登记的模型条目（工程相对路径 → 设置），按路径序。
    pub fn model_entries(&self) -> impl Iterator<Item = (&str, ModelImportSettings)> {
        self.models
            .iter()
            .map(|(path, settings)| (path.as_str(), *settings))
    }

    /// 表里有没有模型设置。没有的话运行期可以整段跳过查表（逐帧热路径用得上）。
    #[must_use]
    pub fn has_model_settings(&self) -> bool {
        !self.models.is_empty()
    }

    /// 已登记的音频条目（工程相对路径 → 设置），按路径序。
    pub fn audio_entries(&self) -> impl Iterator<Item = (&str, AudioImportSettings)> {
        self.audio
            .iter()
            .map(|(path, settings)| (path.as_str(), *settings))
    }

    /// 已登记的字体条目（工程相对路径 → 设置），按路径序。
    pub fn font_entries(&self) -> impl Iterator<Item = (&str, FontImportSettings)> {
        self.fonts
            .iter()
            .map(|(path, settings)| (path.as_str(), *settings))
    }

    /// 原子保存：写临时文件再 rename（与 `resource_db` 同一理由——并发保存时
    /// 固定名会让先完成的那个把临时文件取走）。空表时删掉文件，不留一份只有表头的噪音。
    pub fn save(&self) -> Result<(), ResourceMetaError> {
        let path = self.path();
        if self.is_empty() {
            if path.exists() {
                fs::remove_file(&path)?;
            }
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = String::from(RESOURCE_META_HEADER);
        output.push('\n');
        // 按家族分组写出（每组内部按路径序）：同一份文件里行序稳定，diff 不抖。
        for (key, settings) in &self.textures {
            output.push_str(&format!(
                "texture\t{}\t{}\t{}\n",
                key,
                settings.filter.token(),
                settings.wrap.token()
            ));
        }
        for (key, settings) in &self.models {
            output.push_str(&format!("model\t{}\t{:?}\n", key, settings.scale));
        }
        for (key, settings) in &self.audio {
            output.push_str(&format!("audio\t{}\t{:?}\n", key, settings.gain));
        }
        for (key, settings) in &self.fonts {
            if let Some(weight) = settings.weight {
                output.push_str(&format!("font\t{}\t{}\n", key, weight));
            }
        }
        let temp = path.with_extension("meta.tmp");
        if let Err(error) = fs::write(&temp, &output) {
            let _ = fs::remove_file(&temp);
            return Err(error.into());
        }
        if let Err(error) = fs::rename(&temp, &path) {
            let _ = fs::remove_file(&temp);
            return Err(error.into());
        }
        Ok(())
    }

    fn load(&mut self) -> Result<(), ResourceMetaError> {
        // 读走 `vfs`，不走裸 `std::fs`：发布形态下这份文件在 `.rlb` 容器里（或浏览器
        // 宿主的资源表里），磁盘上根本不存在。`atlas_redirect.json` 是同族文件，
        // 它的运行期读取就是 `vfs::read` + `pack::decode`（见 `rustlua-render::atlas`）——
        // 这里换成 `std::fs` 会让"编辑器里设了、开发跑生效、发布包不生效"，而所有面板读数都是绿的。
        let bytes = rustlua_infra::vfs::read(&self.path())?;
        let bytes = rustlua_infra::pack::decode(&bytes).map_err(ResourceMetaError::Decode)?;
        let text = String::from_utf8(bytes)
            .map_err(|error| ResourceMetaError::Decode(error.to_string()))?;
        let mut seen_header = false;
        for (line_no, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            if line.starts_with('#') {
                if line.trim() != RESOURCE_META_HEADER {
                    return Err(ResourceMetaError::UnsupportedSchema(line.trim().to_owned()));
                }
                seen_header = true;
                continue;
            }
            if !seen_header {
                return Err(ResourceMetaError::UnsupportedSchema("缺少表头".to_owned()));
            }
            let fields: Vec<_> = line.split('\t').collect();
            let line = line_no + 1;
            match fields.first().copied() {
                // 只认自己这一族；认得的类型 + 不认识的**字段**都不该静默吞掉。
                Some("texture") if fields.len() == 4 => {
                    let filter = TextureFilter::parse(fields[2])
                        .ok_or_else(|| ResourceMetaError::UnknownToken(fields[2].to_owned()))?;
                    let wrap = TextureWrap::parse(fields[3])
                        .ok_or_else(|| ResourceMetaError::UnknownToken(fields[3].to_owned()))?;
                    let key = validate_key(fields[1])?;
                    let settings = TextureImportSettings::new(filter, wrap);
                    // 手工编辑出来的默认值行不入库也不报错，但也不留在表里。
                    if settings.is_default() {
                        self.textures.remove(&key);
                    } else {
                        self.textures.insert(key, settings);
                    }
                }
                Some("model") if fields.len() == 3 => {
                    let key = validate_key(fields[1])?;
                    let settings = ModelImportSettings::new(parse_scale(fields[2])?);
                    if settings.is_default() {
                        self.models.remove(&key);
                    } else {
                        self.models.insert(key, settings);
                    }
                }
                Some("audio") if fields.len() == 3 => {
                    let key = validate_key(fields[1])?;
                    let settings = AudioImportSettings::new(parse_gain(fields[2])?);
                    if settings.is_default() {
                        self.audio.remove(&key);
                    } else {
                        self.audio.insert(key, settings);
                    }
                }
                Some("font") if fields.len() == 3 => {
                    let key = validate_key(fields[1])?;
                    let weight = fields[2]
                        .parse::<u16>()
                        .map_err(|_| ResourceMetaError::UnknownToken(fields[2].to_owned()))?;
                    if !(FONT_WEIGHT_MIN..=FONT_WEIGHT_MAX).contains(&weight) {
                        return Err(ResourceMetaError::OutOfRange {
                            field: "字重",
                            value: fields[2].to_owned(),
                        });
                    }
                    let settings = FontImportSettings::new(Some(weight));
                    self.fonts.insert(key, settings);
                }
                // 未知记录类型：跳过。前向兼容（新版本引擎写的设置不该让老引擎打不开工程）。
                Some(other) if !other.is_empty() => {}
                _ => return Err(ResourceMetaError::Malformed { line }),
            }
        }
        Ok(())
    }
}

/// 表里的键：工程相对路径（正斜杠）。空、越界、带盘符的键一律拒绝。
fn validate_key(key: &str) -> Result<String, ResourceMetaError> {
    if key.is_empty() || key.starts_with("../") || key.contains(':') {
        return Err(ResourceMetaError::InvalidKey(key.to_owned()));
    }
    Ok(key.to_owned())
}

/// 解析模型缩放：必须是有限数且在可写范围内。
fn parse_scale(raw: &str) -> Result<f32, ResourceMetaError> {
    let value = raw
        .parse::<f32>()
        .map_err(|_| ResourceMetaError::UnknownToken(raw.to_owned()))?;
    if !value.is_finite() || !(MODEL_SCALE_MIN..=MODEL_SCALE_MAX).contains(&value) {
        return Err(ResourceMetaError::OutOfRange {
            field: "模型缩放",
            value: raw.to_owned(),
        });
    }
    Ok(value)
}

/// 解析音量：必须是有限数且在可写范围内。
fn parse_gain(raw: &str) -> Result<f32, ResourceMetaError> {
    let value = raw
        .parse::<f32>()
        .map_err(|_| ResourceMetaError::UnknownToken(raw.to_owned()))?;
    if !value.is_finite() || !(AUDIO_GAIN_MIN..=AUDIO_GAIN_MAX).contains(&value) {
        return Err(ResourceMetaError::OutOfRange {
            field: "音量",
            value: raw.to_owned(),
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.subsec_nanos())
            .unwrap_or(0);
        path.push(format!("rustlua-meta-{tag}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).expect("create temp project root");
        path
    }

    #[test]
    fn tokens_round_trip_and_default_matches_todays_engine() {
        for filter in [TextureFilter::Nearest, TextureFilter::Linear] {
            assert_eq!(TextureFilter::parse(filter.token()), Some(filter));
        }
        for wrap in [
            TextureWrap::ClampToEdge,
            TextureWrap::Repeat,
            TextureWrap::MirroredRepeat,
        ] {
            assert_eq!(TextureWrap::parse(wrap.token()), Some(wrap));
        }
        assert_eq!(TextureFilter::parse("bilinear"), None);
        assert_eq!(TextureWrap::parse("wrap"), None);
        // 默认必须是"引擎今天的行为"，否则这条门保不住"没设过的图不许变样"。
        assert_eq!(
            TextureImportSettings::default(),
            TextureImportSettings::new(TextureFilter::Nearest, TextureWrap::ClampToEdge)
        );
        assert!(TextureImportSettings::default().is_default());
    }

    #[test]
    fn a_project_without_the_file_reads_as_all_defaults() {
        let root = temp_root("absent");
        let meta = ResourceMeta::open(&root).expect("缺文件不是错误");
        assert!(meta.is_empty());
        assert_eq!(
            meta.texture_settings("assets/art/tile.png"),
            TextureImportSettings::default()
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn settings_survive_a_save_and_reload_keyed_by_project_relative_path() {
        let root = temp_root("roundtrip");
        let mut meta = ResourceMeta::open(&root).expect("open");
        assert!(
            meta.set_texture_settings(
                root.join("assets/art/grass.png"),
                TextureImportSettings::new(TextureFilter::Linear, TextureWrap::Repeat),
            )
            .expect("set"),
            "从默认改成非默认算改变"
        );
        assert!(
            !meta
                .set_texture_settings(
                    "assets/art/grass.png",
                    TextureImportSettings::new(TextureFilter::Linear, TextureWrap::Repeat),
                )
                .expect("set"),
            "写同一个值不该标脏"
        );
        meta.save().expect("save");
        let text = fs::read_to_string(meta.path()).expect("read");
        assert!(text.starts_with(RESOURCE_META_HEADER), "{text}");
        assert_eq!(text.lines().count(), 2, "只写一行：{text}");
        let reloaded = ResourceMeta::open(&root).expect("reload");
        assert_eq!(
            reloaded.texture_settings("assets/art/grass.png"),
            TextureImportSettings::new(TextureFilter::Linear, TextureWrap::Repeat)
        );
        // 反斜杠与正斜杠是同一张图。
        assert_eq!(
            reloaded.texture_settings(Path::new("assets\\art\\grass.png")),
            TextureImportSettings::new(TextureFilter::Linear, TextureWrap::Repeat)
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn writing_the_default_back_removes_the_line_and_the_file_when_empty() {
        let root = temp_root("revert");
        let mut meta = ResourceMeta::open(&root).expect("open");
        meta.set_texture_settings(
            "assets/art/grass.png",
            TextureImportSettings::new(TextureFilter::Linear, TextureWrap::ClampToEdge),
        )
        .expect("set");
        meta.save().expect("save");
        assert!(meta.path().exists());
        assert!(
            meta.set_texture_settings("assets/art/grass.png", TextureImportSettings::default())
                .expect("set"),
            "删掉一行也是改变"
        );
        assert_eq!(meta.len(), 0, "回到默认就不该占 diff");
        meta.save().expect("save");
        assert!(
            !meta.path().exists(),
            "空表不该在工程里留一份只有表头的噪音"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_foreign_or_broken_file_fails_loudly_instead_of_losing_settings() {
        let root = temp_root("broken");
        // 未知记录类型：跳过（前向兼容）。
        fs::write(
            root.join(RESOURCE_META_FILE),
            format!(
                "{RESOURCE_META_HEADER}\ntexture\ta.png\tlinear\tclamp\nmesh\ta.glb\twhatever\n"
            ),
        )
        .expect("write");
        let meta = ResourceMeta::open(&root).expect("未知类型不该拦打开");
        assert_eq!(meta.len(), 1);
        assert_eq!(meta.texture_entries().next().unwrap().0, "a.png");
        // 认得的类型里出现不认识的取值：必须报错，不能当成默认。
        fs::write(
            root.join(RESOURCE_META_FILE),
            format!("{RESOURCE_META_HEADER}\ntexture\ta.png\tbilinear\tclamp\n"),
        )
        .expect("write");
        let error = ResourceMeta::open(&root).expect_err("未知取值必须报出来");
        assert!(
            matches!(&error, ResourceMetaError::UnknownToken(token) if token == "bilinear"),
            "{error:?}"
        );
        // 表头不对：整份不认，别把别人的新格式当自己的读。
        fs::write(
            root.join(RESOURCE_META_FILE),
            "# rustlua-resource-meta-v9\ntexture\ta.png\tlinear\tclamp\n",
        )
        .expect("write");
        let error = ResourceMeta::open(&root).expect_err("表头不认");
        assert!(
            matches!(error, ResourceMetaError::UnsupportedSchema(_)),
            "{error:?}"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn keys_that_would_break_the_line_format_are_refused() {
        let root = temp_root("keys");
        let mut meta = ResourceMeta::open(&root).expect("open");
        for bad in ["", "../outside.png", "assets\ta.png", "assets\na.png"] {
            let error = meta
                .set_texture_settings(
                    Path::new(bad),
                    TextureImportSettings::new(TextureFilter::Linear, TextureWrap::ClampToEdge),
                )
                .expect_err("带制表符/换行/越界的键不能写");
            assert!(
                matches!(error, ResourceMetaError::InvalidKey(_)),
                "{bad:?} -> {error:?}"
            );
        }
        fs::remove_dir_all(root).ok();
    }

    /// 四个家族共用一份文件：每族各写一条，往返后逐条读回来。
    ///
    /// 这一条钉住"键是工程相对路径"在四族上同口径——模型/音频/字体如果各自用别的键
    /// 口径（文件名、AssetId），同一份文件就会有两套寻址规则。
    #[test]
    fn every_family_round_trips_through_the_same_file() {
        let root = temp_root("families");
        let mut meta = ResourceMeta::open(&root).expect("open");
        meta.set_texture_settings(
            "assets/art/tile.png",
            TextureImportSettings::new(TextureFilter::Linear, TextureWrap::Repeat),
        )
        .expect("texture");
        meta.set_model_settings("assets/models/hero.glb", ModelImportSettings::new(0.5))
            .expect("model");
        meta.set_audio_settings("assets/audio/hit.ogg", AudioImportSettings::new(0.25))
            .expect("audio");
        meta.set_font_settings("assets/fonts/main.ttf", FontImportSettings::new(Some(700)))
            .expect("font");
        meta.save().expect("save");

        let text = fs::read_to_string(meta.path()).expect("read");
        assert!(
            text.contains("model\tassets/models/hero.glb\t0.5\n"),
            "{text}"
        );
        assert!(
            text.contains("audio\tassets/audio/hit.ogg\t0.25\n"),
            "{text}"
        );
        assert!(
            text.contains("font\tassets/fonts/main.ttf\t700\n"),
            "{text}"
        );

        let reloaded = ResourceMeta::open(&root).expect("reload");
        assert_eq!(reloaded.len(), 4, "四个家族各一条");
        assert_eq!(
            reloaded.texture_settings("assets/art/tile.png"),
            TextureImportSettings::new(TextureFilter::Linear, TextureWrap::Repeat)
        );
        assert_eq!(
            reloaded.model_settings("assets/models/hero.glb"),
            ModelImportSettings::new(0.5)
        );
        assert_eq!(
            reloaded.audio_settings("assets/audio/hit.ogg"),
            AudioImportSettings::new(0.25)
        );
        assert_eq!(
            reloaded.font_settings("assets/fonts/main.ttf"),
            FontImportSettings::new(Some(700))
        );
        // 别的家族不会串味：模型那张表不该在音频键上找到东西。
        assert_eq!(
            reloaded.model_settings("assets/audio/hit.ogg"),
            ModelImportSettings::default()
        );
        assert_eq!(reloaded.model_settings("assets/audio/hit.ogg").scale, 1.0);
        fs::remove_dir_all(root).ok();
    }

    /// 缩放/音量这类浮点值必须**原样回来**：写进去 0.1，读回来还是 0.1（不是 0.1000001）。
    #[test]
    fn float_values_survive_the_text_round_trip_exactly() {
        let root = temp_root("floats");
        let mut meta = ResourceMeta::open(&root).expect("open");
        for scale in [0.1f32, 0.01, 2.5, 1.0 / 3.0, 12.75] {
            meta.set_model_settings("assets/models/m.glb", ModelImportSettings::new(scale))
                .expect("model");
            meta.save().expect("save");
            let reloaded = ResourceMeta::open(&root).expect("reload");
            assert_eq!(
                reloaded.model_settings("assets/models/m.glb").scale,
                scale,
                "{scale} 往返后必须逐位相同"
            );
        }
        fs::remove_dir_all(root).ok();
    }

    /// 默认档不占 diff：缩放 1.0、音量 1.0、字重不覆盖 ⇒ 都不该写进文件。
    #[test]
    fn default_rows_are_not_written() {
        let root = temp_root("defaults");
        let mut meta = ResourceMeta::open(&root).expect("open");
        assert!(
            !meta
                .set_model_settings("assets/models/m.glb", ModelImportSettings::default())
                .expect("model"),
            "写成默认值不算改变"
        );
        assert!(
            !meta
                .set_audio_settings("assets/audio/hit.ogg", AudioImportSettings::default())
                .expect("audio"),
            "写成默认值不算改变"
        );
        assert!(
            !meta
                .set_font_settings("assets/fonts/main.ttf", FontImportSettings::default())
                .expect("font"),
            "写成默认值不算改变"
        );
        meta.save().expect("save");
        assert!(
            !meta.path().exists(),
            "全是默认档就不该留下文件：{}",
            meta.path().display()
        );
        fs::remove_dir_all(root).ok();
    }

    /// 越界取值必须报错，不能悄悄夹到范围里（夹了就是在骗人：面板写 10000，运行期按 1000 跑）。
    #[test]
    fn out_of_range_values_are_refused_instead_of_being_clamped() {
        let root = temp_root("range");
        let mut meta = ResourceMeta::open(&root).expect("open");
        assert!(
            meta.set_model_settings("assets/models/m.glb", ModelImportSettings::new(0.0))
                .is_err(),
            "0 倍缩放会让模型塌成一点：必须拒绝"
        );
        assert!(
            meta.set_model_settings("assets/models/m.glb", ModelImportSettings::new(1e9))
                .is_err(),
            "离谱的缩放必须拒绝"
        );
        assert!(
            meta.set_audio_settings("assets/audio/hit.ogg", AudioImportSettings::new(-1.0))
                .is_err(),
            "负音量必须拒绝"
        );
        assert!(
            meta.set_audio_settings("assets/audio/hit.ogg", AudioImportSettings::new(99.0))
                .is_err(),
            "离谱的音量必须拒绝"
        );
        assert!(
            meta.set_font_settings("assets/fonts/main.ttf", FontImportSettings::new(Some(0)))
                .is_err(),
            "字重 0 不存在"
        );
        assert!(
            meta.set_font_settings("assets/fonts/main.ttf", FontImportSettings::new(Some(5000)))
                .is_err(),
            "字重 5000 不存在"
        );
        // 手写进文件的越界行同样要报错（不是只拦 API）。
        fs::write(
            root.join(RESOURCE_META_FILE),
            format!("{RESOURCE_META_HEADER}\nmodel\ta.glb\t0\n"),
        )
        .expect("write");
        let error = ResourceMeta::open(&root).expect_err("越界行必须报出来");
        assert!(
            matches!(error, ResourceMetaError::OutOfRange { .. }),
            "{error:?}"
        );
        fs::remove_dir_all(root).ok();
    }
}
