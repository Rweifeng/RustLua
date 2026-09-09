# M4-05：Lua API

- 状态：PASS
- 完成日期：2026-08-20

## 实施内容

- 扩展 Lua VM 桥接，提供第一版游戏脚本 API；
- 接入输入、时间、日志、实体、Transform、生命值和最小物理查询；
- 增加场景、资源、UI、存档和应用退出接口；
- 使用带 generation 的 Entity ID，拒绝非法和过期实体；
- 将宿主错误转换为 Lua 错误，避免脚本错误导致宿主 panic；
- 增加单元测试和 Release Demo。

## 验收结果

- [x] Lua 可以创建、移动、查找和销毁实体；
- [x] Lua 可以完成受伤/扣血流程；
- [x] Lua 可以读取输入轴和 Delta Time；
- [x] Lua 可以执行物理重叠查询；
- [x] Lua API 错误可观测且不导致宿主 panic；
- [x] 单元测试通过；
- [x] Release Demo 通过；
- [x] Clippy 和格式检查通过。
