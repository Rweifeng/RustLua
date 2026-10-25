//! M75-03 内置 AI 助手面板：进程内 MCP 客户端 + Provider 抽象 + 内联确认。
//!
//! 三条硬约束（对应用户验收门）：
//!
//! 1. **没有第二套写路径**：模型返回的 tool call 一律走
//!    [`rustlua_editor_ai::execute_tool`] + [`crate::ai_host::EditorAiHost`]，与外部
//!    MCP 客户端、agent、确定性回放共用同一个 registry 和确认网关；
//! 2. **密钥不落工程、不进上下文**：`api_key` 只在内存里参与请求构造；持久化时单独
//!    走 `~/.rustlua/assistant.key`（Windows DPAPI 加密，仅当前用户可解密），
//!    `settings_snapshot()` 的返回类型 [`editor::AssistantLayout`] 没有密钥字段，
//!    `.cache/editor/layout.json` 与用户级 `editor.json` 都不含 Key；
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
/// 避免模型在"读→写→读"里无限打转；长任务按"继续"再开新一轮，不丢历史。
const MAX_TOOL_ROUNDS: u32 = 32;
/// 工具结果写进聊天记录时的显示截断长度（给人看的那一份）。
const MAX_RESULT_CHARS: usize = 4_000;
/// 单条工具结果进模型历史的上限：预览图/日志这类长结果不能一条就占几十 KB，
/// 历史每轮都要整份重发。
const MAX_TOOL_RESULT_CHARS: usize = 32_000;
/// 送请求前的历史预算，按「1 token ≥ 2 字符」保守折算（中文接近下界，ASCII 会被高估）。
/// 100k tokens 给常见 128k 上下文模型留出余量；超了先丢最早的整轮，再削正文。
const HISTORY_TOKEN_BUDGET: usize = 100_000;
const HISTORY_CHAR_BUDGET: usize = HISTORY_TOKEN_BUDGET * 2;
/// 预算兜底时每条消息正文保底保留的字符数。
const HISTORY_CLIP_CHARS: usize = 400;
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
    /// 工具结果太长时默认只显示前几行；展开状态只影响显示，不影响模型历史。
    expanded: bool,
    /// 实测高度与测量时的内容宽度：转录区按视口只画可见条目，靠它跳过屏外内容
    /// （长会话每帧重排全部记录是卡顿的主因）。宽度变化时缓存作废，重测一帧。
    height: f32,
    measured_width: f32,
}

impl TranscriptEntry {
    fn new(kind: EntryKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            expanded: false,
            height: 0.0,
            measured_width: -1.0,
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
/// 用户级配置（`~/.rustlua`）与旧工程布局加载后会由 [`Self::apply_layout`] 覆盖。
pub(crate) struct AssistantState {
    settings: ProviderSettings,
    /// 参与请求构造；持久化时只进 DPAPI 加密的 `~/.rustlua/assistant.key`。
    api_key: String,
    /// 是否拿得到用户级配置目录（拿不到时面板如实说"只在本次进程内存"）。
    user_config_available: bool,
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
    /// 上一帧输入区（含确认卡）的内容高度：面板高度差一帧，用它触发补帧重绘。
    bottom_content_height: f32,
    /// 本帧真实排版的转录条目数（屏外条目只占高度、不重排）：几何/性能门用。
    pub(crate) last_drawn_entries: usize,
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
            user_config_available: false,
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
            bottom_content_height: 0.0,
            last_drawn_entries: 0,
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

    /// 从用户配置恢复的 Key（已解密）直接回填输入框，面板里显示为掩码。
    pub(crate) fn restore_api_key(&mut self, key: String) {
        self.api_key = key;
    }

    /// 启用用户级持久化（拿到 `~/.rustlua` 之后由启动流程调用）。
    pub(crate) fn set_user_config_available(&mut self, available: bool) {
        self.user_config_available = available;
    }

    /// 当前 Key（裁剪首尾空白）；只交给 [`editor::UserConfigStore`] 加密落盘。
    pub(crate) fn api_key_snapshot(&self) -> String {
        self.api_key.trim().to_owned()
    }

    #[cfg(test)]
    pub(crate) fn set_offline(&mut self) {
        self.offline = true;
    }

    #[cfg(test)]
    pub(crate) fn set_api_key_for_test(&mut self, key: &str) {
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
            self.error = Some("接口地址不能为空".to_owned());
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
                "Anthropic 需要接口密钥：在设置里填写，或设置 ANTHROPIC_API_KEY 环境变量"
                    .to_owned(),
            );
            self.continuation = false;
            return;
        }

        let dropped_turns = trim_history(&mut self.messages, HISTORY_CHAR_BUDGET);
        if dropped_turns > 0 {
            self.push_entry(
                EntryKind::Notice,
                format!(
                    "已省略最早的 {dropped_turns} 轮对话以控制上下文长度（系统提示与最新一轮保留）。"
                ),
            );
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
                    self.fail("服务线程没有返回结果".to_owned());
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
                format!(
                    "已连续执行 {MAX_TOOL_ROUNDS} 轮工具调用，本轮暂停；发任意消息即可接着做（历史保留）。"
                ),
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
                self.record_tool_result(&call, Err(format!("未知工具：{}", call.name)));
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
            Ok(mut value) => {
                strip_binary_payloads(&mut value);
                let compact = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned());
                let content = clamp_tool_result(compact);
                let pretty =
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| content.clone());
                (
                    content,
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

        // egui_dock 的标签体画在 ScrollArea 里：任何一帧横向溢出都会回灌可用宽度
        // （同 dock.rs 工具条注释）。助手把每一行都钉在可见宽度内排版——否则窄叶子
        // 里文字按更宽的行宽折行，右缘被裁剪框切掉。
        let band = ui.available_rect_before_wrap();
        let right = band.right().min(ui.clip_rect().right());
        let band = egui::Rect::from_min_size(
            band.min,
            egui::vec2((right - band.left()).max(0.0), band.height()),
        );
        let card_max_height = (band.height() * 0.5).clamp(80.0, 240.0);
        let mut bottom_content_height = 0.0f32;
        let mut drawn_entries = 0usize;
        ui.scope_builder(egui::UiBuilder::new().max_rect(band), |ui| {
            self.header(ui);
            self.provider_settings(ui);
            if let Some(error) = self.error.clone() {
                egui::Frame::NONE
                    .stroke(egui::Stroke::new(1.0_f32, ui.visuals().error_fg_color))
                    .corner_radius(egui::CornerRadius::same(4))
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(error)
                                    .small()
                                    .color(ui.visuals().error_fg_color),
                            )
                            .wrap(),
                        );
                    });
            }

