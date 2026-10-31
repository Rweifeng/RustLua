//! `net` 表的 Lua 绑定：一条到中继服务器的长连接 + 二进制消息队列。
//!
//! 三条传输共用同一套 Lua 面（`connect`/`send`/`recv`/`status`），脚本只按地址前缀选路：
//! - 桌面裸 TCP（`tcp://host:port`，4 字节小端长度前缀）；
//! - 浏览器 WebSocket（`ws://host:port/ws`）；
//! - 桌面 UDP 中继（`net.connect_udp("udp://host:port", room, player)`，自带可靠有序
//!   与重传，见 [`rustlua_net::UdpTransport`]）。UDP 没有配对控制文本，连接成功即
//!   `online`；`net.send` 的字节被包进 `Message::Input` 传输，`net.recv` 原样还原。
//!
//! 服务器负责配对：房间满 2 人时下发纯文本 `you:<player>:<players>`，之后二进制消息原样互转。
//! 绑定只做三件事：把控制文本翻译成 `State`、把二进制帧推进队列、把 `net.send` 写出去。
//!
//! 状态机（`net.status().state`）：
//! `idle` → `connecting` → `waiting`（已连上、等对手）→ `online`（已配对）
//! 任何一步失败进 `error`，对端离开或本地关闭进 `closed`。

use mlua::{Lua, Result as LuaResult};
use rustlua_lua_api::Handle;
#[cfg(not(target_arch = "wasm32"))]
use rustlua_net::Transport as _;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// 与控制文本同口径的房间名（服务器侧同一正则）。
fn valid_room(room: &str) -> bool {
    (1..=32).contains(&room.len())
        && room
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum State {
    #[default]
    Idle,
    Connecting,
    Waiting,
    Online,
    Closed,
    Error,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Connecting => "connecting",
            Self::Waiting => "waiting",
            Self::Online => "online",
            Self::Closed => "closed",
            Self::Error => "error",
        }
    }
}

/// 回调（wasm 的 JS 事件线程）与 Lua 调用面共享的那部分状态。
#[derive(Default)]
struct Shared {
    state: State,
    player: Option<u8>,
    players: Option<u8>,
    error: Option<String>,
    queue: VecDeque<Vec<u8>>,
}

impl Shared {
    /// 处理服务器控制文本；返回 `true` 表示连接已无法继续。
    fn apply_control(&mut self, text: &str) -> bool {
        if let Some(rest) = text.strip_prefix("you:") {
            let mut parts = rest.split(':');
            if let (Some(player), Some(players), None) = (parts.next(), parts.next(), parts.next())
                && let (Ok(player), Ok(players)) = (player.parse::<u8>(), players.parse::<u8>())
            {
                self.player = Some(player);
                self.players = Some(players);
                self.state = State::Online;
                return false;
            }
            self.error = Some(format!("bad assignment from relay: {text}"));
            self.state = State::Error;
            return true;
        }
        match text {
            "full" => {
                self.error = Some("room is full".to_owned());
                self.state = State::Error;
                true
            }
            "peer-left" => {
                self.error = Some("peer left".to_owned());
                self.state = State::Closed;
                true
            }
            _ => {
                if let Some(message) = text.strip_prefix("err:") {
                    self.error = Some(message.to_owned());
                    self.state = State::Error;
                    return true;
                }
                false
            }
        }
    }
}

/// 注册表捕获的网络状态（热重载后同一份状态重新装回）。
pub struct NetState {
    shared: Rc<RefCell<Shared>>,
    server: Option<String>,
    room: Option<String>,
    transport: &'static str,
    #[cfg(not(target_arch = "wasm32"))]
    native: Option<native::Conn>,
    #[cfg(not(target_arch = "wasm32"))]
    udp: Option<rustlua_net::UdpTransport>,
    #[cfg(not(target_arch = "wasm32"))]
    udp_player: u8,
    #[cfg(not(target_arch = "wasm32"))]
    udp_frame: u32,
    #[cfg(target_arch = "wasm32")]
    web: Option<web::Conn>,
}

