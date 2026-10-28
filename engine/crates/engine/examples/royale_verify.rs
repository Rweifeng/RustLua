//! ClashRoyale 工程的端到端验证：3D 场景 + 导航寻路 + 确定性与真实 TCP 帧同步。
//!
//! 三段证据（都会打印真实读数，任何一段不满足即 panic）：
//!
//! 1. **场景模式**：两个互相独立的 `RuntimeScene` 各跑 900 个固定步（AI vs AI）。
//!    验证场景加载/脚本无错、HUD 有绘制、3D 视口里塔与上阵单位可见、两个 VM 的逐帧
//!    状态校验和完全一致（说明 `battle.lua` 的整数状态机 + `nav` 寻路是确定性的）。
//! 2. **联网模式**：两个裸 Lua VM 各装一份 `nav` 与 `battle.lua`，经 127.0.0.1 的真实
//!    TCP 用 `rustlua-net` 的帧同步会话交换输入与校验和，跑 600 帧后要求两侧逐帧
//!    checksum 完全相等且无 desync。
//! 3. **竖屏 UI 交互**：用真实 `InputSnapshot` 注入鼠标按下/抬起，先点第 4 张手牌，
//!    再点屏幕下方（己方半场）——要求选中槽位生效、`camera3d.pick_plane` 把点击落到
//!    点击处附近（而不是固定出生点）、手牌轮转、圣水扣费。
//!
//! 运行：
//!
//! ```text
//! cargo run -p engine --example royale_verify -- E:/Qoder/ClashRoyale
//! ```
//!
//! 工程路径也可用环境变量 `ROYALE_PROJECT` 指定。

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::time::Duration;

use engine::host::RuntimeScene;
use mlua::{Function, Lua, Table};
use rustlua_camera::camera::Viewport;
use rustlua_input::input::{InputSnapshot, MouseButton};
use rustlua_net::{Desync, Session, SessionConfig, TcpTransport};
use rustlua_scene::scene::Scene;

const SCENE_STEPS: u32 = 900;
const NET_FRAMES: u32 = 600;
const NET_DELAY: u32 = 2;
const SCREEN_W: u32 = 720;
const SCREEN_H: u32 = 1280;

fn project_root() -> PathBuf {
    std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("ROYALE_PROJECT").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("E:/Qoder/ClashRoyale"))
}

fn main() {
    let root = project_root();
    assert!(
        root.join("project.toml").is_file(),
        "项目不存在：{}（第一个参数或 ROYALE_PROJECT 指向 ClashRoyale 工程）",
        root.display()
    );

    let first = run_scene(&root);
    let second = run_scene(&root);
    assert_scene_report(&first, &second);
    println!(
        "scene ok: frame={} spawned={} moved={} damaged_towers={} checksum={} drawables={} visible_units_peak={} ui_draws={} hud_glyphs={} fov_y={:.3}",
        first.stats["frame"],
        first.stats["spawned"],
        first.stats["moved"],
        first.stats["damaged_towers"],
        first.stats["checksum"],
        first.drawables,
        first.visible_units_peak,
        first.ui_draws,
        first.hud_glyphs,
        first.camera_fov,
    );

    let net = run_network(&root);
    assert_network_report(&net);
    println!(
        "net ok: frames={} tcp_checksums={} spawned={} units={} first={} last={}",
        net.frames,
        net.host_logs.len(),
        net.spawned,
        net.units,
        net.host_logs.first().copied().unwrap_or(0),
        net.host_logs.last().copied().unwrap_or(0),
    );

    let ui = run_interaction(&root);
    assert_interaction(&ui);
    println!(
        "ui ok: selected={} played={} hand_before={:?} hand_after={:?} next_before={} next_after={} player_z={:.2} elixir {:.1}->{:.1} spawned {}->{}",
        ui.selected,
        ui.played,
        ui.hand_before,
        ui.hand_after,
        ui.next_before,
        ui.next_after,
        ui.player_z,
        ui.elixir_before,
        ui.elixir_after,
        ui.spawned_before,
        ui.spawned_after,
    );
}

