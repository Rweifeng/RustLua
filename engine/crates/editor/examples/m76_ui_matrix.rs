//! M76-①：PC 宿主的 UI 证据**生产端**（docs/75 §4.4 的编辑器侧导出 → 真宿主读数）。
//!
//! 为什么需要它：`ui/hosts.json` 三宿主证据门在落地时只有消费端，没有任何生产端，
//! 所以每份文档永远是 `untested`——门是绿的，但它绿在"没人claim"上。这条 example 把
//! 一份 `.ui.json` 经 [`editor::ui_scene_export`] 落成 `.scene`，交给**真实运行时**
//! （`RuntimeScene`）解算并提交几何，再把宿主自己算出的矩形与绘制先后写成 JSON 产物。
//! 产物带文档名、宿主键、revision 与检查项，`ui_asset_gate` 才会把 `pc` 从 `untested`
//! 翻成 `tested`；文档一改，revision 变了，产物立刻过期（这是整套设计的意义）。
//!
//! 两项检查（都是宿主实测，不拿预览器的数字自证）：
//! 1. `layout_rects`：`RuntimeScene::ui_rects_by_name` 的本帧矩形 vs 预览器
//!    `rustlua_ui_spec::preview::layout_rects` 逐键对齐，最大偏差进产物；
//! 2. `draw_order`：`RuntimeScene::vertices` 提交序列里，每个带 `Image` 的节点其四边形
//!    顶点的**首次出现下标**单调递增，且等于场景实体前序（= 文档树前序 = 画家顺序）。
//!
//! 只测 `pc`：web/微信宿主的同类读数在第二批（要各自跑无头 harness），产物里也不写它们。
//! 文本固有尺寸（宿主量出来的字宽 vs 预览的 120×32 默认）不参与断言，只作为读数写进产物。

use std::{env, fs, path::PathBuf, process};

use editor::ui_scene_export::export_ui_scene;
use editor::ui_spec_host::UiSpecStore;
use engine::host::{OverlayVertex, RuntimeScene, SceneMesh};
use rustlua_ui_spec::preview::layout_rects;
use serde_json::{Value, json};

const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 720;
/// 逐键矩形对齐容差：预览与宿主都算同一套锚点数学，差异只来自浮点舍入。
const RECT_TOLERANCE: f32 = 0.51;
/// NDC 位置匹配容差（顶点是 `f32`，四边形角点按 `px / viewport * 2 - 1` 反算）。
/// 取一像素的一半：比任何一次版面漂移都小，又大到不会因浮点末位漏匹配。
const CORNER_TOLERANCE: f32 = 0.5 / VIEW_W as f32 * 2.0;

fn main() {
    let args = Args::parse();
    let store = UiSpecStore::new(args.project.clone());
    let document = store
        .load(&args.document)
        .unwrap_or_else(|error| fail(&format!("读文档 {} 失败：{error:?}", args.document)));
    let scene = export_ui_scene(&document.spec, "ui_matrix")
        .unwrap_or_else(|errors| fail(&format!("导出失败：{}", errors.join(" / "))));

    let mut runtime = RuntimeScene::from_scene_edit(Some(store.root()), scene.clone())
        .unwrap_or_else(|error| fail(&format!("宿主构建场景失败：{error}")));
    let rects = runtime.ui_rects_by_name([VIEW_W as f32, VIEW_H as f32]);
    let mesh = runtime
        .vertices(VIEW_W, VIEW_H)
        .unwrap_or_else(|error| fail(&format!("宿主提交几何失败：{error}")));

    let layout = check_layout_rects(&document.spec, &rects);
    let draw = check_draw_order(&mesh, &scene, &rects);
    let report = json!({
        "report_version": "m76-ui-host-evidence-v1",
        "document": args.document,
        "host": "pc",
        "revision": document.revision,
        "checks": ["layout_rects", "draw_order"],
        "canvas": [VIEW_W, VIEW_H],
        "command": args.command_label(),
        "entities": scene.entities.len(),
        "vertices": mesh.vertices.len(),
        // 字形顶点单列：`sdf=1` 才走 SDF 着色。无头宿主没装字体图集时这里是 0，
        // 产物必须把它摊开，不能让"只有图块提交"看起来像"整份版面都画了"。
        "sdf_vertices": mesh.vertices.iter().filter(|v| v.sdf == 1).count(),
        "segments": mesh.draws.len(),
        "paths": mesh.paths,
        "layout_rects": layout,
        "draw_order": draw,
    });
    let problems = collect_problems(&report);
    let text = serde_json::to_string_pretty(&report).expect("序列化产物");
    println!(
        "m76_ui_matrix document={} revision={} entities={} layout_max_delta={:.4} draw_rank_max_error={:.4}",
        args.document,
        document.revision,
        scene.entities.len(),
        report["layout_rects"]["max_abs_delta"]
            .as_f64()
            .unwrap_or_default(),
        report["draw_order"]["max_vertex_error"]
            .as_f64()
            .unwrap_or_default(),
    );
    for problem in &problems {
        println!("  ✗ {problem}");
    }
    // 红了就不落盘：证据只能由绿的一次运行产生。写一份"结构合法、内容判红"的产物，
    // 消费端只校 document/host/revision/checks 四项，会把它当成已测——那是假绿。
    if !problems.is_empty() {
        if let Some(path) = &args.output {
            println!("不写产物（{} 项红）：{}", problems.len(), path.display());
        }
        fail(&format!("PC 宿主证据不成立（{} 项）", problems.len()));
    }
    match &args.output {
        Some(path) => {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(path, &text).unwrap_or_else(|error| fail(&format!("写产物失败：{error}")));
            println!("产物：{}", path.display());
        }
        None => println!("{text}"),
    }
    println!(
        "m76_ui_matrix_ok checks=2 document={} revision={}",
        args.document, document.revision
    );
}

