//! 结构门：`new_async` 的**资源创建序列**必须与登记的一致。
//!
//! ## 为什么需要它
//!
//! `docs/62` 的 1.7 版更正记下了一条没销的账：
//!
//! > WP12 当时对 `new_async` 写的是"只搬位置，不拆函数体"，理由是
//! > "必须另开工作包（**PBR/HDR 落地时顺势做**）"。而 PBR + HDR 已于 2026-10-23/24 交付。
//! > 触发条件满足了，工作包没开；`new_async` 从搬迁时的 1,070 行长到 2,371 行。
//!
//! 拆那个函数之前，必须先有**拆分前后等价**的证据。WP12 对文件级拆分用的办法是
//! "纯搬运"证明（去注释去空白后，每个行段必须原样出现在目标文件里）。那个办法
//! 对**拆函数体**不成立——变量要重新绑定、作用域要重新组织，行段必然变。
//!
//! 所以这里换一个判据：**资源创建序列**。拆函数体时，这一串东西的顺序、类型、
//! 数量一个都不能变——它们是"渲染器造出来什么"的定义。顺序变了，管线依赖的
//! 绑定组可能还没建；数量变了，就是少建或多建了一张纹理。
//!
//! ## 为什么是静态分析而不是跑起来快照
//!
//! 跑一遍 `new_async` 需要真实 `Device`（`new_offscreen` 收 `Device` + `Queue`），
//! 而 CI runner 没有 GPU——见 `renderer/tests/mod.rs` 的 `gpu_skipped` 注释：
//! 那些用例在 CI 上**恒为跳过**。一个只在本地能跑的证据，拦不住 CI 上的回归。
//!
//! 静态扫描不碰 GPU，因此在任何机器上都真跑。代价是它只能看"写了什么"，
//! 看不到"实际创出了什么"——但那正是拆分要守的东西：**源码层面的等价**。
//!
//! ## 失败了怎么办
//!
//! 两种可能，先分清再动手：
//!
//! 1. **有意的改动**（新增一条管线、换了采样器过滤）：更新本文件的 `EXPECTED` 清单，
//!    并在提交信息里说清为什么多一个/少一个。
//! 2. **拆分 `new_async` 时不小心漏了或搬错了顺序**：清单会精确指出第几项对不上。
//!
//! 这个门**不判断哪个是对的**——它只保证"变化必须是有人决定的"。
//!
//! ## 它测不到什么（2026-11-18 实测，必须说清）
//!
//! 静态扫描数的是**创建语句出现的次数**，不是**运行时真正建了几个**。
//! 实测：把 `texture::texture_samplers` 的内层循环从 3 个 wrap 减到 2 个，
//! 创建语句仍然只有一处、顺序没变，**本门照常绿**——而实际少建了 2 个采样器。
//!
//! 这个盲区不能靠静态扫描补（要看得懂循环边界就等于写了个小型解释器）。
//! 它由两道运行时判据兜着：
//!
//! - `texture::texture_samplers` 里的 `debug_assert_eq!`（数量必须等于
//!   `TextureFilter::COUNT × TextureWrap::COUNT`）；
//! - `the_undocumented_texture_path_keeps_todays_pixels`（第一格的参数口径，
//!   它管的是"采样器参数对不对"，不是数量）。
//!
//! 换句话说：**本门守顺序，运行时的门守数量**。两者职责不重叠，改这里之前先想清楚
//! 要守的是哪一半。

use std::path::{Path, PathBuf};

/// `construct/` 目录（`mod.rs` 与各批次子模块都在这里）。
fn construct_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/renderer/construct")
}

/// `mod.rs` 的源码。
fn construct_source() -> String {
    let path = construct_dir().join("mod.rs");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读不到 {}：{error}", path.display()))
}

