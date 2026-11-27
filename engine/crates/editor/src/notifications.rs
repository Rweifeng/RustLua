//! 编辑器通知中心（P4）：**跨面板、可回看**的一小撮失败与警告。
//!
//! 在这之前，"布局保存失败""用户设置写盘失败""布局文件损坏回落默认"这类消息只有两条出路：
//! 一行会被下一帧覆盖的状态栏，或一条埋在控制台里的 `tracing::warn!`。用户正在拖动面板时
//! 布局保存失败，他是看不见的——直到下次启动发现布局没记住。
//!
//! 这里只做最小集合：有界队列（容量上限）、连续重复去重（同一句话不刷屏）、
//! 右下角最多显示三条（最新的在上），点一下可以跳到相关位置、按 × 关掉。
//! 不做"任务型通知"（进度、取消、重试）——那属于进度面板，混进来会让这个面板变成第二个状态栏。

use eframe::egui;

/// 通知级别：只用于取色与图标，不改变行为。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoticeLevel {
    /// 信息级：当前没有生产路径（通知中心只收失败与警告，避免变成第二个状态栏）；
    /// 保留是因为"级别"是通知的中心概念，测试也拿它当另一档的样例。
    #[allow(dead_code)]
    Info,
    Warning,
    Error,
}

impl NoticeLevel {
    fn color(self) -> egui::Color32 {
        match self {
            Self::Info => egui::Color32::from_rgb(120, 180, 240),
            Self::Warning => egui::Color32::from_rgb(230, 170, 60),
            Self::Error => egui::Color32::from_rgb(230, 100, 100),
        }
    }
}

/// 一条通知：级别 + 一句话 + 可选的跳转位置（工程内相对路径/绝对路径 + 行号）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Notice {
    pub(crate) level: NoticeLevel,
    pub(crate) text: String,
    /// 有位置时点击通知可以跳过去（复用 `jump_to_diagnostic` 的同一条通道）。
    pub(crate) location: Option<(String, Option<u32>)>,
    /// 连续重复次数（同一句话再次 push 时 +1，不新增行）。
    pub(crate) repeats: u32,
}

/// 通知中心：有界 + 连续去重。
#[derive(Debug, Default)]
pub(crate) struct NotificationCenter {
    entries: Vec<Notice>,
}

impl NotificationCenter {
    /// 容量上限：通知不是日志，留太多就等于没人看。
    pub(crate) const CAPACITY: usize = 24;
    /// 右下角同时最多显示几条（最新的在上）。
    const VISIBLE: usize = 3;

    /// 新通知（最新的在末尾）。与上一条**文字与级别都相同**时只累加计数。
    pub(crate) fn push(&mut self, level: NoticeLevel, text: impl Into<String>) {
        let text = text.into();
        if let Some(last) = self.entries.last_mut()
            && last.level == level
            && last.text == text
        {
            last.repeats += 1;
            return;
        }
        if self.entries.len() >= Self::CAPACITY {
            self.entries.remove(0);
        }
        self.entries.push(Notice {
            level,
            text,
            location: None,
            repeats: 1,
        });
    }

    /// 带跳转位置的通知（链接到某份文件的某一行）。
    pub(crate) fn push_at(
        &mut self,
        level: NoticeLevel,
        text: impl Into<String>,
        location: (String, Option<u32>),
    ) {
        self.push(level, text);
        if let Some(last) = self.entries.last_mut() {
            last.location = Some(location);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 最近 `VISIBLE` 条，最新的在前（UI 从上往下画）。
    pub(crate) fn visible(&self) -> Vec<(usize, &Notice)> {
        let start = self.entries.len().saturating_sub(Self::VISIBLE);
        self.entries.iter().enumerate().skip(start).rev().collect()
    }

    /// 按内部下标移除一条（`visible()` 给的就是这个下标）。
    pub(crate) fn dismiss(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.remove(index);
        }
    }

    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

/// 右下角通知条：只画最近三条，不拦截其它面板的输入（`Area` + 固定右下锚点）。
pub(crate) fn draw_notifications(
    context: &egui::Context,
    center: &mut NotificationCenter,
    mut jump: impl FnMut(&str, Option<u32>),
) {
    if center.is_empty() {
        return;
    }
    let mut dismissed: Option<usize> = None;
    let mut jump_to: Option<(String, Option<u32>)> = None;
    egui::Area::new(egui::Id::new("editor_notifications"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
        .order(egui::Order::Foreground)
        .show(context, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_max_width(420.0);
                    for (index, notice) in center.visible() {
                        ui.horizontal(|ui| {
                            ui.colored_label(notice.level.color(), "●");
                            let repeats = if notice.repeats > 1 {
                                format!("（×{}）", notice.repeats)
                            } else {
                                String::new()
                            };
                            let text = format!("{}{repeats}", notice.text);
                            let response = ui.label(egui::RichText::new(text));
                            if let Some((path, line)) = &notice.location {
                                if response
                                    .interact(egui::Sense::click())
                                    .on_hover_text("点击跳到相关位置")
                                    .clicked()
                                {
                                    jump_to = Some((path.clone(), *line));
                                }
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("×").on_hover_text("关闭").clicked() {
                                        dismissed = Some(index);
                                    }
                                },
                            );
                        });
                    }
                });
        });
    if let Some(index) = dismissed {
        center.dismiss(index);
    }
    if let Some((path, line)) = jump_to {
        jump(&path, line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_are_bounded_deduped_and_newest_first() {
        let mut center = NotificationCenter::default();
        for _ in 0..3 {
            center.push(NoticeLevel::Warning, "布局保存失败");
        }
        assert_eq!(center.len(), 1, "连续同一句话只占一行");
        assert_eq!(center.entries[0].repeats, 3);

        // 级别不同 / 文字不同都算新通知。
        center.push(NoticeLevel::Error, "布局保存失败");
        center.push(NoticeLevel::Error, "另一件事");
        assert_eq!(center.len(), 3);

        // 容量上限：超过就丢最旧的，不会无限长。
        for index in 0..NotificationCenter::CAPACITY + 5 {
            center.push(NoticeLevel::Info, format!("第 {index} 条"));
        }
        assert_eq!(center.len(), NotificationCenter::CAPACITY);
        let expected = format!("第 {} 条", NotificationCenter::CAPACITY + 4);
        assert_eq!(
            center.entries.last().map(|notice| notice.text.as_str()),
            Some(expected.as_str())
        );

        // 最新在前，且最多三条。
        let visible = center.visible();
        assert_eq!(visible.len(), 3);
        assert!(visible[0].1.text.starts_with("第 "));
        assert_eq!(visible[0].1.text, center.entries.last().unwrap().text);
    }

    #[test]
    fn dismiss_removes_exactly_the_clicked_row_and_push_at_carries_a_location() {
        let mut center = NotificationCenter::default();
        center.push(NoticeLevel::Info, "第一条");
        center.push_at(
            NoticeLevel::Error,
            "第二行",
            ("scenes/main.scene".to_owned(), Some(12)),
        );
        let visible = center.visible();
        let (index, notice) = visible[0];
        assert_eq!(
            notice
                .location
                .as_ref()
                .map(|(path, line)| (path.as_str(), *line)),
            Some(("scenes/main.scene", Some(12)))
        );
        center.dismiss(index);
        assert_eq!(center.len(), 1);
        assert_eq!(center.entries[0].text, "第一条");
        center.clear();
        assert!(center.is_empty());
    }
}
