# M8-02 glTF/GLB CPU 导入与资源缓存

## 状态

ACCEPTED

## 目标

支持 glTF 2.0 `.gltf`/`.glb` 的 CPU 侧静态 Mesh、节点、基础材质和纹理引用读取，并接入现有 ImportCache 增量缓存，不改变 M7 缓存文件格式。

## 不做什么

- 不实现 GPU Mesh 上传、材质绑定、深度或渲染；
- 不实现 skin、动画、morph target 或任意 glTF 扩展；
- 不重写场景格式和 ResourceDatabase。

## 技术方案

- 新增 `engine_core::gltf_asset::GltfAsset` CPU 导入模型；
- 使用成熟 `gltf` crate 读取 `.gltf` 和 `.glb`；
- 支持三角形 primitive、POSITION、可选 NORMAL/TEXCOORD_0、索引、节点层级、节点矩阵、基础 PBR base color 和纹理索引；
- 无索引 primitive 自动生成顺序索引；非三角形、缺失 POSITION、属性数量不一致和越界索引返回带路径上下文的错误；
- ImportCache 对 glTF/GLB 执行真实内容校验，assetc 导入器版本升级为 `assetc-m8-02`；失败导入令 `assetc import` 返回非成功结果。

## 修改范围

- `engine/Cargo.toml`
- `engine/Cargo.lock`
- `engine/crates/engine_core/Cargo.toml`
- `engine/crates/engine_core/src/gltf_asset.rs`
- `engine/crates/engine_core/src/import_cache.rs`
- `engine/crates/engine_core/src/lib.rs`
- `engine/crates/assetc/src/lib.rs`

## 验收标准

- 自制 GLB 能解析为 1 个 Mesh、1 个 primitive、3 个顶点和索引 `[0,1,2]`；
- 非法 GLB 错误包含资源路径；
- 有效 GLB 首次导入后第二次复用缓存；源损坏时返回失败且不替换原缓存；
- 真实 `game` 项目 21 个资源首次导入成功，第二次 21 个全部复用；
- workspace fmt、Clippy、测试、Debug/Release 构建通过。

## 验收证据

- `engine/artifacts/logs/m8-02-import.log`
- `cargo test -p engine_core gltf_asset`：2 passed
- `cargo test -p engine_core import_cache::tests::glb_import_is_validated_and_cached_incrementally`：1 passed
- `cargo test -p assetc import`：3 passed
- 真实项目导入：`total=21 imported=21 reused=0 failed=0`
- 真实项目二次导入：`total=21 imported=0 reused=21 failed=0`

## 已知限制

当前只建立 CPU 导入模型，未将 Mesh 接入 GPU；材质只读取基础颜色和纹理索引；动画和 skin 留给 M8-07。
