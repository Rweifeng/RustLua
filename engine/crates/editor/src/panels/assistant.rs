//! M75-03 内置 AI 助手面板：进程内 MCP 客户端 + Provider 抽象 + 内联确认。
//!
//! 三条硬约束（对应用户验收门）：
//!
//! 1. **没有第二套写路径**：模型返回的 tool call 一律走
//!    [`rustlua_editor_ai::execute_tool`] + [`crate::ai_host::EditorAiHost`]，与外部
//!    MCP 客户端、agent、确定性回放共用同一个 registry 和确认网关；
//! 2. **密钥不落盘、不进上下文**：`api_key` 只驻留本结构的进程内字段（输入框/环境变量），
//!    `settings_snapshot()` 的返回类型 [`editor::AssistantLayout`] 根本没有密钥字段，
//!    请求体只由 Provider 层构造；
//! 3. **写动作必须人确认**：T1 可用显式勾选自动批准；T2/T3 每次一张内联卡片，
//!    拒绝时只把错误写回模型，不碰任何文档。
//!
//! 请求在后台线程里跑（`curl` 子进程），UI 线程只做 `try_recv`；完成后通过
//! `egui::Context::request_repaint()` 唤醒空闲的 egui，面板不会"等到下一次鼠标移动才更新"。

use crate::EditorApp;
use eframe::egui;
use rustlua_editor_ai::{
    ChatMessage, GatePolicy, ModelResponse, ModelToolCall, Provider, ProviderConfig, ProviderKind,
    ProviderSettings, SYSTEM_PROMPT, ToolTier, execute_tool, tier_label, tool_by_name, tools,
};
use rustlua_scene::scene::SceneEntityId;
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Instant,
};

/// 一轮用户输入最多允许的模型请求数（含工具续写）。超过就停下并把控制权交回用户，
/// 避免模型在"读→写→读"里无限打转。
const MAX_TOOL_ROUNDS: u32 = 8;
/// 工具结果写进聊天记录时的截断长度（完整结果仍在 `messages` 里回给模型）。
const MAX_RESULT_CHARS: usize = 4_000;
/// 聊天记录最多保留的可见条目（模型历史不受影响）。
const MAX_TRANSCRIPT_ENTRIES: usize = 240;

/// 一条可见的聊天记录。`messages` 是给模型的消息，这里是给人看的（含工具执行摘要）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind {
    User,
    Assistant,
    Tool,
    Notice,
    Error,
}

#[derive(Clone, Debug)]
pub(crate) struct TranscriptEntry {
    pub kind: EntryKind,
    pub text: String,
}