            // 底部先占位：确认卡与输入区按内容定高，对话区吃剩下的高度。
            // 于是确认卡的参数再长也只会压缩对话区，不会把输入框顶出面板。
            egui::TopBottomPanel::bottom("assistant_composer_area")
                .frame(egui::Frame::NONE)
                .show_separator_line(false)
                .show_inside(ui, |ui| {
                    self.confirm_card(ui, app, card_max_height);
                    self.composer(ui, app);
                    bottom_content_height = ui.min_rect().height();
                });

            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show_inside(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("assistant_transcript")
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show_viewport(ui, |ui, viewport| {
                            drawn_entries = draw_transcript(ui, &mut self.transcript, viewport);
                        });
                });
        });
        self.last_drawn_entries = drawn_entries;
        // 面板高度沿用上一帧的内容高度（TopBottomPanel 的固有行为）：内容变化的那一帧
        // 补一次重绘，用户看不到"半截输入框"，也不会每帧空转。
        if (bottom_content_height - self.bottom_content_height).abs() > 0.5 {
            self.bottom_content_height = bottom_content_height;
            ui.ctx().request_repaint();
        }
    }

    /// 顶栏：标题 + 状态 + 操作按钮。按钮先右对齐占位，长状态文本不会再把它挤出面板。
    fn header(&mut self, ui: &mut egui::Ui) {
        let busy = self
            .inflight
            .as_ref()
            .map(|inflight| inflight.started.elapsed().as_secs_f32());
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("新对话").clicked() {
                    self.reset_conversation();
                }
                if ui
                    .add_enabled(busy.is_some(), egui::Button::new("停止"))
                    .clicked()
                {
                    self.inflight = None;
                    self.queued.clear();
                    self.confirm = None;
                    self.continuation = false;
                    self.status = "已停止".to_owned();
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.strong("AI 助手");
                    ui.add_space(6.0);
                    if busy.is_some() {
                        ui.add(egui::Spinner::new().size(12.0));
                    }
                    let status = busy
                        .map(|seconds| format!("请求中… {seconds:.1}s"))
                        .unwrap_or_else(|| self.status.clone());
                    // 窄了省略，悬停看全文；确认卡本身还会显示完整工具名。
                    ui.add(
                        egui::Label::new(egui::RichText::new(status.clone()).small().weak())
                            .truncate(),
                    )
                    .on_hover_text(status);
                });
            });
        });
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "{} 个工具 · 与外部 MCP 客户端共用同一工具注册表",
                    tools().len()
                ))
                .small()
                .weak(),
            )
            .wrap(),
        );
    }

    fn provider_settings(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(format!("服务商设置 · {}", self.settings.kind.label()))
            .id_salt("assistant_provider_settings")
            .default_open(false)
            .show(ui, |ui| {
                // 展开后整体限高：短面板里设置项自己滚动，不吃掉对话区。
                egui::ScrollArea::vertical()
                    .id_salt("assistant_provider_body")
                    .max_height(240.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let mut kind = self.settings.kind;
                        egui::ComboBox::from_id_salt("assistant_provider")
                            .selected_text(kind.label())
                            .width(ui.available_width().min(200.0))
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

                        // 字段竖排：窄面板里两列表格会被挤到裁剪框外。
                        labeled_field(
                            ui,
                            "接口地址",
                            &mut self.settings.base_url,
                            false,
                            "留空 = 服务商默认地址",
                        );
                        labeled_field(
                            ui,
                            "模型",
                            &mut self.settings.model,
                            false,
                            "留空 = 服务商默认模型",
                        );
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(
                                egui::RichText::new("最大令牌数").small().weak(),
                            ));
                            ui.add(
                                egui::DragValue::new(&mut self.settings.max_tokens)
                                    .range(1..=32_768),
                            );
                        });
                        let has_env = ProviderConfig::from_settings(&self.settings).has_env_key();
                        labeled_field(
                            ui,
                            "接口密钥",
                            &mut self.api_key,
                            true,
                            if has_env {
                                "已从环境变量读到密钥（此处留空即可）"
                            } else if self.user_config_available {
                                "保存到 ~/.rustlua（Windows DPAPI 加密，仅当前用户可解密）"
                            } else {
                                "找不到用户主目录：只保存在本次进程内存"
                            },
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "密钥不写入工程目录、不进模型上下文；保存时单独加密到 \
                                     ~/.rustlua/assistant.key。请求经系统 curl 从标准输入读取，\
                                     不出现在命令行参数里。",
                                )
                                .small()
                                .weak(),
                            )
                            .wrap(),
                        );
                    });
            });
    }

    fn confirm_card(&mut self, ui: &mut egui::Ui, app: &mut EditorApp, max_height: f32) {
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
        // 参数预览给整张卡留出上限：矮面板里确认卡不许把输入框挤出可见区。
        let preview_height = (max_height - 70.0).clamp(40.0, 140.0);
        let frame = egui::Frame::group(ui.style())
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        egui::RichText::new("需要确认").strong(),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("{name}（{}）", tier_label(tier)))
                                .monospace()
                                .small(),
                        )
                        .wrap(),
                    );
                });
                egui::ScrollArea::vertical()
                    .id_salt("assistant_confirm_args")
                    .max_height(preview_height)
                    .auto_shrink([false, true])
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
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if ui.button("批准执行").clicked() {
                        decision = Some(true);
                    }
                    if ui.button("拒绝").clicked() {
                        decision = Some(false);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("拒绝不会改动任何文档").small().weak(),
                            )
                            .truncate(),
                        );
                    });
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
        ui.separator();
        let (zone, dropped) =
            ui.dnd_drop_zone::<crate::dnd::DropPayload, _>(egui::Frame::default(), |ui| {
                if !self.attachments.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for (index, id) in self.attachments.iter().enumerate() {
                            let label = entity_label(app, *id);
                            ui.label(egui::RichText::new(format!("@{label}")).small());
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
                ui.add_space(2.0);
                send_clicked = ui
                    .horizontal(|ui| {
                        // 整行按按钮高度对齐：提示文案与按钮同一个垂直中心。
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let mut send = egui::Button::new(egui::RichText::new("发送").strong())
                                .min_size(egui::vec2(88.0, 30.0))
                                .corner_radius(egui::CornerRadius::same(6));
                            if can_send {
                                send = send.fill(ui.visuals().selection.bg_fill);
                            }
                            let response = ui.add_enabled(can_send, send);
                            self.send_rect = response.rect;
                            ui.add_space(8.0);
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    let hint = if self.confirm.is_some() {
                                        "先处理上方的确认卡"
                                    } else {
                                        "Ctrl+Enter 发送 · 可拖入层级实体作为上下文"
                                    };
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(hint).small().weak())
                                            .truncate(),
                                    );
                                },
                            );
                            response.clicked()
                        })
                        .inner
                    })
                    .inner;
                ui.add_space(6.0);
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
    clipped(text, MAX_RESULT_CHARS, "\n…（已截断）")
}

