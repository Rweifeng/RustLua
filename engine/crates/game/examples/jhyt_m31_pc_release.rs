//! Headless acceptance drive for the P3 release slice: clean PC publish, the
//! per-user data directory, and the real-device environment evidence.
//!
//! 覆盖四类"发布出去才算数"的缺陷/契约（每条都能独立验收）：
//!
//! 1. **存档与玩家设置写进程序目录**：发布包装到 `C:\Program Files\...` 时对普通
//!    用户不可写；即使可写，重装/卸载即丢档。现在发布包形态（`<exe>/pack` +
//!    同级 `release.toml`，或 `game --pack`）把 `save.sav` 与 `settings.toml`
//!    都写到用户数据目录（`%APPDATA%\<app>`，可用 `RUSTLUA_USER_DIR` 覆盖），
//!    **发布包内的字节一个都不改**。开发工程形态不变（设置留工程根、存档进
//!    `<project>/.cache`），编辑器与既有验收样例因此不受影响。
//! 2. **升级覆盖玩家设置**：首次运行把包内 `settings.toml` 播种为默认值，之后
//!    用户目录那份是唯一真相——再次播种（= 换发布包）不得覆盖玩家改键/音量。
//! 3. **设备后端与 Lua 面板读到不同的设置**：手柄死区由 Rust 侧从
//!    `settings.toml` 读，改键/音量由 Lua 侧读写；两处必须同一个根，否则
//!    "面板显示一套、设备用另一套"。
//! 4. **发布包内容与设备环境不能靠猜**：本示例直接检查真实发布目录的形态与
//!    卫生（无 `.cache`/存档/`.staging-*` 残留）、真机手柄槽位采样结果、
//!    WebGL 浏览器验证摘要（`artifacts/logs/webgl/summary-*.json`）以及 WebGL
//!    发布包**产物层启动**摘要（`artifacts/logs/webgl/bundle-summary-*.json`，
//!    来自 `scripts/verify-webgl-bundle.ps1`，当前为已知平台缺口只记录不阻塞）。
//!    没有可用硬件/浏览器时**明确记录环境限制**，不伪造通过。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m31_pc_release [-- <game_jhyt-root>]

