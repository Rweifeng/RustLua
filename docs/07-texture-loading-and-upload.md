# M3-02 PNG 加载与纹理上传约定

## 状态

- 状态：PASS
- 完成日期：2026-08-20
- 前置任务：M3-01 坐标、颜色和纹理约定

## 实施结果

- `engine_core::texture::TextureAsset`：PNG 解码为 RGBA8 CPU 像素；
- `TextureColorSpace`：区分 `Srgb` 和 `Linear` 纹理语义；
- PNG 文件和内存字节加载入口；
- RGBA8 尺寸和字节长度校验；
- `game_runner` 新增 `texture_upload` 示例；
- 示例通过 wgpu 创建设备、纹理、纹理视图，并使用 `Queue::write_texture` 上传像素；
- sRGB 纹理使用 `Rgba8UnormSrgb`，线性数据纹理使用 `Rgba8Unorm`；
- 新增 1x1 红色 PNG 测试资源。

## API

实现位置：`engine/crates/engine_core/src/texture.rs`

```rust
TextureAsset::load_png(path, TextureColorSpace::Srgb)
TextureAsset::from_png_bytes(bytes, TextureColorSpace::Srgb)
TextureAsset::from_rgba8(width, height, pixels, color_space)
```

资产提供：

- `width()`；
- `height()`；
- `pixels()`；
- `bytes_per_row()`；
- `color_space()`。

## 上传规则

- 所有图片先解码为 RGBA8；
- 每个像素 4 字节，行布局为 `width * 4`；
- mip 数量暂定为 1；
- 纹理维度暂定为 2D；
- 使用 `TEXTURE_BINDING | COPY_DST`；
- 颜色纹理按 sRGB 格式创建；
- 法线、遮罩、材质数据按线性格式创建；
- M3-02 暂不生成 mipmap，后续按 Sprite/材质需求扩展。

## 自动化测试

`engine/crates/engine_core/src/texture.rs` 覆盖：

- 1x1 PNG 解码为 RGBA8；
- 宽高和 bytes-per-row 正确；
- sRGB/Linear 语义保留；
- 空输入和非法 PNG 拒绝；
- 原始 RGBA8 尺寸校验。

## 真实 GPU 验收

命令：

```text
cargo run -p game_runner --example texture_upload --release
```

结果：

```text
texture_upload_ok adapter="NVIDIA GeForce GT 1030" dimensions=1x1 format=Rgba8UnormSrgb bytes=4 color_space=Srgb
```

这证明 PNG 已成功解码、创建为 sRGB GPU 纹理并通过 `Queue::write_texture` 上传。示例不创建窗口，适合在独立开发和 CI 环境中作为资源上传烟测。

## 边界

- 暂不支持 JPEG、GIF 或动画纹理；
- 暂不生成 mipmap；
- 暂不实现纹理缓存、热重载和资产数据库；
- 暂不提供完整 Sprite 绘制，这些属于后续 M3 任务。