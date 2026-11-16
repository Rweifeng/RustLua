//! M82-B2：`texture` 模块的 Lua 面（生成/释放/宿主取走上传）。
//!
//! 覆盖三件事：`generate` 返回自增 id 且把像素登记进注册表；非法 spec/kind 抛错（不静默成功）；
//! `free` 移除句柄、`drain_generated_textures` 只交还仍存活的贴图（键是哨兵 `__texture__?<id>`）。

use super::*;

#[test]
fn texture_generate_registers_drains_and_frees() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script(
        "local a = texture.generate{kind='checker', size=8, scale=2, colors={{255,0,0},{0,0,255}}}\n\
         assert(a == 0, 'first id is 0')\n\
         local b = texture.generate{kind='noise', size=16, seed=7}\n\
         assert(b == 1, 'second id is 1')\n\
         assert(pcall(function() texture.generate{kind='bogus'} end) == false, 'unknown kind raises')\n\
         assert(pcall(function() texture.generate{size=8} end) == false, 'missing kind raises')\n\
         assert(pcall(function() texture.generate{kind='checker', size=0} end) == false, 'size 0 raises')\n\
         assert(pcall(function() texture.generate{kind='checker', colors={{255,0}}} end) == false, '2-channel color raises')\n\
         assert(texture.free(0) == true, 'free existing')\n\
         assert(texture.free(0) == false, 'second free is false')\n\
         function update() end",
    );
    runtime.execute_project_script(&path).unwrap();
    // id 0 在 drain 前已 free ⇒ 只剩 id 1（noise）待上传。
    let drained = runtime.drain_generated_textures();
    assert_eq!(drained.len(), 1, "free 掉的 id 不该再被上传");
    assert_eq!(drained[0].0, "__texture__?1");
    assert_eq!(drained[0].1.width(), 16);
    // 取走后清空：同一帧不重复上传。
    assert!(runtime.drain_generated_textures().is_empty());
    let _ = fs::remove_file(path);
}

/// 默认值：只写 `kind` 时 size=64/scale=8/seed=0，生成 64² 的不透明贴图。
#[test]
fn texture_generate_uses_spec_defaults() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script(
        "local id = texture.generate{kind='gradient'}\n\
         assert(id == 0, 'id')\n\
         function update() end",
    );
    runtime.execute_project_script(&path).unwrap();
    let drained = runtime.drain_generated_textures();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].1.width(), 64);
    assert_eq!(drained[0].1.height(), 64);
    assert!(
        drained[0]
            .1
            .pixels()
            .iter()
            .skip(3)
            .step_by(4)
            .all(|a| *a == 255),
        "默认调色板 alpha 必须不透明"
    );
    let _ = fs::remove_file(path);
}

/// `texture.from_text` 用工程字体把文本栅格化：返回句柄、进 drain、同文本同 spec 逐字节一致；
/// 空文本 / 越界字号抛错（不静默）。
#[test]
fn texture_from_text_rasterizes_with_project_font() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.set_project_root(production_sample_root());
    let path = script(
        "local a = texture.from_text('AB 12', {size=16, color={255,255,255}, bg={0,0,0,255}})\n\
         assert(a == 0, 'first from_text id is 0')\n\
         local b = texture.from_text('AB 12', {size=16, color={255,255,255}, bg={0,0,0,255}})\n\
         assert(b == 1, 'second id is 1')\n\
         assert(pcall(function() texture.from_text('') end) == false, 'empty text raises')\n\
         assert(pcall(function() texture.from_text('x', {size=2}) end) == false, 'tiny size raises')\n\
         function update() end",
    );
    runtime.execute_project_script(&path).unwrap();
    let drained = runtime.drain_generated_textures();
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].0, "__texture__?0");
    assert!(drained[0].1.width() > 4 && drained[0].1.height() > 4);
    // 同文本同 spec：两句逐字节相同（字体 + 光栅器确定的体现）。
    assert_eq!(drained[0].1.pixels(), drained[1].1.pixels());
    let _ = fs::remove_file(path);
}
