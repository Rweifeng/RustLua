//! M19-03 运行时调试面板：Play/预览运行期间以**只读**探针读取游戏状态。
//!
//! 数据来源只有一处 —— 预览会话里那份真实的 `LuaRuntime`（Play 时由
//! `RuntimeScene::from_scene` 加载 `main.lua` 建立），探针表达式逐个经
//! `LuaRuntime::eval_json`（M19-03 新增的 Lua↔JSON 桥）转成 JSON 展示。
//!
//! 硬约束：
//! - 面板**只读**：所有探针只调用 `map_manager.current`/`player`/`npc.capture`/
//!   `quest.capture`/`farm.capture`/`building.list`/`crafting.capture`/
//!   `save_manager.last_summary` 这类读取入口，不写游戏状态（提交操作留给游戏输入）。
//! - 未运行时显示「无运行态」占位；读取失败只显示错误文本，**不 panic**。
//! - 每个探针在 Lua 侧自带 `pcall`，任何模块缺失/运行期错误都被收敛成结构化
//!   结果，不会中断其它探针的读取。

use crate::EditorApp;
use eframe::egui;
use engine::core::script::LuaRuntime;
use serde_json::Value;

/// 一条只读探针：显示标签 + Lua 表达式（表达式必须返回 `{ value = ... }` 或
/// `{ error = ... }`）。
pub(crate) struct RuntimeProbe {
    pub label: &'static str,
    pub expression: &'static str,
}

/// 一次探针读取的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeProbeResult {
    pub label: String,
    /// `false` = 该探针读取失败（模块缺失/运行期错误），文本即错误说明。
    pub ok: bool,
    pub text: String,
}

/// 面板展示的探针清单（覆盖任务书要求的：地图、玩家、NPC、任务、农田地块、
/// 建筑与制作、存档摘要；另附时间/日历）。
pub(crate) fn runtime_probes() -> &'static [RuntimeProbe] {
    &[
        RuntimeProbe {
            label: "当前地图",
            expression: r#"(function()
  local ok, current, scene_name = pcall(function()
    return require("map_manager").current(), scene.reload()
  end)
  if not ok then return { error = tostring(current) } end
  return { value = { map = current, scene = scene_name } }
end)()"#,
        },
        RuntimeProbe {
            label: "玩家坐标",
            expression: r#"(function()
  local ok, value = pcall(function()
    local cell_x, cell_y = require("map_manager").player_cell()
    local id = require("player").ensure()
    local pos = nil
    if id then
      local ok_transform, t = pcall(transform.get, id)
      if ok_transform and type(t) == "table" then
        pos = { x = t.x, y = t.y }
      end
    end
    return { cell_x = cell_x, cell_y = cell_y, pos = pos }
  end)
  if not ok then return { error = tostring(value) } end
  return { value = value }
end)()"#,
        },
        RuntimeProbe {
            label: "NPC 列表与活动",
            expression: r#"(function()
  local ok, value = pcall(function()
    local npc = require("npc")
    return { capture = npc.capture(), diagnostics = npc.diagnostics() }
  end)
  if not ok then return { error = tostring(value) } end
  return { value = value }
end)()"#,
        },
        RuntimeProbe {
            label: "任务状态",
            expression: r#"(function()
  local ok, value = pcall(function()
    local quest = require("quest")
    return { capture = quest.capture(), active = quest.active() }
  end)
  if not ok then return { error = tostring(value) } end
  return { value = value }
end)()"#,
        },
        RuntimeProbe {
            label: "农田地块",
            expression: r#"(function()
  local ok, value = pcall(function()
    local farm = require("farm")
    return { stats = farm.stats(), capture = farm.capture() }
  end)
  if not ok then return { error = tostring(value) } end
  return { value = value }
end)()"#,
        },
        RuntimeProbe {
            label: "建筑与制作",
            expression: r#"(function()
  local ok, value = pcall(function()
    return {
      buildings = require("building").list(),
      crafting = require("crafting").capture()
    }
  end)
  if not ok then return { error = tostring(value) } end
  return { value = value }
end)()"#,
        },
        RuntimeProbe {
            label: "存档摘要",
            expression: r#"(function()
  local ok, value = pcall(function()
    return require("save_manager").last_summary()
  end)
  if not ok then return { error = tostring(value) } end
  return { value = value }
end)()"#,
        },
        RuntimeProbe {
            label: "时间与天气",
            expression: r#"(function()
  local ok, value = pcall(function()
    local now = clock.now()
    return {
      day = now.day,
      minute = now.minute,
      season = require("calendar").current(),
      weather = require("weather").current()
    }
  end)
  if not ok then return { error = tostring(value) } end
  return { value = value }
end)()"#,
        },
    ]
}