struct Args {
    project: PathBuf,
    document: String,
    output: Option<PathBuf>,
}

impl Args {
    fn parse() -> Self {
        let mut project = default_project();
        let mut document = String::from("ui/matrix.ui.json");
        let mut output = None;
        let mut values = env::args().skip(1);
        while let Some(value) = values.next() {
            let mut operand = |flag: &str| {
                values
                    .next()
                    .unwrap_or_else(|| fail(&format!("{flag} 缺少取值")))
            };
            match value.as_str() {
                "--project" => project = PathBuf::from(operand("--project")),
                "--document" => document = operand("--document"),
                // `--output -` 或 `--print-only`：只打印不落盘（反面对照要看产物、又不能污染真证据）。
                "--output" => {
                    let path = operand("--output");
                    output = if path == "-" {
                        None
                    } else {
                        Some(PathBuf::from(path))
                    };
                }
                "--print-only" => output = None,
                other => fail(&format!("未知参数 {other:?}")),
            }
        }
        Self {
            project,
            document,
            output,
        }
    }

    /// 产物里回写「这条证据是怎么跑出来的」——清单的 `command` 必须与它一致才可复现。
    fn command_label(&self) -> String {
        format!(
            "cargo run -q -p editor --example m76_ui_matrix -- --project {{project}} --document {}",
            self.document
        )
    }
}

fn default_project() -> PathBuf {
    rustlua_infra::games_dir().join("examples").join("game_api")
}

/// ① 版面：宿主本帧真正用于绘制的矩形，逐键对齐预览器。
fn check_layout_rects(
    spec: &rustlua_ui_spec::UiSpec,
    rects: &std::collections::BTreeMap<String, rustlua_ui::ui_layout::UiRect>,
) -> Value {
    let mut rows = Vec::new();
    let mut max_delta = 0.0_f32;
    let mut missing = Vec::new();
    for item in layout_rects(spec) {
        let Some(rect) = rects.get(&item.key) else {
            // 预览器算了这块版面、宿主却没有它 ⇒ 这个节点在宿主侧根本不存在（多半是没落成组件）。
            missing.push(item.key.clone());
            continue;
        };
        let delta = [
            (rect.x - item.rect[0]).abs(),
            (rect.y - item.rect[1]).abs(),
            (rect.w - item.rect[2]).abs(),
            (rect.h - item.rect[3]).abs(),
        ]
        .into_iter()
        .fold(0.0_f32, f32::max);
        max_delta = max_delta.max(delta);
        rows.push(json!({
            "key": item.key,
            "host": [rect.x, rect.y, rect.w, rect.h],
            "preview": item.rect,
            "abs_delta": delta,
        }));
    }
    json!({
        "compared": rows.len(),
        "max_abs_delta": max_delta,
        "tolerance": RECT_TOLERANCE,
        "missing_in_host": missing,
        "rows": rows,
    })
}

