//! P3-R05-2/3：shader 源的**校验与绑定接口比对**（纯 CPU，不碰 GPU）。
//!
//! 热重载真正的危险不是"编译慢"，而是**绑定接口漂移**：新 shader 少一个/多一个绑定、换了
//! 地址空间或类型，而 bind group layout 是 Rust 侧的常量、不会跟着重建。那种 shader 塞进旧
//! 布局后，wgpu 要么在校验期报错（信息难读），要么更糟——按错位的方式读到别的 buffer，
//! 画面"看起来还行"。所以这里在重建管线**之前**先把接口逐 slot 比对一次，变了就拒绝并说明
//! 哪里变了（计划 §6.5 的硬门）。
//!
//! 可见性（visibility）不在这份接口里：本项目所有管线都用**显式** `PipelineLayout`，
//! 阶段可见性由 Rust 侧的 layout 决定，shader 文本改不了它；真改了会在校验期失败并由
//! [`Renderer::set_shader_override_dir`] 保留旧管线。
//!
//! [`Renderer::set_shader_override_dir`]: crate::renderer::Renderer::set_shader_override_dir

use std::fmt::Write as _;

use naga::{
    AddressSpace, ArraySize, ImageClass, Module, Scalar, ScalarKind, Type, TypeInner, VectorSize,
};

/// WGSL 入口点约定：网格/阴影/后处理管线都只认这两个名字（见 `construct.rs` 的 `entry_point`）。
pub const REQUIRED_ENTRY_POINTS: [&str; 2] = ["vs_main", "fs_main"];

/// 一个资源绑定的身份。
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BindingSlot {
    pub group: u32,
    pub binding: u32,
    /// `uniform` / `storage(read+write)` / `handle` / `work_group` …
    pub space: String,
    /// 类型签名（WGSL 风格的形状：`vec4<f32>`、`struct<GpuLights>{…}`、`texture_2d<f32>`）。
    pub ty: String,
}

impl BindingSlot {
    /// 人读的一行（也是接口哈希的输入）。
    #[must_use]
    pub fn label(&self) -> String {
        format!(
            "@group({}) @binding({}) var<{}> : {}",
            self.group, self.binding, self.space, self.ty
        )
    }
}

/// 一份（已预处理的）WGSL 源的绑定接口 + 入口点集合。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderInterface {
    pub slots: Vec<BindingSlot>,
    /// `(阶段, 入口名)`，按字典序稳定排序。
    pub entry_points: Vec<(String, String)>,
}

impl ShaderInterface {
    /// 接口哈希：与源文本哈希分开——**内容变了但接口没变**才允许热重载。
    #[must_use]
    pub fn hash(&self) -> u64 {
        let mut text = String::new();
        for slot in &self.slots {
            let _ = writeln!(text, "{}", slot.label());
        }
        let _ = writeln!(text, "--");
        for (stage, name) in &self.entry_points {
            let _ = writeln!(text, "{stage} {name}");
        }
        crate::shader::fnv1a64(text.as_bytes())
    }

    /// 与另一份接口的差异（人读，用于"拒绝原因"）。两侧各列自己独有的 slot/入口点。
    #[must_use]
    pub fn describe_diff(&self, other: &Self) -> String {
        let mut out = String::new();
        for line in self
            .slots
            .iter()
            .filter(|slot| !other.slots.contains(slot))
            .map(BindingSlot::label)
        {
            let _ = writeln!(out, "  旧: {line}");
        }
        for line in other
            .slots
            .iter()
            .filter(|slot| !self.slots.contains(slot))
            .map(BindingSlot::label)
        {
            let _ = writeln!(out, "  新: {line}");
        }
        if self.entry_points != other.entry_points {
            let _ = writeln!(
                out,
                "  入口点: {:?} -> {:?}",
                self.entry_points, other.entry_points
            );
        }
        if out.is_empty() {
            // 逐项相同只可能是 slot 集合之外的差异（理论上不会发生，留着便于定位）。
            out.push_str("  （无逐项差异，仅接口哈希不同）\n");
        }
        out
    }
}

