//! M76-07：页面导航栈、模态遮罩与焦点恢复。
//!
//! 返回键语义固定：有模态先关模态，否则弹出一页；根页不响应返回。弹出时恢复
//! 进入该页前的焦点 id，避免返回后焦点落到空处。

/// 页面条目。`restore_focus` 表示**这一页被弹出时**要恢复的焦点（即离开它之前
/// 停留的焦点），因此返回给调用方的 focus 取自被弹出的条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageEntry {
    pub page: String,
    /// 弹出这一页后应恢复的焦点；`None` 表示由页面自己决定。
    pub restore_focus: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModalEntry {
    pub id: String,
    pub restore_focus: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NavigationStack {
    pages: Vec<PageEntry>,
    modals: Vec<ModalEntry>,
}

/// 一次返回键处理的结果。调用方据此做转场与焦点设置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackOutcome {
    /// 关闭了最上层模态，焦点恢复到该值。
    ClosedModal {
        id: String,
        restore_focus: Option<u64>,
    },
    /// 弹出了一页；`focus` 为返回后应聚焦的 id。
    PoppedPage { page: String, focus: Option<u64> },
    /// 已在根页且没有模态：消费返回键但不动栈。
    AtRoot,
    /// 栈为空：调用方可以退出应用/交给系统。
    Empty,
}

impl NavigationStack {
    #[must_use]
    pub fn new(root: impl Into<String>) -> Self {
        Self {
            pages: vec![PageEntry {
                page: root.into(),
                restore_focus: None,
            }],
            modals: Vec::new(),
        }
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.pages.len()
    }

    #[must_use]
    pub fn current(&self) -> Option<&PageEntry> {
        self.pages.last()
    }

    #[must_use]
    pub fn current_page(&self) -> Option<&str> {
        self.current().map(|entry| entry.page.as_str())
    }

    #[must_use]
    pub fn modal(&self) -> Option<&ModalEntry> {
        self.modals.last()
    }

    #[must_use]
    pub fn has_modal(&self) -> bool {
        !self.modals.is_empty()
    }

    /// 有模态时底层页面不接收指针输入（遮罩层吞掉事件）。
    #[must_use]
    pub fn page_accepts_input(&self) -> bool {
        self.modals.is_empty()
    }

    /// 压入新页；`restore_focus` 存在这一页上，返回时交给调用方。
    pub fn push(&mut self, page: impl Into<String>, restore_focus: Option<u64>) {
        self.pages.push(PageEntry {
            page: page.into(),
            restore_focus,
        });
    }

    pub fn push_modal(&mut self, id: impl Into<String>, restore_focus: Option<u64>) {
        self.modals.push(ModalEntry {
            id: id.into(),
            restore_focus,
        });
    }

    /// 返回键：先模态、后页面，根页不弹。
    pub fn back(&mut self) -> BackOutcome {
        if let Some(modal) = self.modals.pop() {
            return BackOutcome::ClosedModal {
                id: modal.id,
                restore_focus: modal.restore_focus,
            };
        }
        if self.pages.len() > 1 {
            let entry = self.pages.pop().expect("depth checked");
            return BackOutcome::PoppedPage {
                page: entry.page,
                focus: entry.restore_focus,
            };
        }
        if self.pages.is_empty() {
            BackOutcome::Empty
        } else {
            BackOutcome::AtRoot
        }
    }

    /// 关闭指定模态（例如点遮罩）；不存在时报错，不静默改栈。
    pub fn close_modal(&mut self, id: &str) -> Result<Option<u64>, String> {
        let Some(index) = self.modals.iter().rposition(|modal| modal.id == id) else {
            return Err(format!("模态不存在：{id}"));
        };
        let modal = self.modals.remove(index);
        Ok(modal.restore_focus)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_pop_restores_focus_chain() {
        let mut stack = NavigationStack::new("title");
        stack.push("shop", Some(11));
        stack.push("detail", Some(22));
        assert_eq!(stack.current_page(), Some("detail"));
        assert_eq!(
            stack.back(),
            BackOutcome::PoppedPage {
                page: "detail".to_owned(),
                focus: Some(22),
            }
        );
        assert_eq!(
            stack.back(),
            BackOutcome::PoppedPage {
                page: "shop".to_owned(),
                focus: Some(11),
            }
        );
        assert_eq!(stack.back(), BackOutcome::AtRoot);
        assert_eq!(stack.depth(), 1);
    }

    #[test]
    fn back_closes_modal_before_page_and_blocks_underlying_input() {
        let mut stack = NavigationStack::new("title");
        stack.push("shop", None);
        stack.push_modal("confirm", Some(7));
        assert!(!stack.page_accepts_input());
        assert_eq!(
            stack.back(),
            BackOutcome::ClosedModal {
                id: "confirm".to_owned(),
                restore_focus: Some(7),
            }
        );
        assert!(stack.page_accepts_input());
        assert_eq!(stack.current_page(), Some("shop"));
    }

    #[test]
    fn missing_modal_close_is_an_error() {
        let mut stack = NavigationStack::new("title");
        let error = stack.close_modal("ghost").expect_err("不存在必须报错");
        assert!(error.contains("ghost"));
        stack.push_modal("sheet", None);
        assert_eq!(stack.close_modal("sheet"), Ok(None));
        assert!(!stack.has_modal());
        assert_eq!(stack.back(), BackOutcome::AtRoot);
    }
}
