//! M76-①：PC 宿主的 UI 证据**生产端**（docs/75 §4.4 的编辑器侧导出 → 真宿主读数）。
//!
//! 为什么需要它：`ui/hosts.json` 三宿主证据门在落地时只有消费端，没有任何生产端，
//! 所以每份文档永远是 `untested`——门是绿的，但它绿在"没人 claim"上。这条 example 把
//! 一份 `.ui.json` 交给 [`rustlua_ui_host_evidence`]（与 Web/微信 harness 共用的唯一读数
//! 实现）：导出 `.scene` → 交给**真实运行时**（`RuntimeScene`）解算并提交几何 → 把宿主
//! 自己算出的矩形与绘制先后写成 JSON 产物。产物带文档名、宿主键、revision 与检查项，
//! `ui_asset_gate` 才会把 `pc` 从 `untested` 翻成 `tested`；文档一改，revision 变了，
//! 产物立刻过期（这是整套设计的意义）。
//!
//! 两项检查（都是宿主实测，不拿预览器的数字自证）：
//! 1. `layout_rects`：`RuntimeScene::ui_rects_by_name` 的本帧矩形 vs 预览器
//!    `rustlua_ui_spec::preview::layout_rects` 逐键对齐，最大偏差进产物；
//! 2. `draw_order`：`RuntimeScene::vertices` 提交序列里，每个带 `Image` 的节点其四边形
//!    顶点的**首次出现下标**单调递增，且等于场景实体前序（= 文档树前序 = 画家顺序）。
//!
//! 预览器暂时不参与对账的键（流向子树、`Scrollbar` 的 `handle`）只断言宿主解出正尺寸，
//! 口径见 `rustlua_ui_scene_export::preview_unreconciled_keys` 与 docs/75 §14 第 4 条分歧。
//! Web/微信的同类产物由 `scripts/verify/ui-host-evidence.ps1` 生成，读数实现是同一份。
//!
//! 文本固有尺寸（宿主量出来的字宽 vs 预览的 120×32 默认）不参与断言，只作为读数写进产物。

use std::{env, fs, path::PathBuf, process};

use editor::ui_spec_host::UiSpecStore;
use rustlua_ui_host_evidence::{HostRequest, run_evidence};

fn main() {
    let args = Args::parse();
    let store = UiSpecStore::new(args.project.clone());
    let bytes = store
        .snapshot(&args.document)
        .unwrap_or_else(|error| fail(&format!("读文档 {} 失败：{error:?}", args.document)));
    let evidence = run_evidence(
        &bytes,
        &HostRequest {
            document: &args.document,
            host: "pc",
            command: &args.command_label(),
            scene_name: "ui_matrix",
            project_root: Some(store.root()),
        },
    )
    .unwrap_or_else(|error| fail(&error));
    let report = &evidence.report;
    println!(
        "m76_ui_matrix document={} revision={} entities={} layout_compared={} layout_unreconciled={} layout_max_delta={:.4} draw_rank_max_error={:.4}",
        args.document,
        evidence.revision,
        report["entities"].as_u64().unwrap_or_default(),
        report["layout_rects"]["compared"]
            .as_u64()
            .unwrap_or_default(),
        report["layout_rects"]["unreconciled_rows_count"]
            .as_u64()
            .unwrap_or_default(),
        report["layout_rects"]["max_abs_delta"]
            .as_f64()
            .unwrap_or_default(),
        report["draw_order"]["max_vertex_error"]
            .as_f64()
            .unwrap_or_default(),
    );
    for problem in &evidence.problems {
        println!("  ✗ {problem}");
    }
    // 红了就不落盘：证据只能由绿的一次运行产生。写一份"结构合法、内容判红"的产物，
    // 消费端只校 document/host/revision/checks 四项，会把它当成已测——那是假绿。
    if !evidence.problems.is_empty() {
        if let Some(path) = &args.output {
            println!(
                "不写产物（{} 项红）：{}",
                evidence.problems.len(),
                path.display()
            );
        }
        fail(&format!(
            "PC 宿主证据不成立（{} 项）",
            evidence.problems.len()
        ));
    }
    let text = serde_json::to_string_pretty(report).expect("序列化产物");
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
        args.document, evidence.revision
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

fn fail(message: &str) -> ! {
    // `process::exit` 不过析构函数：先把 stdout 冲干净，否则重定向到文件时最后几行读数会丢。
    let _ = std::io::Write::flush(&mut std::io::stdout());
    eprintln!("m76_ui_matrix_failed: {message}");
    process::exit(1)
}
