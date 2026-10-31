//! HonorArena 工程的端到端验证：整数确定性战场 + 动态 UI + 真实 UDP 中继上的回滚联机。
//!
//! 三段证据（全部打印真实读数，任何一段不满足即 panic）：
//!
//! 1. **确定性**：两个互相独立的 `RuntimeScene` 各跑 1500 个固定步（AI vs AI）。要求
//!    场景/脚本无错、动态 UI 真有几何与字形、3D 视图里真有透视相机与按类别出现的实体
//!    （地块/树/石头/塔/基地/英雄/小兵），两边的 checksum 与 3D drawable 数完全一致，
//!    并且战场上确实发生了战斗（有存活小兵、有被摧毁/受损的塔）。
//! 2. **干净中继上的 1v1**：启动项目自带的 `server/relay.mjs`，两个运行时各以 0/1 号
//!    座位经 UDP 入场，跑过 ready 握手后推进 480 帧。要求两边都进入 started、逐帧
//!    对拍无 DESYNC、同一帧的 checksum 相同（两端开局帧能差一两帧，拿"当前世界"比
//!    没有意义；对拍取落后方的帧号再往回 60 帧）。
//! 3. **丢包中继上的 1v1**：同一个中继开 `RELAY_DROP_EVERY=5`，实时 60Hz 步进（丢包后
//!    的重传要 150ms 才能到）。要求传输层 `retransmits > 0`、回滚账本 `resimulations > 0`，
//!    且仍然无 DESYNC、同一帧 checksum 相同——这条才证明"迟到输入"真的走了重模拟。
//!
//! 运行：
//!
//! ```text
//! cargo run -p engine --example arena_verify -- E:/Qoder/HonorArena
//! ```
//!
//! 工程路径也可用环境变量 `ARENA_PROJECT` 指定；需要 PATH 里有 `node`。
//! `ARENA_ONLY=clean|lossy` 只跑某一段联机；`ARENA_DEBUG=1` 打印每 100 tick 的进度。

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use engine::host::RuntimeScene;
use rustlua_camera::camera::Viewport;
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::Scene;
use serde_json::Value;

const SCENE_STEPS: u32 = 1_500;
const NET_FRAMES: u64 = 480;
const TICK_MS: u64 = 16;
const SCREEN_W: u32 = 1_280;
const SCREEN_H: u32 = 720;

fn project_root() -> PathBuf {
    std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("ARENA_PROJECT").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("E:/Qoder/HonorArena"))
}

fn main() {
    assert!(
        std::env::var_os("RUSTLUA_NET_AUTO").is_none(),
        "验证时不能带 RUSTLUA_NET_AUTO：会让工程一启动就进大厅，scene 段就不是纯 AI 对局了"
    );
    let root = project_root();
    assert!(
        root.join("project.toml").is_file(),
        "项目不存在：{}（第一个参数或 ARENA_PROJECT 指向 HonorArena 工程）",
        root.display()
    );

    // ARENA_ONLY=clean|lossy 只跑联机段（调参时省掉 1500 帧的确定性段）。
    let only = std::env::var("ARENA_ONLY").unwrap_or_default();
    if only.is_empty() {
        let first = run_scene(&root);
        let second = run_scene(&root);
        assert_scene(&first, &second);
        println!(
            "scene ok: frame={} minions={} damaged_structures={} alive_structures={} checksum={} \
             ui_draws={} glyphs={} drawables={} fov={:.1} level={:?}",
            first.stats["frame"],
            first.stats["minions"],
            first.stats["damaged_structures"],
            first.stats["alive_structures"],
            first.stats["checksum"],
            first.ui_draws,
            first.glyphs,
            first.drawables.len(),
            first.fov,
            first.stats["hero_level"],
        );
    }

    if only.is_empty() || only == "clean" {
        let clean = run_online(&root, 0, "clean");
        assert_online(&clean, false);
        println!(
            "online ok: frames={}/{} checksum={}/{} retx={} dup={} resim={} predicted={} stale={}",
            clean.frame_a,
            clean.frame_b,
            clean.checksum_a,
            clean.checksum_b,
            clean.retransmits,
            clean.duplicate,
            clean.resimulations,
            clean.predicted,
            clean.stale,
        );
    }

    if only.is_empty() || only == "lossy" {
        let lossy = run_online(&root, 5, "lossy");
        assert_online(&lossy, true);
        println!(
            "lossy ok: frames={}/{} checksum={}/{} retx={} dup={} resim={} resim_frames={} predicted={} stale={}",
            lossy.frame_a,
            lossy.frame_b,
            lossy.checksum_a,
            lossy.checksum_b,
            lossy.retransmits,
            lossy.duplicate,
            lossy.resimulations,
            lossy.resimulated_frames,
            lossy.predicted,
            lossy.stale,
        );
    }
}

