# M3-01 坐标、颜色和纹理约定

## 状态

- 状态：PASS
- 完成日期：2026-08-20
- Git 提交：e2d5ec7

## 目标

为 2D 渲染固定世界坐标、屏幕坐标、相机换算、颜色空间和纹理颜色类型，避免后续 wgpu、Sprite、图集和材质实现出现坐标翻转或颜色错误。

## 已实现 API

实现位置：`engine/crates/engine_core/src/math2d.rs`

- `WORLD_RIGHT = (1, 0)`、`WORLD_UP = (0, 1)`；
- `Viewport::new(width, height)`：拒绝非有限或非正尺寸；
- `Camera2D::new(center, zoom)`：`zoom` 表示每个世界单位对应的屏幕像素数，必须为有限正数；
- `Camera2D::world_to_screen` 与 `screen_to_world`；
- `SrgbColor::to_linear` 与 `LinearColor::to_srgb`；
- RGB 使用标准 sRGB 转换曲线，alpha 保持线性；
- 输入颜色拒绝 NaN/Infinity，转换时将通道限制到 `[0, 1]`。

## 固定约定

### 世界空间

- 使用右手坐标系；
- X 轴向右，Y 轴向上；
- 世界单位约等于米；
- 世界原点由场景语义决定，默认相机中心可放在原点；
- 1 世界单位不等于固定像素数，由 `Camera2D.zoom` 决定。

### 屏幕空间

- 使用左上角为原点的像素坐标；
- X 轴向右，Y 轴向下；
- viewport 宽高必须为正且有限；
- 相机中心映射到 viewport 中心；
- 世界向上移动，在屏幕上表现为 Y 像素值减小。

转换公式：

```text
screen_x = (world_x - center_x) * zoom + viewport_width / 2
screen_y = viewport_height / 2 - (world_y - center_y) * zoom

world_x = (screen_x - viewport_width / 2) / zoom + center_x
world_y = (viewport_height / 2 - screen_y) / zoom + center_y
```

### 颜色空间

- 颜色纹理和面向美术的颜色输入默认按 sRGB 理解；
- 光照、混合和材质计算使用线性空间；
- RGB 经过 sRGB/线性转换，alpha 不做 gamma 转换；
- 超出 `[0, 1]` 的通道在转换时 clamp；非有限输入直接报错；
- 黑、白、灰和往返误差由 `math2d` 单元测试覆盖。

### 纹理类型

M3-01 只固定语义，不实现图片解码或 GPU 上传：

| 纹理用途 | 颜色空间 | 后续 GPU 格式规则 |
|---|---|---|
| Base Color、Sprite、UI、颜色纹理 | sRGB 输入 | 使用 sRGB GPU 纹理格式 |
| 法线、粗糙度、金属度、遮罩、深度 | 线性数据 | 使用线性 GPU 纹理格式 |
| Alpha | 线性 | 不进行 gamma 转换 |

图片加载和 GPU 格式选择分别在 M3-02/M3-04 落地，必须遵守本约定。

## 自动化验收

在 `engine` 目录执行：

```text
cargo test -p engine_core --all-features
```

新增测试覆盖：

- 原点映射到 viewport 中心；
- 世界右方向映射到屏幕右方向；
- 世界上方向映射到屏幕上方；
- 多 viewport、多 zoom 的坐标往返误差；
- 非法 viewport、zoom、非有限坐标拒绝；
- sRGB 黑/灰/白参考值；
- sRGB/线性往返误差；
- alpha 不进行 gamma 转换；
- 越界颜色通道 clamp；
- 非有限颜色拒绝。

## 验收结论

M3-01 已通过代码级验收。当前尚未有 GPU 渲染器，因此“颜色测试图”不在本任务范围内；标准颜色参考值、转换往返和纹理语义规则已由单元测试与本文件替代，真实 GPU 视觉验收放到 M3-02 之后的渲染 Demo。