impl TranscriptEntry {
    fn new(kind: EntryKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

/// 一个等待人工确认的写调用。
#[derive(Clone, Debug)]
pub(crate) struct PendingConfirm {
    pub call: ModelToolCall,
    pub tier: ToolTier,
}

/// 在途的 Provider 请求。
struct Inflight {
    receiver: Receiver<Result<ModelResponse, String>>,
    started: Instant,
    provider: ProviderKind,
}

/// 面板会话状态。
///
/// `Default` 从环境变量取非密钥配置（`RUSTLUA_AI_*` / `OPENAI_BASE_URL` 等）；
/// 持久化布局加载后会由 [`Self::apply_layout`] 覆盖。
pub(crate) struct AssistantState {
    settings: ProviderSettings,
    /// 只在内存里：不序列化、不进日志、不进模型上下文。
    api_key: String,
    messages: Vec<ChatMessage>,
    transcript: Vec<TranscriptEntry>,
    draft: String,
    /// 拖进输入框的实体（只作为下一轮用户消息的上下文，不写文档）。
    attachments: Vec<SceneEntityId>,
    inflight: Option<Inflight>,
    queued: VecDeque<ModelToolCall>,
    confirm: Option<PendingConfirm>,
    /// 显式勾选后才自动批准 T1（场景内可撤销写）。
    auto_t1: bool,
    /// 本轮用户输入已经发起的模型请求数。
    rounds: u32,
    /// 工具队列清空后要续写一轮（模型还在等 tool 结果）。
    continuation: bool,
    /// 测试/回放用：不真的发网络请求。
    offline: bool,
    error: Option<String>,
    status: String,
    /// 几何探针（GUI 门用）：本帧面板内容矩形、输入框/发送按钮/确认卡。
    pub(crate) panel_rect: egui::Rect,
    pub(crate) panel_clip_rect: egui::Rect,
    pub(crate) input_rect: egui::Rect,
    pub(crate) send_rect: egui::Rect,
    pub(crate) confirm_rect: egui::Rect,
}

impl Default for AssistantState {
    fn default() -> Self {
        Self {
            settings: ProviderConfig::from_env().settings(),
            api_key: String::new(),
            messages: vec![ChatMessage::system(SYSTEM_PROMPT)],
            transcript: vec![TranscriptEntry::new(
                EntryKind::Notice,
                "读操作直接执行；写操作会先弹出确认卡。可把层级里的实体拖进输入框作为上下文。",
            )],
            draft: String::new(),
            attachments: Vec::new(),
            inflight: None,
            queued: VecDeque::new(),
            confirm: None,
            auto_t1: false,
            rounds: 0,
            continuation: false,
            offline: false,
            error: None,
            status: "就绪".to_owned(),
            panel_rect: egui::Rect::NOTHING,
            panel_clip_rect: egui::Rect::NOTHING,
            input_rect: egui::Rect::NOTHING,
            send_rect: egui::Rect::NOTHING,
            confirm_rect: egui::Rect::NOTHING,
        }
    }
}

impl AssistantState {
    /// 布局加载/重置时应用持久化的非密钥设置。`None` = 首次运行，保留环境变量初始化值。
    pub(crate) fn apply_layout(&mut self, stored: Option<&editor::AssistantLayout>) {
        let Some(stored) = stored else {
            return;
        };
        let kind = ProviderKind::from_name(&stored.provider).unwrap_or_default();
        let mut settings = ProviderSettings::for_kind(kind);
        if !stored.base_url.trim().is_empty() {
            settings.base_url = stored.base_url.clone();
        }
        if !stored.model.trim().is_empty() {
            settings.model = stored.model.clone();
        }
        settings.max_tokens = stored.max_tokens.clamp(1, 32_768);
        self.settings = settings;
    }

    /// 可落盘的非密钥设置（类型上没有密钥字段）。
    pub(crate) fn settings_snapshot(&self) -> editor::AssistantLayout {
        editor::AssistantLayout {
            provider: provider_name(self.settings.kind).to_owned(),
            base_url: self.settings.base_url.clone(),
            model: self.settings.model.clone(),
            max_tokens: self.settings.max_tokens,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_offline(&mut self) {
        self.offline = true;
    }

    #[cfg(test)]
    pub(crate) fn set_api_key(&mut self, key: &str) {
        self.api_key = key.to_owned();
    }

    #[cfg(test)]
    pub(crate) fn transcript(&self) -> &[TranscriptEntry] {
        &self.transcript
    }

    #[cfg(test)]
    pub(crate) fn pending_confirm(&self) -> Option<(&str, ToolTier)> {
        self.confirm
            .as_ref()
            .map(|pending| (pending.call.name.as_str(), pending.tier))
    }

    #[cfg(test)]
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    // ------------------------------------------------------------------
    // 请求与工具循环
    // ------------------------------------------------------------------

    fn push_entry(&mut self, kind: EntryKind, text: impl Into<String>) {
        self.transcript.push(TranscriptEntry::new(kind, text));
        if self.transcript.len() > MAX_TRANSCRIPT_ENTRIES {
            // 只裁可见记录；`messages` 的 tool/assistant 配对保持完整。
            self.transcript.remove(0);
        }
    }

    fn start_request(&mut self, context: &egui::Context) {
        if self.inflight.is_some() {
            return;
        }
        if self.offline {
            self.status = "离线模式：未发起请求".to_owned();
            self.continuation = false;
            return;
        }
        let mut config = ProviderConfig::from_settings(&self.settings);
        config.base_url = config.base_url.trim().to_owned();
        config.model = config.model.trim().to_owned();
        config.api_key = self.api_key.trim().to_owned();
        if config.base_url.is_empty() {
            self.error = Some("Base URL 不能为空".to_owned());
            self.continuation = false;
            return;
        }
        if config.model.is_empty() {
            self.error = Some("模型名不能为空".to_owned());
            self.continuation = false;
            return;
        }
        if config.kind == ProviderKind::Anthropic && config.resolve_api_key().is_none() {
            self.error = Some(
                "Anthropic 需要 API Key：在设置里填写，或设置 ANTHROPIC_API_KEY 环境变量"
                    .to_owned(),
            );
            self.continuation = false;
            return;
        }

        let messages = self.messages.clone();
        // 与外部 MCP 客户端同一份 registry：工具清单只从 `tools()` 来，不在这里另抄一份。
        let wire_tools: Vec<_> = tools().to_vec();
        let provider_kind = config.kind;
        let (sender, receiver) = mpsc::channel();
        let wake = context.clone();
        let spawned = std::thread::Builder::new()
            .name("rustlua-assistant".to_owned())
            .spawn(move || {
                let provider = Provider::new(config);
                let outcome = provider
                    .complete(&messages, &wire_tools)
                    .map_err(|error| error.to_string());
                let _ = sender.send(outcome);
                // egui 空闲不重绘：后台结果到达时必须主动唤醒主线程。
                wake.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.rounds += 1;
                self.inflight = Some(Inflight {
                    receiver,
                    started: Instant::now(),
                    provider: provider_kind,
                });
                self.status = format!("{} 请求中…", provider_kind.label());
                self.error = None;
            }
            Err(error) => {
                self.error = Some(format!("无法启动请求线程：{error}"));
                self.continuation = false;
            }
        }
    }

    /// 每帧轮询：收结果 → 推进工具队列 → 需要时自动续写。
    fn poll(&mut self, app: &mut EditorApp, context: &egui::Context) {
        if let Some(inflight) = self.inflight.take() {
            match inflight.receiver.try_recv() {
                Ok(Ok(response)) => self.on_model_response(app, response, context),
                Ok(Err(error)) => {
                    self.fail(format!("{} 请求失败：{error}", inflight.provider.label()))
                }
                Err(TryRecvError::Empty) => {
                    self.inflight = Some(inflight);
                    context.request_repaint_after(std::time::Duration::from_millis(100));
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    self.fail("Provider 线程没有返回结果".to_owned());
                }
            }
        }
        if self.inflight.is_some() || self.confirm.is_some() {
            return;
        }
        self.pump_queue(app, context);
    }

    fn fail(&mut self, message: String) {
        self.push_entry(EntryKind::Error, message.clone());
        self.error = Some(message);
        self.status = "请求失败".to_owned();
        self.continuation = false;
        self.rounds = 0;
    }

    fn on_model_response(
        &mut self,
        app: &mut EditorApp,
        response: ModelResponse,
        context: &egui::Context,
    ) {
        let text = response.text.trim().to_owned();
        if !text.is_empty() {
            self.push_entry(EntryKind::Assistant, text.clone());
        }
        let calls = response.tool_calls;
        if calls.is_empty() {
            if !text.is_empty() {
                self.messages.push(ChatMessage::assistant(text, Vec::new()));
            }
            self.status = "回答完成".to_owned();
            self.error = None;
            self.rounds = 0;
            self.continuation = false;
            return;
        }

        self.messages
            .push(ChatMessage::assistant(text, calls.clone()));
        for call in &calls {
            let label = tool_by_name(&call.name)
                .map(|spec| tier_label(spec.tier))
                .unwrap_or("?");
            self.push_entry(EntryKind::Tool, format!("→ 调用 {}({label})", call.name));
        }

        if self.rounds >= MAX_TOOL_ROUNDS {
            // 历史里必须补齐 tool 结果，否则下一轮请求的 assistant/tool 配对不完整。
            for call in calls {
                self.messages.push(ChatMessage::tool(
                    call.id,
                    call.name,
                    "{\"error\":\"已达到最大工具轮数，未执行\"}",
                ));
            }
            self.push_entry(
                EntryKind::Notice,
                format!("已达到 {MAX_TOOL_ROUNDS} 轮工具上限，请确认后继续对话。"),
            );
            self.status = "等待用户".to_owned();
            self.continuation = false;
            return;
        }

        self.queued.extend(calls);
        self.continuation = true;
        self.pump_queue(app, context);
    }

    /// 顺序执行工具：读/自动批准的 T1 立即执行；其它写动作停在确认卡上。
    fn pump_queue(&mut self, app: &mut EditorApp, context: &egui::Context) {
        while self.confirm.is_none() {
            let Some(call) = self.queued.pop_front() else {
                if self.continuation {
                    self.continuation = false;
                    self.start_request(context);
                }
                return;
            };
            let Some(spec) = tool_by_name(&call.name) else {
                self.record_tool_result(&call, Err(format!("未知 tool：{}", call.name)));
                continue;
            };
            let tier = spec.tier;
            if tier.is_write() && !(tier == ToolTier::T1 && self.auto_t1) {
                self.confirm = Some(PendingConfirm { call, tier });
                self.status = format!("等待确认：{}", self.confirm_name());
                return;
            }
            let result = self.run_tool(app, &call, approval_policy(tier));
            self.record_tool_result(&call, result);
        }
    }

    fn confirm_name(&self) -> &str {
        self.confirm
            .as_ref()
            .map_or("<none>", |pending| pending.call.name.as_str())
    }

    fn run_tool(
        &mut self,
        app: &mut EditorApp,
        call: &ModelToolCall,
        policy: GatePolicy,
    ) -> Result<Value, String> {
        // 唯一执行入口：和 MCP 桥/回放共用 registry 与确认网关。
        let mut host = crate::ai_host::EditorAiHost { app };
        execute_tool(&mut host, policy, &call.name, &call.arguments)
            .map_err(|error| error.message())
    }

    fn record_tool_result(&mut self, call: &ModelToolCall, result: Result<Value, String>) {
        let (content, visible, kind) = match result {
            Ok(value) => {
                let compact = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned());
                let pretty = serde_json::to_string_pretty(&value).unwrap_or(compact.clone());
                (
                    compact,
                    format!("✓ {}\n{}", call.name, truncate(&pretty)),
                    EntryKind::Tool,
                )
            }
            Err(error) => (
                serde_json::json!({ "error": error }).to_string(),
                format!("✗ {}：{error}", call.name),
                EntryKind::Error,
            ),
        };
        self.messages.push(ChatMessage::tool(
            call.id.clone(),
            call.name.clone(),
            content,
        ));
        self.push_entry(kind, visible);
    }

    fn approve(&mut self, app: &mut EditorApp, context: &egui::Context) {
        let Some(pending) = self.confirm.take() else {
            return;
        };
        let result = self.run_tool(app, &pending.call, approval_policy(pending.tier));
        self.record_tool_result(&pending.call, result);
        self.status = "已批准并执行".to_owned();
        self.pump_queue(app, context);
    }

    fn reject(&mut self, app: &mut EditorApp, context: &egui::Context) {
        let Some(pending) = self.confirm.take() else {
            return;
        };
        self.record_tool_result(&pending.call, Err("用户拒绝了该动作".to_owned()));
        self.push_entry(
            EntryKind::Notice,
            format!("已拒绝 {}，文档未改动。", pending.call.name),
        );
        self.status = "已拒绝".to_owned();
        self.pump_queue(app, context);
    }

    fn send(&mut self, app: &mut EditorApp, context: &egui::Context) {
        if self.inflight.is_some() || self.confirm.is_some() {
            return;
        }
        let text = self.draft.trim().to_owned();
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        let message = self.compose_user_message(app, &text);
        self.push_entry(EntryKind::User, message.clone());
        self.messages.push(ChatMessage::user(message));
        self.draft.clear();
        self.attachments.clear();
        self.rounds = 0;
        self.queued.clear();
        self.continuation = false;
        self.start_request(context);
    }

    fn compose_user_message(&self, app: &EditorApp, text: &str) -> String {
        if self.attachments.is_empty() {
            return text.to_owned();
        }
        let mut message = if text.is_empty() {
            "请查看以下实体。".to_owned()
        } else {
            text.to_owned()
        };
        message.push_str("\n\n[上下文实体]\n");
        for id in &self.attachments {
            message.push_str(&format!("- {}\n", entity_label(app, *id)));
        }
        message
    }

    fn reset_conversation(&mut self) {
        self.messages.clear();
        self.messages.push(ChatMessage::system(SYSTEM_PROMPT));
        self.transcript.clear();
        self.push_entry(
            EntryKind::Notice,
            "已开始新对话（服务商设置保留；密钥仍在内存中）。",
        );
        self.inflight = None;
        self.queued.clear();
        self.confirm = None;
        self.rounds = 0;
        self.continuation = false;
        self.error = None;
        self.draft.clear();
        self.attachments.clear();
        self.status = "新对话".to_owned();
    }

    // ------------------------------------------------------------------
    // UI
    // ------------------------------------------------------------------

    fn ui(&mut self, ui: &mut egui::Ui, app: &mut EditorApp) {
        self.panel_rect = ui.max_rect();
        self.panel_clip_rect = ui.clip_rect();
        self.confirm_rect = egui::Rect::NOTHING;
        self.poll(app, ui.ctx());

        ui.horizontal_wrapped(|ui| {
            ui.heading("AI 助手");
            let busy = self
                .inflight
                .as_ref()
                .map(|inflight| inflight.started.elapsed().as_secs_f32());
            ui.weak(match busy {
                Some(seconds) => format!("请求中… {seconds:.1}s"),
                None => self.status.clone(),
            });
            if ui
                .add_enabled(self.inflight.is_some(), egui::Button::new("停止"))
                .clicked()
            {
                self.inflight = None;
                self.queued.clear();
                self.confirm = None;
                self.continuation = false;
                self.status = "已停止".to_owned();
            }
            if ui.button("新对话").clicked() {
                self.reset_conversation();
            }
        });
        ui.weak(format!(
            "{} 个工具 · 与外部 MCP 客户端共用同一 registry",
            tools().len()
        ));
        self.provider_settings(ui);

        if let Some(error) = self.error.clone() {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.separator();

        let composer_height = if self.attachments.is_empty() {
            108.0
        } else {
            136.0
        };
        let transcript_height = (ui.available_height() - composer_height).max(60.0);
        egui::ScrollArea::vertical()
            .id_salt("assistant_transcript")
            .stick_to_bottom(true)
            .max_height(transcript_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for entry in &self.transcript {
                    draw_entry(ui, entry);
                }
            });

        self.confirm_card(ui, app);
        self.composer(ui, app);
    }

    fn provider_settings(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("服务商设置")
            .default_open(false)
            .show(ui, |ui| {
                let mut kind = self.settings.kind;
                egui::ComboBox::from_id_salt("assistant_provider")
                    .selected_text(kind.label())
                    .show_ui(ui, |ui| {
                        for candidate in [
                            ProviderKind::OpenAi,
                            ProviderKind::Anthropic,
                            ProviderKind::Ollama,
                        ] {
                            ui.selectable_value(&mut kind, candidate, candidate.label());
                        }
                    });
                if kind != self.settings.kind {
                    // 只替换仍是旧默认值的字段，用户自定义的地址/模型保留。
                    self.settings.switch_kind(kind);
                }

                egui::Grid::new("assistant_provider_fields")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Base URL");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings.base_url)
                                .desired_width(f32::INFINITY)
                                .hint_text("留空 = 服务商默认地址"),
                        );
                        ui.end_row();

                        ui.label("模型");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings.model)
                                .desired_width(f32::INFINITY)
                                .hint_text("留空 = 服务商默认模型"),
                        );
                        ui.end_row();

                        ui.label("最大 tokens");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.max_tokens).range(1..=32_768),
                        );
                        ui.end_row();

                        ui.label("API Key");
                        let has_env = ProviderConfig::from_settings(&self.settings).has_env_key();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.api_key)
                                .password(true)
                                .desired_width(f32::INFINITY)
                                .hint_text(if has_env {
                                    "已从环境变量读到 Key（此处留空即可）"
                                } else {
                                    "只保存在本次进程内存"
                                }),
                        );
                        ui.end_row();
                    });
                ui.weak(
                    "Key 不写入布局文件、不进模型上下文；请求经系统 curl 从 stdin 读取，\
                     不出现在命令行参数里。",
                );
            });
    }

    fn confirm_card(&mut self, ui: &mut egui::Ui, app: &mut EditorApp) {
        let Some((name, tier, arguments)) = self.confirm.as_ref().map(|pending| {
            (
                pending.call.name.clone(),
                pending.tier,
                pending.call.arguments.clone(),
            )
        }) else {
            return;
        };
        let mut decision = None;
        let frame = egui::Frame::group(ui.style())
            .fill(ui.visuals().extreme_bg_color)
            .show(ui, |ui| {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("需要确认：{name}（{}）", tier_label(tier)),
                );
                egui::ScrollArea::vertical()
                    .id_salt("assistant_confirm_args")
                    .max_height(110.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(pretty_json(&arguments))
                                    .monospace()
                                    .small(),
                            )
                            .wrap(),
                        );
                    });
                ui.horizontal(|ui| {
                    if ui.button("批准执行").clicked() {
                        decision = Some(true);
                    }
                    if ui.button("拒绝").clicked() {
                        decision = Some(false);
                    }
                    ui.weak("拒绝不会改动任何文档");
                });
            });
        self.confirm_rect = frame.response.rect;
        match decision {
            Some(true) => self.approve(app, ui.ctx()),
            Some(false) => self.reject(app, ui.ctx()),
            None => {}
        }
    }

    fn composer(&mut self, ui: &mut egui::Ui, app: &mut EditorApp) {
        let can_send = self.inflight.is_none()
            && self.confirm.is_none()
            && (!self.draft.trim().is_empty() || !self.attachments.is_empty());
        let mut remove_attachment = None;
        let mut send_clicked = false;
        let (zone, dropped) =
            ui.dnd_drop_zone::<crate::dnd::DropPayload, _>(egui::Frame::default(), |ui| {
                if !self.attachments.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for (index, id) in self.attachments.iter().enumerate() {
                            let label = entity_label(app, *id);
                            ui.label(format!("@{label}"));
                            if ui.small_button("×").clicked() {
                                remove_attachment = Some(index);
                            }
                        }
                    });
                }
                let edit = ui.add(
                    egui::TextEdit::multiline(&mut self.draft)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .hint_text("描述要做什么；Ctrl+Enter 发送"),
                );
                self.input_rect = edit.rect;
                let send = ui
                    .horizontal(|ui| {
                        let response = ui.add_enabled(can_send, egui::Button::new("发送"));
                        self.send_rect = response.rect;
                        ui.weak("拖入层级实体可作为上下文");
                        response.clicked()
                    })
                    .inner;
                send_clicked = send;
                edit
            });
        if let Some(index) = remove_attachment {
            self.attachments.remove(index);
        }
        if let Some(payload) = dropped.as_deref()
            && let crate::dnd::DropPayload::Entity(id) = payload
            && !self.attachments.contains(id)
        {
            self.attachments.push(*id);
        }
        let keyboard_send = zone.inner.has_focus()
            && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
        if can_send && (send_clicked || keyboard_send) {
            self.send(app, ui.ctx());
        }
    }
}