/// 在给定运行时上执行全部探针（纯逻辑，可在无 GPU 环境下测试）。
pub(crate) fn run_runtime_probes(runtime: &LuaRuntime) -> Vec<RuntimeProbeResult> {
    runtime_probes()
        .iter()
        .map(|probe| {
            let (ok, text) = match runtime.eval_json(probe.expression) {
                Ok(value) => interpret_probe_value(&value),
                Err(error) => (false, format!("读取失败：{error}")),
            };
            RuntimeProbeResult {
                label: probe.label.to_owned(),
                ok,
                text,
            }
        })
        .collect()
}

/// 把 `{ value = ... }` / `{ error = ... }` 探针返回值转成展示文本。
pub(crate) fn interpret_probe_value(value: &Value) -> (bool, String) {
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return (false, format!("读取失败：{error}"));
    }
    match value.get("value") {
        // Lua `nil` 无法进入表：`{ value = nil }` 序列化为 `{}`。
        None | Some(Value::Null) => (true, "nil（当前无值）".to_owned()),
        Some(inner) => (true, render_json(inner)),
    }
}

/// 渲染 JSON 值：优先紧凑单行（短值），较长时格式化并按上限截断。
fn render_json(value: &Value) -> String {
    const MAX_CHARS: usize = 6000;
    let compact = serde_json::to_string(value).unwrap_or_else(|_| "（序列化失败）".to_owned());
    let text = if compact.chars().count() <= 160 {
        compact
    } else {
        serde_json::to_string_pretty(value).unwrap_or(compact)
    };
    if text.chars().count() <= MAX_CHARS {
        return text;
    }
    let truncated: String = text.chars().take(MAX_CHARS).collect();
    format!("{truncated}\n……（输出过长，已截断）")
}

