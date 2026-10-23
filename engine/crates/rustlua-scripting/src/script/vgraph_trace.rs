//! E03-4b：`.vgraph` 运行时「当前执行节点」探针。
//!
//! 生成器（`rustlua_visual_script::compile_traced`）在每个节点入口调用
//! `__vgraph_trace("<id>")`；本模块只保存最近一次事件，供编辑器画布高亮。
//! **不是 debugger**：没有断点、单步、变量查看，也不改变脚本执行。

use std::cell::RefCell;

/// 最近一次节点进入事件。`script` 是工程相对路径（统一 `/`），`sequence` 单调递增。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VgraphTraceEvent {
    pub script: String,
    pub node: String,
    pub sequence: u64,
}

#[derive(Debug, Default)]
pub(crate) struct VgraphTrace {
    enabled: bool,
    last: Option<VgraphTraceEvent>,
    sequence: u64,
}

impl VgraphTrace {
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub(crate) fn clear(&mut self) {
        self.last = None;
    }

    pub(crate) fn record(&mut self, script: &str, node: &str) {
        if !self.enabled {
            return;
        }
        self.sequence += 1;
        self.last = Some(VgraphTraceEvent {
            script: script.replace('\\', "/"),
            node: node.to_owned(),
            sequence: self.sequence,
        });
    }

    pub(crate) fn last(&self) -> Option<VgraphTraceEvent> {
        self.last.clone()
    }

    pub(crate) fn sequence(&self) -> u64 {
        self.sequence
    }
}

/// 共享探针句柄：每个 vgraph Lua 状态的回调都写同一份，宿主按最近事件高亮。
pub(crate) type SharedVgraphTrace = std::rc::Rc<RefCell<VgraphTrace>>;