use engine::{
    core::{
        script::LuaRuntime,
        settings,
        user_dir::{self, USER_DIR_ENV},
    },
    host::gamepad::{GamepadPoller, XInputBackend},
};
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// 发布目录相对工程根的位置（`scripts/package.ps1 -Platform pc` 的输出）。
const RELEASE_REL: &str = "builds/pc";
/// 发布包必须存在的条目（`game.exe` 在上一级）。
///
/// 字体在**包内**（`pack/assets/fonts/…`）而不是发布根：PC 形态不复制 17 MB 完整字体到发布根
/// （见 `scripts/package.ps1` 的 pc 分支），运行时以 `pack/` 为工程根、按 `settings.toml` 的
/// `font` 路径读取；发布根下的 `fonts/` 只有 webgl 的 `index.html` 需要。此前这里按 webgl
/// 的布局要求根下字体，于是**一条正确的 pc 发布反而过不了这个门**。
const REQUIRED_RELEASE_ENTRIES: &[&str] = &[
    "game.exe",
    "release.toml",
    "run.txt",
    "build-info.txt",
    "deps.txt",
    "pack/project.toml",
    "pack/manifest.txt",
    "pack/chars.txt",
    "pack/settings.toml",
    "pack/assets/fonts/NotoSansSC-VF.ttf",
    "pack/scripts/main.lua",
    "pack/scripts/ui/ui_text.lua",
    "pack/scripts/ui/text_fit.lua",
];
/// 发布包里**不得**出现的开发/玩家数据（存在即说明发布不干净）。
const FORBIDDEN_PACK_ENTRIES: &[&str] = &[".cache", ".data", "settings.local.toml"];
/// 发布目录里不得出现的运行期产物（这些属于用户数据目录或日志目录）。
const FORBIDDEN_RELEASE_SUFFIXES: &[&str] = &[".sav", ".log", ".tmp"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| engine::core::games_dir().join("examples").join("game_jhyt"));
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let release = project.join(RELEASE_REL);
    let pack = release.join("pack");
    let work = temp_dir("jhyt-m31")?;
    // 用户数据根：本示例隔离到临时目录，绝不动真实的 `%APPDATA%`。
    let user_base = work.join("user-base");
    fs::create_dir_all(&user_base)?;
    // SAFETY: 本示例是独立进程，进程内没有其他读者，示例退出即消失。
    unsafe {
        env::set_var(USER_DIR_ENV, &user_base);
    }

    // -----------------------------------------------------------------
    banner("① 发布目录形态与卫生（真实 `builds/pc`）");
    if !release.is_dir() {
        return Err(format!(
            "release directory missing: {}（先跑 scripts/package.ps1 -Platform pc）",
            release.display()
        )
        .into());
    }
    for entry in REQUIRED_RELEASE_ENTRIES {
        let path = release.join(entry);
        if !path.is_file() {
            return Err(format!("release is missing {entry}: {}", path.display()).into());
        }
    }
    for entry in FORBIDDEN_PACK_ENTRIES {
        let path = pack.join(entry);
        if path.exists() {
            return Err(format!("release pack must not ship {entry}: {}", path.display()).into());
        }
    }
    let mut stray = Vec::new();
    collect_stray_files(&release, &mut stray, 0)?;
    if !stray.is_empty() {
        return Err(format!("release directory has runtime leftovers: {stray:?}").into());
    }
    println!(
        "   release_entries={} stray=0",
        REQUIRED_RELEASE_ENTRIES.len()
    );

    // -----------------------------------------------------------------
    banner("② 发布包形态 ⇒ 用户数据目录；开发工程形态 ⇒ 工程内（同一份判定）");
    let pack_roots = user_dir::data_roots(&pack, false);
    if !pack_roots.packaged {
        return Err(
            "release layout (<exe>/pack + release.toml) must be detected as packaged".into(),
        );
    }
    if pack_roots.save_dir.starts_with(&pack) || pack_roots.settings_dir.starts_with(&pack) {
        return Err(format!(
            "packaged data must not live in the program directory: {pack_roots:?}"
        )
        .into());
    }
    let expected_app = user_dir::app_name(&pack);
    if pack_roots.save_dir != user_base.join(&expected_app) {
        return Err(format!(
            "save dir {} != user data dir {}",
            pack_roots.save_dir.display(),
            user_base.join(&expected_app).display()
        )
        .into());
    }
    // 显式 `--pack`（双击之外的启动方式）也必须走用户目录。
    let explicit = user_dir::data_roots(&pack, true);
    if explicit != pack_roots {
        return Err("explicit --pack must resolve the same roots".into());
    }
    let dev_roots = user_dir::data_roots(&project, false);
    if dev_roots.packaged {
        return Err("source project must not be treated as a release pack".into());
    }
    let expect_dev_settings = project.join("settings.toml");
    let expect_dev_save = project.join(".cache").join("save.sav");
    if dev_roots.settings_file() != expect_dev_settings || dev_roots.save_file() != expect_dev_save
    {
        return Err(format!("source project roots changed: {dev_roots:?}").into());
    }
    println!(
        "   packaged app={} save={} | dev settings={} save={}",
        expected_app,
        pack_roots.save_file().display(),
        dev_roots.settings_file().display(),
        dev_roots.save_file().display()
    );

    // -----------------------------------------------------------------
    banner("③ 首次运行播种默认设置；升级不覆盖玩家设置；真实脚本落到用户目录");
    let install = work.join("install");
    let fake_pack = install.join("pack");
    copy_project_plane(&project, &fake_pack)?;
    fs::write(
        install.join("release.toml"),
        "publish_name = \"jhyt-acceptance\"\n",
    )?;
    let fake_roots = user_dir::data_roots(&fake_pack, false);
    let pack_settings_before = fs::read(fake_pack.join("settings.toml"))?;
    if !user_dir::seed_settings(&fake_roots, &fake_pack)? {
        return Err("first run must seed settings.toml from the release pack".into());
    }
    if fs::read(fake_roots.settings_file())? != pack_settings_before {
        return Err("seeded settings must equal the packaged defaults".into());
    }
    user_dir::ensure_save_dir(&fake_roots)?;
    // 玩家改死区（编辑器「项目设置 → 输入」面板的落点）：改的是 `[input]` 段里
    // **已有**的那两行——引擎解析"首次出现者生效"，追加第二个 `[input]` 段不会生效。
    let seeded = fs::read_to_string(fake_roots.settings_file())?;
    let edited = set_toml_key(&seeded, "stick_deadzone", "0.33");
    let edited = set_toml_key(&edited, "trigger_deadzone", "0.11");
    if edited == seeded {
        return Err("settings.toml must already declare stick_deadzone/trigger_deadzone".into());
    }
    fs::write(fake_roots.settings_file(), edited)?;
    fs::write(
        fake_pack.join("settings.toml"),
        "[audio]\nmaster = 0.9\nbgm = 0.9\nsfx = 0.9\nui = 0.9\n",
    )?;
    if user_dir::seed_settings(&fake_roots, &fake_pack)? {
        return Err("upgrade must not re-seed over the player's settings".into());
    }
    let player_settings = fs::read_to_string(fake_roots.settings_file())?;
    if !player_settings.contains("stick_deadzone = 0.33") {
        return Err("player settings must survive a package upgrade".into());
    }
    println!("   seeded_then_preserved app={}", fake_roots.app_name);

    // 真实 LuaRuntime：项目根 = 发布包，设置根/存档路径 = 用户目录（宿主同一套解析）。
    let mut runtime = new_runtime(&fake_pack)?;
    runtime.set_settings_root(fake_roots.settings_dir.clone());
    runtime.set_save_path(fake_roots.save_file());
    runtime.execute_project_script(&fake_pack.join("scripts/main.lua"))?;
    let settings_path = query_str(&mut runtime, "return tostring(input.settings_path())")?;
    if settings_path != fake_roots.settings_file().to_string_lossy() {
        return Err(format!(
            "input.settings_path() = {settings_path}, want {}",
            fake_roots.settings_file().display()
        )
        .into());
    }
    let audio_path = query_str(&mut runtime, "return tostring(audio.settings_path())")?;
    if audio_path != fake_roots.settings_file().to_string_lossy() {
        return Err(format!("audio.settings_path() = {audio_path}, want user dir").into());
    }
    // 面板保存改键 → 用户目录；发布包内字节不变。
    // M22/W1：写回的是 v2 物理键文本（`key:keyf`），输入的 v1 写法 `key:f` 仍被接受。
    runtime.execute_console(
        "assert(input.rebind('interact', 'key:f') == true)\n\
         assert(select(1, input.save_settings()) == true)\n\
         assert(select(1, audio.save_settings()) == true)\n\
         save.write('m31_probe', 'release-layout')\n\
         assert(save.commit(), 'save commit')\n\
         return 'ok'",
    )?;
    if !fs::read_to_string(fake_roots.settings_file())?.contains("interact=key:keyf") {
        return Err("rebound key must be written into the user data directory".into());
    }
    // 发布包里的 `settings.toml` 只允许是包自带的默认值：玩家改键/死区一律不进包。
    let pack_settings_after = fs::read_to_string(fake_pack.join("settings.toml"))?;
    if pack_settings_after.contains("interact=key:keyf")
        || pack_settings_after.contains("stick_deadzone")
    {
        return Err("the release pack must never receive player settings".into());
    }
    if !fake_roots.save_file().is_file() {
        return Err(format!(
            "save data must land in {}",
            fake_roots.save_file().display()
        )
        .into());
    }
    if fake_pack.join(".cache").exists() || fake_pack.join("save.sav").exists() {
        return Err("save data must not be written next to the packaged scripts".into());
    }
    // 读回：新 runtime 从用户目录读档成功（发布包的存档是可用的）。
    let mut reader = new_runtime(&fake_pack)?;
    reader.set_settings_root(fake_roots.settings_dir.clone());
    reader.set_save_path(fake_roots.save_file());
    reader.execute_console(
        "local ok, reason = save.load()\n\
         assert(ok, tostring(reason))\n\
         assert(save.read('m31_probe') == 'release-layout', tostring(save.read('m31_probe')))\n\
         return 'ok'",
    )?;
    println!(
        "   runtime_settings={settings_path} save={}",
        fake_roots.save_file().display()
    );

    // -----------------------------------------------------------------
    banner("④ 设备后端与 Lua 面板同源：死区/改键读的是用户目录那份");
    let (user_deadzone, diagnostic) = settings::load_deadzone_section(&fake_roots.settings_dir);
    if let Some(diagnostic) = diagnostic {
        return Err(format!("user settings must parse cleanly: {diagnostic}").into());
    }
    if (user_deadzone.stick - 0.33).abs() > f32::EPSILON
        || (user_deadzone.trigger - 0.11).abs() > f32::EPSILON
    {
        return Err(format!("deadzone from user dir = {user_deadzone:?}, want 0.33/0.11").into());
    }
    let (pack_deadzone, _) = settings::load_deadzone_section(&pack);
    if (pack_deadzone.stick - 0.33).abs() <= f32::EPSILON {
        return Err("release pack must not carry the player's deadzone".into());
    }
    let (default_deadzone, _) = settings::load_deadzone_section(&work.join("no-such-root"));
    if (default_deadzone.stick - 0.15).abs() > f32::EPSILON {
        return Err(format!(
            "missing settings must fall back to the engine default, got {default_deadzone:?}"
        )
        .into());
    }
    println!(
        "   user_deadzone={:.2}/{:.2} pack_deadzone={:.2}/{:.2} missing_default={:.2}/{:.2}",
        user_deadzone.stick,
        user_deadzone.trigger,
        pack_deadzone.stick,
        pack_deadzone.trigger,
        default_deadzone.stick,
        default_deadzone.trigger
    );
    // 开发工程形态：Lua 侧仍读写工程内那个文件（编辑器面板依赖它）。
    let mut dev_runtime = new_runtime(&project)?;
    let dev_path = query_str(&mut dev_runtime, "return tostring(input.settings_path())")?;
    if dev_path != expect_dev_settings.to_string_lossy() {
        return Err(format!(
            "dev input.settings_path() = {dev_path}, want {}",
            expect_dev_settings.display()
        )
        .into());
    }

    // -----------------------------------------------------------------
    banner("⑤ 真实设备/浏览器环境证据（无硬件则记录限制，不伪造通过）");
    let gamepad = probe_gamepad();
    println!("   gamepad: {gamepad}");
    if !gamepad.contains("slots=") {
        return Err("gamepad probe must report sampled slots".into());
    }
    match webgl_contract(&project)? {
        Some(contract) => {
            println!(
                "   webgl: status={} wasm_target={} summary={}",
                contract.status, contract.wasm_target, contract.file
            );
            if contract.status != "ok" {
                return Err(format!(
                    "WebGL browser verification reported {}: {}",
                    contract.status, contract.file
                )
                .into());
            }
            if !contract.wasm_target {
                return Err(format!(
                    "wasm32 target was unavailable during WebGL verification: {}",
                    contract.file
                )
                .into());
            }
        }
        None => {
            println!("   webgl: SKIPPED (no artifacts/logs/webgl/summary-*.json on this machine)")
        }
    }
    // 产物层启动证据：真实 WebGL 发布包在无头浏览器里能不能进到 Lua。当前是**已知平台缺口**
    // （`docs/32` §4、backlog 4-24），只记录不阻塞——PC 发行不受影响；一旦补齐会变成 ok。
    match webgl_bundle(&project)? {
        Some(bundle) => println!(
            "   webgl_bundle: boot={} missing_host_api={} summary={}",
            bundle.boot, bundle.missing, bundle.file
        ),
        None => println!(
            "   webgl_bundle: SKIPPED (no artifacts/logs/webgl/bundle-summary-*.json on this machine)"
        ),
    }

    // -----------------------------------------------------------------
    banner("⑥ 发布包内容契约：中文文本管线与字符集随包发布");
    let manifest = fs::read_to_string(pack.join("manifest.txt"))?;
    for needle in [
        "scripts/ui/ui_text.lua",
        "scripts/ui/text_fit.lua",
        "scripts/ui/hud.lua",
        "scripts/core/save_manager.lua",
    ] {
        if !manifest.contains(needle) {
            return Err(format!("manifest.txt must list {needle}").into());
        }
    }
    let chars = fs::read_to_string(pack.join("chars.txt"))?;
    let cjk = chars
        .chars()
        .filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch))
        .count();
    if cjk < 500 {
        return Err(format!("packaged charset looks truncated: cjk={cjk}").into());
    }
    let settings_text = fs::read_to_string(pack.join("settings.toml"))?;
    // 发布默认设置必须自带 `[input]`（改键 + 死区）：玩家的改键/死区都在这两行上生效；
    // `[audio]` 段由首次 `audio.save_settings()` 建立，不要求随包发布。
    // M22/W1：随包发布的是 v2 物理键文本（工程 `settings.toml` 里 `interact=key:keye`）。
    for needle in [
        "[input]",
        "stick_deadzone",
        "rustlua-actions-v2",
        "interact=key:keye",
    ] {
        if !settings_text.contains(needle) {
            return Err(format!("packaged settings.toml must ship {needle}").into());
        }
    }
    let run_text = fs::read_to_string(release.join("run.txt"))?;
    if !run_text.contains("--pack") {
        return Err("run.txt must document the --pack launch".into());
    }
    println!("   manifest_ok chars={} cjk={cjk}", chars.chars().count());

    let _ = fs::remove_dir_all(&work);
    println!("jhyt_m31_pc_release_ok");
    Ok(())
}

