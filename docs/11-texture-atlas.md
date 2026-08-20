# M3-06：纹理图集

## 目标

将一张纹理中的多个矩形区域命名并复用到 Sprite，避免每个 Sprite 单独绑定纹理；图集区域通过现有 `SpriteBatch` 继续合批绘制。

## 坐标约定

- 像素坐标原点在纹理左上角。
- 区域采用半开区间：`[x, x + width) × [y, y + height)`。
- UV 使用左上角原点约定，转换公式为：`uv_min = (x / texture_width, y / texture_height)`，`uv_max = ((x + width) / texture_width, (y + height) / texture_height)`。
- 纹理尺寸、区域尺寸不能为 0，区域不能越界或发生整数溢出。

## API

`engine_core::atlas` 提供：

- `AtlasRegion::from_pixels`：把像素矩形转换为 UV；
- `AtlasRegion::from_asset`：从 `TextureAsset` 的尺寸生成区域；
- `AtlasRegion::apply`：把区域 UV 应用到 `Sprite`；
- `SpriteAtlas`：保存命名区域；
- `SpriteAtlas::insert`：插入区域，同名插入覆盖旧区域；
- `SpriteAtlas::get`：按名称查询区域。

越界、零尺寸和无效图集尺寸返回 `AtlasError`，不会静默生成错误 UV。

## 验收

单元测试覆盖 UV 转换、越界拒绝、命名区域查询、区域应用和 SpriteBatch UV 传递。真实 GPU Demo 使用程序生成的 4×4 四色图集，注册 `red`、`green`、`blue`、`yellow` 四个 2×2 区域，并以一个纹理绑定、一个绘制批次渲染 4 个 Sprite：

```powershell
cargo run -p game_runner --example atlas_demo --release
```

预期输出包含 `regions=4 sprites=4 vertices=24 draw_calls=1 texture=4x4 target=128x128`。