struct SceneReport {
    stats: serde_json::Value,
    drawables: usize,
    visible_units_peak: usize,
    ui_draws: usize,
    hud_glyphs: usize,
    camera_fov: f32,
}

fn run_scene(root: &Path) -> SceneReport {
    let scene = Scene::load(root.join("scenes").join("battle.scene")).expect("加载 battle.scene");
    let mut runtime = RuntimeScene::from_scene(Some(root), scene).expect("实例化 ClashRoyale 场景");
    runtime
        .with_script(|script| script.eval_json("__royale_autoplay(true) or true"))
        .expect("开启 AI 自走");
    let input = InputSnapshot::default();
    let viewport = Viewport::new(SCREEN_W as f32, SCREEN_H as f32).expect("竖屏 viewport");
    let mut drawables = 0;
    let mut visible_units_peak = 0;
    for step in 0..SCENE_STEPS {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .unwrap_or_else(|error| panic!("第 {step} 步失败：{error}"));
        if step % 60 == 0 {
            let frame = runtime.scene_3d(viewport).expect("3D 视图");
            drawables = drawables.max(frame.drawables.len());
            let slots = frame
                .drawables
                .iter()
                .filter(|drawable| {
                    drawable.name.starts_with("P1Slot") || drawable.name.starts_with("P2Slot")
                })
                .count();
            visible_units_peak = visible_units_peak.max(slots);
        }
    }
    let stats = runtime
        .with_script(|script| script.eval_json("__royale_stats()"))
        .expect("读取 __royale_stats()");
    let mesh = runtime.vertices(SCREEN_W, SCREEN_H).expect("HUD 顶点");
    let ui_draws = mesh.draws.len();
    // 工程字体是矢量图集：HUD 文字走 `vector_glyphs` 通道，不占 `draws`。只数 draw
    // 会把"文字真的画出来了"错当成"只剩面板"，所以两条通道都数。
    let hud_glyphs = mesh.vector_glyphs.len();
    let frame = runtime.scene_3d(viewport).expect("3D 视图");
    SceneReport {
        stats,
        drawables: drawables.max(frame.drawables.len()),
        visible_units_peak,
        ui_draws,
        hud_glyphs,
        camera_fov: frame.camera.fov_y,
    }
}

fn assert_scene_report(first: &SceneReport, second: &SceneReport) {
    for report in [first, second] {
        assert_eq!(
            report.stats["frame"].as_u64(),
            Some(u64::from(SCENE_STEPS)),
            "固定步没有真正推进"
        );
        assert!(
            report.stats["spawned"].as_u64().unwrap_or(0) >= 6,
            "AI 只部署了 {} 个单位，战斗没有真正发生",
            report.stats["spawned"]
        );
        assert!(
            report.stats["moved"].as_u64().unwrap_or(0) >= 1,
            "没有任何单位离开出生点，nav 路径没有走起来"
        );
        assert!(
            report.stats["nav_grid"].as_i64().unwrap_or(0) >= 1,
            "导航网格没有创建"
        );
        assert!(
            report.drawables >= 10,
            "3D 视口里只有 {} 个 drawable",
            report.drawables
        );
        assert!(
            report.visible_units_peak >= 1,
            "整段战斗没有单位在画面里出现"
        );
        assert!(
            report.ui_draws >= 1,
            "HUD 连面板几何都没有：{} 个 draw",
            report.ui_draws
        );
        assert!(
            report.hud_glyphs > 0,
            "HUD 文字走了 0 个矢量字形——工程字体没加载或文字没布局"
        );
        let fov_degrees = report.camera_fov.to_degrees();
        assert!(
            (fov_degrees - 50.0).abs() < 0.5,
            "相机不是场景里写的 50° 透视：{fov_degrees}"
        );
    }
    assert_eq!(
        first.stats["checksum"], second.stats["checksum"],
        "两个独立 VM 的状态校验和不一致：{} vs {}",
        first.stats["checksum"], second.stats["checksum"]
    );
    assert_eq!(
        first.stats["spawned"], second.stats["spawned"],
        "两个独立 VM 的部署数不一致"
    );
}