impl EditorApp {
    /// 运行时调试窗口（只读）。
    pub(crate) fn draw_runtime_debug_panel(&mut self, context: &egui::Context) {
        if !self.show_runtime_debug_panel {
            return;
        }
        // 首次打开且正在运行时自动读取一次，避免用户必须手点刷新。
        if self.runtime_probe_results.is_none() && self.preview.is_some() {
            self.refresh_runtime_probes();
        }
        let mut open = self.show_runtime_debug_panel;
        let mut refresh = false;
        let running = self.preview.is_some();
        egui::Window::new("运行时调试（只读）")
            .open(&mut open)
            .default_size([560.0, 420.0])
            .resizable(true)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(running, egui::Button::new("刷新"))
                        .on_hover_text("重新读取当前运行态（只读）")
                        .clicked()
                    {
                        refresh = true;
                    }
                    if running {
                        ui.label("运行中 · 只读探针");
                    } else {
                        ui.label("无运行态");
                    }
                });
                if !running {
                    ui.separator();
                    ui.label("无运行态：点击工具栏 ▶ 开始运行后再打开本面板。");
                    ui.label("本面板只读；读档/交易/制作等提交操作仍由游戏输入完成。");
                    if let Some(error) = &self.runtime_probe_error {
                        let weak = ui.visuals().weak_text_color();
                        ui.colored_label(weak, format!("上次读取：{error}"));
                    }
                    return;
                }
                if let Some(diagnostic) = self.preview.as_ref().and_then(|p| p.diagnostic()) {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 170, 60),
                        format!("预览诊断：{diagnostic}"),
                    );
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let Some(results) = self.runtime_probe_results.as_ref() else {
                            ui.label("尚未读取。点「刷新」读取运行态。");
                            return;
                        };
                        for result in results {
                            let color = if result.ok {
                                ui.visuals().text_color()
                            } else {
                                egui::Color32::from_rgb(235, 120, 110)
                            };
                            ui.label(egui::RichText::new(&result.label).strong());
                            ui.colored_label(color, egui::RichText::new(&result.text).monospace());
                            ui.add_space(4.0);
                        }
                    });
            });
        self.show_runtime_debug_panel = open;
        if refresh {
            self.refresh_runtime_probes();
        }
    }

    /// 读取一次运行态探针（未运行时清空结果并记录占位说明）。
    pub(crate) fn refresh_runtime_probes(&mut self) {
        let Some(preview) = self.preview.as_mut() else {
            self.runtime_probe_results = None;
            self.runtime_probe_error = Some("无运行态（未 Play）".to_owned());
            return;
        };
        let results = preview.with_script(|runtime| run_runtime_probes(runtime));
        self.runtime_probe_results = Some(results);
        self.runtime_probe_error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无 GPU：探针表达式本身必须是合法 Lua 且不抛错（每个探针自带 pcall），
    /// 未加载游戏模块时逐条给出「读取失败」文本而不是 panic。
    #[test]
    fn probes_fail_gracefully_without_runtime_state() {
        let runtime = LuaRuntime::new().expect("lua runtime");
        let results = run_runtime_probes(&runtime);
        assert_eq!(results.len(), runtime_probes().len());
        assert!(results.len() >= 7, "覆盖地图/玩家/NPC/任务/农田/建筑/存档");
        for result in &results {
            assert!(!result.label.is_empty());
            assert!(!result.text.is_empty());
            assert!(
                !result.ok,
                "空 VM 下 {} 不应读取成功：{}",
                result.label, result.text
            );
            assert!(
                result.text.contains("读取失败"),
                "{} 应显示错误文本：{}",
                result.label,
                result.text
            );
        }
        let labels: Vec<&str> = results.iter().map(|result| result.label.as_str()).collect();
        for expected in [
            "当前地图",
            "玩家坐标",
            "NPC 列表与活动",
            "任务状态",
            "农田地块",
            "建筑与制作",
            "存档摘要",
        ] {
            assert!(labels.contains(&expected), "缺少探针 {expected}");
        }
    }

    /// 探针内部用 `pcall` 收敛错误：即使模块存在但读取抛错，也返回结构化错误。
    #[test]
    fn probe_wrapper_reports_pcall_errors_as_text() {
        let runtime = LuaRuntime::new().expect("lua runtime");
        let value = runtime
            .eval_json(
                r#"(function()
  local ok, value = pcall(function() error("boom inside probe") end)
  if not ok then return { error = tostring(value) } end
  return { value = value }
end)()"#,
            )
            .expect("wrapper must not throw");
        let (ok, text) = interpret_probe_value(&value);
        assert!(!ok);
        assert!(text.contains("boom inside probe"));
    }

    #[test]
    fn interpret_probe_value_handles_nil_and_payload() {
        let (ok, text) = interpret_probe_value(&serde_json::json!({}));
        assert!(ok, "缺 value 键视为 nil 而不是失败");
        assert!(text.contains("nil"));
        let (ok, text) = interpret_probe_value(&serde_json::json!({ "value": { "map": "manor" } }));
        assert!(ok);
        assert!(text.contains("manor"));
        // 长输出走格式化并按上限截断。
        let long =
            serde_json::json!({ "value": (0..400).map(|i| i.to_string()).collect::<Vec<_>>() });
        let (ok, text) = interpret_probe_value(&long);
        assert!(ok);
        assert!(text.contains('\n'), "长输出应格式化显示");
    }
}