struct SceneReport {
    stats: Value,
    ui_draws: usize,
    glyphs: usize,
    /// 3D 视图里的实体名（每帧由 view3d.lua 同步生成）。
    drawables: Vec<String>,
    /// 透视相机的垂直视场角（度）。
    fov: f32,
}

fn run_scene(root: &Path) -> SceneReport {
    let scene = Scene::load(root.join("scenes").join("arena.scene")).expect("加载 arena.scene");
    let mut runtime = RuntimeScene::from_scene(Some(root), scene).expect("实例化 HonorArena 场景");
    runtime
        .with_script(|script| script.eval_json("__arena_autoplay(true) or true"))
        .expect("开启 AI 自走");
    let input = InputSnapshot::default();
    for step in 0..SCENE_STEPS {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .unwrap_or_else(|error| panic!("第 {step} 步失败：{error}"));
    }
    let stats = runtime
        .with_script(|script| script.eval_json("__arena_stats()"))
        .expect("读取 __arena_stats()");
    let mesh = runtime.vertices(SCREEN_W, SCREEN_H).expect("HUD 顶点");
    let ui_draws = mesh.draws.len();
    // 文字两条通道都要数：矢量档进 vector_glyphs，hinting/SDF 档进 sdf=3 顶点。
    let hinted_glyphs = mesh.vertices.iter().filter(|v| v.sdf == 3).count() / 6;
    let viewport = Viewport::new(SCREEN_W as f32, SCREEN_H as f32).expect("viewport");
    let scene3d = runtime.scene_3d(viewport).expect("战场没有 3D 视图");
    let drawables = scene3d
        .drawables
        .iter()
        .map(|drawable| drawable.name.clone())
        .collect::<Vec<_>>();
    SceneReport {
        stats,
        ui_draws,
        glyphs: mesh.vector_glyphs.len() + hinted_glyphs,
        drawables,
        fov: scene3d.camera.fov_y.to_degrees(),
    }
}

fn count_prefix(report: &SceneReport, prefix: &str) -> usize {
    report
        .drawables
        .iter()
        .filter(|name| name.starts_with(prefix))
        .count()
}

fn assert_scene(first: &SceneReport, second: &SceneReport) {
    for report in [first, second] {
        assert_eq!(
            report.stats["frame"].as_u64(),
            Some(u64::from(SCENE_STEPS)),
            "固定步没有真正推进"
        );
        assert_eq!(
            report.stats["screen"].as_str(),
            Some("battle"),
            "AI 自走没有进入战斗屏"
        );
        assert!(
            report.stats["minions"].as_u64().unwrap_or(0) >= 3,
            "1500 帧后场上没有小兵，兵线没有跑起来"
        );
        assert!(
            report.stats["damaged_structures"].as_u64().unwrap_or(0) >= 1,
            "1500 帧里没有任何建筑掉血：兵线没有接战"
        );
        assert!(
            report.stats["kills"][0].as_u64().unwrap_or(0)
                + report.stats["kills"][1].as_u64().unwrap_or(0)
                >= 1,
            "整段对局 0 次击杀，AI 没有真正打起来"
        );
        assert!(
            report.stats["autoplay"].as_bool() == Some(true),
            "autoplay 没有生效"
        );
        assert!(
            report.ui_draws >= 1,
            "动态 UI 只有 {} 个 draw",
            report.ui_draws
        );
        assert!(
            report.glyphs > 0,
            "HUD 文字既没走矢量字形也没走覆盖率字形：文字没布局出来"
        );
        assert!(
            (30.0..80.0).contains(&report.fov),
            "3D 相机不是透视俯视：fov={}",
            report.fov
        );
        for (prefix, floor) in [
            ("lane", 60),
            ("tree", 8),
            ("rock", 6),
            ("st", 10),
            ("mn", 3),
            ("hero", 2),
        ] {
            let found = count_prefix(report, prefix);
            assert!(
                found >= floor,
                "3D 视图里 {prefix}* 只有 {found} 个可见实体（至少要 {floor}）"
            );
        }
    }
    assert_eq!(
        first.drawables.len(),
        second.drawables.len(),
        "两个独立运行时同步出的 3D 实体数不同"
    );
    assert_eq!(
        first.stats["checksum"], second.stats["checksum"],
        "两个独立 VM 同一输入序列的 checksum 不同：战斗核心不是确定性的"
    );
    assert_eq!(
        first.stats["minions"], second.stats["minions"],
        "两边场上小兵数不同"
    );
    assert_eq!(
        first.stats["kills"], second.stats["kills"],
        "两边击杀数不同"
    );
}

