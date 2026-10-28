//! ClashRoyale 工程的端到端验证：3D 场景 + 导航寻路 + 确定性与真实 TCP 帧同步。
//!
//! 两段证据（都会打印真实读数，任何一段不满足即 panic）：
//!
//! 1. **场景模式**：两个互相独立的 `RuntimeScene` 各跑 900 个固定步（AI vs AI）。
//!    验证场景加载/脚本无错、HUD 有绘制、3D 视口里塔与上阵单位可见、两个 VM 的逐帧
//!    状态校验和完全一致（说明 `battle.lua` 的整数状态机 + `nav` 寻路是确定性的）。
//! 2. **联网模式**：两个裸 Lua VM 各装一份 `nav` 与 `battle.lua`，经 127.0.0.1 的真实
//!    TCP 用 `rustlua-net` 的帧同步会话交换输入与校验和，跑 600 帧后要求两侧逐帧
//!    checksum 完全相等且无 desync。
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
use rustlua_input::input::InputSnapshot;
use rustlua_net::{Desync, Session, SessionConfig, TcpTransport};
use rustlua_scene::scene::Scene;

const SCENE_STEPS: u32 = 900;
const NET_FRAMES: u32 = 600;
const NET_DELAY: u32 = 2;

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
        "scene ok: frame={} spawned={} moved={} damaged_towers={} checksum={} drawables={} visible_units_peak={} ui_draws={} fov_y={:.3}",
        first.stats["frame"],
        first.stats["spawned"],
        first.stats["moved"],
        first.stats["damaged_towers"],
        first.stats["checksum"],
        first.drawables,
        first.visible_units_peak,
        first.ui_draws,
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
}

struct SceneReport {
    stats: serde_json::Value,
    drawables: usize,
    visible_units_peak: usize,
    ui_draws: usize,
    camera_fov: f32,
}

fn run_scene(root: &Path) -> SceneReport {
    let scene = Scene::load(root.join("scenes").join("battle.scene")).expect("加载 battle.scene");
    let mut runtime = RuntimeScene::from_scene(Some(root), scene).expect("实例化 ClashRoyale 场景");
    runtime
        .with_script(|script| script.eval_json("__royale_autoplay(true) or true"))
        .expect("开启 AI 自走");
    let input = InputSnapshot::default();
    let viewport = Viewport::new(1280.0, 720.0).expect("viewport");
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
    let ui_draws = runtime.vertices(1280, 720).expect("HUD 顶点").draws.len();
    let frame = runtime.scene_3d(viewport).expect("3D 视图");
    SceneReport {
        stats,
        drawables: drawables.max(frame.drawables.len()),
        visible_units_peak,
        ui_draws,
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
            report.ui_draws >= 2,
            "HUD 只画了 {} 个 draw（面板与文字至少各合批 1 个）",
            report.ui_draws
        );
        let fov_degrees = report.camera_fov.to_degrees();
        assert!(
            (fov_degrees - 48.0).abs() < 0.5,
            "相机不是场景里写的 48° 透视：{fov_degrees}"
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
            let card = payload.first().copied().unwrap_or(0);
            if card > 0 {
                let name = ["knight", "archer", "giant"][(card - 1) as usize];
                entry.set("card", name).expect("card");
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
fn action_payload(player: u8, tick: u32) -> [u8; 2] {
    if tick.is_multiple_of(30) {
        let card = ((tick / 30 + u32::from(player)) % 3 + 1) as u8;
        let lane = ((tick / 60 + u32::from(player)) % 2 + 1) as u8;
        [card, lane]
    } else {
        [0, 1]
    }
}