/// 把 `new_async` 里对子模块的调用**就地展开**成那个子模块的全部资源创建。
///
/// ## 为什么要展开
///
/// 批次 2 起，资源创建开始分散到 `construct/` 的子模块里（`texture::texture_samplers`
/// 就是第一个）。如果门只看 `mod.rs`，那些搬到子模块的创建就会从序列里消失——
/// 2026-11-18 实测：拆完批次 2，门报"第 6 项开始不一致"，少掉 6 个 `create_sampler`。
///
/// **那不是代码错了，是判据需要演进**。批次拆分的目的是"顺序不变"，而不是"创建
/// 必须写在一个文件里"。所以这里按**调用位置**把子模块的内容接回去：
/// 调用点在哪一项，那个子模块的创建序列就插在哪一项。
///
/// 展开规则刻意保守：只认 `super::construct::<module>::<fn>(` 这种完整路径，
/// 不做泛化的跨文件调用图分析——那会引入"分析器自己出错"的风险，而这道门的价值
/// 恰恰在于它简单到可以信任。
fn expand_submodule_calls(body: &str) -> String {
    let mut out = Vec::new();
    for line in body.lines() {
        // 形如：`let texture_samplers = super::construct::texture::texture_samplers(&device);`
        // 子模块内部再调兄弟模块时会写 `super::texture::texture_samplers(...)`（少一层
        // `construct`）。两种都要认，否则拆到第二层就会漏展开。
        // **只认真正的函数调用**：`super::<mod>::<fn>(` 后面必须紧跟 `(`。
        // 只按 `super::` 切会把 `crate::renderer::ESM_LAYERS] = std::array` 这类
        // 常量路径也当成调用（实测过，报了个不存在的 `ESM_LAYERS] = std.rs`）。
        let after_super = line
            .split("super::construct::")
            .nth(1)
            .or_else(|| line.split("super::").nth(1))
            .filter(|rest| {
                // `<mod>::<fn>(` — 形如 `texture::texture_samplers(&device)`
                let mut parts = rest.split("::");
                let _module = parts.next().unwrap_or("");
                parts
                    .next()
                    .map(|tail| tail.contains('(') && !tail.contains('='))
                    .unwrap_or(false)
            });
        if let Some(rest) = after_super {
            let mut parts = rest.split("::");
            let module = parts.next().unwrap_or("").trim();
            let func = parts
                .next()
                .unwrap_or("")
                .split('(')
                .next()
                .unwrap_or("")
                .trim();
            // **只展开 `construct/` 目录下真实存在的子模块**。
            // `super::environment::bind_group_layout` 的 environment 是 `renderer/environment.rs`
            // （上一层的兄弟模块），不是 construct 的子模块——它内部的创建不归本门管。
            // 判据用"文件存在"，比维护一张白名单更不容易腐烂。
            let path = construct_dir().join(format!("{module}.rs"));
            if !module.is_empty() && !func.is_empty() && path.is_file() {
                let source = std::fs::read_to_string(&path).unwrap_or_else(|error| {
                    panic!(
                        "`new_async` 调用了 {module}::{func}，但读不到 {}：{error}",
                        path.display()
                    )
                });
                let module_body = function_body(&source, func);
                // 子模块里可能还有下一层调用（批次继续拆下去会遇到），递归展开。
                out.push(expand_submodule_calls(&module_body));
                continue;
            }
        }
        out.push(line.to_owned());
    }
    out.join("\n")
}

/// 从源码里取出某个函数（或包含该名字的第一段 `fn ... { }`）的函数体。
fn function_body(source: &str, name: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let needle = format!("fn {name}(");
    let start = lines
        .iter()
        .position(|line| line.contains(&needle))
        .unwrap_or_else(|| panic!("`{name}` 不在子模块里——调用点与定义对不上"));
    let mut depth = 0_i32;
    let mut started = false;
    for (offset, line) in lines[start..].iter().enumerate() {
        depth += line.matches('{').count() as i32;
        depth -= line.matches('}').count() as i32;
        if line.contains('{') {
            started = true;
        }
        if started && depth == 0 {
            return lines[start..start + offset + 1].join("\n");
        }
    }
    panic!("`{name}` 的函数体没有闭合");
}

/// 从源码里取出 `new_async` 的函数体（按大括号配平，不认识字符串里的花括号——
/// 这个文件的函数体里没有裸花括号字面量，且配平失败会让下面的断言直接报错）。
fn new_async_body_raw(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.contains("async fn new_async"))
        .expect("`construct.rs` 里必须还有 `async fn new_async`（被搬走或改名了？）");
    let mut depth = 0_i32;
    let mut started = false;
    for (offset, line) in lines[start..].iter().enumerate() {
        depth += line.matches('{').count() as i32;
        depth -= line.matches('}').count() as i32;
        if line.contains('{') {
            started = true;
        }
        if started && depth == 0 {
            return lines[start..start + offset + 1].join("\n");
        }
    }
    panic!("`new_async` 的函数体没有闭合——大括号配平失败");
}