impl Default for NetState {
    fn default() -> Self {
        Self {
            shared: Rc::new(RefCell::new(Shared::default())),
            server: None,
            room: None,
            transport: "none",
            #[cfg(not(target_arch = "wasm32"))]
            native: None,
            #[cfg(not(target_arch = "wasm32"))]
            udp: None,
            #[cfg(not(target_arch = "wasm32"))]
            udp_player: 0,
            #[cfg(not(target_arch = "wasm32"))]
            udp_frame: 0,
            #[cfg(target_arch = "wasm32")]
            web: None,
        }
    }
}

impl NetState {
    fn connect(&mut self, server: &str, room: &str) -> Result<(), String> {
        self.close();
        if !valid_room(room) {
            return Err(format!(
                "room must be 1-32 chars of [A-Za-z0-9_-] (got {room:?})"
            ));
        }
        {
            let mut shared = self.shared.borrow_mut();
            shared.state = State::Connecting;
            shared.player = None;
            shared.players = None;
            shared.error = None;
            shared.queue.clear();
        }
        self.server = Some(server.to_owned());
        self.room = Some(room.to_owned());
        #[cfg(not(target_arch = "wasm32"))]
        let result = {
            self.transport = "tcp";
            self.connect_native(server, room)
        };
        #[cfg(target_arch = "wasm32")]
        let result = {
            self.transport = "ws";
            self.connect_web(server, room)
        };
        if let Err(error) = result {
            self.fail(&error);
            return Err(error);
        }
        Ok(())
    }

    /// UDP 中继（仅桌面）：自带可靠有序与重传，没有配对控制文本，
    /// 加入房间成功即可收发；`player` 是本机在帧同步里的编号。
    #[cfg(not(target_arch = "wasm32"))]
    fn connect_udp(&mut self, server: &str, room: &str, player: u8) -> Result<(), String> {
        self.close();
        if !valid_room(room) {
            return Err(format!(
                "room must be 1-32 chars of [A-Za-z0-9_-] (got {room:?})"
            ));
        }
        let address = server
            .trim()
            .strip_prefix("udp://")
            .unwrap_or(server.trim())
            .to_owned();
        let transport = rustlua_net::UdpTransport::connect(address.as_str(), room, player)
            .map_err(|error| format!("cannot join udp room {room:?} at {address}: {error}"))?;
        {
            let mut shared = self.shared.borrow_mut();
            shared.state = State::Online;
            shared.player = Some(player);
            shared.players = Some(2);
            shared.error = None;
            shared.queue.clear();
        }
        self.server = Some(server.to_owned());
        self.room = Some(room.to_owned());
        self.transport = "udp";
        self.udp = Some(transport);
        self.udp_player = player;
        self.udp_frame = 0;
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    fn connect_udp(&mut self, _server: &str, _room: &str, _player: u8) -> Result<(), String> {
        Err("udp is only available on desktop builds".to_owned())
    }

    fn fail(&mut self, message: &str) {
        self.close();
        let mut shared = self.shared.borrow_mut();
        shared.state = State::Error;
        shared.error = Some(message.to_owned());
    }

    fn close(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(conn) = self.native.take() {
            conn.shutdown();
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.udp = None;
        }
        #[cfg(target_arch = "wasm32")]
        if let Some(conn) = self.web.take() {
            conn.shutdown();
        }
        let mut shared = self.shared.borrow_mut();
        if matches!(
            shared.state,
            State::Connecting | State::Waiting | State::Online
        ) {
            shared.state = State::Closed;
        }
    }

    /// 把平台事件搬进共享状态（wasm 侧回调已经直接写入，这里是空操作）。
    fn pump(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // UDP 的 `poll` 同时收包、按序交付与超时重传；每帧调一次即可。
            if let Some(udp) = &mut self.udp {
                let polled = udp.poll();
                match polled {
                    Ok(messages) => {
                        let mut shared = self.shared.borrow_mut();
                        for (_, message) in messages {
                            if let rustlua_net::Message::Input { payload, .. } = message {
                                shared.queue.push_back(payload);
                            }
                        }
                    }
                    Err(error) => self.fail(&error.to_string()),
                }
            }
            let mut events = Vec::new();
            if let Some(conn) = &self.native {
                while let Ok(event) = conn.events.try_recv() {
                    events.push(event);
                }
            }
            let mut fatal = false;
            for event in events {
                let mut shared = self.shared.borrow_mut();
                match event {
                    native::Event::Data(bytes) => shared.queue.push_back(bytes),
                    native::Event::Control(text) => fatal |= shared.apply_control(&text),
                    native::Event::Error(message) => {
                        shared.error = Some(message);
                        shared.state = State::Error;
                        fatal = true;
                    }
                    native::Event::Closed => {
                        if shared.state != State::Error {
                            shared.state = State::Closed;
                        }
                        fatal = true;
                    }
                }
            }
            if fatal {
                self.close();
            }
        }
    }