/// 校验失败的原因。每种都带可复制的原文，编辑器 Console 直接显示。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShaderValidateError {
    /// `naga` 解析失败，`message` 里带 `文件:行:列`。
    Parse(String),
    /// 语法没问题但**类型/语义**校验失败。`offset` 是出错处的**字节偏移（0 基）**，坐标是
    /// **预处理后的生成源**；调用方用 [`line_of_offset`] 换成行号，再配合行号映射指回作者改的
    /// 那个文件。`message` 是 naga 的原文（带生成源里的 `行:列`）。
    Validate { message: String, offset: u32 },
    /// 缺少管线要求的入口点。
    MissingEntryPoint(String),
    /// 与当前生效的接口不同（拒绝热重载）。
    InterfaceChanged { variant: String, diff: String },
}

impl std::fmt::Display for ShaderValidateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(message) => write!(f, "WGSL 解析失败：{message}"),
            Self::Validate { message, .. } => write!(f, "WGSL 校验失败：{message}"),
            Self::MissingEntryPoint(name) => {
                write!(f, "缺少入口点 {name}（管线要求 vs_main/fs_main）")
            }
            Self::InterfaceChanged { variant, diff } => {
                write!(f, "绑定接口变化，拒绝热重载（变体 {variant}）：\n{diff}")
            }
        }
    }
}

impl std::error::Error for ShaderValidateError {}

/// 解析 WGSL 并抽取绑定接口。
///
/// 只做**结构**解析，不做类型检查/校验：`naga` 的 `parse` 已覆盖语法与声明层错误。热重载的
/// 提交路径用 [`validate_module`]（多做一次类型/语义校验），`path` 只用于错误信息里定位文件。
pub fn parse_interface(path: &str, source: &str) -> Result<ShaderInterface, ShaderValidateError> {
    let module = parse_module(path, source)?;
    Ok(interface_of(&module))
}

/// 只做语法/声明层解析，返回 `naga` 模块；错误文本里的文件名用 `path`。
pub fn parse_module(path: &str, source: &str) -> Result<Module, ShaderValidateError> {
    naga::front::wgsl::parse_str(source).map_err(|error| {
        let label = if path.is_empty() { "<wgsl>" } else { path };
        ShaderValidateError::Parse(error.emit_to_string_with_path(source, label))
    })
}

/// 解析 + `naga` 类型/语义校验：GPU 之前能跑的完整一道门。
///
/// 为什么值得在 CPU 侧先校验一遍：wgpu 也会校验，但它的报错行号是**预处理后的生成源**里的，
/// 而作者改的是覆盖文件里另一套行号（`//#if` 会删行）。这里除了 naga 原文，还把首个出错位置
/// 的字节偏移交回调用方，由它配合 [`PreprocessedShader::origin_of`] 映射回原文件的行号。
///
/// [`PreprocessedShader::origin_of`]: crate::shader_preprocess::PreprocessedShader::origin_of
pub fn validate_module(path: &str, source: &str) -> Result<Module, ShaderValidateError> {
    let module = parse_module(path, source)?;
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    validator.validate(&module).map_or_else(
        |error| {
            let offset = error.location(source).map_or(0, |location| location.offset);
            Err(ShaderValidateError::Validate {
                message: naga::error::ShaderError {
                    source: source.to_owned(),
                    label: Some(path.to_owned()),
                    inner: Box::new(error),
                }
                .to_string(),
                offset,
            })
        },
        |_info| Ok(module),
    )
}

/// 字节偏移 → 1 基行号（在给定那份源里）。
#[must_use]
pub fn line_of_offset(source: &str, offset: u32) -> usize {
    let end = (offset as usize).min(source.len());
    source[..end].bytes().filter(|byte| *byte == b'\n').count() + 1
}

/// 从已解析的模块抽取接口（测试与 `parse_interface` 共用）。
#[must_use]
pub fn interface_of(module: &Module) -> ShaderInterface {
    let mut slots: Vec<BindingSlot> = module
        .global_variables
        .iter()
        .filter_map(|(_handle, variable)| {
            let binding = variable.binding?;
            Some(BindingSlot {
                group: binding.group,
                binding: binding.binding,
                space: space_label(&variable.space),
                ty: describe_type(module, variable.ty, 0),
            })
        })
        .collect();
    slots.sort();
    slots.dedup();
    let mut entry_points: Vec<(String, String)> = module
        .entry_points
        .iter()
        .map(|entry| (stage_label(entry.stage), entry.name.clone()))
        .collect();
    entry_points.sort();
    ShaderInterface {
        slots,
        entry_points,
    }
}

