//! 三宿主 UI 证据的**唯一读数实现**（docs/75 §4.4）。
//!
//! 输入是一份 `.ui.json` 的**原始字节**与宿主名；输出是宿主自己算出来的读数：
//! 本帧用于绘制的矩形（`RuntimeScene::ui_rects_by_name`）、提交顶点流里的块序、
//! 以及逐键对照预览器 / 文档前序的结论。revision 由字节现算（[`revision_hash`]），
//! 不信任调用方传进来的数字——产物"过期"因此是内容变了，而不是清单忘了改。
//!
//! 为什么读数只能有一份：pc 的 `m76_ui_matrix` 例子与 wasm harness（Web/微信）都调
//! 这里。各写一份就会出现"同一份文档在三个宿主上由三套判定说绿"，那正是证据要防的事。
//!
//! 场景实例化刻意走 `from_scene_edit`（不跑脚本），与编辑器"运行当前场景"同一条路：
//! 证据测的是文档本身，而不是工程脚本这一帧碰巧做了什么。

use std::path::Path;

use engine::host::{OverlayVertex, RuntimeScene, SceneMesh};
use rustlua_ui::ui_layout::UiRect;
use rustlua_ui_scene_export::{export_ui_scene, preview_unreconciled_keys};
use rustlua_ui_spec::preview::layout_rects;
use rustlua_ui_spec::revision_hash;
use serde_json::{Value, json};

/// 证据画布（与 game_api 的设计分辨率一致）。
pub const CANVAS: [u32; 2] = [1280, 720];
/// 逐键矩形对齐容差：预览与宿主都算同一套锚点数学，差异只来自浮点舍入。
pub const RECT_TOLERANCE: f32 = 0.51;
/// 证据产物版本；消费端（`review`）只认 `report_version` / document / host / revision / checks。
pub const REPORT_VERSION: &str = "m76-ui-host-evidence-v1";

pub struct HostRequest<'a> {
    /// 工程相对文档名（`ui/matrix.ui.json`），原样写进产物。
    pub document: &'a str,
    /// `pc` / `web` / `wechat`：只允许证据清单认识的三个键。
    pub host: &'a str,
    /// 可复现的命令行；与 `ui/hosts.json` 的 `command` 一致。
    pub command: &'a str,
    /// 写进场景文档的名字（导出件的 `Scene::name`）。
    pub scene_name: &'a str,
    /// 场景实例化的工程根：桌面给真实工程根，wasm 宿主给虚拟 `/project`
    /// （与 `game_web` 同一口径）。它只影响字体/资源的可达性，不改版面数学。
    pub project_root: Option<&'a Path>,
}

pub struct HostReport {
    /// 内容哈希（=`ui/hosts.json` 期望的 revision）。
    pub revision: u64,
    /// 完整产物（调用方决定落盘与否）。
    pub report: Value,
    /// 红项；非空时**不得**落盘（证据只能来自绿的一次运行）。
    pub problems: Vec<String>,
}