    fn send(&mut self, bytes: &[u8]) -> bool {
        if self.shared.borrow().state != State::Online {
            return false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.udp.is_some() {
            use rustlua_net::Transport as _;
            let message = rustlua_net::Message::Input {
                player: self.udp_player,
                frame: self.udp_frame,
                payload: bytes.to_vec(),
            };
            let result = self
                .udp
                .as_mut()
                .expect("checked above")
                .send(None, &message)
                .map_err(|error| error.to_string());
            return match result {
                Ok(()) => {
                    self.udp_frame = self.udp_frame.wrapping_add(1);
                    true
                }
                Err(error) => {
                    self.fail(&error);
                    false
                }
            };
        }
        #[cfg(not(target_arch = "wasm32"))]
        let result = match &mut self.native {
            Some(conn) => conn.send(bytes),
            None => Err("not connected".to_owned()),
        };
        #[cfg(target_arch = "wasm32")]
        let result = match &self.web {
            Some(conn) => conn.send(bytes),
            None => Err("not connected".to_owned()),
        };
        match result {
            Ok(()) => true,
            Err(error) => {
                self.fail(&error);
                false
            }
        }
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        self.pump();
        self.shared.borrow_mut().queue.pop_front()
    }

    fn status(&mut self) -> (State, Option<u8>, Option<u8>, Option<String>, usize) {
        self.pump();
        let shared = self.shared.borrow();
        (
            shared.state,
            shared.player,
            shared.players,
            shared.error.clone(),
            shared.queue.len(),
        )
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io::{Read, Write};
    use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::Duration;

    const MAX_FRAME: usize = 64 * 1024;

    pub enum Event {
        Data(Vec<u8>),
        Control(String),
        Error(String),
        Closed,
    }

    pub struct Conn {
        writer: TcpStream,
        pub events: Receiver<Event>,
    }

    impl Conn {
        pub fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
            let mut header = [0u8; 4];
            header.copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            self.writer
                .write_all(&header)
                .and_then(|()| self.writer.write_all(bytes))
                .map_err(|error| format!("tcp write failed: {error}"))
        }

        pub fn shutdown(&self) {
            let _ = self.writer.shutdown(Shutdown::Both);
        }
    }

    fn resolve(server: &str) -> Result<SocketAddr, String> {
        let text = server
            .trim()
            .strip_prefix("tcp://")
            .unwrap_or(server.trim());
        text.to_socket_addrs()
            .map_err(|error| format!("bad tcp server {server:?}: {error}"))?
            .next()
            .ok_or_else(|| format!("tcp server {server:?} resolved to nothing"))
    }

    impl super::NetState {
        pub(super) fn connect_native(&mut self, server: &str, room: &str) -> Result<(), String> {
            let address = resolve(server)?;
            let stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))
                .map_err(|error| format!("cannot connect to {server}: {error}"))?;
            stream
                .set_nodelay(true)
                .map_err(|error| format!("tcp nodelay failed: {error}"))?;
            let mut writer = stream
                .try_clone()
                .map_err(|error| format!("tcp clone failed: {error}"))?;
            writer
                .set_write_timeout(Some(Duration::from_secs(2)))
                .map_err(|error| format!("tcp write timeout failed: {error}"))?;
            let join = format!("join:{room}");
            let mut header = [0u8; 4];
            header.copy_from_slice(&(join.len() as u32).to_le_bytes());
            writer
                .write_all(&header)
                .and_then(|()| writer.write_all(join.as_bytes()))
                .map_err(|error| format!("cannot send join: {error}"))?;

            let (sender, events) = mpsc::channel();
            thread::Builder::new()
                .name("rustlua-net-lua".to_owned())
                .spawn(move || {
                    let mut reader = stream;
                    loop {
                        let mut header = [0u8; 4];
                        if reader.read_exact(&mut header).is_err() {
                            let _ = sender.send(Event::Closed);
                            break;
                        }
                        let length = u32::from_le_bytes(header) as usize;
                        if length == 0 || length > MAX_FRAME {
                            let _ = sender.send(Event::Error(format!(
                                "relay frame length out of range: {length}"
                            )));
                            break;
                        }
                        let mut body = vec![0u8; length];
                        if reader.read_exact(&mut body).is_err() {
                            let _ = sender.send(Event::Closed);
                            break;
                        }
                        // 控制文本全是可打印 ASCII；应用层 tag 1/2 是控制字符。
                        let event = if body.first().is_some_and(|byte| *byte >= 0x20) {
                            Event::Control(String::from_utf8_lossy(&body).into_owned())
                        } else {
                            Event::Data(body)
                        };
                        if sender.send(event).is_err() {
                            break;
                        }
                    }
                })
                .map_err(|error| format!("cannot start net reader: {error}"))?;

            self.native = Some(Conn { writer, events });
            self.shared.borrow_mut().state = super::State::Waiting;
            Ok(())
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

    pub struct Conn {
        socket: WebSocket,
        _open: Closure<dyn FnMut(Event)>,
        _message: Closure<dyn FnMut(MessageEvent)>,
        _error: Closure<dyn FnMut(Event)>,
        _close: Closure<dyn FnMut(CloseEvent)>,
    }

    impl Conn {
        pub fn send(&self, bytes: &[u8]) -> Result<(), String> {
            self.socket
                .send_with_u8_array(bytes)
                .map_err(|_| "websocket send failed".to_owned())
        }

        pub fn shutdown(&self) {
            let _ = self.socket.close();
            self.socket.set_onopen(None);
            self.socket.set_onmessage(None);
            self.socket.set_onerror(None);
            self.socket.set_onclose(None);
        }
    }

    impl super::NetState {
        pub(super) fn connect_web(&mut self, server: &str, room: &str) -> Result<(), String> {
            let url = super::web_socket_url(server, room)?;
            let socket =
                WebSocket::new(&url).map_err(|_| format!("cannot open websocket {url}"))?;
            socket.set_binary_type(BinaryType::Arraybuffer);
            let shared = self.shared.clone();

            let open_shared = shared.clone();
            let open = Closure::wrap(Box::new(move |_event: Event| {
                let mut shared = open_shared.borrow_mut();
                if shared.state == super::State::Connecting {
                    shared.state = super::State::Waiting;
                }
            }) as Box<dyn FnMut(Event)>);

            let message_shared = shared.clone();
            let message_socket = socket.clone();
            let message = Closure::wrap(Box::new(move |event: MessageEvent| {
                let data = event.data();
                if let Some(text) = data.as_string() {
                    let mut shared = message_shared.borrow_mut();
                    if shared.apply_control(&text) {
                        drop(shared);
                        let _ = message_socket.close();
                    }
                    return;
                }
                if let Some(buffer) = data.dyn_ref::<js_sys::ArrayBuffer>() {
                    let bytes = js_sys::Uint8Array::new(buffer).to_vec();
                    message_shared.borrow_mut().queue.push_back(bytes);
                }
            }) as Box<dyn FnMut(MessageEvent)>);

            let error_shared = shared.clone();
            let error = Closure::wrap(Box::new(move |_event: Event| {
                let mut shared = error_shared.borrow_mut();
                if !matches!(shared.state, super::State::Closed | super::State::Error) {
                    shared.state = super::State::Error;
                    shared.error = Some("websocket error".to_owned());
                }
            }) as Box<dyn FnMut(Event)>);

            let close_shared = shared.clone();
            let close = Closure::wrap(Box::new(move |_event: CloseEvent| {
                let mut shared = close_shared.borrow_mut();
                if !matches!(shared.state, super::State::Closed | super::State::Error) {
                    shared.state = super::State::Closed;
                }
            }) as Box<dyn FnMut(CloseEvent)>);

            socket.set_onopen(Some(open.as_ref().unchecked_ref()));
            socket.set_onmessage(Some(message.as_ref().unchecked_ref()));
            socket.set_onerror(Some(error.as_ref().unchecked_ref()));
            socket.set_onclose(Some(close.as_ref().unchecked_ref()));

            self.web = Some(Conn {
                socket,
                _open: open,
                _message: message,
                _error: error,
                _close: close,
            });
            self.shared.borrow_mut().state = super::State::Connecting;
            Ok(())
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn web_socket_url(server: &str, room: &str) -> Result<String, String> {
    let base = server.trim();
    if !(base.starts_with("ws://") || base.starts_with("wss://")) {
        return Err(format!(
            "web net server must be ws:// or wss:// (got {base:?})"
        ));
    }
    let separator = if base.contains('?') { '&' } else { '?' };
    Ok(format!("{base}{separator}room={room}"))
}

#[cfg(target_arch = "wasm32")]
fn query_param(name: &str) -> Option<String> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get(name).filter(|value| !value.is_empty())
}

fn default_server() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("RUSTLUA_NET_SERVER").unwrap_or_else(|_| "tcp://127.0.0.1:8787".to_owned())
    }
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(server) = query_param("net_server") {
            return server;
        }
        match web_sys::window() {
            Some(window) => {
                let location = window.location();
                let scheme = if location.protocol().unwrap_or_default() == "https:" {
                    "wss"
                } else {
                    "ws"
                };
                let host = location
                    .hostname()
                    .unwrap_or_else(|_| "127.0.0.1".to_owned());
                format!("{scheme}://{host}:8787/ws")
            }
            None => "ws://127.0.0.1:8787/ws".to_owned(),
        }
    }
}

