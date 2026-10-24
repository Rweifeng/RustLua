//! M76-07：文本输入状态与 IME 最小支持。
//!
//! 光标/选区都以**字符**下标表示（不是字节），中文与 emoji 不会被切半。IME 预编辑
//! 文本单独存放，提交时才插入；取消预编辑不改正文。微信端不做进程内组合，只走
//! 原生输入桥，能力边界见 [`InputPolicy::for_host`]。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputHost {
    Desktop,
    Web,
    Wechat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputBackend {
    /// 桌面：进程内组合 + 光标/选区完整支持。
    NativeComposition,
    /// Web：浏览器 composition 事件转进来，能力同桌面。
    BrowserComposition,
    /// 微信：只能唤起原生输入框，拿到最终字符串；无内部光标/选区。
    WechatNativeBridge,
    /// 无法唤起原生输入：只读展示。
    ReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPolicy {
    pub backend: InputBackend,
    pub supports_selection: bool,
    pub note: &'static str,
}

impl InputPolicy {
    #[must_use]
    pub fn for_host(host: InputHost, bridge_available: bool) -> Self {
        match host {
            InputHost::Desktop => Self {
                backend: InputBackend::NativeComposition,
                supports_selection: true,
                note: "桌面：进程内组合与选区完整支持",
            },
            InputHost::Web => Self {
                backend: InputBackend::BrowserComposition,
                supports_selection: true,
                note: "Web：由浏览器 composition 事件驱动",
            },
            InputHost::Wechat if bridge_available => Self {
                backend: InputBackend::WechatNativeBridge,
                supports_selection: false,
                note: "微信：仅唤起原生输入取最终字符串，无内部光标/选区",
            },
            InputHost::Wechat => Self {
                backend: InputBackend::ReadOnly,
                supports_selection: false,
                note: "微信：原生输入桥不可用，输入控件降级为只读",
            },
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInputState {
    text: String,
    /// 字符下标，0..=text.chars().count()。
    cursor: usize,
    /// 选区另一端；`None` 表示没有选区。
    anchor: Option<usize>,
    composition: Option<String>,
    focused: bool,
}

impl TextInputState {
    #[must_use]
    pub fn with_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.chars().count();
        Self {
            text,
            cursor,
            anchor: None,
            composition: None,
            focused: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn composition(&self) -> Option<&str> {
        self.composition.as_deref()
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn focus(&mut self) {
        self.focused = true;
    }

    pub fn blur(&mut self) {
        self.focused = false;
        self.composition = None;
    }

    fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map_or(self.text.len(), |(byte, _)| byte)
    }

    fn selection(&self) -> Option<(usize, usize)> {
        self.anchor
            .map(|anchor| (anchor.min(self.cursor), anchor.max(self.cursor)))
            .filter(|(start, end)| start != end)
    }

    #[must_use]
    pub fn selected_text(&self) -> &str {
        self.selection().map_or("", |(start, end)| {
            &self.text[self.byte_index(start)..self.byte_index(end)]
        })
    }

    pub fn select_range(&mut self, anchor: usize, cursor: usize) {
        let limit = self.char_count();
        self.anchor = Some(anchor.min(limit));
        self.cursor = cursor.min(limit);
    }

    pub fn select_all(&mut self) {
        self.select_range(0, self.char_count());
    }

    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor.min(self.char_count());
        self.anchor = None;
    }

    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        let (start_byte, end_byte) = (self.byte_index(start), self.byte_index(end));
        self.text.replace_range(start_byte..end_byte, "");
        self.cursor = start;
        self.anchor = None;
        true
    }

    /// 插入最终文本（非预编辑）。有选区先整体替换。
    pub fn insert_str(&mut self, value: &str) {
        self.delete_selection();
        let at = self.byte_index(self.cursor);
        self.text.insert_str(at, value);
        self.cursor += value.chars().count();
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() || self.cursor == 0 {
            return;
        }
        let start = self.byte_index(self.cursor - 1);
        let end = self.byte_index(self.cursor);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub fn delete_forward(&mut self) {
        if self.delete_selection() || self.cursor >= self.char_count() {
            return;
        }
        let start = self.byte_index(self.cursor);
        let end = self.byte_index(self.cursor + 1);
        self.text.replace_range(start..end, "");
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.anchor = None;
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.char_count());
        self.anchor = None;
    }

    pub fn home(&mut self) {
        self.cursor = 0;
        self.anchor = None;
    }

    pub fn end(&mut self) {
        self.cursor = self.char_count();
        self.anchor = None;
    }

    /// 开始 IME 预编辑；已有预编辑时替换其内容。
    pub fn begin_composition(&mut self, initial: &str) {
        self.composition = Some(initial.to_owned());
    }

    pub fn update_composition(&mut self, value: &str) {
        if self.composition.is_some() {
            self.composition = Some(value.to_owned());
        }
    }

    /// 提交预编辑文本；没有预编辑时按普通插入处理空串（no-op）。
    pub fn commit_composition(&mut self) -> Option<String> {
        let value = self.composition.take()?;
        self.insert_str(&value);
        Some(value)
    }

    pub fn cancel_composition(&mut self) {
        self.composition = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_editing_uses_char_indices() {
        let mut state = TextInputState::with_text("你好");
        assert_eq!(state.cursor(), 2);
        state.insert_str("a");
        assert_eq!(state.text(), "你好a");
        state.backspace();
        assert_eq!(state.text(), "你好");
        state.move_left();
        assert_eq!(state.cursor(), 1);
        state.backspace();
        assert_eq!(state.text(), "好", "光标在 1 时退格删除前一个字符");
        state.delete_forward();
        assert_eq!(state.text(), "");
        state.end();
        state.insert_str("们");
        assert_eq!(state.text(), "们");
    }

    #[test]
    fn selection_is_replaced_by_next_insert() {
        let mut state = TextInputState::with_text("shop title");
        state.select_range(0, 4);
        assert_eq!(state.selected_text(), "shop");
        state.insert_str("card");
        assert_eq!(state.text(), "card title");
        state.select_all();
        state.backspace();
        assert_eq!(state.text(), "");
        assert_eq!(state.cursor(), 0);
    }

    #[test]
    fn composition_commits_on_finish_and_cancel_leaves_text() {
        let mut state = TextInputState::with_text("Hi ");
        state.begin_composition("ni");
        state.update_composition("nihao");
        assert_eq!(state.composition(), Some("nihao"));
        assert_eq!(state.text(), "Hi ");
        assert_eq!(state.commit_composition().as_deref(), Some("nihao"));
        assert_eq!(state.text(), "Hi nihao");

        state.begin_composition("zz");
        state.cancel_composition();
        assert_eq!(state.composition(), None);
        assert_eq!(state.text(), "Hi nihao");
        assert_eq!(state.commit_composition(), None);
    }

    #[test]
    fn wechat_degrades_to_native_bridge_or_read_only() {
        let bridge = InputPolicy::for_host(InputHost::Wechat, true);
        assert_eq!(bridge.backend, InputBackend::WechatNativeBridge);
        assert!(!bridge.supports_selection);
        assert!(bridge.note.contains("原生输入"));

        let read_only = InputPolicy::for_host(InputHost::Wechat, false);
        assert_eq!(read_only.backend, InputBackend::ReadOnly);
        assert!(read_only.note.contains("只读"));

        assert_eq!(
            InputPolicy::for_host(InputHost::Desktop, false).backend,
            InputBackend::NativeComposition
        );
        assert_eq!(
            InputPolicy::for_host(InputHost::Web, false).backend,
            InputBackend::BrowserComposition
        );
    }
}
