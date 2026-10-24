//! 确定性回放门：把 AI 的 tool 调用序列剥离 LLM 后在 CI 重放。
//!
//! fixture 只描述「调什么、期望成功还是失败、最终文档快照」；不依赖网络、时钟或随机数。
//! 任何一步结果或最终快照不一致都会让门判红，并打印第一个分歧。

use crate::bridge::{GatePolicy, ToolHost, execute_tool};
use crate::surface::tool_by_name;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayStep {
    pub tool: String,
    #[serde(default)]
    pub args: Value,
    /// 默认 true；错误路径的 fixture 写 false。
    #[serde(default = "default_true")]
    pub expect_ok: bool,
    /// 期望错误消息包含的子串（仅 expect_ok=false 时有意义）。
    #[serde(default)]
    pub expect_error_contains: Option<String>,
    /// 期望成功结果包含的 JSON 子集（对象递归、数组逐项、标量相等）。
    /// 修改类 fixture 用它锁住期望 diff，而不必整份结果逐字节相等。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_json: Option<Value>,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayFixture {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub steps: Vec<ReplayStep>,
    /// 回放结束时的 `ToolHost::snapshot()`；None = 不校验快照。
    #[serde(default)]
    pub expect_snapshot: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    UnknownTool {
        index: usize,
        tool: String,
    },
    UnexpectedSuccess {
        index: usize,
        tool: String,
        result: Value,
    },
    UnexpectedFailure {
        index: usize,
        tool: String,
        error: String,
    },
    ResultMismatch {
        index: usize,
        tool: String,
        expected: Value,
        actual: Value,
    },
    SnapshotMismatch {
        expected: Value,
        actual: Option<Value>,
    },
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTool { index, tool } => {
                write!(formatter, "第 {index} 步引用了未知 tool `{tool}`")
            }
            Self::UnexpectedSuccess {
                index,
                tool,
                result,
            } => {
                write!(
                    formatter,
                    "第 {index} 步 `{tool}` 期望失败但成功了：{result}"
                )
            }
            Self::UnexpectedFailure { index, tool, error } => {
                write!(formatter, "第 {index} 步 `{tool}` 失败：{error}")
            }
            Self::ResultMismatch {
                index,
                tool,
                expected,
                actual,
            } => write!(
                formatter,
                "第 {index} 步 `{tool}` 结果不含期望子集\n期望：{expected}\n实际：{actual}"
            ),
            Self::SnapshotMismatch { expected, actual } => write!(
                formatter,
                "最终文档快照不一致\n期望：{expected}\n实际：{}",
                actual
                    .as_ref()
                    .map_or_else(|| "<host 未提供 snapshot>".to_owned(), Value::to_string)
            ),
        }
    }
}

impl std::error::Error for ReplayError {}

#[derive(Clone, Debug, Serialize)]
pub struct ReplayReport {
    pub name: String,
    pub steps: usize,
    pub failures: Vec<String>,
}

impl ReplayReport {
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }
}

/// 回放固定用无头策略：T1 自动，T2/T3 拒绝。fixture 若需要 T2，必须显式走拒绝路径，
/// 避免回放门悄悄获得「自动改跨文件资源」的能力。
pub fn run_replay(
    fixture: &ReplayFixture,
    host: &mut dyn ToolHost,
) -> Result<ReplayReport, ReplayError> {
    let policy = GatePolicy::HEADLESS;
    for (index, step) in fixture.steps.iter().enumerate() {
        tool_by_name(&step.tool).ok_or_else(|| ReplayError::UnknownTool {
            index,
            tool: step.tool.clone(),
        })?;
        match execute_tool(host, policy, &step.tool, &step.args) {
            Ok(result) => {
                if !step.expect_ok {
                    return Err(ReplayError::UnexpectedSuccess {
                        index,
                        tool: step.tool.clone(),
                        result,
                    });
                }
                if let Some(expected) = &step.expect_json
                    && !json_contains(&result, expected)
                {
                    return Err(ReplayError::ResultMismatch {
                        index,
                        tool: step.tool.clone(),
                        expected: expected.clone(),
                        actual: result,
                    });
                }
            }
            Err(error) => {
                if step.expect_ok {
                    return Err(ReplayError::UnexpectedFailure {
                        index,
                        tool: step.tool.clone(),
                        error: error.message(),
                    });
                }
                if let Some(needle) = &step.expect_error_contains
                    && !error.message().contains(needle)
                {
                    return Err(ReplayError::UnexpectedFailure {
                        index,
                        tool: step.tool.clone(),
                        error: format!("错误消息不含期望子串 `{needle}`：{}", error.message()),
                    });
                }
            }
        }
    }
    if let Some(expected) = &fixture.expect_snapshot {
        let actual = host.snapshot();
        if actual.as_ref() != Some(expected) {
            return Err(ReplayError::SnapshotMismatch {
                expected: expected.clone(),
                actual,
            });
        }
    }
    Ok(ReplayReport {
        name: fixture.name.clone(),
        steps: fixture.steps.len(),
        failures: Vec::new(),
    })
}

