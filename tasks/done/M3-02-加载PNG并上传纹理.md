# M3-02 加载 PNG 并上传纹理

## 状态

- 状态：PASS
- 完成日期：2026-08-20
- 前置任务：M3-01 `f397456`

## 完成内容

1. 新增 `image` PNG 解码依赖；
2. 新增 `engine_core::texture` 模块；
3. 实现 PNG 文件/内存字节加载；
4. 统一解码为 RGBA8；
5. 增加 sRGB/Linear 纹理语义；
6. 增加非法输入、尺寸和像素长度校验；
7. 新增 1x1 红色 PNG 测试资源；
8. 新增 `game_runner` 的 `texture_upload` Release 示例；
9. 使用 wgpu 创建设备和 RGBA8 纹理，并通过 `Queue::write_texture` 完成上传。

## 验收矩阵

| 验收项 | 结果 | 证据 |
|---|---|---|
| PNG 可加载 | PASS | `TextureAsset` 单元测试，62 项 workspace 测试通过 |
| 解码为 RGBA8 | PASS | 1x1 红色纹理像素为 `[255, 0, 0, 255]` |
| 尺寸和行跨度正确 | PASS | `width=1`、`height=1`、`bytes_per_row=4` |
| sRGB 格式规则正确 | PASS | GPU 示例输出 `Rgba8UnormSrgb` |
| GPU 纹理创建成功 | PASS | NVIDIA GeForce GT 1030 真实 wgpu 设备 |
| GPU 像素上传成功 | PASS | `Queue::write_texture` 执行完成且示例正常退出 |
| 非法 PNG 拒绝 | PASS | 空输入和非法字节测试 |
| Debug/Release 质量门 | PASS | 完整质量门日志 |

## 验收命令

```text
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --workspace
cargo build --workspace --release
cargo run -p game_runner --example texture_upload --release
./scripts/verify.ps1 -KeepLog
./scripts/verify.ps1 -Release -KeepLog
```

## 产物

- `assets/m3-02/red-1x1.png`；
- `engine/crates/engine_core/src/texture.rs`；
- `engine/crates/game_runner/examples/texture_upload.rs`；
- `docs/07-texture-loading-and-upload.md`。