fn clipped(text: &str, limit: usize, note: &str) -> String {
    let mut out: String = text.chars().take(limit).collect();
    out.push_str(note);
    out
}

/// 单条工具结果进历史前的封顶；超了宁可截断也不能让一条结果吃掉整个预算。
fn clamp_tool_result(compact: String) -> String {
    if compact.chars().count() <= MAX_TOOL_RESULT_CHARS {
        return compact;
    }
    clipped(&compact, MAX_TOOL_RESULT_CHARS, "\n…（结果过长，已截断）")
}

/// 预览图 base64、生成图字节数组一律不进历史：面板不渲染图片，模型需要知道的只是
/// 「有图、多大」，而这几百 KB 每轮请求都要整份重发。
fn strip_binary_payloads(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in ["png_base64", "preview_png_base64"] {
                let encoded = map
                    .remove(key)
                    .and_then(|encoded| encoded.as_str().map(str::len));
                if let Some(encoded) = encoded {
                    map.insert(
                        "png_bytes".to_owned(),
                        Value::from((encoded * 3 / 4) as u64),
                    );
                    map.insert(
                        "png_omitted".to_owned(),
                        Value::from("图像字节未写入对话历史"),
                    );
                }
            }
            let byte_array = match map.get("png") {
                Some(Value::Array(bytes)) => Some(bytes.len()),
                _ => None,
            };
            if let Some(bytes) = byte_array {
                map.remove("png");
                map.insert("png_bytes".to_owned(), Value::from(bytes as u64));
                map.insert(
                    "png_omitted".to_owned(),
                    Value::from("图像字节未写入对话历史"),
                );
            }
            for child in map.values_mut() {
                strip_binary_payloads(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_binary_payloads(item);
            }
        }
        _ => {}
    }
}

fn message_chars(message: &ChatMessage) -> usize {
    match message {
        ChatMessage::System(text) | ChatMessage::User(text) => text.chars().count(),
        ChatMessage::Assistant { text, tool_calls } => {
            text.chars().count()
                + tool_calls
                    .iter()
                    .map(|call| {
                        call.name.chars().count() + call.arguments.to_string().chars().count()
                    })
                    .sum::<usize>()
        }
        ChatMessage::Tool { content, .. } => content.chars().count(),
    }
}

fn history_chars(messages: &[ChatMessage]) -> usize {
    messages.iter().map(message_chars).sum()
}

/// 超预算时丢最早的整轮，返回丢掉的轮数。一轮 = 一条 user 起头直到下一条 user 前的
/// 全部消息；只按整轮丢，`assistant.tool_calls` 与 tool 结果永远同生共死。
/// 系统提示与最新一轮不参与丢弃；只剩一轮仍超预算时改为削正文（保留消息条数）。
fn trim_history(messages: &mut Vec<ChatMessage>, budget: usize) -> usize {
    let mut dropped = 0;
    while history_chars(messages) > budget {
        let Some((start, end)) = oldest_turn(messages) else {
            break;
        };
        messages.drain(start..end);
        dropped += 1;
    }
    if history_chars(messages) > budget {
        clip_oldest_texts(messages, budget);
    }
    dropped
}

fn oldest_turn(messages: &[ChatMessage]) -> Option<(usize, usize)> {
    let start = messages
        .iter()
        .position(|message| matches!(message, ChatMessage::User(_)))?;
    let end = messages
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, message)| matches!(message, ChatMessage::User(_)).then_some(index))?;
    Some((start, end))
}

