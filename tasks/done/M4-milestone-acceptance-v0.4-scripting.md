# M4 里程碑验收：v0.4-scripting

- 状态：PASS
- 验收日期：2026-08-20
- Git 标签：`v0.4-scripting`
- 验收基线：提交 `1687781`（M4-08 完成）

## 验收范围

M4 资产与脚本阶段全部任务：

- M4-01 资源数据库；
- M4-02 导入缓存；
- M4-03 场景格式；
- M4-04 预制体；
- M4-05 Lua API；
- M4-06 Lua 热重载；
- M4-07 脚本调试；
- M4-08 场景脚本 Demo。

## 质量门结果

| 检查项 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace --all-features` | PASS，98 passed; 0 failed |
| `git diff --check` | PASS |
| M4 Release Demo | 8/8 PASS |

## Release Demo 结果

```text
resource_db_demo_ok records=1 stable_id=true failed_status=Some(Failed) dependents=1
import_cache_demo_ok first_reused=false hit_reused=true changed_reused=false rebuilt_reused=false cache_files=1
scene_format_demo_ok version=1 entities=2 roundtrip=true error_has_line=true
prefab_demo_ok entities=4 instances=2 unique_roots=true override=25 roundtrip=true
lua_api_demo_ok update=lua-api-ok quit=false
lua_hot_reload_demo_ok unchanged=true reloaded=true rollback=true update=v2
lua_script_debug_demo_ok diagnostics=1 deduplicated=true filtered=true stack=true
scene_script_demo_ok entities=5 rules=lua victory=true reload_consistent=true hot_reload=failure
```

## 里程碑结论

- 资源数据库、导入缓存、场景格式和预制体链路可用；
- Lua 可访问实体、Transform、生命值、输入、物理、场景、资源、UI、存档和退出 API；
- Lua 热重载具备失败回滚和生命周期回调；
- 脚本错误具备路径、行号/调用栈、过滤、去重和清理能力；
- 场景脚本 Demo 的游戏规则全部由 Lua 驱动；
- M4 验收通过，创建标签 `v0.4-scripting`。

## 已知边界

- 场景和资源 API 当前仍有内存适配层，编辑器和真实资源导入工作流留给后续阶段；
- 当前场景脚本 Demo 是 CPU/脚本验收 Demo，尚未接入完整渲染实体；
- Lua 闭包、协程和全局状态不跨热重载 VM 迁移；
- 当前验收覆盖单工作区、单进程基线，不代表跨平台发布包验收。