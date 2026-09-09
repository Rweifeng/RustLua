# M7-01：assetc 资源构建工具

- 状态：ACCEPTED（待 M7 里程碑汇总验收）
- 验收日期：2026-08-22
- 所属里程碑：M7 工程化与发布
- 前置任务：M6 里程碑验收

## 目标

提供 CLI 资源构建工具：扫描、注册、增量导入、清理、打包，为发布配置（M7-02）提供资源管线。

## 实施

- 新增 `engine/crates/assetc`（bin + lib，仅依赖 engine_core，无窗口/GPU）；
- 复用 `ResourceDatabase`（`.cache/resources.db`）与 `ImportCache`（`.cache/import-cache.db`、`.cache/cache/`）；
- 命令：`scan` / `import` / `clean` / `list` / `package -o <dir>` / `--check` / `--help`；
- `package` 复制 `project.toml` + `scenes/` + `scripts/` + `assets/`，生成 `manifest.txt`（路径/大小/FNV-1a 校验和）；
- 跳过 `.cache/.temp/.git/target`；失败返回非零退出码 + 可读报告。

## 验收结果

- 真实游戏项目实测：`scan total=22 supported=19 unsupported=3 failed=0`；`import total=19 imported=19 reused=0 failed=0`；重复 import `reused` 全为 true（增量生效）；`clean` 后重导入正常；
- `package` 生成 22 文件清单，`manifest.txt` 含路径/大小/校验和；
- 单元测试 4 个（scan 注册/增量复用/clean 清理/package 清单）全过；
- fmt / clippy -D warnings / Debug / Release 构建通过。

## 证据

- `assetc --check` → `assetc_check_ok scanned=1 imported=1 reused=0`
- `assetc scan/import/list/package/clean` 实测输出（本会话）
- `tasks/done/M7-01-assetc资源构建工具.md`

## 已知限制

- `project.toml` 与 `.scene` 场景文件不进入导入缓存（ImportCache 不支持其类型），作为数据文件直接随 pack 复制；
- 清单校验和为 FNV-1a（非加密，仅完整性比对）。

## 建议下一步

- M7-02 发布配置：`--pack` 加载发布资源目录 + 启动期资源校验。