// ---------------------------------------------------------------------
// 手柄与 WebGL 环境探针
// ---------------------------------------------------------------------

/// 采样真实 XInput 槽位。没有硬件时返回"未连接"，这本身就是可复现的环境证据：
/// 逻辑链路（死区/导航/按键映射）由 `jhyt_m20_gamepad` 的 Fake 后端覆盖。
fn probe_gamepad() -> String {
    let mut poller = GamepadPoller::new(Box::new(XInputBackend::new()));
    let frame = poller.poll_frame();
    let slots = poller.connected_slots();
    format!(
        "connected={} active={} slots={slots:?} axes=({:.3},{:.3})",
        !slots.is_empty(),
        frame.connected,
        frame.axes[0],
        frame.axes[1]
    )
}

/// WebGL 浏览器**契约**验证摘要（`scripts/verify-webgl.ps1` 产出）。
struct WebglContract {
    file: String,
    status: String,
    wasm_target: bool,
}

/// WebGL 发布包**产物层启动**摘要（`scripts/verify-webgl-bundle.ps1` 产出）。
struct WebglBundle {
    file: String,
    boot: String,
    missing: usize,
}

/// 最近的 WebGL 浏览器验证摘要：`status` 必须为 `ok`、验证时本机必须已装 wasm 目标。
/// 本机从未跑过时返回 `None`（调用方打印 SKIPPED，而不是伪造通过）。
fn webgl_contract(project: &Path) -> Result<Option<WebglContract>, Box<dyn std::error::Error>> {
    let Some((file, text)) = latest_webgl_log(project, "summary-")? else {
        return Ok(None);
    };
    let status = extract_json_field(&text, "status").unwrap_or_else(|| "unknown".to_owned());
    let wasm_target = extract_json_field(&text, "wasmTarget").is_some_and(|value| value == "true");
    Ok(Some(WebglContract {
        file,
        status,
        wasm_target,
    }))
}