fn clip_oldest_texts(messages: &mut [ChatMessage], budget: usize) {
    let last = messages.len().saturating_sub(1);
    let mut total = history_chars(messages);
    for message in messages.iter_mut().take(last).skip(1) {
        if total <= budget {
            break;
        }
        let before = message_chars(message);
        if before <= HISTORY_CLIP_CHARS {
            continue;
        }
        clip_message(message);
        total = total.saturating_sub(before - message_chars(message));
    }
}

fn clip_message(message: &mut ChatMessage) {
    fn clip(text: &mut String) {
        if text.chars().count() > HISTORY_CLIP_CHARS {
            *text = clipped(text, HISTORY_CLIP_CHARS, "…（较早内容已省略）");
        }
    }
    match message {
        ChatMessage::System(text) | ChatMessage::User(text) => clip(text),
        ChatMessage::Assistant { text, .. } => clip(text),
        ChatMessage::Tool { content, .. } => clip(content),
    }
}

fn labeled_field(ui: &mut egui::Ui, label: &str, value: &mut String, password: bool, hint: &str) {
    ui.add(egui::Label::new(egui::RichText::new(label).small().weak()));
    let mut edit = egui::TextEdit::singleline(value)
        .desired_width(f32::INFINITY)
        .hint_text(hint);
    if password {
        edit = edit.password(true);
    }
    ui.add(edit);
}

/// 转录区：只画视口附近的条目，屏外用缓存的实测高度占位。
///
/// 跳过时用 `add_space(height)` 前进，与真实绘制后 `cursor` 的前进量一致
/// （两者都只差一次 item_spacing），滚动条长度不随跳过的条目数漂移。
/// 宽度变化会让缓存作废，重测一帧。
fn draw_transcript(
    ui: &mut egui::Ui,
    entries: &mut [TranscriptEntry],
    viewport: egui::Rect,
) -> usize {
    const OVERSCAN: f32 = 240.0;
    let width = ui.available_width();
    let origin = ui.cursor().top();
    let mut drawn = 0usize;
    for entry in entries {
        let top = ui.cursor().top() - origin;
        let measured = entry.height > 0.0 && (entry.measured_width - width).abs() < 0.5;
        let outside =
            top + entry.height < viewport.top() - OVERSCAN || top > viewport.bottom() + OVERSCAN;
        if measured && outside {
            ui.add_space(entry.height);
            continue;
        }
        let before = ui.cursor().top();
        draw_entry(ui, entry);
        entry.height = ui.cursor().top() - before;
        entry.measured_width = width;
        drawn += 1;
    }
    drawn
}

fn role_chip(ui: &mut egui::Ui, label: &str, color: egui::Color32) {
    egui::Frame::NONE
        .fill(color.gamma_multiply(0.16))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.add(egui::Label::new(
                egui::RichText::new(label).small().strong().color(color),
            ));
        });
}

fn draw_entry(ui: &mut egui::Ui, entry: &mut TranscriptEntry) {
    let (speaker, color) = match entry.kind {
        EntryKind::User => ("你", ui.visuals().strong_text_color()),
        EntryKind::Assistant => ("AI", ui.visuals().selection.bg_fill),
        EntryKind::Tool => ("工具", ui.visuals().weak_text_color()),
        EntryKind::Notice => ("说明", ui.visuals().weak_text_color()),
        EntryKind::Error => ("错误", ui.visuals().error_fg_color),
    };
    let mut frame = egui::Frame::NONE
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(8, 6));
    frame = match entry.kind {
        EntryKind::User => frame.fill(ui.visuals().faint_bg_color),
        EntryKind::Error => frame.stroke(egui::Stroke::new(1.0_f32, ui.visuals().error_fg_color)),
        _ => frame,
    };
    // 工具条目的首行（`→ 调用 name(T1)` / `✓ name`）并进角色行，省一层竖排。
    let tool_head = (entry.kind == EntryKind::Tool).then(|| {
        entry
            .text
            .split_once('\n')
            .map_or(entry.text.as_str(), |(head, _)| head)
    });
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            role_chip(ui, speaker, color);
            if let Some(head) = tool_head {
                ui.add(
                    egui::Label::new(egui::RichText::new(head).monospace().small().weak())
                        .truncate(),
                )
                .on_hover_text(head);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(egui::RichText::new("复制").small().weak()).frame(false))
                    .on_hover_text("复制这条内容")
                    .clicked()
                {
                    ui.ctx().copy_text(entry.text.clone());
                }
            });
        });
        ui.add_space(2.0);
        match entry.kind {
            EntryKind::Tool => {
                if let Some((_, body)) = entry.text.split_once('\n') {
                    draw_json_block(ui, body, &mut entry.expanded);
                }
            }
            EntryKind::Notice => {
                ui.add(egui::Label::new(egui::RichText::new(&entry.text).small().weak()).wrap());
            }
            EntryKind::Error => {
                ui.add(egui::Label::new(egui::RichText::new(&entry.text).small()).wrap());
            }
            EntryKind::User | EntryKind::Assistant => draw_markdown(ui, &entry.text),
        }
    });
    ui.add_space(6.0);
}

/// 代码块/JSON 预览的统一外形：宽度钉满可用宽度，所有块左右边界一致。
fn draw_code_frame(ui: &mut egui::Ui, text: &str) {
    let text = text.trim_end();
    if text.is_empty() {
        return;
    }
    egui::Frame::NONE
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(egui::RichText::new(text).monospace()).wrap());
        });
}