/// `new_async` 的函数体，**子模块调用已就地展开**。
///
/// 两个测试都用这个而不是 `_raw`：批次拆分后创建会分散到 `construct/*.rs`，
/// 只看 `mod.rs` 会把搬走的创建漏掉（批次 2 实测少 6 个 `create_sampler`）。
fn new_async_body(source: &str) -> String {
    expand_submodule_calls(&new_async_body_raw(source))
}

/// 去掉行注释与块注释（换行符保留，后面还要按行切分）。
///
/// 不追求完整的 Rust 词法分析——这个文件里的注释只有 `//` 与 `/* */` 两种，
/// 没有嵌套块注释、没有 `r#"..."#` 里藏着 `//` 的字符串。够用且好读。
fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let bytes: Vec<char> = source.chars().collect();
    let mut index = 0;
    // 字符串字面量里也可能出现 `//`（例如 URL），这里只处理最基本的 `"` 配对。
    let mut in_string = false;
    while index < bytes.len() {
        let rest: String = bytes[index..].iter().take(2).collect();
        if !in_string && rest == "//" {
            // 跳到行尾，换行符保留（下面按行切分要用）。
            while index < bytes.len() && bytes[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if !in_string && rest == "/*" {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == '*' && bytes[index + 1] == '/') {
                if bytes[index] == '\n' {
                    out.push('\n');
                }
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            continue;
        }
        if bytes[index] == '"' {
            in_string = !in_string;
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

/// 一次资源创建：`device.create_xxx(...)`，按出现顺序记录。
///
/// **只看代码，不看注释**。这一点很要紧：拆分 `new_async` 时最常见的中间态是
/// "把旧代码注释掉、新代码写在别处"，如果注释里的 `device.create_*` 也算数，
/// 门就会把"已经搬走的调用"当成"还在"——正是它该抓的那种漏搬。
/// （2026-11-18 实测：用块注释包住一处 `create_texture`，未剥注释的版本照样绿。）
fn creation_sequence(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in strip_comments(body).lines() {
        let trimmed = line.trim();
        // 只认 `device.create_*`——`Self::`、`super::` 种的辅助函数不在这里算，
        // 它们内部创建什么由各自的模块负责（拆 `new_async` 也不该改那些）。
        let Some(rest) = trimmed.split("device.create_").nth(1) else {
            continue;
        };
        // 取到第一个 `(` 为止：`create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {`
        // 只该得到 `bind_group_layout`。早一版按"字母数字和下划线"取，会把
        // `(&wgpu::…` 里的 `wgpuBindGroupLayoutDescriptor` 一起吞进去（`&`/`:` 被跳过，
        // 后续字母连着算），于是每个名字都变长、清单永远对不上。
        let kind = rest.split('(').next().unwrap_or("").trim();
        if kind.is_empty() {
            continue;
        }
        out.push(format!("create_{kind}"));
    }
    out
}

/// 登记的资源创建序列（`new_async` 当前的样子，2026-11-18 实测）。
///
/// 拆分 `new_async` 时**这一串必须一字不变**（顺序、类型、数量）。
/// 如果拆分过程中需要调换两行的顺序（例如把采样器提前），那说明拆分改变了语义，
/// 应当停下来重新设计，而不是改这份清单。
const EXPECTED: &[&str] = &[
    "create_bind_group_layout",
    "create_pipeline_layout",
    "create_bind_group_layout",
    "create_shader_module",
    "create_sampler",
    "create_sampler",
    "create_sampler",
    "create_bind_group_layout",
    "create_pipeline_layout",
    "create_shader_module",
    "create_buffer",
    "create_buffer",
    "create_buffer",
    "create_buffer",
    "create_bind_group_layout",
    "create_sampler",
    "create_sampler",
    "create_buffer_init",
    "create_buffer_init",
    "create_buffer_init",
    "create_buffer_init",
    "create_buffer_init",
    "create_buffer_init",
    "create_buffer_init",
    "create_buffer_init",
    "create_buffer",
    "create_buffer",
    "create_sampler",
    "create_texture",
    "create_texture",
    "create_buffer_init",
    "create_buffer_init",
    "create_texture",
    "create_command_encoder",
    "create_bind_group_layout",
    "create_buffer_init",
    "create_bind_group",
    "create_shader_module",
    "create_pipeline_layout",
    "create_render_pipeline",
    "create_bind_group",
    "create_buffer",
    "create_bind_group",
    "create_texture",
    "create_bind_group_layout",
    "create_bind_group",
    "create_bind_group",
    "create_buffer_init",
    "create_pipeline_layout",
    "create_pipeline_layout",
    "create_pipeline_layout",
    "create_query_set",
    "create_bind_group_layout",
    "create_buffer_init",
    "create_bind_group",
    "create_shader_module",
    "create_pipeline_layout",
    "create_buffer",
    "create_bind_group_layout",
    "create_bind_group_layout",
    "create_pipeline_layout",
    "create_shader_module",
    "create_bind_group_layout",
    "create_pipeline_layout",
    "create_sampler",
    "create_buffer",
    "create_bind_group_layout",
    "create_pipeline_layout",
    "create_shader_module",
    "create_buffer",
    "create_texture",
    "create_bind_group_layout",
    "create_pipeline_layout",
    "create_shader_module",
    "create_buffer",
    "create_texture",
    "create_texture",
    "create_bind_group_layout",
    "create_pipeline_layout",
    "create_shader_module",
    "create_buffer",
    "create_bind_group_layout",
    "create_pipeline_layout",
    "create_shader_module",
    "create_buffer",
    "create_texture",
    "create_texture",
    "create_texture",
    "create_bind_group_layout",
    "create_pipeline_layout",
    "create_shader_module",
    "create_buffer",
];

/// 创建序列与登记一致（顺序 + 类型 + 数量）。
///
/// 失败信息给出**第一个**不一致的位置和双方的上下文——拆分时定位到"第几步开始对不上"
/// 比只报总数有用得多。
#[test]
fn new_async_creates_resources_in_the_registered_order() {
    let body = new_async_body(&construct_source());
    let actual = creation_sequence(&body);

    let mismatch = actual
        .iter()
        .zip(EXPECTED.iter())
        .position(|(got, want)| got != want);

    if let Some(index) = mismatch {
        let context = |list: &[String], at: usize| {
            let from = at.saturating_sub(2);
            let to = (at + 3).min(list.len());
            list[from..to].join(" → ")
        };
        panic!(
            "`new_async` 的资源创建序列在第 {index} 项开始不一致：\n\
             实际：{}\n\
             登记：{}\n\
             若这是有意的改动，更新本文件的 `EXPECTED` 并说明原因；\
             若是拆分 `new_async` 引入的，说明拆分改变了创建顺序",
            context(&actual, index),
            context(
                &EXPECTED.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
                index
            )
        );
    }
    assert_eq!(
        actual.len(),
        EXPECTED.len(),
        "创建数量对不上（多出或缺少资源）：实际 {} 个、登记 {} 个",
        actual.len(),
        EXPECTED.len()
    );
}

/// 每条管线都有对应的 layout，且**layout 先于 pipeline**创建。
///
/// 这条守的是顺序里最要紧的一种：`create_render_pipeline` 需要 `&PipelineLayout`，
/// Rust 的借用检查通常能拦住"先用后建"，但拆分之后如果 layout 被挪进另一个函数、
/// 由返回值带出来，借用检查就拦不住了（`new_async` 里已经能看到
/// `esm_pipeline_layout` / `mesh_pipeline_layout` / `pbr_pipeline_layout` 这种
/// "先建变量、后面才用"的形态）。
#[test]
fn every_pipeline_layout_precedes_its_pipeline() {
    let body = new_async_body(&construct_source());
    let lines: Vec<&str> = body.lines().collect();

    let layouts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("device.create_pipeline_layout("))
        .map(|(index, _)| index)
        .collect();
    let pipelines: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("device.create_render_pipeline("))
        .map(|(index, _)| index)
        .collect();

    assert!(
        !layouts.is_empty(),
        "`new_async` 里一条 `create_pipeline_layout` 都没有——扫描规则失效了？"
    );
    assert!(
        !pipelines.is_empty(),
        "`new_async` 里一条 `create_render_pipeline` 都没有——扫描规则失效了？"
    );
    // 每条管线都必须有一个**在它之前**建的 layout。
    for pipeline in &pipelines {
        let preceding = layouts.iter().filter(|layout| *layout < pipeline).count();
        assert!(
            preceding >= 1,
            "第 {pipeline} 行的管线之前没有任何 `create_pipeline_layout`——\
             拆分把 layout 搬到了管线之后"
        );
    }
}
