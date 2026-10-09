//! 平台音频出口的行为契约（WP6-08）。
//!
//! 为什么单独开一个集成测试文件：provider 是**进程级一次性安装**的（与 `vfs` / `save_slot`
//! 同一个理由）。单元测试跑在同一个进程里，装了它会把同进程其它音频测试一起改道；
//! 独立的集成测试各自一个进程，互不干扰。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rustlua_audio::audio::{AudioBus, AudioSystem};
use rustlua_audio::sink::{self, AudioRequest, AudioSinkProvider};

/// provider 收到的每一次请求，便于断言"平台到底拿到了什么"。
static RECORDS: Mutex<Vec<Recorded>> = Mutex::new(Vec::new());

struct Recorded {
    name: String,
    bus: AudioBus,
    volume: f32,
    asset_dir: Option<PathBuf>,
    looping: bool,
}

struct Recorder;

impl AudioSinkProvider for Recorder {
    fn play(&self, request: AudioRequest<'_>) -> Result<(), String> {
        RECORDS.lock().unwrap().push(Recorded {
            name: request.name.to_owned(),
            bus: request.bus,
            volume: request.volume,
            asset_dir: request.asset_dir.map(PathBuf::from),
            looping: request.looping,
        });
        Ok(())
    }
}

/// 装了平台出口之后：播放**整段交给平台**，不再读文件、不再找设备。
///
/// 这条盯着的是 wasm 上"游戏全程静音"的真问题：装了槽之后 `play_on` 必须成功，
/// 即使资源目录指向一个根本不存在的地方（那正是 wasm 上的处境）。同时钉住平台拿到的是
/// **已算好的有效音量**、**总线与循环标记**、**资源目录**——平台要自己解析路径，缺了目录它只能猜。
#[test]
fn play_goes_through_the_platform_sink_with_resolved_volume() {
    sink::set_provider(Box::new(Recorder)).expect("provider 每个进程只能装一次");
    assert!(sink::is_installed(), "装完必须处于已安装状态");

    let mut audio = AudioSystem::new();
    // 故意指向不存在的地方：只要还有任何一条代码路径偷偷 `std::fs::read`，这条就会红。
    let dir = PathBuf::from("/rustlua-nonexistent-audio");
    audio.set_asset_dir(dir.clone());
    audio.set_volume(0.5);
    audio.set_bus_volume(AudioBus::Sfx, 0.5);
    audio.set_bus_volume(AudioBus::Bgm, 1.0);

    audio
        .play_on(AudioBus::Sfx, "sfx_harvest")
        .expect("走平台出口应当成功");
    audio.play_music("bgm_title").expect("音乐也走平台出口");
    audio.play_on(AudioBus::Ui, "ui_click").expect("第三次播放");

    let records = RECORDS.lock().unwrap();
    assert_eq!(records.len(), 3, "三次播放请求都该到平台");

    let sfx = &records[0];
    assert_eq!(sfx.name, "sfx_harvest");
    assert_eq!(sfx.bus, AudioBus::Sfx);
    // 有效音量 = 主音量 × 总线音量：平台拿到的是**算好的值**，不是原始总线值。
    assert!(
        (sfx.volume - 0.25).abs() < f32::EPSILON,
        "音量应为 0.5×0.5=0.25，实际 {}",
        sfx.volume
    );
    assert_eq!(
        sfx.asset_dir.as_deref(),
        Some(Path::new("/rustlua-nonexistent-audio")),
        "平台要自己把逻辑名解析成 URL，必须拿到资源目录"
    );
    assert!(!sfx.looping, "音效不循环");

    let music = &records[1];
    assert_eq!(music.name, "bgm_title");
    assert_eq!(music.bus, AudioBus::Bgm);
    assert!(music.looping, "BGM 必须带循环标记（平台自己循环）");

    assert_eq!(records[2].bus, AudioBus::Ui);
}