fn draw_json_block(ui: &mut egui::Ui, body: &str, expanded: &mut bool) {
    const COLLAPSED_LINES: usize = 8;
    let lines: Vec<&str> = body.lines().collect();
    let overflow = lines.len() > COLLAPSED_LINES;
    let shown = if *expanded || !overflow {
        lines.len()
    } else {
        COLLAPSED_LINES
    };
    draw_code_frame(ui, &lines[..shown].join("\n"));
    if overflow {
        let label = if *expanded {
            "收起".to_owned()
        } else {
            format!("展开全部 {} 行", lines.len())
        };
        let button = egui::Button::new(egui::RichText::new(label).small().weak()).frame(false);
        if ui
            .add_sized([ui.available_width(), 20.0], button)
            .on_hover_text("只影响显示，不影响发给模型的结果")
            .clicked()
        {
            *expanded = !*expanded;
        }
    }
}

/// 模型回答里的 Markdown 子集：标题、列表、引用、表格行与 ``` 代码块；
/// 行内只认 `**粗体**` 与反引号代码，其余原样显示（不引入渲染依赖）。
fn draw_markdown(ui: &mut egui::Ui, text: &str) {
    let mut in_code = false;
    let mut code = String::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.starts_with("```") {
            if in_code {
                draw_code_frame(ui, code.trim_end());
                code.clear();
                in_code = false;
            } else {
                in_code = true;
            }
            continue;
        }
        if in_code {
            code.push_str(line);
            code.push('\n');
            continue;
        }
        if trimmed.is_empty() {
            ui.add_space(4.0);
            continue;
        }
        if trimmed == "---" || trimmed == "***" {
            ui.separator();
        } else if let Some(rest) = trimmed.strip_prefix("# ") {
            draw_heading(ui, rest, 3.0);
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            draw_heading(ui, rest, 2.0);
        } else if let Some(rest) = trimmed.strip_prefix("### ") {
            draw_heading(ui, rest, 1.0);
        } else if let Some(rest) = trimmed.strip_prefix('>') {
            let job = inline_job(ui, rest.trim_start(), true);
            ui.add(egui::Label::new(job).wrap());
        } else if trimmed.starts_with('|') {
            // 表格单元格不逐列对齐，按原文等宽显示，至少不串行。
            ui.add(egui::Label::new(egui::RichText::new(trimmed).monospace().small()).wrap());
        } else if let Some(rest) = bullet_item(trimmed) {
            draw_list_item(ui, indent, "•", rest);
        } else if let Some((marker, rest)) = numbered_item(trimmed) {
            draw_list_item(ui, indent, marker, rest);
        } else {
            let pad = " ".repeat(indent.min(8));
            let body = format!("{pad}{trimmed}");
            let job = inline_job(ui, &body, false);
            ui.add(egui::Label::new(job).wrap());
        }
    }
    if in_code {
        draw_code_frame(ui, code.trim_end());
    }
}

fn bullet_item(line: &str) -> Option<&str> {
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
}

/// `1. xxx` → (`1.`, `xxx`)；标记必须是纯数字，避免把普通句子判成列表。
fn numbered_item(line: &str) -> Option<(&str, &str)> {
    let (marker, rest) = line.split_once(". ")?;
    if !marker.is_empty() && marker.chars().all(|ch| ch.is_ascii_digit()) {
        Some((&line[..marker.len() + 1], rest))
    } else {
        None
    }
}

/// 列表项：标记与正文分开排版，折行后正文仍与首行正文左对齐（悬挂缩进）。
fn draw_list_item(ui: &mut egui::Ui, indent: usize, marker: &str, rest: &str) {
    ui.horizontal_top(|ui| {
        if indent > 0 {
            ui.add_space((indent as f32 * 8.0).min(64.0));
        }
        ui.add(egui::Label::new(egui::RichText::new(marker).small().weak()));
        ui.add(egui::Label::new(inline_job(ui, rest, false)).wrap());
    });
}

fn draw_heading(ui: &mut egui::Ui, text: &str, size_bump: f32) {
    let body_size = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Body)
        .map(|font| font.size)
        .unwrap_or(14.0);
    let mut job = inline_job(ui, text, false);
    // egui 默认字体没有粗体字面：标题靠放大 + 高对比色区分。
    for section in &mut job.sections {
        section.format.font_id = egui::FontId::proportional(body_size + size_bump);
        section.format.color = ui.visuals().strong_text_color();
        section.format.background = egui::Color32::TRANSPARENT;
    }
    ui.add_space(4.0);
    ui.add(egui::Label::new(job).wrap());
    ui.add_space(2.0);
}

/// 行内格式解析：`**粗体**` 与 `` `代码` ``（未闭合的标记按原文显示）。
fn inline_job(ui: &egui::Ui, text: &str, weak: bool) -> egui::text::LayoutJob {
    let body = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Body)
        .cloned()
        .unwrap_or_else(|| egui::FontId::proportional(14.0));
    let mono = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Monospace)
        .cloned()
        .unwrap_or_else(|| egui::FontId::monospace(13.0));
    let normal = egui::TextFormat {
        font_id: body.clone(),
        color: if weak {
            ui.visuals().weak_text_color()
        } else {
            ui.visuals().text_color()
        },
        ..Default::default()
    };
    let bold = egui::TextFormat {
        font_id: body,
        color: ui.visuals().strong_text_color(),
        ..Default::default()
    };
    let code = egui::TextFormat {
        font_id: mono,
        color: ui.visuals().text_color(),
        background: ui.visuals().code_bg_color,
        ..Default::default()
    };

    let mut job = egui::text::LayoutJob::default();
    let mut plain = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let bold_span = rest
            .strip_prefix("**")
            .and_then(|after| after.find("**").map(|end| (after, end)));
        let code_span = rest
            .strip_prefix('`')
            .and_then(|after| after.find('`').map(|end| (after, end)));
        match (bold_span, code_span) {
            (Some((after, end)), _) => {
                flush_plain(&mut job, &mut plain, &normal);
                job.append(&after[..end], 0.0, bold.clone());
                rest = &after[end + 2..];
            }
            (None, Some((after, end))) => {
                flush_plain(&mut job, &mut plain, &normal);
                job.append(&after[..end], 0.0, code.clone());
                rest = &after[end + 1..];
            }
            (None, None) => {
                let ch = rest.chars().next().expect("rest 非空");
                plain.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
    }
    flush_plain(&mut job, &mut plain, &normal);
    job
}

