//! 发布面板：发布参数配置与手动发布入口。

use crate::EditorApp;
use crate::publish::{DEFAULT_ANDROID_APPLICATION_ID, PublishPlatform};
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
                if self.publish.platform == PublishPlatform::Android {
                    let (changed, lost_focus) = draw_android_path(
                        ui,
                        "applicationId",
                        &mut self.publish.android_application_id,
                    );
                    self.publish.android_application_id_edited |= changed;
                    if lost_focus {
                        self.commit_android_application_id();
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "默认值 {DEFAULT_ANDROID_APPLICATION_ID}；APK 使用 arm64-v8a。容器资源已启用块压缩。"
                        ))
                        .small()
                        .weak(),
                    );
                    if !self.publish.message.is_empty() {
                        ui.label(egui::RichText::new(self.publish.message.as_str()).small());
                    }
                    ui.collapsing("Android 构建环境", |ui| {
                        ui.label(
                            egui::RichText::new(
                                "路径保存在用户级 editor.json。安装包约 1–2 GB；需要网络和接受 Android SDK licenses。",
                            )
                            .small()
                            .weak(),
                        );
                        ui.checkbox(
                            &mut self.publish.android_license_accepted,
                            "我已阅读并接受 Android SDK licenses（安装时会提交接受）",
                        );
                        let fields = [
                            ("安装目录", &mut self.publish.android_toolchain.install_root),
                            ("Android SDK", &mut self.publish.android_toolchain.sdk_root),
                            ("Android NDK", &mut self.publish.android_toolchain.ndk_root),
                            ("JDK 17", &mut self.publish.android_toolchain.jdk_home),
                            (
                                "Gradle",
                                &mut self.publish.android_toolchain.gradle_executable,
                            ),
                        ];
                        let mut save_settings = false;
                        let mut lost_focus = false;
                        for (label, value) in fields {
                            let (changed, field_lost_focus) = draw_android_path(ui, label, value);
                            save_settings |= changed;
                            lost_focus |= field_lost_focus;
                        }
                        self.publish.android_toolchain_edited |= save_settings;
                        if lost_focus {
                            self.commit_android_toolchain();
                        }
                        if ui
                            .add_enabled(
                                !self.progress.running
                                    && self.publish.android_license_accepted,
                                egui::Button::new("下载/安装 Android 打包环境"),
                            )
                            .clicked()
                        {
                            self.install_android_toolchain();
                        }
                    });
                }
                ui.add_enabled(
                    self.publish.platform != PublishPlatform::Android,
                    egui::Checkbox::new(&mut self.publish.compress, "压缩"),
                );
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
            self.commit_android_application_id();
            self.commit_android_toolchain();
        }
        self.publish.show_panel = open && self.publish.show_panel;
    }
}

fn draw_android_path(ui: &mut egui::Ui, label: &str, value: &mut String) -> (bool, bool) {
    let response = ui
        .horizontal(|ui| {
            ui.label(label);
            ui.text_edit_singleline(value)
        })
        .inner;
    (response.changed(), response.lost_focus())
}
