# M19-00 Tilemap 编辑器

## 状态
BACKLOG

## 类型
[编辑器]

## 目标
多层 Tilemap 编辑：地面/装饰/前景/碰撞/交互层、自动铺边、水面动画、阴影遮挡、tile 元数据与可种植属性。

## 背景
缺口清单 4.3 要求多层 Tilemap、自动铺边、水面动画、阴影、遮挡、Tile 元数据和可种植属性。这是农场内容生产的主要工具，直接服务 M16-04 的耕地元数据格式。

## 输入
- `engine/crates/editor/`（场景视图/Inspector 基础）
- Tilemap 组件格式（`tilemap.rs`、场景格式 `16-scene-format.md`）
- M16-04 的 tile 元数据约定

## 不做什么
- 不做图层管理多撤销栈（沿用 Undo/Redo）；
- 不做程序化生成地形；
- 不做编辑器内 Tile 动画预览以外的高级特效。

## 依赖
M16-04 的数据格式

## 验收标准
1. 在场景视图可编辑多层 Tilemap：选择层（地面/装饰/前景/碰撞/交互）画笔绘制/擦除。
2. 自动铺边：相邻同类 tile 自动选边角帧（数据驱动规则）。
3. 水面动画 tile 在编辑器内可标记并运行时播放。
4. 碰撞层可视化与 `solid` 一致；tile 元数据（含 `tilled` 可种植属性）可在 Inspector 编辑并保存到场景/地图。
5. 编辑结果在 Game 视图与运行时即时生效。

## 测试命令
```powershell
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace --all-features && cargo build --workspace --release
```
实机：编辑器画一张多层农田 Tilemap → Play 后碰撞/元数据/动画正确。

## 风险
- 图层数据与 Tilemap 组件格式冲突 → 扩展组件字符串格式并向后兼容；
- 编辑器渲染与运行时渲染分叉 → 沿用场景提取统一（复盘结论 §9.1 方向）。

## AI 输出（实施后填写）
- 已完成内容、修改文件、验证结果、已知问题、下一步建议。
