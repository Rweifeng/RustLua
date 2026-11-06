# Lua 模块 `camera`

> 运行期 API 面基线里本模块有 5 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

`camera` 是**透视相机 payload** 的读写与选择链：`set_active` 只认带 `Camera` 组件的
实体，由它的投影决定服务 3D 世界还是 2D overlay；`get`/`set` 读写 position/target/fov。
`set_clip_plane` 是会话级世界空间裁剪平面，渲染器每帧从活动相机读取，不写场景文件。

## `camera.get(id: i64)`

> 绑定：`engine/crates/rustlua-camera/src/camera_lua.rs:172`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `camera.get_active()`

> 绑定：`engine/crates/rustlua-camera/src/camera_lua.rs:164`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `camera.set(id: i64, x: f32, y: f32, z: f32, tx: f32, ty: f32, tz: f32, fov: Option<f32>)`

> 绑定：`engine/crates/rustlua-camera/src/camera_lua.rs:204`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `camera.set_active(id: i64)`

> 绑定：`engine/crates/rustlua-camera/src/camera_lua.rs:141`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `camera.set_clip_plane(id: i64, nx: f32, ny: f32, nz: f32, d: f32)`

> 绑定：`engine/crates/rustlua-camera/src/camera_lua.rs:249`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

设置/清除相机的**会话级世界空间裁剪平面**：保留 `n·x + d >= 0` 的一侧，负侧
片元丢弃。用于 X-ray/剖面一类的临时切面；不改场景组件串，重启/重载后回到“不裁剪”。

- 参数：`id` 透视相机实体；`nx, ny, nz` 平面法线（内部归一化）；`d` 平面偏移。
  四个数全为 0 表示清除裁剪。
- 返回：`true`。
- 失败：数值非有限；法线为零但 `d` 非零；实体不存在/已死亡；实体没有 `Camera3D`。
- 示例：
  ```lua
  -- 只显示世界 y >= 1.0 的部分（水平切面）
  camera.set_clip_plane(cam_id, 0, 1, 0, -1.0)
  -- 清除
  camera.set_clip_plane(cam_id, 0, 0, 0, 0)
  ```
- 注意：裁剪只在 **PBR** 片元着色器生效（游戏默认开 PBR）；旧的非 PBR 回退路径
  不受影响。阴影 pass 不做裁剪，切面边缘可能留下阴影。