impl EditorApp {
    /// dock 标签体入口：把状态临时取出来，避免 `&mut self.assistant` 与 `&mut self` 双借用。
    pub(crate) fn draw_assistant_panel(&mut self, ui: &mut egui::Ui) {
        let mut state = std::mem::take(&mut self.assistant);
        state.ui(ui, self);
        self.assistant = state;
    }
}

/// 给已批准/免确认的调用构造**恰好放行该层**的策略。
///
/// 不直接用 `GatePolicy::HEADLESS`：那会顺手放行所有 T1；这里的每一项都必须与
/// 用户刚刚点击的卡片对应（T2 只放行 T2，T3 只放行 T3）。
fn approval_policy(tier: ToolTier) -> GatePolicy {
    match tier {
        ToolTier::Read => GatePolicy::HEADLESS,
        ToolTier::T1 => GatePolicy {
            auto_t1: true,
            allow_t2: false,
            allow_t3: false,
            queue_unconfirmed: false,
        },
        ToolTier::T2 => GatePolicy {
            auto_t1: false,
            allow_t2: true,
            allow_t3: false,
            queue_unconfirmed: false,
        },
        ToolTier::T3 => GatePolicy {
            auto_t1: false,
            allow_t2: false,
            allow_t3: true,
            queue_unconfirmed: false,
        },
    }
}