/// 跑一次宿主证据。任何一步失败都返回 `Err`（消息直接进 stderr / 宿主日志）。
pub fn run_evidence(spec_bytes: &[u8], request: &HostRequest<'_>) -> Result<HostReport, String> {
    let spec: rustlua_ui_spec::UiSpec = serde_json::from_slice(spec_bytes)
        .map_err(|error| format!("解析 {} 失败：{error}", request.document))?;
    let scene = export_ui_scene(&spec, request.scene_name)
        .map_err(|errors| format!("导出失败：{}", errors.join(" / ")))?;
    let mut runtime = RuntimeScene::from_scene_edit(request.project_root, scene.clone())
        .map_err(|error| format!("宿主构建场景失败：{error}"))?;
    let [width, height] = CANVAS;
    let rects = runtime.ui_rects_by_name([width as f32, height as f32]);
    let mesh = runtime
        .vertices(width, height)
        .map_err(|error| format!("宿主提交几何失败：{error}"))?;

    let layout = check_layout_rects(&spec, &rects);
    let draw = check_draw_order(&mesh, &scene, &rects);
    let revision = revision_hash(spec_bytes);
    let report = json!({
        "report_version": REPORT_VERSION,
        "document": request.document,
        "host": request.host,
        "revision": revision,
        "checks": ["layout_rects", "draw_order"],
        "canvas": [width, height],
        "command": request.command,
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
    Ok(HostReport {
        revision,
        report,
        problems,
    })
}

/// ① 版面：宿主本帧真正用于绘制的矩形，逐键对齐预览器。
///
/// **预览器口径还没对上的键不参与对账**（[`preview_unreconciled_keys`]）：流向容器的
/// 子项及其子树、`Scrollbar` 的 `handle` 子项。对这些键只断言运行时解出**正尺寸**——
/// "塌成 0×0"才是这座桥真会犯的错；宿主矩形照记进 `unreconciled_rows`，预览补齐后
/// 逐键纳回。
fn check_layout_rects(
    spec: &rustlua_ui_spec::UiSpec,
    rects: &std::collections::BTreeMap<String, UiRect>,
) -> Value {
    let unreconciled = preview_unreconciled_keys(spec);
    let mut rows = Vec::new();
    let mut unreconciled_rows = Vec::new();
    let mut collapsed = Vec::new();
    let mut max_delta = 0.0_f32;
    let mut missing = Vec::new();
    for item in layout_rects(spec) {
        let Some(rect) = rects.get(&item.key) else {
            // 预览器算了这块版面、宿主却没有它 ⇒ 这个节点在宿主侧根本不存在。
            missing.push(item.key.clone());
            continue;
        };
        let host = [rect.x, rect.y, rect.w, rect.h];
        if unreconciled.contains(&item.key) {
            if rect.w <= 0.0 || rect.h <= 0.0 {
                collapsed.push(item.key.clone());
            }
            unreconciled_rows.push(json!({ "key": item.key, "host": host }));
            continue;
        }
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
            "host": host,
            "preview": item.rect,
            "abs_delta": delta,
        }));
    }
    json!({
        "compared": rows.len(),
        "max_abs_delta": max_delta,
        "tolerance": RECT_TOLERANCE,
        "missing_in_host": missing,
        "collapsed_in_host": collapsed,
        "unreconciled_rows_count": unreconciled_rows.len(),
        "rows": rows,
        "unreconciled_rows": unreconciled_rows,
    })
}

/// ② 层序：每个带 `Image` 的节点，它那块四边形在提交顶点流里的首次下标。
///
/// 下标是宿主自己排的（前序遍历提交），所以「首下标单调递增」就是把"文档顺序 = 画家顺序"
/// 量成了数字；`expected_ndc` 由矩形反算，能同时钉住位置没漂。
fn check_draw_order(
    mesh: &SceneMesh,
    scene: &rustlua_scene::scene::Scene,
    rects: &std::collections::BTreeMap<String, UiRect>,
) -> Value {
    let tolerance = corner_tolerance();
    let [width, height] = CANVAS;
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
            rect.x / width as f32 * 2.0 - 1.0,
            1.0 - rect.y / height as f32 * 2.0,
        ];
        let Some((index, actual)) = find_corner(&mesh.vertices, expected, tolerance) else {
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
        "tolerance": tolerance,
        "rows": rows,
    })
}

/// NDC 位置匹配容差：取半像素（顶点是 `f32`，角点按 `px / viewport * 2 - 1` 反算）。
fn corner_tolerance() -> f32 {
    0.5 / CANVAS[0] as f32 * 2.0
}

/// 在顶点流里找第一个位置落在期望角点邻域内的顶点。
fn find_corner(
    vertices: &[OverlayVertex],
    expected: [f32; 2],
    tolerance: f32,
) -> Option<(usize, [f32; 2])> {
    vertices.iter().enumerate().find_map(|(index, vertex)| {
        let near = (vertex.position[0] - expected[0]).abs() < tolerance
            && (vertex.position[1] - expected[1]).abs() < tolerance;
        near.then_some((index, vertex.position))
    })
}

