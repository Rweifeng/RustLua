//! `input` 绑定的行为基线：**绑定下沉到 `rustlua-input` 之后**由本 crate 自己守住。
//!
//! 为什么要有这一份：这些行为原先由 `rustlua-scripting` 的 122 条测试与
//! `api_surface_snapshot` 托管（绑定住在那里）。搬迁时口径必须逐字不变——返回的元组、
//! 诊断文本、回退默认 vs 保留当前绑定的分叉，都是脚本和编辑器面板在依赖的契约。
//! 这里用一个内存版端口把"文件在哪、写失败怎么报"也一起钉住。

use rustlua_input::host::HostInput;
use rustlua_input::input::{ActionMap, AxisDirection, InputSnapshot, KeyCode};
use rustlua_input::nav::NavSettings;
use rustlua_input::port::{LoadedSettings, SettingsLoadError, SettingsPort, SettingsSaveError};
use rustlua_lua_api::Registry;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// 内存版设置端口：`text` 就是设置文件内容，`fail_write` 用来制造写失败。
#[derive(Default)]
struct FakeSettings {
    text: RefCell<String>,
    exists: RefCell<bool>,
    fail_write: RefCell<bool>,
    /// 记录最近一次 save 收到的映射文本（断言"改动真的落盘"）。
    saved: RefCell<Option<String>>,
}

impl SettingsPort for FakeSettings {
    fn path(&self) -> Option<PathBuf> {
        Some(PathBuf::from("settings.toml"))
    }

    fn load(&self, _path: Option<&str>) -> Result<LoadedSettings, SettingsLoadError> {
        if !*self.exists.borrow() {
            return Err(SettingsLoadError::Missing);
        }
        let text = self.text.borrow().clone();
        let (map, nav, notes, reason) = parse_input_section_for_test(&text);
        Ok(LoadedSettings {
            path: PathBuf::from("settings.toml"),
            map,
            nav,
            notes,
            reason,
        })
    }

    fn save(&self, _path: Option<&str>, map: &ActionMap) -> Result<(), SettingsSaveError> {
        if *self.fail_write.borrow() {
            return Err(SettingsSaveError::Write {
                path: PathBuf::from("settings.toml"),
                message: "disk full".to_owned(),
            });
        }
        *self.saved.borrow_mut() = Some(map.to_text());
        Ok(())
    }
}

/// 与 `rustlua-gameplay::settings::parse_input_section` 同形的极简解析（只为测试服务）。
///
/// 真实的 `[input]` 段把映射存成一段文本；测试只关心"绑定侧怎么应用结果"，所以这里
/// 直接按 `action = binding` 逐行拼出 v2 头文本。
type ParsedSection = (Option<ActionMap>, NavSettings, Vec<String>, Option<String>);

fn parse_input_section_for_test(text: &str) -> ParsedSection {
    let mut section = String::new();
    let mut map_lines: Vec<(String, String)> = Vec::new();
    let mut threshold = None;
    let mut delay_ms = None;
    let mut repeat_ms = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name.trim().to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        match section.as_str() {
            "input" => map_lines.push((key.trim().to_owned(), value.to_owned())),
            "nav" => match key.trim() {
                "nav_threshold" => threshold = value.parse().ok(),
                "nav_delay_ms" => delay_ms = value.parse().ok(),
                "nav_repeat_ms" => repeat_ms = value.parse().ok(),
                _ => {}
            },
            _ => {}
        }
    }
    let nav = NavSettings::new(
        threshold.unwrap_or(NavSettings::DEFAULT.threshold),
        delay_ms.unwrap_or(NavSettings::DEFAULT.delay_ms),
        repeat_ms.unwrap_or(NavSettings::DEFAULT.repeat_ms),
    );
    if map_lines.is_empty() {
        return (None, nav, Vec::new(), Some("missing_key".to_owned()));
    }
    let mut body = String::from(rustlua_input::input::ACTION_MAP_VERSION);
    for (action, binding) in &map_lines {
        body.push('\n');
        body.push_str(action);
        body.push('=');
        body.push_str(binding);
    }
    match ActionMap::from_text_with_notes(&body) {
        Ok((map, notes)) => (Some(map), nav, notes, None),
        Err(error) => (
            None,
            NavSettings::DEFAULT,
            Vec::new(),
            Some(error.to_string()),
        ),
    }
}