/// 最近的 WebGL 发布包启动摘要：`boot` 是页面上的机器可读标记，`missing` 是失败时页面
/// 报出的"缺失引擎宿主 API"个数（成功启动时为 0）。
fn webgl_bundle(project: &Path) -> Result<Option<WebglBundle>, Box<dyn std::error::Error>> {
    let Some((file, text)) = latest_webgl_log(project, "bundle-summary-")? else {
        return Ok(None);
    };
    let boot = extract_json_field(&text, "boot").unwrap_or_else(|| "unknown".to_owned());
    // 优先读 `missingCount`（`verify-webgl-bundle.ps1` 直接产出的数值字段，JSON 里逗号与
    // 转义引号不会干扰）；老摘要没有该字段时退回解析 `detail` 里的
    // `[missing engine host API: a, b, c]`。
    let missing = extract_json_field(&text, "missingCount")
        .and_then(|value| value.parse::<usize>().ok())
        .or_else(|| {
            let detail = extract_json_field(&text, "detail").unwrap_or_default();
            detail
                .split_once("missing engine host API:")
                .map(|(_, list)| {
                    list.split(',')
                        .filter(|name| !name.trim().is_empty())
                        .count()
                })
        })
        .unwrap_or(0);
    Ok(Some(WebglBundle {
        file,
        boot,
        missing,
    }))
}