/// 入口点齐不齐（缺了的话管线创建会失败，但失败信息来得晚且难读，所以先自己查）。
///
/// 检查的是"源里声明的入口点"，不是"这个变体用到了哪些"：预处理后的单个变体仍保留两个阶段
/// （`shader_preprocess` 只裁剪特性分支，不删入口）。
pub fn require_entry_points(interface: &ShaderInterface) -> Result<(), ShaderValidateError> {
    for name in REQUIRED_ENTRY_POINTS {
        if !interface
            .entry_points
            .iter()
            .any(|(_, entry)| entry == name)
        {
            return Err(ShaderValidateError::MissingEntryPoint(name.to_owned()));
        }
    }
    Ok(())
}

fn stage_label(stage: naga::ShaderStage) -> String {
    match stage {
        naga::ShaderStage::Vertex => "vertex".to_owned(),
        naga::ShaderStage::Fragment => "fragment".to_owned(),
        naga::ShaderStage::Compute => "compute".to_owned(),
        // naga 27 预留的 mesh/task 管线（wgpu 还没有对应入口）：留可区分的标签，
        // 将来出现这类入口点时接口比对必然判为"变化"，不会被静默放行。
        other => format!("{other:?}").to_lowercase(),
    }
}

fn space_label(space: &AddressSpace) -> String {
    match space {
        AddressSpace::Function => "function".to_owned(),
        AddressSpace::Private => "private".to_owned(),
        AddressSpace::WorkGroup => "work_group".to_owned(),
        AddressSpace::Uniform => "uniform".to_owned(),
        AddressSpace::Handle => "handle".to_owned(),
        AddressSpace::PushConstant => "push_constant".to_owned(),
        AddressSpace::Storage { access } => {
            format!("storage({})", access_label(*access))
        }
    }
}

/// 访问标志的稳定拼写（位标志的 `Debug` 顺序不保证，且直接暴露给日志会漂移）。
fn access_label(access: naga::StorageAccess) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(3);
    if access.contains(naga::StorageAccess::LOAD) {
        parts.push("read");
    }
    if access.contains(naga::StorageAccess::STORE) {
        parts.push("write");
    }
    if access.contains(naga::StorageAccess::ATOMIC) {
        parts.push("atomic");
    }
    if parts.is_empty() {
        return "none".to_owned();
    }
    parts.join("+")
}

/// 类型的稳定签名：递归展开结构体字段与数组/指针的基类型（限深，避免异常源递归爆栈）。
///
/// 这里刻意输出**接近 WGSL 原文**的形状，而不是 `Debug` 转储：`Debug` 里带 `Handle` 索引，
/// 索引会随无关声明的顺序漂移，用它做接口比对会出现"没改绑定却哈希变了"的假拒绝。
fn describe_type(module: &Module, handle: naga::Handle<Type>, depth: u32) -> String {
    let ty = &module.types[handle];
    let named = ty.name.as_deref().unwrap_or("");
    let inner = match &ty.inner {
        TypeInner::Scalar(scalar) => return scalar_label(*scalar),
        TypeInner::Vector { size, scalar } => {
            format!("vec{}<{}>", vector_len(*size), scalar_label(*scalar))
        }
        TypeInner::Matrix {
            columns,
            rows,
            scalar,
        } => format!(
            "mat{}x{}<{}>",
            vector_len(*columns),
            vector_len(*rows),
            scalar_label(*scalar)
        ),
        TypeInner::Atomic(scalar) => format!("atomic<{}>", scalar_label(*scalar)),
        TypeInner::Pointer { base, space } if depth < 6 => {
            format!(
                "ptr<{},{}>",
                describe_type(module, *base, depth + 1),
                space_label(space)
            )
        }
        TypeInner::ValuePointer {
            size,
            scalar,
            space,
        } => {
            let base = match size {
                Some(size) => format!("vec{}<{}>", vector_len(*size), scalar_label(*scalar)),
                None => scalar_label(*scalar),
            };
            format!("ptr<{},{}>", base, space_label(space))
        }
        TypeInner::Array { base, size, .. } if depth < 6 => format!(
            "array<{},{}>",
            describe_type(module, *base, depth + 1),
            array_size_label(*size)
        ),
        TypeInner::BindingArray { base, size } if depth < 6 => format!(
            "binding_array<{},{}>",
            describe_type(module, *base, depth + 1),
            array_size_label(*size)
        ),
        TypeInner::Struct { members, .. } if depth < 6 => {
            // 字段名与类型都进签名：uniform/storage 的布局由它们决定，名字本身也是 WGSL
            // 里绑定校验的一部分（同名换序 = 布局变了两回事）。
            let mut parts = Vec::with_capacity(members.len());
            for member in members {
                parts.push(format!(
                    "{}:{}",
                    member.name.as_deref().unwrap_or("_"),
                    describe_type(module, member.ty, depth + 1)
                ));
            }
            return format!("struct<{named}>{{{}}}", parts.join(","));
        }
        TypeInner::Image {
            dim,
            arrayed,
            class,
        } => image_label(*dim, *arrayed, class),
        TypeInner::Sampler { comparison } => {
            if *comparison {
                "sampler_comparison".to_owned()
            } else {
                "sampler".to_owned()
            }
        }
        TypeInner::AccelerationStructure { vertex_return } => {
            format!("acceleration_structure(vertex_return={vertex_return})")
        }
        TypeInner::RayQuery { vertex_return } => {
            format!("ray_query(vertex_return={vertex_return})")
        }
        // 超过递归深度（畸形源）或没覆盖到的新变体：退化成一个可区分的标签，宁可拒绝重载。
        other => format!("depth_limited<{other:?}>"),
    };
    if named.is_empty() {
        inner
    } else {
        format!("{named}#{inner}")
    }
}