struct NetworkReport {
    frames: u32,
    host_logs: Vec<u64>,
    client_logs: Vec<u64>,
    spawned: i64,
    units: i64,
    desync: Option<Desync>,
}

struct PeerReport {
    logs: Vec<u64>,
    spawned: i64,
    units: i64,
    desync: Option<Desync>,
}

fn run_network(root: &Path) -> NetworkReport {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑定本地 TCP 端口");
    let addr = listener.local_addr().expect("本地地址");
    let host_root = root.to_path_buf();
    let host = std::thread::spawn(move || run_peer(&host_root, PeerMode::Accept(listener), 0));
    let client = run_peer(root, PeerMode::Connect(addr), 1);
    let host = host.join().expect("host 线程");
    let desync = host.desync.or(client.desync);
    let spawned = host.spawned.max(client.spawned);
    let units = host.units.max(client.units);
    NetworkReport {
        frames: NET_FRAMES,
        host_logs: host.logs,
        client_logs: client.logs,
        spawned,
        units,
        desync,
    }
}

fn assert_network_report(report: &NetworkReport) {
    assert!(
        report.host_logs.len() >= (report.frames + NET_DELAY) as usize,
        "帧同步只放行了 {} 帧（应至少 {}）",
        report.host_logs.len(),
        report.frames + NET_DELAY
    );
    assert_eq!(
        report.host_logs, report.client_logs,
        "两端逐帧校验和不一致（帧同步 desync）"
    );
    assert!(
        report.desync.is_none(),
        "会话报告了 desync：{:?}",
        report.desync
    );
    assert!(
        report.spawned >= 4,
        "联网对局只部署了 {} 个单位",
        report.spawned
    );
    assert!(report.units >= 1, "联网对局结束时场上一个单位都没有");
}

struct InteractionReport {
    selected: i64,
    played: String,
    hand_before: Vec<String>,
    hand_after: Vec<String>,
    next_before: String,
    next_after: String,
    queue_after: Vec<String>,
    player_z: f64,
    elixir_before: f64,
    elixir_after: f64,
    spawned_before: i64,
    spawned_after: i64,
    player_units: i64,
    cost: f64,
}

fn read_stats(runtime: &mut RuntimeScene) -> serde_json::Value {
    runtime
        .with_script(|script| script.eval_json("__royale_stats()"))
        .expect("读取 __royale_stats()")
}

