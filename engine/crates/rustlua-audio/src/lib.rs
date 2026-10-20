pub mod audio;
/// 平台输出设备：桌面临 `rodio`，wasm/微信是 no-op stub。`rodio` 只在非 wasm 目标编译
/// （见 `Cargo.toml` 的 target 条件依赖与 `docs/62` P1 第 4 条），所以 `audio.rs` 里没有 cfg。
pub(crate) mod device;
/// `audio` 的 Lua 绑定（`docs/62` P1：绑定下沉到 owning crate；`rustlua-scripting` 不再依赖本 crate）。
pub mod lua;
pub mod sink;
/// 空间化：听者位姿 → 增益/声像的唯一来源（M74-01；纯函数，可在 headless 用例里逐值断言）。
pub mod spatial;