fn array_size_label(size: ArraySize) -> String {
    match size {
        ArraySize::Constant(len) => len.get().to_string(),
        // `Pending` 是 override 表达式（管线常量），重载时按名字阶段区分即可；
        // `Dynamic` 意味着运行时长度，绑定槽的缓冲区尺寸由宿主决定，两者都必须与旧源一致。
        ArraySize::Pending(_) => "pending".to_owned(),
        ArraySize::Dynamic => "dynamic".to_owned(),
    }
}

fn vector_len(size: VectorSize) -> u8 {
    match size {
        VectorSize::Bi => 2,
        VectorSize::Tri => 3,
        VectorSize::Quad => 4,
    }
}

fn scalar_label(scalar: Scalar) -> String {
    let Scalar { kind, width } = scalar;
    match (kind, width) {
        (ScalarKind::Float, 4) => "f32".to_owned(),
        (ScalarKind::Float, 2) => "f16".to_owned(),
        (ScalarKind::Float, 8) => "f64".to_owned(),
        (ScalarKind::Sint, 4) => "i32".to_owned(),
        (ScalarKind::Sint, 8) => "i64".to_owned(),
        (ScalarKind::Uint, 4) => "u32".to_owned(),
        (ScalarKind::Uint, 8) => "u64".to_owned(),
        (ScalarKind::Bool, _) => "bool".to_owned(),
        (ScalarKind::AbstractInt, _) => "abstract_int".to_owned(),
        (ScalarKind::AbstractFloat, _) => "abstract_float".to_owned(),
        // 未覆盖的组合（新宽度/新 kind）仍要能进签名，不能和已知形状撞车。
        (kind, width) => format!("{kind:?}{width}"),
    }
}