/// 把两处检查的红项汇总成退出码依据。产物先落盘再退出，坏产物也要能被门读到并判过期。
#[must_use]
pub fn collect_problems(report: &Value) -> Vec<String> {
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
    if !layout["collapsed_in_host"]
        .as_array()
        .is_some_and(|items| items.is_empty())
    {
        problems.push(format!(
            "不对账的子项在宿主塌成 0×0：{}",
            layout["collapsed_in_host"]
                .as_array()
                .map(|items| items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(","))
                .unwrap_or_default()
        ));
    }
    let draw = &report["draw_order"];
    if draw["placed"].as_u64().unwrap_or(0) == 0 {
        problems.push("一个图块顶点都没定位到（层序无从谈起）".to_owned());
    } else {
        if !draw["monotonic_preorder"].as_bool().unwrap_or(false) {
            problems.push("提交下标不随文档前序单调递增".to_owned());
        }
        if draw["blocks"].as_u64().unwrap_or(0) != draw["placed"].as_u64().unwrap_or(0) {
            problems.push("有 `Image` 节点没在提交顶点里找到自己的块".to_owned());
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一份形状正确、两项检查全绿的产物。逐条反面对照都从它出发只改一处，
    /// 免得"红"来自测试自己搭的坏输入而不是被测判据。
    fn green() -> Value {
        json!({
            "layout_rects": {
                "compared": 3,
                "max_abs_delta": 0.0,
                "tolerance": RECT_TOLERANCE,
                "missing_in_host": [],
                "collapsed_in_host": [],
                "rows": [],
                "unreconciled_rows": [],
            },
            "draw_order": {
                "blocks": 2,
                "placed": 2,
                "monotonic_preorder": true,
                "max_vertex_error": 0.0006,
            },
        })
    }

    fn problems_after(mutate: impl FnOnce(&mut Value)) -> Vec<String> {
        let mut report = green();
        mutate(&mut report);
        collect_problems(&report)
    }

    #[test]
    fn green_report_is_green() {
        assert_eq!(collect_problems(&green()), Vec::<String>::new());
    }

    #[test]
    fn a_node_the_host_did_not_build_is_red() {
        let problems = problems_after(|report| {
            report["layout_rects"]["missing_in_host"] = json!(["button_1"]);
        });
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("button_1"), "{problems:?}");
    }

    /// 偏差恰好等于容差也算红：门是"小于容差"，不是"不超过"。
    #[test]
    fn layout_delta_at_tolerance_is_red() {
        let problems = problems_after(|report| {
            report["layout_rects"]["max_abs_delta"] = json!(RECT_TOLERANCE);
        });
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn comparing_zero_keys_is_red() {
        let problems = problems_after(|report| {
            report["layout_rects"]["compared"] = json!(0);
        });
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("一行都没跑"), "{problems:?}");
    }

    #[test]
    fn an_unreconciled_child_collapsing_to_zero_is_red() {
        let problems = problems_after(|report| {
            report["layout_rects"]["collapsed_in_host"] = json!(["v_1"]);
        });
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("v_1"), "{problems:?}");
    }

    #[test]
    fn non_monotonic_submission_is_red() {
        let problems = problems_after(|report| {
            report["draw_order"]["monotonic_preorder"] = json!(false);
        });
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("单调"), "{problems:?}");
    }

    #[test]
    fn an_image_block_without_vertices_is_red() {
        let problems = problems_after(|report| {
            report["draw_order"]["blocks"] = json!(3);
        });
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("没在提交顶点"), "{problems:?}");
    }

    /// `placed == 0` 只报"层序无从谈起"，不再叠一条"块数不匹配"——
    /// 一个根因一条红，读数才指得动。
    #[test]
    fn nothing_placed_reports_one_root_cause() {
        let problems = problems_after(|report| {
            report["draw_order"]["placed"] = json!(0);
            report["draw_order"]["blocks"] = json!(3);
        });
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains("一个图块顶点都没定位到"),
            "{problems:?}"
        );
    }
}