/// ② 层序：每个带 `Image` 的节点，它那块四边形在提交顶点流里的首次下标。
///
/// 下标是宿主自己排的（前序遍历提交），所以「首下标单调递增」就是把"文档顺序 = 画家顺序"
/// 量成了数字；`expected_first` 由矩形反算 NDC 得到，能同时钉住位置没漂。
fn check_draw_order(
    mesh: &SceneMesh,
    scene: &rustlua_scene::scene::Scene,
    rects: &std::collections::BTreeMap<String, rustlua_ui::ui_layout::UiRect>,
) -> Value {
    let mut rows = Vec::new();
    let mut ranks = Vec::new();
    let mut max_vertex_error = 0.0_f32;
    for entity in scene.entities.values() {
        if !entity.components.contains_key("Image") {
            continue;
        }
        let Some(rect) = rects.get(&entity.name) else {
            rows.push(json!({"key": entity.name, "first_vertex": null, "reason": "无宿主矩形"}));
            continue;
        };
        let expected = [
            rect.x / VIEW_W as f32 * 2.0 - 1.0,
            1.0 - rect.y / VIEW_H as f32 * 2.0,
        ];
        let Some((index, actual)) = find_corner(&mesh.vertices, expected) else {
            rows.push(json!({
                "key": entity.name,
                "first_vertex": null,
                "expected_ndc": expected,
                "reason": "提交顶点里找不到这块角点",
            }));
            continue;
        };
        let error = [
            (actual[0] - expected[0]).abs(),
            (actual[1] - expected[1]).abs(),
        ]
        .into_iter()
        .fold(0.0_f32, f32::max);
        max_vertex_error = max_vertex_error.max(error);
        ranks.push(index);
        rows.push(json!({
            "key": entity.name,
            "first_vertex": index,
            "expected_ndc": expected,
            "vertex_abs_error": error,
        }));
    }
    let monotonic = ranks.windows(2).all(|pair| pair[0] < pair[1]);
    json!({
        "blocks": rows.len(),
        "placed": ranks.len(),
        "monotonic_preorder": monotonic,
        "max_vertex_error": max_vertex_error,
        "tolerance": CORNER_TOLERANCE,
        "rows": rows,
    })
}

/// 在顶点流里找第一个位置落在期望角点邻域内的顶点。
fn find_corner(vertices: &[OverlayVertex], expected: [f32; 2]) -> Option<(usize, [f32; 2])> {
    vertices.iter().enumerate().find_map(|(index, vertex)| {
        let near = (vertex.position[0] - expected[0]).abs() < CORNER_TOLERANCE
            && (vertex.position[1] - expected[1]).abs() < CORNER_TOLERANCE;
        near.then_some((index, vertex.position))
    })
}

/// 把两处检查的红项汇总成退出码依据。产物先落盘再退出，坏产物也要能被门读到并判过期。
fn collect_problems(report: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let layout = &report["layout_rects"];
    if !layout["missing_in_host"]
        .as_array()
        .is_some_and(|items| items.is_empty())
    {
        problems.push(format!(
            "宿主缺少的版面节点：{}",
            layout["missing_in_host"]
                .as_array()
                .map(|items| items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(","))
                .unwrap_or_default()
        ));
    }
    let max_delta = layout["max_abs_delta"].as_f64().unwrap_or_default() as f32;
    // NaN 也算红：偏差读不出来就等于没测。
    if max_delta >= RECT_TOLERANCE || max_delta.is_nan() {
        problems.push(format!("版面最大偏差 {max_delta} ≥ {RECT_TOLERANCE}"));
    }
    if layout["compared"].as_u64().unwrap_or(0) == 0 {
        problems.push("版面逐项对照一行都没跑（等于没测）".to_owned());
    }
    let draw = &report["draw_order"];
    if draw["placed"].as_u64().unwrap_or(0) == 0 {
        problems.push("一个图块顶点都没定位到（层序无从谈起）".to_owned());
    } else if !draw["monotonic_preorder"].as_bool().unwrap_or(false) {
        problems.push("提交下标不随文档前序单调递增".to_owned());
    }
    if draw["blocks"].as_u64().unwrap_or(0) != draw["placed"].as_u64().unwrap_or(0) {
        problems.push("有 `Image` 节点没在提交顶点里找到自己的块".to_owned());
    }
    problems
}

fn fail(message: &str) -> ! {
    // `process::exit` 不过析构函数：先把 stdout 冲干净，否则重定向到文件时最后几行读数会丢。
    let _ = std::io::Write::flush(&mut std::io::stdout());
    eprintln!("m76_ui_matrix_failed: {message}");
    process::exit(1)
}