fn image_label(dim: naga::ImageDimension, arrayed: bool, class: &ImageClass) -> String {
    let dim = match dim {
        naga::ImageDimension::D1 => "1d",
        naga::ImageDimension::D2 => "2d",
        naga::ImageDimension::D3 => "3d",
        naga::ImageDimension::Cube => "cube",
    };
    let suffix = if arrayed { "_array" } else { "" };
    match class {
        ImageClass::Sampled { kind, multi } => {
            let kind = scalar_label(Scalar {
                kind: *kind,
                width: 4,
            });
            if *multi {
                format!("texture_{dim}{suffix}_multisampled<{kind}>")
            } else {
                format!("texture_{dim}{suffix}<{kind}>")
            }
        }
        ImageClass::Depth { multi } => {
            let multi = if *multi { "_multisampled" } else { "" };
            format!("texture_depth_{dim}{multi}{suffix}")
        }
        ImageClass::External => format!("texture_external{suffix}"),
        ImageClass::Storage { format, access } => format!(
            "texture_storage_{dim}{suffix}<{format:?},{}>",
            access_label(*access)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r"
struct Params { intensity: f32, color: vec4<f32> }
@group(0) @binding(0) var<uniform> params: Params;
@group(1) @binding(3) var<storage, read_write> items: array<vec4<f32>, 16>;
@group(1) @binding(2) var albedo: texture_2d<f32>;
@group(1) @binding(1) var point: sampler;

struct VertexOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs_main() -> VertexOut {
    var out: VertexOut;
    out.pos = params.color;
    out.uv = vec2<f32>(0.0);
    return out;
}
@fragment fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(albedo, point, in.uv) * params.intensity + items[0];
}
";

    /// 接口必须是**可读且稳定**的：按 group/binding 升序，类型签名是 WGSL 形状。
    #[test]
    fn parses_slots_in_stable_order_with_readable_signatures() {
        let interface = parse_interface("minimal.wgsl", MINIMAL).expect("合法源");
        let labels: Vec<String> = interface.slots.iter().map(BindingSlot::label).collect();
        assert_eq!(
            labels,
            vec![
                "@group(0) @binding(0) var<uniform> : struct<Params>{intensity:f32,color:vec4<f32>}"
                    .to_owned(),
                "@group(1) @binding(1) var<handle> : sampler".to_owned(),
                "@group(1) @binding(2) var<handle> : texture_2d<f32>".to_owned(),
                "@group(1) @binding(3) var<storage(read+write)> : array<vec4<f32>,16>".to_owned(),
            ],
            "实际读数：{labels:#?}"
        );
        assert_eq!(
            interface.entry_points,
            vec![
                ("fragment".to_owned(), "fs_main".to_owned()),
                ("vertex".to_owned(), "vs_main".to_owned())
            ]
        );
        require_entry_points(&interface).expect("入口点齐全");
    }

    /// 全部 8 个内置 shader 都要能被**完整校验**（语法 + 类型/语义），且入口点齐全。
    ///
    /// 这条同时钉住校验层的"不误伤"：`ValidationFlags::all()` 若拒绝了一份 wgpu 实际接受
    /// 的内置源，热重载就会把工程自带的 shader 也判成坏源——那是比漏判更糟的失败。
    #[test]
    fn every_builtin_shader_yields_a_valid_interface() {
        const FEATURES: [&str; 3] = ["SKIN", "IBL_DIFFUSE", "IBL_SPECULAR"];
        for shader in crate::shader::ALL_SHADERS {
            let preprocessed = crate::shader_preprocess::preprocess(
                shader.path,
                shader.source,
                crate::shader_preprocess::PreprocessOptions {
                    features: &FEATURES,
                    known_features: &FEATURES,
                    ..Default::default()
                },
            )
            .unwrap_or_else(|error| panic!("{} 预处理失败：{error}", shader.path));
            let module = validate_module(shader.path, preprocessed.source())
                .unwrap_or_else(|error| panic!("{} 校验失败：{error}", shader.path));
            let interface = interface_of(&module);
            require_entry_points(&interface)
                .unwrap_or_else(|error| panic!("{}：{error}", shader.path));
            assert_eq!(
                interface,
                parse_interface(shader.path, preprocessed.source()).expect("解析成功"),
                "{}：`validate_module` 与 `parse_interface` 必须给出同一份接口",
                shader.path
            );
            assert!(
                !interface.slots.is_empty(),
                "{} 应有绑定：{:?}",
                shader.path,
                interface.slots
            );
        }
    }

    /// 语法没错、语义不过：`naga` 校验层必须拒绝，并给出可映射回原文件的偏移。
    ///
    /// 选的例子是"运行时长度数组不在结构体末尾"（`struct Item { data: array<u32>, len: u32 }`）：
    /// 前端照单全收，只有校验层的布局检查会拦。这类源进了 GPU 之后要么编不过要么画出错位数据。
    #[test]
    fn validate_module_rejects_a_validator_only_error() {
        let source = MINIMAL
            .replace(
                "@group(1) @binding(3) var<storage, read_write> items: array<vec4<f32>, 16>;",
                "@group(1) @binding(3) var<storage, read_write> items: Item;",
            )
            .replace(
                "    return textureSample(albedo, point, in.uv) * params.intensity + items[0];",
                "    return textureSample(albedo, point, in.uv) * params.intensity;",
            )
            + "\nstruct Item { data: array<u32>, len: u32 }\n";
        assert_ne!(source, MINIMAL, "替换必须命中绑定声明那一行");
        let error =
            validate_module("broken.wgsl", &source).expect_err("运行时长度数组不在末尾必须失败");
        let ShaderValidateError::Validate { message, offset } = &error else {
            panic!("应是校验错误而不是解析错误：{error:?}");
        };
        assert!(
            parse_interface("broken.wgsl", &source).is_ok(),
            "解析层必须放行这条源，否则用例没有区分度：{error}"
        );
        assert!(
            message.contains("broken.wgsl"),
            "校验原文要能指到文件（带 label 才会打出行号）：{message}"
        );
        // 偏移要落在结构体那一行附近——`line_of_offset` + 行号映射靠它指回作者那份文件。
        let line = line_of_offset(&source, *offset);
        assert!(
            (17..=21).contains(&line),
            "偏移应指向新增结构体附近，实际第 {line} 行：{message}"
        );
    }

    /// 有的校验错误**没有位置**（入口点级的错误整条源都没有 span）。这类错误照样必须失败，
    /// 只是没法指到行——`locate` 因此在偏移映射不到时保留原文，而不是编一个行号。
    #[test]
    fn validate_module_rejects_an_unspanned_stage_error() {
        let source = MINIMAL.replace(
            "    out.uv = vec2<f32>(0.0);",
            "    out.uv = vec2<f32>(dpdx(params.intensity));",
        );
        assert_ne!(source, MINIMAL, "替换必须命中顶点函数那一行");
        let error = validate_module("broken.wgsl", &source).expect_err("顶点阶段里的导数必须失败");
        let ShaderValidateError::Validate { message, offset } = &error else {
            panic!("应是校验错误而不是解析错误：{error:?}");
        };
        assert!(
            message.contains("Vertex"),
            "原文要说明错在哪个阶段：{message}"
        );
        // 没有 span ⇒ 偏移退回 0（第一行），映射层据此判定"指不出行号"。
        assert_eq!(*offset, 0);
    }

    /// 返回值类型不匹配在前端就被拦住：错误文本同样要带文件名与行号。
    #[test]
    fn validate_module_reports_a_parse_level_type_error() {
        let source = MINIMAL.replace(
            "    return textureSample(albedo, point, in.uv) * params.intensity + items[0];",
            "    return 1.0;",
        );
        assert_ne!(source, MINIMAL, "替换必须命中片元那一行");
        let error = validate_module("broken.wgsl", &source).expect_err("返回值类型不匹配必须失败");
        let ShaderValidateError::Parse(message) = &error else {
            panic!("这条应在前端就被拦住：{error:?}");
        };
        assert!(message.contains("broken.wgsl"), "原文要带文件名：{message}");
    }

    /// 偏移 → 行号（热重载用它把生成源的行号换回作者看的那份文件）。
    #[test]
    fn line_of_offset_counts_newlines_before_the_offset() {
        let source = "a\nbb\nccc\n";
        assert_eq!(line_of_offset(source, 0), 1);
        assert_eq!(line_of_offset(source, 2), 2);
        assert_eq!(line_of_offset(source, 5), 3);
        assert_eq!(line_of_offset(source, 99), 4);
    }

    /// 接口的**稳定性**：只改函数体（颜色常量）不改接口，哈希不变——这正是允许热重载的情形。
    #[test]
    fn body_only_changes_keep_the_interface_hash() {
        let before = parse_interface("a.wgsl", MINIMAL).unwrap();
        let after = parse_interface(
            "a.wgsl",
            &MINIMAL.replace("vec2<f32>(0.0)", "vec2<f32>(0.25)"),
        )
        .unwrap();
        assert_eq!(before.hash(), after.hash(), "只改片元数学不该触发接口拒绝");
        assert_eq!(before, after);
    }

    /// 重排声明顺序（索引漂移）不应改变接口——这是不用 `Debug` 转储的原因。
    #[test]
    fn declaration_order_does_not_change_the_interface() {
        let before = parse_interface("a.wgsl", MINIMAL).unwrap();
        let swapped = MINIMAL.replace(
            "@group(1) @binding(2) var albedo: texture_2d<f32>;\n@group(1) @binding(1) var point: sampler;",
            "@group(1) @binding(1) var point: sampler;\n@group(1) @binding(2) var albedo: texture_2d<f32>;",
        );
        let after = parse_interface("a.wgsl", &swapped).unwrap();
        assert_eq!(before.hash(), after.hash(), "声明顺序不该影响接口哈希");
    }

    /// 接口的**敏感**：换 binding、换地址空间、换类型维度都必须识别为变化（硬门）。
    #[test]
    fn binding_moves_and_space_changes_are_detected() {
        let before = parse_interface("a.wgsl", MINIMAL).unwrap();
        let moved = parse_interface(
            "a.wgsl",
            &MINIMAL.replace("@group(1) @binding(3)", "@group(1) @binding(4)"),
        )
        .unwrap();
        assert_ne!(before.hash(), moved.hash(), "@binding 变了必须发现");
        let respaced = parse_interface(
            "a.wgsl",
            &MINIMAL.replace("var<uniform> params", "var<storage, read> params"),
        )
        .unwrap();
        assert_ne!(before.hash(), respaced.hash(), "地址空间/访问变了必须发现");
        let retyped = parse_interface(
            "a.wgsl",
            &MINIMAL.replace(
                "var albedo: texture_2d<f32>;",
                "var albedo: texture_1d<f32>;",
            ),
        )
        .unwrap();
        assert_ne!(before.hash(), retyped.hash(), "采样纹理维度变了必须发现");
        let resized = parse_interface(
            "a.wgsl",
            &MINIMAL.replace("array<vec4<f32>, 16>", "array<vec4<f32>, 32>"),
        )
        .unwrap();
        assert_ne!(
            before.hash(),
            resized.hash(),
            "数组长度变了必须发现（缓冲区尺寸不同）"
        );
        let diff = before.describe_diff(&moved);
        assert!(diff.contains("旧: @group(1) @binding(3)"), "{diff}");
        assert!(diff.contains("新: @group(1) @binding(4)"), "{diff}");
    }

    /// 结构体字段类型变了（uniform 布局随之变化）也要发现。
    #[test]
    fn struct_field_type_change_is_detected() {
        let before = parse_interface("a.wgsl", MINIMAL).unwrap();
        let after = parse_interface(
            "a.wgsl",
            &MINIMAL.replace("intensity: f32", "intensity: u32"),
        )
        .unwrap();
        assert_ne!(before.hash(), after.hash());
    }

    #[test]
    fn bad_wgsl_reports_a_located_parse_error() {
        let broken = MINIMAL.replace("struct Params {", "struct Params (");
        let error = parse_interface("broken.wgsl", &broken).expect_err("坏语法");
        let ShaderValidateError::Parse(message) = &error else {
            panic!("应当是解析失败：{error:?}");
        };
        assert!(message.contains("broken.wgsl"), "错误要带文件名：{message}");
        assert!(
            message.contains("broken.wgsl:"),
            "错误要定位到文件:行:列：{message}"
        );
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn missing_entry_point_is_rejected() {
        let no_fragment = MINIMAL.replace("@fragment fn fs_main", "@fragment fn not_the_entry");
        let interface = parse_interface("a.wgsl", &no_fragment).unwrap();
        let error = require_entry_points(&interface).expect_err("缺 fs_main");
        assert_eq!(
            error,
            ShaderValidateError::MissingEntryPoint("fs_main".to_owned())
        );
        assert!(error.to_string().contains("vs_main/fs_main"));
    }

    /// 没有 `@group/@binding` 的全局（private）不进接口：它们不由宿主提供，改了也不会错绑。
    #[test]
    fn globals_without_bindings_are_not_slots() {
        let with_private = MINIMAL.replace(
            "struct VertexOut {",
            "var<private> scratch: f32;\nstruct VertexOut {",
        );
        let interface = parse_interface("a.wgsl", &with_private).unwrap();
        assert_eq!(interface.slots.len(), 4);
    }

    /// `InterfaceChanged` 的展示文案（编辑器 Console 直接打出来，要能复制）。
    #[test]
    fn interface_changed_error_is_printable_with_the_variant_name() {
        let error = ShaderValidateError::InterfaceChanged {
            variant: "mesh3d-pbr-skin-hdr-ibl-full".to_owned(),
            diff: "  旧: @group(0) @binding(0)\n".to_owned(),
        };
        let text = error.to_string();
        assert!(text.contains("mesh3d-pbr-skin-hdr-ibl-full"), "{text}");
        assert!(text.contains("拒绝热重载"), "{text}");
    }
}