fn default_room() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("RUSTLUA_NET_ROOM").unwrap_or_else(|_| "royale".to_owned())
    }
    #[cfg(target_arch = "wasm32")]
    {
        query_param("room").unwrap_or_else(|| "royale".to_owned())
    }
}

fn auto_start() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("RUSTLUA_NET_AUTO")
            .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false)
    }
    #[cfg(target_arch = "wasm32")]
    {
        query_param("net_auto")
            .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false)
    }
}

/// 建一份网络状态能力单元（注册表捕获同一份）。
#[must_use]
pub fn state() -> Handle<NetState> {
    Handle::new(NetState::default(), "net")
}

/// 装 `net` 全局表。
pub fn install(lua: &Lua, state: &Handle<NetState>) -> LuaResult<()> {
    let net = lua.create_table()?;

    let s = state.clone();
    net.set(
        "connect",
        lua.create_function(move |_, (server, room): (String, String)| {
            Ok(s.borrow_mut()?.connect(&server, &room).is_ok())
        })?,
    )?;

    let s = state.clone();
    net.set(
        "connect_udp",
        lua.create_function(move |_, (server, room, player): (String, String, u8)| {
            Ok(s.borrow_mut()?.connect_udp(&server, &room, player).is_ok())
        })?,
    )?;

    let s = state.clone();
    net.set(
        "status",
        lua.create_function(move |lua, ()| {
            let mut state = s.borrow_mut()?;
            let (status, player, players, error, pending) = state.status();
            let transport = state.transport;
            #[cfg(target_arch = "wasm32")]
            let stats = None;
            #[cfg(not(target_arch = "wasm32"))]
            let stats = state.udp.as_ref().map(|udp| udp.stats());
            let table = lua.create_table()?;
            table.set("state", status.name())?;
            table.set("player", player)?;
            table.set("players", players)?;
            table.set("error", error)?;
            table.set("pending", pending)?;
            table.set("server", state.server.clone())?;
            table.set("room", state.room.clone())?;
            table.set("transport", transport)?;
            if let Some(stats) = stats {
                let udp = lua.create_table()?;
                udp.set("sent", stats.sent)?;
                udp.set("received", stats.received)?;
                udp.set("duplicate", stats.duplicate)?;
                udp.set("malformed", stats.malformed)?;
                udp.set("retransmits", stats.retransmits)?;
                udp.set("delivered", stats.delivered)?;
                table.set("udp", udp)?;
            }
            Ok(table)
        })?,
    )?;

    let s = state.clone();
    net.set(
        "send",
        lua.create_function(move |_, bytes: mlua::String| {
            Ok(s.borrow_mut()?.send(lua_string_bytes(&bytes).as_slice()))
        })?,
    )?;

    let s = state.clone();
    net.set(
        "recv",
        lua.create_function(move |lua, ()| {
            let Some(bytes) = s.borrow_mut()?.recv() else {
                return Ok(mlua::Value::Nil);
            };
            Ok(mlua::Value::String(lua.create_string(&bytes)?))
        })?,
    )?;

    let s = state.clone();
    net.set(
        "close",
        lua.create_function(move |_, ()| {
            s.borrow_mut()?.close();
            Ok(())
        })?,
    )?;

    net.set(
        "default_server",
        lua.create_function(move |_, ()| Ok(default_server()))?,
    )?;
    net.set(
        "default_room",
        lua.create_function(move |_, ()| Ok(default_room()))?,
    )?;
    net.set(
        "auto_start",
        lua.create_function(move |_, ()| Ok(auto_start()))?,
    )?;
    net.set("now", lua.create_function(move |_, ()| Ok(now_ms()))?)?;

    lua.globals().set("net", net)?;
    Ok(())
}

