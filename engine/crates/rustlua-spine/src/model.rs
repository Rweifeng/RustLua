//! 中性数据模型：渲染、组件、Lua 三层只认这些类型，不碰第三方 crate 的类型。
//! 升级到 Spine 4.4/5.x 或切换 backend 时，本文件与上层接口保持不变。

use std::fmt;

/// Spine 的四种图集混合模式。P1 只渲染 `Normal`，其余仍随 draw 一起输出，
/// 由 GPU 侧明确跳过并计数（不静默丢片）；P2 补完整管线变体。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpineBlend {
    Normal,
    Additive,
    Multiply,
    Screen,
}

impl SpineBlend {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Additive => "additive",
            Self::Multiply => "multiply",
            Self::Screen => "screen",
        }
    }
}

impl fmt::Display for SpineBlend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一份 draw 引用的图集页路径。与 `SceneMesh.paths` 同一语义：
/// 渲染器按路径解析纹理，宿主在提取期把它登记进 `paths` 并换成下标。
#[derive(Clone, Debug, PartialEq)]
pub struct SpineDraw {
    /// 图集页的**工程内相对路径**。
    ///
    /// `Arc<str>` 而不是 `String`：页路径在骨架加载期就解析完了，逐帧产出 draw 时只是
    /// 克隆一个引用计数（一次原子加），**不分配**。改成 `String` 的话，一帧 20 个 draw
    /// 就是 20 次堆分配——P4 基准实测每帧 161 次分配里，有一大块来自这里。
    pub texture: std::sync::Arc<str>,
    pub first_index: u32,
    pub index_count: u32,
    pub blend: SpineBlend,
    pub premultiplied_alpha: bool,
}

/// CPU 侧采样后的世界空间三角网格。顶点位置是世界坐标，投影由世界通道的相机完成。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct SpineVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    /// 顶点色（light color）。P2 才加 dark color 属性。
    pub color: [f32; 4],
}

/// 每帧可被 backend 复用的一整份渲染输出。构造后按 draw order 顺序排列，
/// 上层不得重新排序、不得按纹理合批（会打乱 slot draw order）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpineRenderOutput {
    pub vertices: Vec<SpineVertex>,
    pub indices: Vec<u32>,
    pub draws: Vec<SpineDraw>,
}

impl SpineRenderOutput {
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.draws.clear();
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }

    /// 顶点数上限受 u32 索引范围约束；超限由 backend 拒绝而不是静默截断。
    ///
    /// Spine 输出、帧网格与 GPU 索引缓冲全链路使用 u32，允许多个实体合并后超过 65535 个顶点。
    pub const MAX_VERTICES: usize = u32::MAX as usize;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpineEventKind {
    Event,
    Start,
    Interrupt,
    End,
    Dispose,
    Complete,
}

impl SpineEventKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Start => "start",
            Self::Interrupt => "interrupt",
            Self::End => "end",
            Self::Dispose => "dispose",
            Self::Complete => "complete",
        }
    }
}

/// 交给 Lua 的中性动画事件。`int/value/string` 由 P2 补齐；P1 至少把
/// 事件名与类型送到脚本，满足“事件能到脚本”的验收口径。
#[derive(Clone, Debug, PartialEq)]
pub struct SpineEvent {
    pub kind: SpineEventKind,
    pub name: String,
    pub animation: String,
}

/// `.spine` 清单：工程里登记的唯一资源身份，引用的二进制/atlas/纹理页是依赖。
///
/// 扩展名 `.spineatlas` 是刻意改名：Spine 导出的 `.atlas` 与引擎自有图集格式同名，
/// 直接放进工程会被 `ResourceKind::from_path` 归错类。
#[derive(Clone, Debug, PartialEq)]
pub struct SpineManifest {
    pub spine_version: String,
    pub skeleton: String,
    pub atlas: String,
    pub pages: Vec<String>,
}

impl SpineManifest {
    /// 版本冻结：只接受 4.3.x。不匹配时报明确诊断，不做“尽力解析”。
    pub fn parse(text: &str) -> Result<Self, SpineError> {
        let mut version = None;
        let mut skeleton = None;
        let mut atlas = None;
        let mut pages = Vec::new();
        for (line_no, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(SpineError::Manifest(format!(
                    "line {}: expected key=value",
                    line_no + 1
                )));
            };
            let (key, value) = (key.trim(), value.trim());
            if value.is_empty() {
                return Err(SpineError::Manifest(format!(
                    "line {}: `{key}` is empty",
                    line_no + 1
                )));
            }
            match key {
                "spine_version" => version = Some(value.to_owned()),
                "skeleton" => skeleton = Some(value.to_owned()),
                "atlas" => atlas = Some(value.to_owned()),
                "page" => pages.push(value.to_owned()),
                other => {
                    return Err(SpineError::Manifest(format!(
                        "line {}: unknown key `{other}`",
                        line_no + 1
                    )));
                }
            }
        }
        let version =
            version.ok_or_else(|| SpineError::Manifest("missing spine_version".into()))?;
        let skeleton = skeleton.ok_or_else(|| SpineError::Manifest("missing skeleton".into()))?;
        let atlas = atlas.ok_or_else(|| SpineError::Manifest("missing atlas".into()))?;
        if !version.starts_with("4.3.") {
            return Err(SpineError::Version(version));
        }
        if pages.is_empty() {
            return Err(SpineError::Manifest("missing page".into()));
        }
        Ok(Self {
            spine_version: version,
            skeleton,
            atlas,
            pages,
        })
    }

    /// 清单引用的全部工程内相对路径（资源依赖边）。
    #[must_use]
    pub fn dependencies(&self) -> Vec<String> {
        let mut all = Vec::with_capacity(2 + self.pages.len());
        all.push(self.skeleton.clone());
        all.push(self.atlas.clone());
        all.extend(self.pages.iter().cloned());
        all
    }
}