fn hand_names(stats: &serde_json::Value) -> Vec<String> {
    stats["hand"]
        .as_array()
        .map(|hand| {
            hand.iter()
                .map(|entry| entry["name"].as_str().unwrap_or("").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn string_list(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// 与 `game_app` 同一条鼠标边沿：按下帧带 `mouse_just_pressed`，抬起帧先清边沿再松开。
fn click_at(runtime: &mut RuntimeScene, x: f64, y: f64) {
    let mut press = InputSnapshot::default();
    press.cursor_moved(x, y);
    press.mouse_event(MouseButton::Left, true);
    runtime
        .run_script_steps(1, 1.0 / 60.0, &press)
        .expect("点击按下");
    let mut release = InputSnapshot::default();
    release.cursor_moved(x, y);
    release.mouse_event(MouseButton::Left, true);
    release.finish_frame();
    release.mouse_event(MouseButton::Left, false);
    runtime
        .run_script_steps(1, 1.0 / 60.0, &release)
        .expect("点击抬起");
}

fn run_interaction(root: &Path) -> InteractionReport {
    let scene = Scene::load(root.join("scenes").join("battle.scene")).expect("加载 battle.scene");
    let mut runtime = RuntimeScene::from_scene(Some(root), scene).expect("实例化 ClashRoyale 场景");
    // 竖屏设计分辨率：既给 HUD，也把 3D 相机视口回填给 `camera3d.pick_plane`。
    runtime.vertices(SCREEN_W, SCREEN_H).expect("竖屏 HUD 顶点");

    let before = read_stats(&mut runtime);
    let hand_before = hand_names(&before);
    let next_before = before["next_card"].as_str().unwrap_or("").to_owned();
    let elixir_before = before["elixir"][0].as_f64().unwrap_or(0.0);
    let spawned_before = before["spawned"].as_i64().unwrap_or(0);

    // 第 4 张手牌的中心：x = 12 + 3*176 + 84，y = 1100。
    click_at(&mut runtime, 624.0, 1100.0);
    let after_card = read_stats(&mut runtime);
    let selected = after_card["selected"].as_i64().unwrap_or(0);
    assert_eq!(
        after_card["spawned"].as_i64().unwrap_or(-1),
        spawned_before,
        "点卡牌只在选中，不应部署"
    );
    let cost = after_card["hand"][3]["cost"].as_f64().unwrap_or(0.0) / 10.0;

    // 屏幕下方 = 己方半场：投影到地面约 z=-6，而不是固定出生点 z=-9.2。
    click_at(&mut runtime, 360.0, 700.0);
    let after = read_stats(&mut runtime);
    let played = after["player_cards"]
        .as_array()
        .and_then(|cards| cards.first())
        .and_then(|card| card.as_str())
        .unwrap_or("")
        .to_owned();
    InteractionReport {
        selected,
        played,
        hand_before,
        hand_after: hand_names(&after),
        next_before,
        next_after: after["next_card"].as_str().unwrap_or("").to_owned(),
        queue_after: string_list(&after["queue"]),
        player_z: after["player_z"].as_f64().unwrap_or(0.0),
        elixir_before,
        elixir_after: after["elixir"][0].as_f64().unwrap_or(0.0),
        spawned_before,
        spawned_after: after["spawned"].as_i64().unwrap_or(0),
        player_units: after["player_units"].as_i64().unwrap_or(0),
        cost,
    }
}

fn assert_interaction(report: &InteractionReport) {
    assert_eq!(report.selected, 4, "点击第 4 张手牌没有改变选中槽位");
    assert_eq!(report.spawned_before, 0, "重置后不应已有部署");
    assert_eq!(report.spawned_after, 1, "点击己方半场应只部署 1 个单位");
    assert_eq!(report.player_units, 1, "P1 场上应只有刚点的这个单位");
    assert_eq!(
        report.hand_before.len(),
        4,
        "手牌应有 4 张：{:?}",
        report.hand_before
    );
    assert_eq!(
        report.played, report.hand_before[3],
        "部署的应该是选中的第 4 张牌"
    );
    assert_eq!(
        report.hand_after[3], report.next_before,
        "打出的槽位应由下一张牌补上（手牌轮转）"
    );
    assert_eq!(
        report.next_after, report.queue_after[0],
        "下一张牌应是牌库顶：{:?}",
        report.queue_after
    );
    assert_eq!(
        report.queue_after.last().map(String::as_str),
        Some(report.played.as_str()),
        "打出的牌应进入牌库尾：{:?}",
        report.queue_after
    );
    assert_eq!(
        report.queue_after.len(),
        4,
        "牌库应保持 4 张：{:?}",
        report.queue_after
    );
    assert!(
        report.player_z > -8.5 && report.player_z < 0.0,
        "点击部署应落在点击处附近（z={:.2}），而不是固定的 -9.2 出生点",
        report.player_z
    );
    assert!(
        (report.elixir_before - report.elixir_after - report.cost).abs() < 0.05,
        "圣水应扣掉卡费 {:.1}：{:.1} -> {:.1}",
        report.cost,
        report.elixir_before,
        report.elixir_after
    );
}

enum PeerMode {
    Accept(TcpListener),
    Connect(SocketAddr),
}

fn run_peer(root: &Path, mode: PeerMode, player: u8) -> PeerReport {
    let lua = Lua::new();
    let nav = rustlua_gameplay::nav_lua::state();
    rustlua_gameplay::nav_lua::install(&lua, &nav).expect("安装 nav");
    let source =
        std::fs::read_to_string(root.join("scripts").join("battle.lua")).expect("读取 battle.lua");
    let battle: Table = lua
        .load(&source)
        .set_name("battle.lua")
        .eval()
        .expect("加载 battle.lua");
    let new_battle: Function = battle.get("new").expect("battle.new");
    let step_battle: Function = battle.get("step").expect("battle.step");
    let checksum: Function = battle.get("checksum").expect("battle.checksum");
    let snapshot: Function = battle.get("snapshot").expect("battle.snapshot");
    let state: Table = new_battle.call(()).expect("battle.new()");

    let transport = match mode {
        PeerMode::Accept(listener) => TcpTransport::accept(&listener, player),
        PeerMode::Connect(addr) => TcpTransport::connect(addr, player),
    }
    .expect("建立 TCP 传输");
    let config = SessionConfig::new(2, player, NET_DELAY).expect("会话参数");
    let mut session = Session::new(config, transport);
    session
        .prime(vec![0, 1], NET_DELAY)
        .expect("预热输入延迟帧");

    let total = NET_FRAMES + NET_DELAY;
    let mut logs = Vec::with_capacity(total as usize);
    let step_frame = |session: &mut Session<TcpTransport>,
                      frame: rustlua_net::FrameInputs,
                      logs: &mut Vec<u64>| {
        let inputs = lua.create_table().expect("inputs table");
        for (who, payload) in &frame.inputs {
            let entry = lua.create_table().expect("input entry");
            let slot = payload.first().copied().unwrap_or(0);
            if slot > 0 {
                entry.set("slot", slot).expect("slot");
                entry
                    .set("lane", payload.get(1).copied().unwrap_or(1))
                    .expect("lane");
            }
            inputs
                .set(usize::from(*who) + 1, entry)
                .expect("input slot");
        }
        step_battle
            .call::<()>((state.clone(), inputs))
            .expect("battle.step");
        let hex: String = checksum.call(state.clone()).expect("battle.checksum");
        let value = u64::from_str_radix(&hex, 16).expect("checksum 是十六进制");
        session
            .record_checksum(frame.frame, value)
            .expect("记录校验和");
        logs.push(value);
    };

    for tick in 0..NET_FRAMES {
        let payload = action_payload(player, tick).to_vec();
        session.submit(payload).expect("提交输入");
        session.poll().expect("收取远端消息");
        while let Some(frame) = session.try_advance() {
            step_frame(&mut session, frame, &mut logs);
        }
    }

    let mut idle = 0u32;
    while (logs.len() as u32) < total {
        session.poll().expect("收取远端消息");
        let mut advanced = false;
        while let Some(frame) = session.try_advance() {
            step_frame(&mut session, frame, &mut logs);
            advanced = true;
        }
        if !advanced {
            idle += 1;
            assert!(
                idle < 200_000,
                "帧同步停在 {}/{} 帧，远端没有继续提交输入",
                logs.len(),
                total
            );
            std::thread::sleep(Duration::from_micros(200));
        }
    }

    // 等对端把最后一帧的校验和发过来，再读 desync。
    for _ in 0..50 {
        session.poll().expect("收取校验和");
        if session.first_desync().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let snap: Table = snapshot.call(state).expect("battle.snapshot");
    let spawned: i64 = snap.get("spawned").unwrap_or(0);
    let units: i64 = snap
        .get::<Table>("units")
        .map(|units| units.len().unwrap_or(0))
        .unwrap_or(0);
    PeerReport {
        logs,
        spawned,
        units,
        desync: session.first_desync(),
    }
}

/// 两侧用同一套纯函数生成输入：只依赖（玩家号，提交序号），不读状态/时钟。
/// 槽位 1..4 对应本地手牌；两边手牌初始相同，因此同一串槽位输入 ⇒ 同一串出牌。
fn action_payload(player: u8, tick: u32) -> [u8; 2] {
    if tick.is_multiple_of(30) {
        let slot = ((tick / 30 + u32::from(player)) % 4 + 1) as u8;
        let lane = ((tick / 60 + u32::from(player)) % 2 + 1) as u8;
        [slot, lane]
    } else {
        [0, 1]
    }
}