fn flush_plain(job: &mut egui::text::LayoutJob, plain: &mut String, format: &egui::TextFormat) {
    if !plain.is_empty() {
        job.append(plain, 0.0, format.clone());
        plain.clear();
    }
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

    /// 预览图 base64 与生成图字节数组绝不能进模型历史：面板不渲染图片，但历史
    /// 每轮都整份重发，几百 KB 的图几次就能把上下文打到接口上限。
    #[test]
    fn binary_payloads_never_enter_the_model_history() {
        let mut state = AssistantState::default();
        state.record_tool_result(
            &tool_call("ui_preview_spec", json!({"spec": {}})),
            Ok(json!({
                "width": 1280,
                "height": 720,
                "png_base64": "iVBORw0KGgo".repeat(50_000),
            })),
        );
        let Some(ChatMessage::Tool { content, .. }) = state.messages.last() else {
            panic!("缺少 tool 消息");
        };
        assert!(!content.contains("iVBOR"), "预览 base64 进了历史");
        assert!(content.contains("\"png_bytes\""), "{content}");
        assert!(
            content.chars().count() < MAX_TOOL_RESULT_CHARS,
            "净化后不该再触发截断：{} 字符",
            content.chars().count()
        );

        state.record_tool_result(
            &tool_call("ui_generate_assets", json!({})),
            Ok(json!({
                "generated": [{"key": "icon", "png": vec![7u8; 200_000]}],
            })),
        );
        let Some(ChatMessage::Tool { content, .. }) = state.messages.last() else {
            panic!("缺少 tool 消息");
        };
        assert!(!content.contains("7,7,7"), "生成图字节数组进了历史");
        assert!(content.contains("\"png_bytes\":200000"), "{content}");
    }

    /// 单条结果超上限就截断；给人看的那份仍是更短的摘要。
    #[test]
    fn an_oversized_tool_result_is_clipped_before_it_reaches_the_model() {
        let mut state = AssistantState::default();
        let huge = "x".repeat(MAX_TOOL_RESULT_CHARS + 5_000);
        state.record_tool_result(
            &tool_call("read_console", json!({})),
            Ok(json!({"lines": [huge]})),
        );
        let Some(ChatMessage::Tool { content, .. }) = state.messages.last() else {
            panic!("缺少 tool 消息");
        };
        assert!(
            content.chars().count() <= MAX_TOOL_RESULT_CHARS + 40,
            "历史里的结果没有封顶：{} 字符",
            content.chars().count()
        );
        assert!(content.ends_with("…（结果过长，已截断）"), "{content}");
        let Some(entry) = state
            .transcript()
            .iter()
            .find(|entry| entry.kind == EntryKind::Tool)
        else {
            panic!("缺少工具条目");
        };
        assert!(entry.text.contains("read_console"));
        assert!(entry.text.chars().count() < MAX_RESULT_CHARS + 100);
    }

    /// 超预算先丢最早的整轮：系统提示与最新一轮保留，assistant/tool 配对不能断。
    #[test]
    fn the_history_budget_drops_oldest_turns_but_keeps_the_tool_pairing() {
        let mut messages = vec![ChatMessage::system("sys")];
        for turn in 0..6 {
            messages.push(ChatMessage::user(format!("u{turn}{}", "u".repeat(9_000))));
            let call = ModelToolCall {
                id: format!("call-{turn}"),
                name: "read_console".to_owned(),
                arguments: json!({}),
            };
            messages.push(ChatMessage::assistant("", vec![call.clone()]));
            messages.push(ChatMessage::tool(call.id, call.name, "r".repeat(9_000)));
        }
        let dropped = trim_history(&mut messages, 25_000);
        assert!(dropped >= 4, "超预算应丢掉最早的整轮：dropped={dropped}");
        assert!(
            matches!(messages[0], ChatMessage::System(_)),
            "系统提示不能被丢"
        );
        assert!(
            matches!(messages[1], ChatMessage::User(_)),
            "剩下的必须从一整轮开始"
        );
        let mut open = std::collections::BTreeSet::new();
        for message in &messages {
            match message {
                ChatMessage::Assistant { tool_calls, .. } => {
                    open = tool_calls.iter().map(|call| call.id.clone()).collect();
                }
                ChatMessage::Tool { call_id, .. } => {
                    assert!(open.remove(call_id), "孤儿 tool 结果：{call_id}");
                }
                _ => {}
            }
        }
        assert!(history_chars(&messages) <= 25_000, "裁剪后仍超预算");
    }

    /// 只剩一轮时不能整轮丢，改为削正文：消息条数不变，最新一条结果原样保留。
    #[test]
    fn a_single_oversized_turn_is_clipped_instead_of_dropped() {
        let mut messages = vec![ChatMessage::system("sys"), ChatMessage::user("hello")];
        for round in 0..4 {
            let call = ModelToolCall {
                id: format!("call-{round}"),
                name: "read_console".to_owned(),
                arguments: json!({}),
            };
            messages.push(ChatMessage::assistant("", vec![call.clone()]));
            messages.push(ChatMessage::tool(call.id, call.name, "r".repeat(80_000)));
        }
        let before = messages.len();
        let dropped = trim_history(&mut messages, 100_000);
        assert_eq!(dropped, 0, "只剩一轮时不能整轮丢");
        assert_eq!(messages.len(), before, "削正文不能删消息，否则配对断掉");
        assert!(matches!(messages[0], ChatMessage::System(_)));
        assert!(history_chars(&messages) <= 100_000, "裁剪后仍超预算");
        let Some(ChatMessage::Tool { content, .. }) = messages.last() else {
            panic!("最新一条工具结果必须保留");
        };
        assert_eq!(content.chars().count(), 80_000, "最新一条不参与削");
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

    /// 密钥不进非密钥快照；它只走单独加密的 `assistant.key`。
    #[test]
    fn api_key_never_reaches_the_persisted_layout() {
        let mut state = AssistantState::default();
        state.set_api_key_for_test("sk-very-secret");
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
        // 底部面板高度沿用上一帧内容：画三帧才是稳态几何。
        let output = run_frames(&context, raw, 3, |context| {
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
        assert!(text.contains("AI 助手"), "面板标题没画出来：{text}");
        assert!(text.contains("服务商设置"), "设置区没画出来：{text}");
        assert!(
            text.contains("个工具"),
            "工具数量/同一注册表的说明没画出来：{text}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 窄叶子 + 被 egui_dock 的 ScrollArea 撑宽的可用宽度：文字必须按可见宽度折行，
    /// 不能按溢出宽度排版再从右缘裁掉（用户截图里的半行字就是这么来的）。
    #[test]
    fn narrow_leaf_wraps_text_instead_of_clipping_it() {
        let (mut app, root) = test_app("narrow");
        app.assistant.transcript = vec![
            TranscriptEntry::new(
                EntryKind::Assistant,
                "**B. 百分比拉伸布局**：每张卡片 `anchors` 改成比例（如 example = x 0.05→0.33），\
                 尺寸随窗口缩放，铺满效果最好但高宽比变化时卡片会被拉伸、文字不缩放。"
                    .repeat(3),
            ),
            TranscriptEntry::new(
                EntryKind::Tool,
                "✓ create_component\n{\n  \"new_entities\": [\n    6646\n  ],\n  \
                 \"result\": \"applied\",\n  \"undo_entries_added\": 1,\n  \"undo_len\": 1,\n  \
                 \"entity\": \"CardsGrid\",\n  \"name\": \"CardsGridContainer\"\n}"
                    .to_owned(),
            ),
            TranscriptEntry::new(
                EntryKind::Error,
                "✗ set_component_fields：参数错误：字段值只支持字符串/数字/布尔（收到数组）"
                    .to_owned(),
            ),
        ];
        let visible = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 640.0));
        let context = egui::Context::default();
        let output = run_frames(
            &context,
            egui::RawInput {
                screen_rect: Some(visible),
                ..Default::default()
            },
            3,
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    // 模拟 dock 叶子：max_rect 被撑到 1100，可见裁剪框只有 320。
                    let inflated = egui::Rect::from_min_size(
                        ui.min_rect().min,
                        egui::vec2(1100.0, ui.available_height()),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(inflated), |ui| {
                        ui.set_clip_rect(visible);
                        app.draw_assistant_panel(ui);
                    });
                });
            },
        );
        assert!(
            app.assistant.panel_rect.is_positive(),
            "助手面板必须真的画出来"
        );
        let tolerance = 2.0;
        for clipped in &output.shapes {
            let egui::Shape::Text(text) = &clipped.shape else {
                continue;
            };
            let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
            if !rect.intersects(visible) {
                continue;
            }
            assert!(
                rect.right() <= visible.right() + tolerance,
                "文字越出可见宽度被裁：{rect:?} / {}",
                text.galley.text()
            );
        }
        let drawn = collect_text(&output);
        assert!(
            drawn.contains("百分比拉伸布局"),
            "回答正文没画出来：{drawn}"
        );
        assert!(!drawn.contains("**"), "行内粗体标记没被吃掉：{drawn}");
        assert!(drawn.contains("展开全部"), "长工具结果应默认折叠：{drawn}");
        let _ = std::fs::remove_dir_all(root);
    }

    /// 长参数确认卡 + 撑满的对话区：确认卡与输入框都留在面板内，且确认卡不压输入框。
    #[test]
    fn confirm_card_and_composer_stay_inside_a_short_panel() {
        let (mut app, root) = test_app("short-panel");
        app.assistant.transcript = (0..30)
            .map(|index| {
                TranscriptEntry::new(
                    EntryKind::Assistant,
                    format!(
                        "第 {index} 条很长的回答，用来把对话区撑满，验证底部区域不被顶出面板。"
                    ),
                )
            })
            .collect();
        app.assistant.confirm = Some(PendingConfirm {
            call: tool_call(
                "set_component_fields",
                json!({
                    "entity": 6646,
                    "fields": {
                        "anchors": [0.05, 0.05, 0.33, 0.33],
                        "offset": [0, 0],
                        "pivot": [0, 0],
                        "size": [480, 36]
                    }
                }),
            ),
            tier: ToolTier::T1,
        });
        app.assistant.status = "等待确认：set_component_fields".to_owned();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 520.0));
        let context = egui::Context::default();
        run_frames(
            &context,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            3,
            |context| {
                egui::CentralPanel::default().show(context, |ui| app.draw_assistant_panel(ui));
            },
        );
        let state = &app.assistant;
        let tolerance = 2.0;
        assert!(state.confirm_rect.is_positive(), "确认卡必须画出来");
        assert!(state.input_rect.is_positive(), "输入框必须画出来");
        for (label, rect) in [
            ("输入框", state.input_rect),
            ("发送按钮", state.send_rect),
            ("确认卡", state.confirm_rect),
        ] {
            assert!(
                rect.bottom() <= screen.bottom() + tolerance
                    && rect.top() >= screen.top() - tolerance,
                "{label} 越出面板：{rect:?} vs {screen:?}"
            );
        }
        assert!(
            state.confirm_rect.bottom() <= state.input_rect.top() + tolerance,
            "确认卡压住输入框：{:?} / {:?}",
            state.confirm_rect,
            state.input_rect
        );
        assert!(
            !state.input_rect.intersects(state.send_rect),
            "输入框与发送按钮重叠"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 发送按钮要够大，且标签在按钮里水平/垂直都居中（用户截图里的按钮又小又贴底）。
    #[test]
    fn the_send_button_is_large_and_its_label_is_centered() {
        let (mut app, root) = test_app("send-button");
        app.assistant.draft = "改一下界面适配".to_owned();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 520.0));
        let context = egui::Context::default();
        let output = run_frames(
            &context,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            3,
            |context| {
                egui::CentralPanel::default().show(context, |ui| app.draw_assistant_panel(ui));
            },
        );
        let send = app.assistant.send_rect;
        assert!(send.is_positive(), "发送按钮必须有矩形");
        assert!(
            send.height() >= 28.0 && send.width() >= 80.0,
            "发送按钮太小：{send:?}"
        );
        let label = text_rects(&output, "发送")
            .into_iter()
            .next()
            .expect("发送按钮的文字必须画出来");
        let offset = label.center() - send.center();
        assert!(
            offset.x.abs() <= 2.5 && offset.y.abs() <= 2.5,
            "发送标签没在按钮里居中：偏移 {offset:?}，按钮 {send:?}，文字 {label:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 代码块与工具 JSON 预览都钉满可用宽度：块与块的左右边界必须一致。
    #[test]
    fn code_and_json_blocks_share_the_same_width() {
        let (mut app, root) = test_app("block-width");
        app.assistant.transcript = vec![
            TranscriptEntry::new(
                EntryKind::Assistant,
                "示例：\n\n```lua\nlocal grid = ui.find(\"CardsGrid\")\ngrid.layout = \"grid\"\n```\n",
            ),
            TranscriptEntry::new(
                EntryKind::Tool,
                "✓ get_scene_tree\n{\n  \"dirty\": false,\n  \"entities\": [\n    1,\n    2,\n    3,\n    4,\n    5,\n    6,\n    7,\n    8\n  ]\n}".to_owned(),
            ),
        ];
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(480.0, 620.0));
        let context = egui::Context::default();
        let output = run_frames(
            &context,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            3,
            |context| {
                egui::CentralPanel::default().show(context, |ui| app.draw_assistant_panel(ui));
            },
        );
        let blocks = code_block_rects(
            &output,
            &context,
            app.assistant.panel_rect,
            app.assistant.input_rect,
        );
        assert_eq!(
            blocks.len(),
            2,
            "代码块与 JSON 块各一个，不能多也不能少：{blocks:?}"
        );
        let width = blocks[0].width();
        for rect in &blocks {
            assert!(
                (rect.width() - width).abs() <= 1.0,
                "代码区域宽度不一致：{rect:?} vs {blocks:?}"
            );
        }
        assert!(
            width >= app.assistant.panel_rect.width() - 80.0,
            "代码区域没有铺满可用宽度：{width} vs {:?}",
            app.assistant.panel_rect
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 长会话只画视口附近的条目：旧实现每帧重排全部记录，越长越卡。
    #[test]
    fn long_transcripts_only_draw_the_visible_window() {
        let (mut app, root) = test_app("virtual");
        app.assistant.transcript = (0..120)
            .map(|index| {
                TranscriptEntry::new(
                    EntryKind::Assistant,
                    format!("第 {index} 条回答：{}", "内容很长的一段说明。".repeat(30)),
                )
            })
            .collect();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 600.0));
        let context = egui::Context::default();
        // 首帧测高、次帧起才按视口跳过屏外条目。
        let output = run_frames(
            &context,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            4,
            |context| {
                egui::CentralPanel::default().show(context, |ui| app.draw_assistant_panel(ui));
            },
        );
        let text = collect_text(&output);
        let drawn = app.assistant.last_drawn_entries;
        assert!(drawn > 0, "视口里的条目必须画出来：{text}");
        assert!(
            drawn < 40,
            "屏外条目没有被跳过，一帧真实排版了 {drawn}/120 条"
        );
        assert!(
            text.contains("第 119 条回答"),
            "最新一条必须始终可见（stick_to_bottom）"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 连续画几帧并返回最后一帧：底部面板的高度沿用上一帧内容，单帧只是过渡态。
    fn run_frames(
        context: &egui::Context,
        raw: egui::RawInput,
        frames: usize,
        mut draw: impl FnMut(&egui::Context),
    ) -> egui::FullOutput {
        let mut last = None;
        for _ in 0..frames.max(1) {
            last = Some(context.run(raw.clone(), &mut draw));
        }
        last.expect("至少画一帧")
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

    fn text_rects(output: &egui::FullOutput, needle: &str) -> Vec<egui::Rect> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text_shape) if text_shape.galley.text() == needle => Some(
                    egui::Rect::from_min_size(text_shape.pos, text_shape.galley.size()),
                ),
                _ => None,
            })
            .collect()
    }

    /// 转录区里的代码块矩形：填充是 `extreme_bg_color`、无描边，且不与输入框重叠
    /// （输入框是同一填充色的整宽矩形，靠位置排除）。
    fn code_block_rects(
        output: &egui::FullOutput,
        context: &egui::Context,
        panel: egui::Rect,
        input: egui::Rect,
    ) -> Vec<egui::Rect> {
        let fill = context.style().visuals.extreme_bg_color;
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if rect.fill == fill
                        && rect.stroke.width == 0.0
                        && panel.contains_rect(rect.rect)
                        && !rect.rect.intersects(input) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect()
    }
}