fn provider_name(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Ollama => "ollama",
    }
}

fn entity_label(app: &EditorApp, id: SceneEntityId) -> String {
    app.document
        .current_scene
        .as_ref()
        .and_then(|document| document.scene.entities.get(&id))
        .map(|entity| format!("#{} {}", id.0, entity.name))
        .unwrap_or_else(|| format!("#{}（已删除）", id.0))
}

fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_RESULT_CHARS {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(MAX_RESULT_CHARS).collect();
    clipped.push_str("\n…（已截断，完整结果已回给模型）");
    clipped
}

fn draw_entry(ui: &mut egui::Ui, entry: &TranscriptEntry) {
    let (prefix, color) = match entry.kind {
        EntryKind::User => ("你", ui.visuals().strong_text_color()),
        EntryKind::Assistant => ("AI", ui.visuals().text_color()),
        EntryKind::Tool => ("工具", ui.visuals().weak_text_color()),
        EntryKind::Notice => ("说明", ui.visuals().weak_text_color()),
        EntryKind::Error => ("错误", ui.visuals().error_fg_color),
    };
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(0, 2))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(color, format!("{prefix}："));
                let rich = match entry.kind {
                    EntryKind::Tool => egui::RichText::new(&entry.text).monospace().small(),
                    EntryKind::Error => egui::RichText::new(&entry.text).small(),
                    _ => egui::RichText::new(&entry.text),
                };
                ui.add(egui::Label::new(rich).wrap());
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditorApp, SceneViewMode};
    use editor::{ProjectInfo, SceneDocument};
    use rustlua_infra::logging::LogSink;
    use serde_json::json;
    use std::path::PathBuf;

    fn temp_root(label: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "rustlua-assistant-{label}-{}-{counter}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    fn test_app(label: &str) -> (EditorApp, PathBuf) {
        let root = temp_root(label);
        let mut app = EditorApp::new(
            ProjectInfo {
                root: root.clone(),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(64),
        );
        app.project_loaded = true;
        let mut document = SceneDocument::new_scene("main");
        document
            .scene
            .add_entity(SceneEntityId(1), "Root", None)
            .expect("Root");
        app.document.current_scene = Some(document);
        (app, root)
    }

    fn tool_call(name: &str, args: Value) -> ModelToolCall {
        ModelToolCall {
            id: format!("call-{name}"),
            name: name.to_owned(),
            arguments: args,
        }
    }

    fn response(calls: Vec<ModelToolCall>) -> ModelResponse {
        ModelResponse {
            text: String::new(),
            tool_calls: calls,
            stop_reason: Some("tool_calls".to_owned()),
        }
    }

    /// 写工具必须停在确认卡；拒绝后文档一字不动，且结果作为 tool 消息回给模型。
    #[test]
    fn a_write_tool_waits_for_confirmation_and_rejection_leaves_the_scene_untouched() {
        let (mut app, root) = test_app("confirm");
        let mut state = AssistantState::default();
        state.set_offline();
        let context = egui::Context::default();
        state.on_model_response(
            &mut app,
            response(vec![tool_call("create_entity", json!({"name": "AI"}))]),
            &context,
        );
        assert_eq!(
            state.pending_confirm().map(|(name, _)| name),
            Some("create_entity")
        );
        assert_eq!(
            app.document
                .current_scene
                .as_ref()
                .unwrap()
                .scene
                .entities
                .len(),
            1,
            "确认前不得写文档"
        );

        state.reject(&mut app, &context);
        assert!(state.pending_confirm().is_none());
        assert_eq!(
            app.document
                .current_scene
                .as_ref()
                .unwrap()
                .scene
                .entities
                .len(),
            1,
            "拒绝后文档必须原样"
        );
        assert!(matches!(
            state.messages.last(),
            Some(ChatMessage::Tool { .. })
        ));
        assert!(state.transcript().iter().any(|entry| {
            entry.kind == EntryKind::Error && entry.text.contains("用户拒绝")
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    /// 批准后走的是与外部 MCP 客户端完全相同的 `execute_tool` + `EditorAiHost` 路径：
    /// 一条命令 = 一条 Undo，结果形状与直接调用一致。
    #[test]
    fn approval_executes_through_the_shared_registry_and_is_undoable() {
        let (mut app, root) = test_app("approve");
        let mut state = AssistantState::default();
        state.set_offline();
        let context = egui::Context::default();
        state.on_model_response(
            &mut app,
            response(vec![tool_call("create_entity", json!({"name": "AI"}))]),
            &context,
        );
        assert!(state.pending_confirm().is_some());
        state.approve(&mut app, &context);
        assert!(state.pending_confirm().is_none());
        let created = app
            .document
            .current_scene
            .as_ref()
            .unwrap()
            .scene
            .entities
            .values()
            .find(|entity| entity.name == "AI")
            .map(|entity| entity.id)
            .expect("批准后实体必须存在");
        assert!(state.transcript().iter().any(|entry| {
            entry.kind == EntryKind::Tool && entry.text.contains("create_entity")
        }));
        // 同一条命令在历史里可撤销：证明走的是 EditorCommand 而不是旁路写。
        app.dispatch(editor::EditorCommand::Undo);
        assert!(
            !app.document
                .current_scene
                .as_ref()
                .unwrap()
                .scene
                .entities
                .contains_key(&created),
            "AI 写动作必须可撤销"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 读工具立即执行，不需要确认，并且读到的是宿主真实场景。
    #[test]
    fn read_tools_run_immediately_against_the_real_scene() {
        let (mut app, root) = test_app("read");
        let mut state = AssistantState::default();
        state.set_offline();
        let context = egui::Context::default();
        state.on_model_response(
            &mut app,
            response(vec![tool_call("get_scene_tree", json!({}))]),
            &context,
        );
        assert!(state.pending_confirm().is_none());
        assert!(
            state
                .transcript()
                .iter()
                .any(|entry| { entry.kind == EntryKind::Tool && entry.text.contains("Root") })
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// T1 自动批准是显式勾选才生效；默认仍然要确认。
    #[test]
    fn auto_t1_is_opt_in_only() {
        let (mut app, root) = test_app("auto-t1");
        let mut state = AssistantState::default();
        state.set_offline();
        let context = egui::Context::default();
        state.auto_t1 = true;
        state.on_model_response(
            &mut app,
            response(vec![tool_call("create_entity", json!({"name": "Auto"}))]),
            &context,
        );
        assert!(state.pending_confirm().is_none(), "勾选后 T1 应直接执行");
        assert_eq!(
            app.document
                .current_scene
                .as_ref()
                .unwrap()
                .scene
                .entities
                .len(),
            2
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// T2/T3 即使勾了自动 T1 也必须逐次确认。
    #[test]
    fn destructive_tiers_always_need_confirmation() {
        let (mut app, root) = test_app("t2");
        let mut state = AssistantState::default();
        state.set_offline();
        state.auto_t1 = true;
        let context = egui::Context::default();
        state.on_model_response(
            &mut app,
            response(vec![tool_call("delete_entity", json!({"entities": [1]}))]),
            &context,
        );
        assert_eq!(
            state.pending_confirm().map(|(_, tier)| tier),
            Some(ToolTier::T2)
        );
        assert!(
            app.document
                .current_scene
                .as_ref()
                .unwrap()
                .scene
                .entities
                .contains_key(&SceneEntityId(1))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 密钥只存在内存：布局快照的任何字段都不含它。
    #[test]
    fn api_key_never_reaches_the_persisted_layout() {
        let mut state = AssistantState::default();
        state.set_api_key("sk-very-secret");
        let stored = state.settings_snapshot();
        let json = serde_json::json!({
            "provider": stored.provider,
            "base_url": stored.base_url,
            "model": stored.model,
            "max_tokens": stored.max_tokens,
        })
        .to_string();
        assert!(!json.contains("secret"), "密钥进了布局设置：{json}");
        // `AssistantLayout` 类型上就没有 key 字段；密钥的 Debug 脱敏由 Provider 层单测覆盖。
    }

    /// 配置不完整时在本地就报出来，不启动线程、不把半截请求发出去。
    #[test]
    fn a_missing_model_is_reported_before_any_request_starts() {
        let mut state = AssistantState::default();
        state.settings.model.clear();
        let context = egui::Context::default();
        state.start_request(&context);
        assert!(state.inflight.is_none(), "缺模型名时不得发起请求");
        assert!(
            state.error().is_some_and(|error| error.contains("模型名")),
            "错误要直接指出缺的是模型名：{:?}",
            state.error()
        );
    }

    /// egui 几何门：助手在真实 dock 叶子里的输入框/发送按钮不越界、互不重叠。
    #[test]
    fn assistant_layout_stays_inside_its_dock_leaf() {
        let (mut app, root) = test_app("geometry");
        app.show_assistant_panel = true;
        app.scene_view_mode = SceneViewMode::View2D;
        {
            let mut state = app.dock_state.take().expect("初始 dock 树");
            assert!(
                crate::dock::focus_tab(&mut state, crate::dock::DockTab::Assistant),
                "默认树必须包含助手标签"
            );
            app.dock_state = Some(state);
        }
        let context = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };
        let output = context.run(raw, |context| {
            crate::dock::show_workspace(context, &mut app)
        });
        let state = &app.assistant;
        assert!(state.panel_rect.is_positive(), "助手面板必须真的画出来");
        assert!(state.input_rect.is_positive(), "输入框必须有矩形");
        assert!(state.send_rect.is_positive(), "发送按钮必须有矩形");
        let tolerance = 2.0;
        for (label, rect) in [("输入框", state.input_rect), ("发送按钮", state.send_rect)] {
            assert!(
                rect.left() >= state.panel_rect.left() - tolerance
                    && rect.right() <= state.panel_rect.right() + tolerance
                    && rect.top() >= state.panel_rect.top() - tolerance
                    && rect.bottom() <= state.panel_rect.bottom() + tolerance,
                "{label}越出助手面板：{rect:?} vs {:?}",
                state.panel_rect
            );
        }
        assert!(
            !state.input_rect.intersects(state.send_rect),
            "输入框与发送按钮重叠：{:?} / {:?}",
            state.input_rect,
            state.send_rect
        );
        let text = collect_text(&output);
        assert!(text.contains("AI 助手"), "面板标题没画出来");
        assert!(text.contains("服务商设置"), "设置区没画出来");
        assert!(
            text.contains("与外部 MCP 客户端共用同一 registry"),
            "同一 registry 的说明没画出来：{text}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn collect_text(output: &egui::FullOutput) -> String {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text_shape) => Some(text_shape.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