struct OnlineReport {
    frame_a: u64,
    frame_b: u64,
    checksum_a: Value,
    checksum_b: Value,
    retransmits: u64,
    duplicate: u64,
    resimulations: u64,
    resimulated_frames: u64,
    predicted: u64,
    stale: u64,
}

fn assert_online(report: &OnlineReport, lossy: bool) {
    assert!(
        report.frame_a >= NET_FRAMES && report.frame_b >= NET_FRAMES,
        "没过完 {NET_FRAMES} 帧：{} / {}",
        report.frame_a,
        report.frame_b
    );
    assert_eq!(
        report.checksum_a, report.checksum_b,
        "联机结束时两边世界 checksum 不同：{} vs {}",
        report.checksum_a, report.checksum_b
    );
    assert_eq!(report.stale, 0, "有输入落到 240 帧回滚窗口之外");
    if lossy {
        assert!(
            report.retransmits > 0,
            "丢包中继上 retransmits 仍为 0：丢包没有走到可靠层"
        );
        assert!(
            report.resimulations > 0,
            "丢包中继上 resimulations 仍为 0：迟到输入没有触发重模拟"
        );
        assert!(report.resimulated_frames > 0, "重模拟帧数为 0");
    }
}

fn open_online(root: &Path, port: u16, seat: u8) -> RuntimeScene {
    let scene = Scene::load(root.join("scenes").join("arena.scene")).expect("加载 arena.scene");
    let mut runtime = RuntimeScene::from_scene(Some(root), scene).expect("实例化 HonorArena 场景");
    let connected = runtime
        .with_script(|script| {
            script.eval_json(&format!(
                "__arena_connect('udp://127.0.0.1:{port}','verify',{seat})"
            ))
        })
        .expect("__arena_connect");
    assert_eq!(connected, Value::Bool(true), "座位 {seat} 没有连上中继");
    runtime
}

const CHECKSUM_DELAY: u64 = 60;

fn arena_status(runtime: &mut RuntimeScene) -> Value {
    runtime
        .with_script(|script| script.eval_json("__arena_net_status()"))
        .expect("读取 __arena_net_status()")
}

fn arena_frame_checksum(runtime: &mut RuntimeScene, frame: u64) -> Value {
    runtime
        .with_script(|script| script.eval_json(&format!("__arena_frame_checksum({frame})")))
        .unwrap_or(Value::Null)
}

