//! 发布面板：发布参数配置与手动发布入口。

use crate::EditorApp;
use crate::publish::PublishPlatform;
use eframe::egui;

impl EditorApp {
    /// 绘制发布面板：只配置参数，用户点击“发布”后才启动发布流程。
    pub(crate) fn draw_publish_panel(&mut self, context: &egui::Context) {
        if !self.publish.show_panel {
            return;
        }
        let mut open = self.publish.show_panel;
        egui::Window::new("发布项目")
            .open(&mut open)
            .resizable(false)
            .show(context, |ui| {
                egui::ComboBox::from_label("发布平台")
                    .selected_text(self.publish.platform.label())
                    .show_ui(ui, |ui| {
                        for platform in PublishPlatform::ALL {
                            ui.selectable_value(
                                &mut self.publish.platform,
                                platform,
                                platform.label(),
                            );
                        }
                    });
                ui.horizontal(|ui| {
                    ui.label("发布名");
                    ui.text_edit_singleline(&mut self.publish.name);
                });
                if self.publish.platform == PublishPlatform::WechatMiniGame {
                    ui.horizontal(|ui| {
                        ui.label("微信 AppID");
                        let edit = ui.text_edit_singleline(&mut self.publish.wechat_appid);
                        if edit.changed() {
                            self.publish.appid_edited = true;
                        }
                        // 失焦就记进 `<工程>/project.toml`：否则只点关闭不发布的话，
                        // 下次打开面板还得重填一遍。
                        if edit.lost_focus() {
                            self.commit_wechat_appid();
                        }
                    });
                    ui.label(
                        egui::RichText::new(
                            "留空则沿用模板的 touristappid（只能本地预览，不能上传）",
                        )
                        .small()
                        .weak(),
                    );
                    if !self.publish.message.is_empty() {
                        ui.label(egui::RichText::new(self.publish.message.as_str()).small());
                    }
                }
                ui.checkbox(&mut self.publish.compress, "压缩");
                ui.checkbox(&mut self.publish.encrypt, "加密");
                ui.separator();
                if ui
                    .add_enabled(!self.progress.running, egui::Button::new("发布"))
                    .clicked()
                {
                    self.start_publish();
                }
            });
        if !open {
            self.commit_wechat_appid();
        }
        self.publish.show_panel = open && self.publish.show_panel;
    }
}
