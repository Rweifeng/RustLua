# Lua 模块 `mesh`

> 运行期 API 面基线里本模块有 4 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

`mesh` 给实体挂/换 3D 网格与材质覆盖。网格路径来自 `.glb/.gltf`，或引擎哨兵
`__ground_plane__`/`__terrain__`/`__water__`；材质覆盖是项目相对 `.material` 路径，
`nil`/空串表示回到 glTF 内嵌材质。绑定不校验材质文件是否存在，渲染器加载失败会
告警并回退，实体不会变不可见。

## `mesh.clear(id: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:335`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

移除实体的 `MeshRenderer`，3D 网格立刻不再绘制；之后 `mesh.get` 返回 `nil`。

- 参数：`id` 实体。
- 返回：`true`。
- 失败：实体不存在/已死亡，或世界不可用时报错。
- 示例：`mesh.clear(id)`

## `mesh.get(id: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:321`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读当前网格路径；实体没有 `MeshRenderer` 时返回 `nil`，不是空串。

- 参数：`id` 实体。
- 返回：网格路径（String）或 `nil`。
- 失败：实体不存在/已死亡时。
- 示例：`local path = mesh.get(id)`

## `mesh.set(id: i64, path: String)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:283`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

给实体挂/换 3D 网格。路径可以是项目相对 `.glb/.gltf`，也可以是内置哨兵；替换不会
清掉已有的材质覆盖。

- 参数：`id` 实体；`path` 网格路径。
- 返回：`true`。
- 失败：路径不合 `MeshRenderer` 文法、实体不存在/已死亡时。
- 示例：`mesh.set(id, "__water__?size=10&res=8&y=-0.33")`

## `mesh.set_material(id: i64, path: Option<String>)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:299`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

给已有 `MeshRenderer` 的实体设置 `.material` 覆盖；这是 X-ray/半透明外壳一类
逐实例观感的入口。传 `nil` 或空串恢复 glTF 内嵌材质。

- 参数：`id` 实体；`path` 项目相对 `.material` 路径，或 `nil`。
- 返回：`true`。
- 失败：实体没有 `MeshRenderer`（不能只改材质而没有网格），或实体不存在/已死亡时。
- 示例：
  ```lua
  mesh.set_material(nest, "assets/materials/nest_xray.material")
  mesh.set_material(nest, nil) -- 回退
  ```
- 注意：渲染器在首帧加载材质；路径写错只告警并回退，不报错。