fn status_u64(status: &Value, key: &str) -> u64 {
    status.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn run_online(root: &Path, drop_every: u32, tag: &str) -> OnlineReport {
    let debug = std::env::var_os("ARENA_DEBUG").is_some();
    let relay = spawn_relay(root, drop_every, tag);
    let mut a = open_online(root, relay.port, 0);
    if debug {
        eprintln!("[{tag}] seat 0 connected");
    }
    let mut b = open_online(root, relay.port, 1);
    if debug {
        eprintln!("[{tag}] seat 1 connected");
    }
    let input = InputSnapshot::default();

    let mut ticks = 0u32;
    let mut started = false;
    // 两端收到对方 ready 的先后不同，开局帧能差一两帧：对拍必须按帧号取同一帧，
    // 而且要取 60 帧前（CHECKSUM_DELAY）的值——更近的帧还可能被迟到输入重模拟改写。
    let mut settled = false;
    let mut compared = 0u64;
    // 阶段 1：握手；阶段 2：推进到 NET_FRAMES + 60；阶段 3：等对拍稳定。
    while ticks < 2_000 {
        a.run_script_steps(1, 1.0 / 60.0, &input)
            .unwrap_or_else(|error| panic!("[{tag}] A 第 {ticks} 步失败：{error}"));
        b.run_script_steps(1, 1.0 / 60.0, &input)
            .unwrap_or_else(|error| panic!("[{tag}] B 第 {ticks} 步失败：{error}"));
        ticks += 1;
        thread::sleep(Duration::from_millis(TICK_MS));
        if !ticks.is_multiple_of(10) {
            continue;
        }
        let sa = arena_status(&mut a);
        let sb = arena_status(&mut b);
        if sa["net_started"].as_bool() == Some(true) && sb["net_started"].as_bool() == Some(true) {
            started = true;
        }
        if !started {
            continue;
        }
        let common = status_u64(&sa, "net_frame").min(status_u64(&sb, "net_frame"));
        if debug && ticks.is_multiple_of(100) {
            eprintln!(
                "[{tag}] t={ticks} state={}/{} started={}/{} frame={}/{} resim={}/{}",
                sa["state"],
                sb["state"],
                sa["net_started"],
                sb["net_started"],
                sa["net_frame"],
                sb["net_frame"],
                sa["net_resim"],
                sb["net_resim"],
            );
        }
        if common >= NET_FRAMES + CHECKSUM_DELAY {
            let at = common - CHECKSUM_DELAY;
            let ca = arena_frame_checksum(&mut a, at);
            let cb = arena_frame_checksum(&mut b, at);
            let desync = sa["net_desync"].is_null() && sb["net_desync"].is_null();
            if !ca.is_null() && ca == cb && desync {
                settled = true;
                compared = at;
                break;
            }
        }
    }
    assert!(started, "[{tag}] 2000 个 tick 内 ready 握手没有完成");
    assert!(
        settled,
        "[{tag}] 推进 {NET_FRAMES} 帧后第 {} 帧的 checksum 两端仍不一致",
        ticks
    );

    let sa = arena_status(&mut a);
    let sb = arena_status(&mut b);
    assert!(
        sa["net_desync"].is_null() && sb["net_desync"].is_null(),
        "[{tag}] 出现 DESYNC：A={} B={}",
        sa["net_desync"],
        sb["net_desync"]
    );
    let udp = |status: &Value, key: &str| -> u64 {
        status
            .get("udp")
            .and_then(|udp| udp.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let checksum_a = arena_frame_checksum(&mut a, compared);
    let checksum_b = arena_frame_checksum(&mut b, compared);
    drop(relay);
    OnlineReport {
        frame_a: status_u64(&sa, "net_frame"),
        frame_b: status_u64(&sb, "net_frame"),
        checksum_a,
        checksum_b,
        retransmits: udp(&sa, "retransmits") + udp(&sb, "retransmits"),
        duplicate: udp(&sa, "duplicate") + udp(&sb, "duplicate"),
        resimulations: status_u64(&sa, "net_resim") + status_u64(&sb, "net_resim"),
        resimulated_frames: status_u64(&sa, "net_resim_frames")
            + status_u64(&sb, "net_resim_frames"),
        predicted: status_u64(&sa, "net_pending") + status_u64(&sb, "net_pending"),
        stale: status_u64(&sa, "net_stale") + status_u64(&sb, "net_stale"),
    }
}

struct Relay {
    port: u16,
    child: Child,
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 起项目自带的 Node 中继。端口经 `RELAY_PORT_FILE` 传回：Windows 上 Node 的 stdout
/// 走管道时 flush 不保证及时（偶发一个字都不出），把启动信号押在 stdout 上会假死。
fn spawn_relay(root: &Path, drop_every: u32, tag: &str) -> Relay {
    let script = root.join("server").join("relay.mjs");
    let port_file = std::env::temp_dir().join(format!(
        "honorarena-relay-{}-{tag}.port",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&port_file);
    let mut child = Command::new("node")
        .arg(&script)
        .env("PORT", "0")
        .env("HOST", "127.0.0.1")
        .env("RELAY_DROP_EVERY", drop_every.to_string())
        .env("RELAY_PORT_FILE", &port_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap_or_else(|error| panic!("起不来 node {script:?}：{error}（需要 PATH 里有 node）"));

    // 立刻开始排空 stdout，日志再多也不会把中继卡在写管道上。
    let stdout = child.stdout.take().expect("relay stdout");
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut sink = String::new();
        while reader.read_line(&mut sink).unwrap_or(0) > 0 {
            sink.clear();
        }
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut port = None;
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&port_file)
            && let Ok(value) = text.trim().parse::<u16>()
        {
            port = Some(value);
            break;
        }
        if let Ok(Some(status)) = child.try_wait() {
            panic!("[{tag}] 中继进程提前退出：{status}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = std::fs::remove_file(&port_file);
    let port = port.unwrap_or_else(|| panic!("[{tag}] 中继 10s 内没有写出监听端口"));
    println!("[{tag}] relay udp://127.0.0.1:{port} drop_every={drop_every}");
    Relay { port, child }
}
