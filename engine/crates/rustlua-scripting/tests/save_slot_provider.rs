//! 平台存档槽的行为契约（WP6-07）。
//!
//! 为什么单独开一个集成测试文件：provider 是**进程级一次性安装**的（与 `vfs` 同一个理由：
//! 谁最后装的会变成一个查不出来的问题）。单元测试跑在同一个进程里，装了它就会把
//! `save_data.rs` 那一批"走文件系统"的测试一起改道；独立的集成测试各自一个进程，互不干扰。

use std::collections::BTreeMap;
use std::sync::Mutex;

use rustlua_infra::save_slot::{self, SaveSlotProvider};
use rustlua_scripting::script::LuaRuntime;

/// 内存槽：记下收到的键，便于断言"引擎把键算成了什么"。
#[derive(Default)]
struct MemorySlot(Mutex<BTreeMap<String, String>>);

impl SaveSlotProvider for MemorySlot {
    fn read(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }

    fn write(&self, key: &str, text: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), text.to_owned());
        Ok(())
    }
}

/// 装了平台槽之后，`save.commit()` / `save.load()` 走槽，**不碰文件系统**。
///
/// 这条测试盯着的是 wasm 上那个"存不下来"的真问题：没有槽时 commit 恒 false、load 恒 missing。
/// 装槽后必须是一次真的往返：写进去 → 清内存 → 从槽里读回来。
/// 同时钉住键名与 JS 世界一致（`rustlua:save`）——不一致就是"换了通道读不到旧档"。
#[test]
fn save_commit_and_load_go_through_the_platform_slot() {
    let slot = Box::new(MemorySlot::default());
    // 路径指向一个**打不开的地方**：只要还有任何一条代码路径偷偷走 std::fs，这条就会红。
    let _ = std::fs::remove_file("/rustlua-nonexistent-root/save.sav");
    save_slot::set_provider(slot).expect("provider 每个进程只能装一次");

    let mut runtime = LuaRuntime::new().expect("建 Lua 运行时");
    runtime.set_save_path(std::path::PathBuf::from(
        "/rustlua-nonexistent-root/save.sav",
    ));

    let out = runtime
        .execute_console(
            "save.write('k','v'); local ok = save.commit(); save.clear(); \
             local loaded = save.load(); \
             return tostring(ok) .. '/' .. tostring(loaded) .. '/' .. tostring(save.read('k'))",
        )
        .expect("console 执行");

    assert_eq!(
        out.trim_matches('"'),
        "true/true/v",
        "commit 必须真写进槽、load 必须真读回来"
    );

    // 键名与旧运行时逐字一致：默认档就是 `rustlua:save`。
    assert_eq!(
        save_slot::read("rustlua:save").is_some(),
        true,
        "引擎必须用与旧运行时同一把键写槽"
    );
    assert!(
        !std::path::Path::new("/rustlua-nonexistent-root/save.sav").exists(),
        "装了槽之后不该再去碰文件系统"
    );
}