/// `SpineSkeleton` 组件载荷。沿用引擎的字符串载荷惯例（场景/Prefab 原文往返）。
#[derive(Clone, Debug, PartialEq)]
pub struct SpineSkeletonComponent {
    pub data: String,
    pub animation: String,
    pub skin: String,
    pub speed: f32,
    pub scale: f32,
    pub z: f32,
    pub looped: bool,
    pub enabled: bool,
    pub sorting_layer: String,
    pub order: i32,
}

impl Default for SpineSkeletonComponent {
    fn default() -> Self {
        Self {
            data: String::new(),
            animation: String::new(),
            skin: "default".into(),
            speed: 1.0,
            scale: 1.0,
            z: 0.0,
            looped: true,
            enabled: true,
            sorting_layer: "default".into(),
            order: 0,
        }
    }
}

impl SpineSkeletonComponent {
    pub fn from_component_str(value: &str) -> Result<Self, SpineError> {
        let mut out = Self::default();
        let mut seen_data = false;
        for (index, part) in value.split(';').enumerate() {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let Some((key, raw)) = part.split_once('=') else {
                return Err(SpineError::Component(format!(
                    "field {index}: expected key=value, got `{part}`"
                )));
            };
            let (key, raw) = (key.trim(), raw.trim());
            match key {
                "data" => {
                    if raw.is_empty() {
                        return Err(SpineError::Component("data is empty".into()));
                    }
                    out.data = raw.to_owned();
                    seen_data = true;
                }
                "animation" => out.animation = raw.to_owned(),
                "skin" => out.skin = raw.to_owned(),
                "speed" => {
                    out.speed = parse_f32(key, raw)?;
                    if !out.speed.is_finite() || out.speed < 0.0 {
                        return Err(SpineError::Component("speed must be >= 0".into()));
                    }
                }
                "scale" => {
                    out.scale = parse_f32(key, raw)?;
                    if !out.scale.is_finite() || out.scale <= 0.0 {
                        return Err(SpineError::Component("scale must be > 0".into()));
                    }
                }
                "z" => {
                    out.z = parse_f32(key, raw)?;
                    if !out.z.is_finite() {
                        return Err(SpineError::Component("z must be finite".into()));
                    }
                }
                "loop" => out.looped = parse_bool(key, raw)?,
                "enabled" => out.enabled = parse_bool(key, raw)?,
                "sorting_layer" => {
                    if raw.is_empty() {
                        return Err(SpineError::Component("sorting_layer is empty".into()));
                    }
                    out.sorting_layer = raw.to_owned();
                }
                "order" => {
                    out.order = raw.parse::<i32>().map_err(|error| {
                        SpineError::Component(format!("order `{raw}`: {error}"))
                    })?;
                }
                other => {
                    return Err(SpineError::Component(format!("unknown field `{other}`")));
                }
            }
        }
        if !seen_data {
            return Err(SpineError::Component("missing data".into()));
        }
        Ok(out)
    }

    /// 稳定序列化：字段顺序固定，场景往返不会改写内容。
    #[must_use]
    pub fn to_component_str(&self) -> String {
        format!(
            "data={};animation={};skin={};speed={:.6};scale={:.6};z={:.6};loop={};enabled={};sorting_layer={};order={}",
            self.data,
            self.animation,
            self.skin,
            self.speed,
            self.scale,
            self.z,
            self.looped,
            self.enabled,
            self.sorting_layer,
            self.order,
        )
    }

    #[must_use]
    pub fn asset_paths(&self) -> Vec<String> {
        vec![self.data.clone()]
    }
}

fn parse_f32(key: &str, raw: &str) -> Result<f32, SpineError> {
    raw.parse::<f32>()
        .map_err(|error| SpineError::Component(format!("{key} `{raw}`: {error}")))
}

fn parse_bool(key: &str, raw: &str) -> Result<bool, SpineError> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(SpineError::Component(format!(
            "{key} `{other}` must be true or false"
        ))),
    }
}

/// 一个附件在骨架里的定位（`skin` / `slot` / 附件名）。用于给"不支持的特性"报可定位诊断。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpineAttachment {
    pub skin: String,
    pub slot: String,
    pub name: String,
}

impl fmt::Display for SpineAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "skin `{}` slot `{}` attachment `{}`",
            self.skin, self.slot, self.name
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpineError {
    #[error("spine 4.3.x required, got `{0}`")]
    Version(String),
    #[error("invalid .spine manifest: {0}")]
    Manifest(String),
    #[error("invalid SpineSkeleton component: {0}")]
    Component(String),
    #[error("spine runtime: {0}")]
    Runtime(String),
    #[error("animation `{0}` not found")]
    AnimationNotFound(String),
    #[error("skin `{0}` not found")]
    SkinNotFound(String),
    #[error("mesh exceeds supported u32 vertex index range: {0} vertices")]
    TooManyVertices(usize),
    #[error("unsupported blend `{0}` in P1 pipeline")]
    UnsupportedBlend(SpineBlend),
    /// 骨架用到本渲染管线还没实现的特性（P2 的 clipping）。
    ///
    /// 刻意是**导入期硬错**而不是运行时静默跳过：clipping 不生效的症状是"该裁的地方
    /// 没裁"，画面看着像对了但实际在漏内容——那比报错难查得多。
    #[error(
        "Spine clipping attachments are not supported yet (found {count}: {first}). \
         在 Spine 编辑器里删除 clipping 附件后重新导出，或等待裁剪支持"
    )]
    UnsupportedClipping {
        count: usize,
        first: SpineAttachment,
    },
}