/// 子集匹配：对象只要求期望键存在且递归匹配；数组要求等长逐项匹配；
/// 标量完全相等。`null` 也按标量处理，避免把“显式 null”当成通配。
fn json_contains(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => expected.iter().all(|(key, value)| {
            actual
                .get(key)
                .is_some_and(|found| json_contains(found, value))
        }),
        (Value::Array(actual), Value::Array(expected)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(found, want)| json_contains(found, want))
        }
        _ => actual == expected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::ToolError;
    use serde_json::json;

    #[derive(Default)]
    struct Host {
        value: i64,
        fail_next: bool,
    }

    impl ToolHost for Host {
        fn call_tool(&mut self, name: &str, _args: &Value) -> Result<Value, ToolError> {
            if self.fail_next {
                return Err(ToolError::Rejected("故意失败".to_owned()));
            }
            match name {
                "create_entity" => {
                    self.value += 1;
                    Ok(json!({"id": self.value}))
                }
                "undo" => {
                    self.value -= 1;
                    Ok(json!({"value": self.value}))
                }
                _ => Ok(json!({})),
            }
        }

        fn read_resource(&mut self, _uri: &str) -> Result<String, ToolError> {
            Ok(String::new())
        }

        fn snapshot(&mut self) -> Option<Value> {
            Some(json!({"value": self.value}))
        }
    }

    fn fixture(steps: Value, snapshot: Value) -> ReplayFixture {
        serde_json::from_value(json!({
            "name": "test",
            "steps": steps,
            "expect_snapshot": snapshot
        }))
        .unwrap()
    }

    #[test]
    fn replay_runs_and_checks_snapshot() {
        let fixture = fixture(
            json!([
                {"tool":"create_entity","args":{"name":"A"}},
                {"tool":"create_entity","args":{"name":"B"}},
                {"tool":"undo","args":{}}
            ]),
            json!({"value": 1}),
        );
        let mut host = Host::default();
        let report = run_replay(&fixture, &mut host).unwrap();
        assert!(report.ok());
    }

    #[test]
    fn replay_reports_first_snapshot_divergence() {
        let fixture = fixture(
            json!([{"tool":"create_entity","args":{"name":"A"}}]),
            json!({"value": 99}),
        );
        let mut host = Host::default();
        assert!(matches!(
            run_replay(&fixture, &mut host),
            Err(ReplayError::SnapshotMismatch { .. })
        ));
    }

    #[test]
    fn replay_honours_expected_failure() {
        let fixture = fixture(
            json!([{
                "tool": "delete_entity",
                "args": {"entities": [1]},
                "expect_ok": false,
                "expect_error_contains": "确认"
            }]),
            json!({"value": 0}),
        );
        let mut host = Host::default();
        assert!(run_replay(&fixture, &mut host).unwrap().ok());
    }

    #[test]
    fn replay_checks_expected_result_subset() {
        let good = fixture(
            json!([{
                "tool": "create_entity",
                "args": {"name": "A"},
                "expect_json": {"id": 1}
            }]),
            json!({"value": 1}),
        );
        let mut host = Host::default();
        assert!(run_replay(&good, &mut host).unwrap().ok());

        let bad = fixture(
            json!([{
                "tool": "create_entity",
                "args": {"name": "A"},
                "expect_json": {"id": 2}
            }]),
            json!({"value": 1}),
        );
        let mut host = Host::default();
        assert!(matches!(
            run_replay(&bad, &mut host),
            Err(ReplayError::ResultMismatch { .. })
        ));
    }

    #[test]
    fn expected_subset_does_not_accept_nested_mismatch() {
        assert!(json_contains(
            &json!({"proposal": {"diff": {"nodes_changed": 1}}}),
            &json!({"proposal": {"diff": {"nodes_changed": 1}}})
        ));
        assert!(!json_contains(
            &json!({"proposal": {"diff": {"nodes_changed": 2}}}),
            &json!({"proposal": {"diff": {"nodes_changed": 1}}})
        ));
        assert!(!json_contains(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
    }
}