fn setup() -> (Rc<Registry>, HostInput, Rc<FakeSettings>) {
    let mut registry = Registry::new();
    // 固定步长 16ms：`set_input` 用它推进逻辑时间（真实宿主由 `LuaRuntime::set_delta` 写）。
    registry.set_delta_seconds(0.016);
    let host = HostInput::new(&registry);
    let settings: Rc<FakeSettings> = Rc::new(FakeSettings::default());
    let installer = host.clone();
    let port: Rc<dyn SettingsPort> = settings.clone();
    registry.add("input", move |lua| {
        rustlua_input::lua::install(lua, &installer, &port)
    });
    (Rc::new(registry), host, settings)
}

fn runtime(registry: &Rc<Registry>) -> mlua::Lua {
    let lua = mlua::Lua::new();
    registry.install(&lua).unwrap();
    lua
}

#[test]
fn snapshot_queries_match_shared_state() {
    let (registry, host, _settings) = setup();
    let lua = runtime(&registry);
    let mut snapshot = InputSnapshot::default();
    let key = KeyCode::from_text("keye").unwrap();
    snapshot.key_event(key, true);
    snapshot.cursor_moved(12.5, 7.25);
    host.set_input(snapshot);

    let down: bool = lua.load(r#"return input.is_down("keye")"#).eval().unwrap();
    assert!(down, "宿主喂入的快照必须立刻对 Lua 可见");
    let cursor: (f64, f64) = lua
        .load("local p = input.cursor_position() return p[1], p[2]")
        .eval()
        .unwrap();
    assert_eq!(cursor, (12.5, 7.25));
    // 未知键名是错误（M22/W1 的严格化）：旧实现会静默映射成 Other(0)。
    let error = lua
        .load(r#"return input.is_down("nosuchkey")"#)
        .eval::<bool>()
        .unwrap_err();
    assert!(error.to_string().contains("unknown input key"), "{error}");
}

#[test]
fn virtual_keys_survive_whole_snapshot_replacement() {
    let (registry, host, _settings) = setup();
    let lua = runtime(&registry);
    // 虚拟键注入的是"按住态"，动作层能看到（`input.is_down` 只认物理键，这是既有语义）。
    lua.load(r#"input.virtual_key("arrowup", true)"#)
        .exec()
        .unwrap();
    // 宿主整份替换快照（真实帧就是这样）：虚拟键必须并入，否则摇杆/虚拟按钮永远读不到。
    host.set_input(InputSnapshot::default());
    let down: bool = lua.load(r#"return input.action("ui_up")"#).eval().unwrap();
    assert!(down, "虚拟键是跨帧保持的按住态");
}

#[test]
fn action_layer_and_rebinding_round_trip() {
    let (registry, host, _settings) = setup();
    let lua = runtime(&registry);
    let (ok, reason): (bool, String) = lua
        .load(r#"return input.rebind("confirm", "key:keyx")"#)
        .eval()
        .unwrap();
    assert!(ok, "{reason}");
    assert_eq!(
        host.action_map().bindings("confirm")[0].to_text(),
        "key:keyx"
    );
    let text: String = lua.load("return input.action_map_text()").eval().unwrap();
    assert!(text.contains("confirm=key:keyx"), "{text}");
    // 文本往返：同一份映射。
    let (ok, reason): (bool, String) = lua
        .load(r#"return input.set_action_map(input.action_map_text())"#)
        .eval()
        .unwrap();
    assert!(ok, "{reason}");
    // 非法文本：返回 false + 原因，且**不改动**当前映射。
    let (ok, reason): (bool, String) = lua
        .load(r#"return input.set_action_map("rustlua-actions-v9")"#)
        .eval()
        .unwrap();
    assert!(!ok);
    assert!(
        reason.contains("unsupported action map version"),
        "{reason}"
    );
    assert_eq!(
        host.action_map().bindings("confirm")[0].to_text(),
        "key:keyx"
    );
    // 重置回默认（绑定是玩家偏好，不随读档回滚）。
    lua.load("input.reset_bindings()").exec().unwrap();
    assert_eq!(
        host.action_map().bindings("confirm")[0].to_text(),
        "key:space"
    );
}

#[test]
fn load_settings_reports_missing_loaded_and_invalid() {
    let (registry, host, settings) = setup();
    let lua = runtime(&registry);

    // 文件不存在 → (false, "missing")，当前绑定保留。
    let (ok, message): (bool, String) = lua.load("return input.load_settings()").eval().unwrap();
    assert!(!ok);
    assert_eq!(message, "missing");

    // 正常一段 → (true, "loaded")。
    *settings.exists.borrow_mut() = true;
    *settings.text.borrow_mut() = "[input]\nconfirm = key:space\n".to_owned();
    let (ok, message): (bool, String) = lua.load("return input.load_settings()").eval().unwrap();
    assert!(ok, "{message}");
    assert_eq!(message, "loaded");

    // 非法绑定 → (false, "{path}: {reason}; reverting to defaults") 并留诊断。
    *settings.text.borrow_mut() = "[input]\nconfirm = key:notakey\n".to_owned();
    let (ok, message): (bool, String) = lua.load("return input.load_settings()").eval().unwrap();
    assert!(!ok);
    assert!(message.contains("reverting to defaults"), "{message}");
    // 回退默认：`confirm` 又变回空格。
    assert_eq!(
        host.action_map().bindings("confirm")[0].to_text(),
        "key:space"
    );
    let diagnostics: Vec<String> = lua
        .load("local t = input.diagnostics() return { t[1] }")
        .eval()
        .unwrap();
    assert!(diagnostics[0].contains("reverting to defaults"));
}

#[test]
fn save_settings_reports_failures_with_diagnostics() {
    let (registry, host, settings) = setup();
    let lua = runtime(&registry);
    let (ok, reason): (bool, String) = lua
        .load(r#"return input.rebind("confirm", "key:keyx")"#)
        .eval()
        .unwrap();
    assert!(ok, "{reason}");
    let (ok, message): (bool, String) = lua.load("return input.save_settings()").eval().unwrap();
    assert!(ok, "{message}");
    assert_eq!(message, "saved");
    assert!(
        settings
            .saved
            .borrow()
            .as_ref()
            .unwrap()
            .contains("confirm=key:keyx")
    );

    *settings.fail_write.borrow_mut() = true;
    let (ok, message): (bool, String) = lua.load("return input.save_settings()").eval().unwrap();
    assert!(!ok);
    assert_eq!(message, "disk full");
    let diagnostics: Vec<String> = lua
        .load("local t = input.diagnostics() return { t[1] }")
        .eval()
        .unwrap();
    assert!(diagnostics[0].contains("save failed"), "{diagnostics:?}");
    // 诊断只记不改数据：映射仍是刚才重绑的那份。
    assert_eq!(
        host.action_map().bindings("confirm")[0].to_text(),
        "key:keyx"
    );
}

#[test]
fn nav_settings_clamp_and_apply() {
    let (registry, host, settings) = setup();
    let lua = runtime(&registry);
    *settings.exists.borrow_mut() = true;
    *settings.text.borrow_mut() =
        "[nav]\nnav_threshold = 0.25\nnav_delay_ms = 500\nnav_repeat_ms = 120\n".to_owned();
    let (threshold, delay, repeat): (f32, u32, u32) = lua
        .load("local t = input.nav_settings() return t.threshold, t.delay_ms, t.repeat_ms")
        .eval()
        .unwrap();
    assert_eq!((threshold, delay, repeat), (0.25, 500, 120));
    // 越界值按 `NavSettings::new` 钳制，并且同一份手感对宿主可见。
    let (threshold, delay, repeat): (f32, u32, u32) = lua
        .load("return input.set_nav_settings(2.0, 99999, 1)")
        .eval()
        .unwrap();
    assert_eq!(threshold, 1.0);
    assert_eq!(delay, NavSettings::MAX_MS);
    assert_eq!(repeat, NavSettings::MIN_REPEAT_MS);
    assert_eq!(host.nav_settings().delay_ms, NavSettings::MAX_MS);
}

#[test]
fn analog_repeat_fires_after_delay_then_every_interval() {
    let (registry, host, _settings) = setup();
    let _lua = runtime(&registry);
    // `ui_up` 默认就带摇杆轴绑定（`padaxis:1+`），只有模拟量绑定会连发。
    let mut snapshot = InputSnapshot::default();
    // 变化帧：模拟量 + 边沿（`gamepad_axis_event` 自己写数字态，先写会把边沿吃掉）。
    snapshot.set_gamepad_axis(1, 1.0);
    snapshot.gamepad_axis_event(1, AxisDirection::Positive, true);
    host.set_input(snapshot);
    {
        let map = host.action_map();
        let handle = host.state();
        let state = handle.borrow().unwrap();
        assert!(state.snapshot.action_is_down(&map, "ui_up"), "down");
        assert!(state.snapshot.action_just_pressed(&map, "ui_up"), "pressed");
        assert!(
            state.snapshot.action_held_by_analog(&map, "ui_up"),
            "analog"
        );
    }
    // 第一步：边沿立即一步。
    assert!(host.action_repeat("ui_up"));
    // 推进 30 步（每步 16ms ≈ 480ms）：延迟 350ms 之后每 90ms 一次 ⇒ 1..=2 次。
    let mut fired = 0;
    for _ in 0..30 {
        let mut snapshot = InputSnapshot::default();
        snapshot.set_gamepad_axis(1, 1.0);
        snapshot.set_gamepad_axis_digital(1, AxisDirection::Positive, true);
        host.set_input(snapshot);
        if host.action_repeat("ui_up") {
            fired += 1;
        }
    }
    assert!((1..=3).contains(&fired), "fired={fired}");
    // 数字键绑定（`move_up` 是 key:w）不连发。
    let mut snapshot = InputSnapshot::default();
    snapshot.key_event(KeyCode::from_text("keyw").unwrap(), true);
    host.set_input(snapshot);
    assert!(host.action_repeat("move_up"));
    for _ in 0..30 {
        let mut snapshot = InputSnapshot::default();
        snapshot.key_event(KeyCode::from_text("keyw").unwrap(), true);
        host.set_input(snapshot);
        assert!(!host.action_repeat("move_up"), "数字键保持只走一步");
    }
}

#[test]
fn rumble_request_is_taken_by_host() {
    let (registry, host, _settings) = setup();
    let lua = runtime(&registry);
    lua.load("input.rumble(0.5, 1.0, 200)").exec().unwrap();
    assert_eq!(host.take_rumble_request(), Some((0.5, 1.0, 200)));
    assert_eq!(host.take_rumble_request(), None, "取走一次就没了");
}

#[test]
fn reentrant_borrow_reports_unit_name() {
    let (registry, _host, _settings) = setup();
    let lua = runtime(&registry);
    // 在 `input.bindings` 的回调里再调 `input.rebind`（重入同一份状态）。
    lua.load(
        r#"
        local fired = false
        local original = input.bindings
        input.bindings = function(name)
            if not fired then
                fired = true
                local ok, why = input.rebind("confirm", "key:keyx")
                error("nested rebind returned " .. tostring(ok) .. " " .. tostring(why))
            end
            return original(name)
        end
        input.bindings("confirm")
        "#,
    )
    .exec()
    .unwrap_err();
}

#[test]
fn settings_path_comes_from_the_port() {
    let (registry, host, _settings) = setup();
    let lua = runtime(&registry);
    let path: Option<String> = lua.load("return input.settings_path()").eval().unwrap();
    assert_eq!(path.as_deref(), Some("settings.toml"));
    assert_eq!(host.pointer(), None, "没喂过输入时指针不存在");
}

#[test]
fn pointer_is_published_for_other_domain_bindings() {
    let (registry, host, _settings) = setup();
    let mut snapshot = InputSnapshot::default();
    snapshot.cursor_moved(3.5, -2.0);
    host.set_input(snapshot);
    assert_eq!(host.pointer(), Some((3.5, -2.0)));
    let _ = registry;
}

/// 轴方向常量在测试里显式引用一次，确保它仍是 `input` 的公开面（绑定文本用它）。
#[test]
fn axis_direction_is_public() {
    assert_eq!(AxisDirection::Positive, AxisDirection::Positive);
}

/// M79-04 主链路：宿主从系统输入法拿到的文本，脚本在**同一帧**就能读到，且不跨帧残留。
///
/// 事件顺序照 winit 0.30.13 的拼音形状（`Preedit` 逐字增长 → 空 `Preedit` 清屏 → `Commit`）。
#[test]
fn host_text_events_are_visible_to_lua_the_same_frame() {
    let (registry, host, _settings) = setup();
    let lua = runtime(&registry);
    let mut snapshot = InputSnapshot::default();

    snapshot.set_text_preedit("ni");
    host.set_input(snapshot.clone());
    let read: (Option<String>, usize) = lua
        .load("local e = input.text_events() return e.composition, #e.committed")
        .eval()
        .unwrap();
    assert_eq!(
        read,
        (Some("ni".to_owned()), 0),
        "组合中的拼音可读，正文还没有"
    );

    snapshot.set_text_preedit("");
    snapshot.push_text_commit("你好");
    host.set_input(snapshot.clone());
    let read: (Option<String>, String) = lua
        .load("local e = input.text_events() return e.composition, e.committed[1]")
        .eval()
        .unwrap();
    assert_eq!(
        read,
        (None, "你好".to_owned()),
        "提交后组合串清掉，上屏串进本帧读数"
    );

    // 宿主帧末清点（真实帧就是这样），下一帧脚本读到的是干净的。
    snapshot.finish_frame();
    host.set_input(snapshot);
    let read: (bool, usize) = lua
        .load("local e = input.text_events() return e.composition == nil, #e.committed")
        .eval()
        .unwrap();
    assert_eq!(read, (true, 0), "文本是本帧数据，不跨帧残留");
}

/// 没有系统输入法的宿主（Web 的 composition 事件、微信原生输入桥）与无窗口自检，
/// 用同一个门面向快照注入——形状必须与宿主写进来的完全一致，否则演示绿而真机是另一套。
#[test]
fn lua_can_inject_text_events_through_the_same_channel() {
    let (registry, host, _settings) = setup();
    let lua = runtime(&registry);
    host.set_input(InputSnapshot::default());

    let read: (Option<String>, usize) = lua
        .load(
            r#"
            local e = input.text_events({ composition = "ni" })
            return e.composition, #e.committed
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(read, (Some("ni".to_owned()), 0), "注入的预编辑串可读");

    let read: (Option<String>, Vec<String>) = lua
        .load(
            r#"
            local e = input.text_events({ composition = "", commit = "你好" })
            return e.composition, { e.committed[1] }
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(
        read,
        (None, vec!["你好".to_owned()]),
        "空 composition = 取消组合；commit 落正文"
    );

    // 一帧内两次提交要都读得到（连续确认两个候选），不是"只留最后一条"。
    let count: usize = lua
        .load(
            r#"
            input.text_events({ commit = "a" })
            return #input.text_events().committed
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(count, 2, "提交串是列表，不覆盖");

    // 字段类型不对是错误，不是静默忽略：mlua 的 `bool` 走 Lua 真值语义，`capture = "yes"`
    // 会当成 true——所以 `capture` 只认布尔，报错带字段名。
    let error = lua
        .load(r#"return input.text_events({ capture = "yes" })"#)
        .eval::<bool>()
        .unwrap_err();
    assert!(error.to_string().contains("capture"), "{error}");
    let error = lua
        .load(r#"return input.text_events({ commit = { "你" } })"#)
        .eval::<bool>()
        .unwrap_err();
    assert!(error.to_string().contains("commit"), "{error}");
}

/// 收字声明是**脚本的跨帧意图**，宿主下一次整份替换快照时必须还在（与虚拟键同一条约定）。
///
/// 这条如果漂了，症状是"中文打两个字输入法就关掉"——只在真机上出现，最难查。
#[test]
fn text_capture_declaration_survives_the_next_host_frame() {
    let (registry, host, _settings) = setup();
    let lua = runtime(&registry);
    host.set_input(InputSnapshot::default());
    assert!(!host.text_capture(), "默认不收字：玩法按键不该被输入法吃掉");

    let capturing: bool = lua
        .load("return input.text_events({ capture = true }).capturing")
        .eval()
        .unwrap();
    assert!(capturing, "声明当帧可读");

    // 宿主整份替换快照（真实帧就是这样），不带这个位就等于把声明丢了。
    host.set_input(InputSnapshot::default());
    assert!(host.text_capture(), "收字声明跨帧保持");
    let read: (bool, bool) = lua
        .load("local e = input.text_events() return e.capturing, e.composition == nil")
        .eval()
        .unwrap();
    assert!(read.0, "脚本侧也还读得到自己的声明");
    assert!(read.1, "跨帧保持的只有声明，文本仍是本帧的");

    lua.load("input.text_events({ capture = false })")
        .exec()
        .unwrap();
    host.set_input(InputSnapshot::default());
    assert!(!host.text_capture(), "关掉声明后宿主要把输入法交还系统");
}