/// 单调毫秒时钟：脚本用它给 ping 打时间戳，算 RTT。
///
/// 不能用 `time.delta()` 累加：它是固定的逻辑步长，渲染掉到 30fps 时每帧仍只加 1/60，
/// 累出来的"时钟"只有真实时间的一半，量到的 RTT 会偏小一半。
fn now_ms() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::OnceLock;
        use std::time::Instant;
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
    }
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.performance())
            .map(|performance| performance.now())
            .unwrap_or(0.0)
    }
}

fn lua_string_bytes(value: &mlua::String) -> Vec<u8> {
    value.as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_names_match_the_relay_grammar() {
        assert!(valid_room("royale"));
        assert!(valid_room("Room_1-a"));
        assert!(!valid_room(""));
        assert!(!valid_room("a b"));
        assert!(!valid_room(&"x".repeat(33)));
    }

    #[test]
    fn clock_is_monotonic_and_milliseconds() {
        let start = now_ms();
        std::thread::sleep(std::time::Duration::from_millis(12));
        let later = now_ms();
        assert!(start >= 0.0);
        let elapsed = later - start;
        assert!(
            (8.0..200.0).contains(&elapsed),
            "expected ~12ms elapsed, measured {elapsed}"
        );
    }

    #[test]
    fn control_text_drives_the_state_machine() {
        let mut shared = Shared::default();
        assert!(!shared.apply_control("you:1:2"));
        assert_eq!(shared.state, State::Online);
        assert_eq!(shared.player, Some(1));
        assert_eq!(shared.players, Some(2));

        assert!(shared.apply_control("peer-left"));
        assert_eq!(shared.state, State::Closed);
        assert_eq!(shared.error.as_deref(), Some("peer left"));

        let mut full = Shared::default();
        assert!(full.apply_control("full"));
        assert_eq!(full.state, State::Error);
        assert_eq!(full.error.as_deref(), Some("room is full"));

        assert!(shared.apply_control("err:bad-join"));
        assert_eq!(shared.state, State::Error);
        assert_eq!(shared.error.as_deref(), Some("bad-join"));
    }

    /// 本机起一个假中继，验证原生 TCP 的 join → 状态 → 收发 → 关闭链路。
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_round_trip_against_a_scripted_relay() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::{Duration, Instant};

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr");
        let relay = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            let mut header = [0u8; 4];
            socket.read_exact(&mut header).expect("join header");
            let length = u32::from_le_bytes(header) as usize;
            let mut join = vec![0u8; length];
            socket.read_exact(&mut join).expect("join body");
            assert_eq!(join, b"join:royale");
            for body in [b"you:0:2".as_slice(), &[1, 5, 0, 0, 0, 9]] {
                socket
                    .write_all(&(body.len() as u32).to_le_bytes())
                    .and_then(|()| socket.write_all(body))
                    .expect("write");
            }
            // 等客户端发一帧回来再关。
            socket.read_exact(&mut header).expect("client header");
            let length = u32::from_le_bytes(header) as usize;
            let mut body = vec![0u8; length];
            socket.read_exact(&mut body).expect("client body");
            assert_eq!(body, [2, 9, 0, 0, 0, 7]);
            let left = b"peer-left";
            let _ = socket.write_all(&(left.len() as u32).to_le_bytes());
            socket.write_all(left).expect("close text");
        });

        let mut state = NetState::default();
        state
            .connect(&format!("tcp://{address}"), "royale")
            .expect("connect");

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if state.status().0 == State::Online {
                break;
            }
            assert!(Instant::now() < deadline, "never paired");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(state.status().1, Some(0));
        assert_eq!(state.recv().as_deref(), Some(&[1u8, 5, 0, 0, 0, 9][..]));
        assert!(state.send(&[2, 9, 0, 0, 0, 7]));

        loop {
            if state.status().0 == State::Closed {
                break;
            }
            assert!(Instant::now() < deadline, "never closed");
            std::thread::sleep(Duration::from_millis(2));
        }
        relay.join().expect("relay thread");
    }

    /// 本机起一个假 UDP 中继：验证 `net.connect_udp` 的 join、收发与透传口径。
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn udp_round_trip_against_a_scripted_relay() {
        use std::net::UdpSocket;
        use std::time::{Duration, Instant};

        let relay = UdpSocket::bind("127.0.0.1:0").expect("bind");
        relay
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let address = relay.local_addr().expect("addr");
        let handle = std::thread::spawn(move || {
            let mut buffer = [0u8; 2048];
            let (_, peer) = relay.recv_from(&mut buffer).expect("join");
            assert_eq!(&buffer[0..4], b"RLU1");
            assert_eq!(buffer[4], 1, "first datagram must be the join");
            let len = u16::from_le_bytes([buffer[21], buffer[22]]) as usize;
            assert_eq!(&buffer[23..23 + len], b"\x05arena\x00");

            // 回一条 `Message::Input{player:1, payload:"hello"}`。
            let payload = rustlua_net::Message::Input {
                player: 1,
                frame: 0,
                payload: b"hello".to_vec(),
            }
            .encode();
            let mut datagram = vec![0u8; 23];
            datagram[0..4].copy_from_slice(b"RLU1");
            datagram[4] = 2;
            datagram[5..9].copy_from_slice(&1u32.to_le_bytes());
            datagram[9..13].copy_from_slice(&0u32.to_le_bytes());
            datagram[13..21].copy_from_slice(&0u64.to_le_bytes());
            datagram[21..23].copy_from_slice(&(payload.len() as u16).to_le_bytes());
            datagram.extend_from_slice(&payload);
            relay.send_to(&datagram, peer).expect("reply");

            // 客户端发来的 `net.send("move")` 应被包成 Input 且负载逐字节一致。
            // 期间可能先收到纯 ACK（头 23 字节、负载为空），跳过——它和那条数据报谁先到
            // 取决于线程调度，所以判「空负载」而不是判顺序。
            loop {
                let (size, _) = relay.recv_from(&mut buffer).expect("data");
                assert!(size >= 23, "short datagram: {size}");
                assert_eq!(buffer[4], 2, "expected a data datagram");
                let len = u16::from_le_bytes([buffer[21], buffer[22]]) as usize;
                assert_eq!(size, 23 + len, "payload length must match header");
                if len == 0 {
                    continue;
                }
                let message =
                    rustlua_net::Message::decode(&buffer[23..23 + len]).expect("decode client");
                match message {
                    rustlua_net::Message::Input {
                        player,
                        frame,
                        payload,
                    } => {
                        assert_eq!(player, 0);
                        assert_eq!(frame, 0);
                        assert_eq!(payload, b"move");
                    }
                    other => panic!("unexpected client message: {other:?}"),
                }
                break;
            }
        });

        let mut state = NetState::default();
        state
            .connect_udp(&format!("udp://{address}"), "arena", 0)
            .expect("connect_udp");
        let (status, player, players, _, _) = state.status();
        assert_eq!(status, State::Online);
        assert_eq!(player, Some(0));
        assert_eq!(players, Some(2));
        assert!(state.send(b"move"));

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(bytes) = state.recv() {
                assert_eq!(bytes, b"hello");
                break;
            }
            assert!(Instant::now() < deadline, "never received the relay reply");
            std::thread::sleep(Duration::from_millis(2));
        }
        handle.join().expect("relay thread");
    }
}