/// 读 `artifacts/logs/webgl/<prefix>*.json` 里按文件名排序最新的一个，返回 `(文件名, 全文)`。
fn latest_webgl_log(
    project: &Path,
    prefix: &str,
) -> Result<Option<(String, String)>, Box<dyn std::error::Error>> {
    // `<workspace>/artifacts/logs/webgl`（工程根 = `<workspace>/examples/game_jhyt`）。
    let Some(workspace) = project.parent().and_then(Path::parent) else {
        return Ok(None);
    };
    let log_root = workspace.join("artifacts").join("logs").join("webgl");
    let Ok(entries) = fs::read_dir(&log_root) else {
        return Ok(None);
    };
    let mut summaries: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".json"))
        })
        .collect();
    summaries.sort();
    let Some(latest) = summaries.pop() else {
        return Ok(None);
    };
    let text = fs::read_to_string(&latest)?;
    Ok(Some((latest.display().to_string(), text)))
}

/// 从摘要 JSON 里取一个标量字段（避免为一行读取引入 JSON 依赖）。
fn extract_json_field(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = text.find(&needle)? + needle.len();
    let rest = text[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let value = rest.trim_start_matches('"');
    let end = value.find(['"', ',', '\n', '}']).unwrap_or(value.len());
    Some(value[..end].trim().to_owned())
}

/// 改写 `settings.toml` 里某个键的值（保留缩进与其余行）。键必须已存在——
/// 这是"玩家在面板上改设置"的等价操作，键不存在说明发布默认值不完整。
fn set_toml_key(text: &str, key: &str, value: &str) -> String {
    let prefix = format!("{key} =");
    let mut replaced = false;
    let mut out = String::with_capacity(text.len() + 8);
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !replaced && trimmed.starts_with(&prefix) {
            let indent = &line[..line.len() - trimmed.len()];
            out.push_str(indent);
            out.push_str(&format!("{key} = {value}"));
            replaced = true;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------

fn banner(title: &str) {
    println!("\n== {title}");
}

fn temp_dir(tag: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = env::temp_dir().join(format!(
        "{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 复制"发布包脚本面"（project.toml + settings.toml + scripts/**）到临时发布包。
/// 只复制脚本与数据：本示例不渲染、不需要纹理/场景资源。
fn copy_project_plane(project: &Path, pack: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(pack)?;
    for file in ["project.toml", "settings.toml"] {
        fs::copy(project.join(file), pack.join(file))?;
    }
    copy_tree(&project.join("scripts"), &pack.join("scripts"))?;
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let destination = target.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &destination)?;
        } else {
            fs::copy(&path, &destination)?;
        }
    }
    Ok(())
}

/// 发布目录里的运行期残留（存档/日志/临时文件），最多递归两层（`pack/**` 与根）。
fn collect_stray_files(
    dir: &Path,
    out: &mut Vec<String>,
    depth: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".staging-") {
            out.push(format!("{} (stale staging)", entry.path().display()));
            continue;
        }
        if entry.file_type()?.is_dir() {
            if depth >= 2 {
                continue;
            }
            collect_stray_files(&entry.path(), out, depth + 1)?;
        } else if FORBIDDEN_RELEASE_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
        {
            out.push(entry.path().display().to_string());
        }
    }
    Ok(())
}

fn new_runtime(project: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
}

fn query_str(runtime: &mut LuaRuntime, block: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(runtime.execute_console(block)?.trim().to_owned())
}
