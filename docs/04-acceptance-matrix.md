# 验收矩阵

## 使用说明

本矩阵记录每个任务或里程碑的可验证结果。`PASS` 表示已完成验收，`BLOCKED` 表示实现已准备但受外部条件阻塞，`PENDING` 表示尚未开始，`FAIL` 表示验收失败。

| ID | 功能 | 前置条件 | 操作/命令 | 预期结果 | 自动化测试 | 手工证据 | 状态 | 版本 |
|---|---|---|---|---|---|---|---|---|
| M0-A01 | 项目结构 | M0-01 | 检查目录 | 所有基线目录存在 | 目录检查脚本 | `artifacts/screenshots/M0/` | PASS | M0 |
| M0-A02 | 项目章程 | M0-01 | 阅读章程 | 目标、范围、里程碑和验收门完整 | 文档检查 | 章程文件 | PASS | M0 |
| M0-A03 | 任务模板 | M0-01 | 创建示例任务 | 任务含目标、范围、测试和验收 | 模板检查 | 示例任务文件 | PASS | M0 |
| M0-A04 | 可继续开发 | M0-01 | 新 AI 会话读取文档 | 能理解当前状态和下一步 | 文档检查 | 会话验收记录 | PASS | M0 |
| M0-A05 | 质量门脚本 | M0-03 | `./scripts/verify.ps1` | 执行 metadata、fmt、clippy、test、build | PowerShell 脚本 | `artifacts/logs/quality/verify-20260820-113948.log` | PASS | M0 |
| M0-A06 | 测试入口 | M0-03 | `./scripts/test.ps1` | 执行测试质量门 | PowerShell 脚本 | `artifacts/logs/quality/verify-20260820-113952.log` | PASS | M0 |
| M0-A07 | Release 构建入口 | M0-03 | `./scripts/build-release.ps1` | 生成 Release 运行器 | PowerShell 脚本 | `artifacts/builds/release/game_runner.exe` | PASS | M0 |
| M0-A08 | CI 质量门 | M0-03 | GitHub Actions Windows runner | format、clippy、test、Debug/Release build 通过 | CI workflow | Actions 记录 | PENDING | M0 |
| M0-A09 | M0-04 文件完整性 | M0-04 | 检查验收矩阵和风险登记 | 两份文档 UTF-8 正常、字段完整、可直接使用 | 文档检查 | 文件内容 | PASS | M0 |
| M0-A10 | 风险登记 | M0-04 | 检查风险表 | 风险有概率、影响、应对策略、负责人和状态 | 文档检查 | 风险登记表 | PASS | M0 |
| M0-A11 | M0 里程碑验收 | M0-01 至 M0-04 | 执行完整回归验收 | 本地质量门、构建、Demo、文档和风险检查通过 | 回归命令集 | `tasks/done/M0-05-M0里程碑验收.md` | PASS | M0 |
| M0-A12 | M0-06 基线构建和标签 | M0-01 至 M0-05 | 执行质量门并检查 Git 标签 | 基线提交可构建且存在 `v0.0.1-baseline` | 质量门和 Git 检查 | `tasks/done/M0-06-首次基线构建和版本标签.md` | PASS | M0 |

## M0-04/M0-05 验收结论

- 验收矩阵已覆盖 M0-01、M0-03、M0-04 和 M0-05 的当前状态；
- 已区分通过、阻塞、待处理三种状态；
- M0-02/M0-03 的 Rust 命令级验证已完成，结果已回填到任务记录和本矩阵；
- M0-04 文件完整性和风险登记验收通过；
- M0-05 本地里程碑回归验收通过；
- M0-06 基线构建和 `v0.0.1-baseline` 标签验收通过，项目放行进入 M1；
- M0-A08 GitHub Actions 远程执行仍为 `PENDING`，不影响本地 M0 放行。
| M1-A01 | 窗口与应用生命周期 | M0 | `./scripts/verify.ps1`、运行 `game_runner` | 窗口创建、关闭、缩放、最小化/恢复不崩溃，生命周期日志顺序正确 | 状态机单元测试 | `artifacts/logs/M1-01-window-session.stdout.log`、`artifacts/logs/M1-01-window-interaction.stdout.log` | PASS | M1 |
| M1-A02 | wgpu 初始化与清屏 | M1-A01 | `./scripts/verify.ps1`、运行 GPU Demo 并调整窗口尺寸 | Debug/Release 均初始化 GPU、清屏、缩放后继续渲染，错误分支有处理 | workspace 测试与 Clippy | `artifacts/logs/M1-02-wgpu-session.stdout.log` | PASS | M1 |
| M1-A03 | Lua VM 桥接 | M1-A02 | `cargo test --workspace`、`./scripts/verify.ps1`、运行 Lua Demo | Lua 调 Rust API、Rust 调 Lua 函数、脚本路径/行号错误、运行时错误日志、Debug/Release 构建 | `engine_core` 单元测试 | `artifacts/logs/M1-03-lua-session.stdout.log`、`artifacts/logs/quality/verify-20260820-144431.log`、`artifacts/logs/quality/verify-20260820-144146.log` | PASS | M1 |
| M1-A04 | 日志与错误系统 | M1-A03 | `cargo test --workspace`；`./scripts/verify.ps1`；`./scripts/verify.ps1 -Release`；运行真实 Debug Demo | 控制台和文件日志包含时间、级别、target；支持模块过滤；Lua/渲染错误包含上下文和解决建议 | logging 单元测试；工作区质量门 | `engine/artifacts/logs/M1-04-logging-session-final.stdout.log`；`engine/artifacts/logs/game-runner.log`；`engine/artifacts/logs/quality/verify-20260820-151137.log`；`engine/artifacts/logs/quality/verify-20260820-151819.log` | PASS | M1 |
| M1-A05 | 固定时间步 | M1-A04 | `cargo test --workspace`、`./scripts/verify.ps1`、`./scripts/verify.ps1 -Release`、运行 Debug/Release Demo | 60Hz 固定 dt、最大追赶步数、防止螺旋死亡、暂停恢复不重放旧时间，输出 dt/accumulator/步数统计 | `time_step` 单元测试；工作区质量门 | `artifacts/logs/M1-05-fixed-timestep-session-final.stdout.log`；`artifacts/logs/M1-05-fixed-timestep-session-release.stdout.log`；`artifacts/logs/quality/verify-20260820-160225.log`；`artifacts/logs/quality/verify-20260820-160242.log` | PASS | M1 |
| M1-A06 | 输入快照 | M1-A05 | `cargo test --workspace`、`./scripts/verify.ps1`、`./scripts/verify.ps1 -Release`、运行 Debug/Release Demo | 键盘/鼠标/光标事件进入快照；支持按下、持续、释放；WASD 轴值；焦点丢失和暂停不产生卡键；Lua 可读取输入 API | `input` 单元测试；Lua 桥接测试；工作区质量门 | `artifacts/logs/M1-06-input-snapshot-session-debug.stdout.log`；`artifacts/logs/M1-06-input-snapshot-session-release.stdout.log`；`artifacts/logs/quality/verify-20260820-162504.log`；`artifacts/logs/quality/verify-20260820-162526.log` | PASS | M1 |
| M1-A07 | ????? Debug overlay | M1-A06 | `cargo test --workspace`?`./scripts/verify.ps1`?`./scripts/verify.ps1 -Release`??? Debug/Release Demo | ?? FPS?Frame ms?Update ms?Lua ms ???????????? overlay?F3 ????Release ??? | `performance` ????????????????/GPU Demo | `artifacts/logs/M1-07-overlay-session-debug.stdout.log`?`artifacts/logs/M1-07-overlay-session-debug.stderr.log`?`artifacts/logs/M1-07-overlay-session-release.stdout.log`?`artifacts/logs/M1-07-overlay-session-release.stderr.log`?`artifacts/logs/quality/verify-20260820-164230.log`?`artifacts/logs/quality/verify-20260820-164231.log` | PASS | M1 |
| M1-A08 | M1 基础运行时里程碑验收 | M1-A01 至 M1-A07 | `./scripts/verify.ps1 -KeepLog`、`./scripts/verify.ps1 -Release -KeepLog`、运行 Debug/Release Demo、检查 Git 历史 | M1-01 至 M1-07 全部 PASS；Debug/Release 质量门通过；真实窗口、GPU、Lua、固定时间步、输入快照和性能 overlay 可运行；提交可追溯并创建 `v0.1-foundation` 标签 | M1 任务记录、工作区测试、Clippy、Debug/Release 构建、真实 Demo | `artifacts/logs/quality/verify-20260820-170201.log`；`artifacts/logs/quality/verify-20260820-170204.log`；`artifacts/logs/M1-foundation-session-debug.stdout.log`；`artifacts/logs/M1-foundation-session-debug.stderr.log`；`artifacts/logs/M1-foundation-session-release.stdout.log`；`artifacts/logs/M1-foundation-session-release.stderr.log` | PASS | M1 |
| M2-A01 | EntityId 与 generation 句柄 | M1 | `cargo test -p engine_core --all-features`、`./scripts/verify.ps1 -KeepLog`、`./scripts/verify.ps1 -Release -KeepLog` | 删除实体后旧句柄失效；slot 复用生成新 generation；旧句柄不能误操作新实体；Debug/Release 质量门通过 | `entity` 单元测试；workspace Clippy、测试和构建 | `tasks/done/M2-01-EntityId.md`；`artifacts/logs/quality/verify-20260820-170812.log`；`artifacts/logs/quality/verify-20260820-170836.log` | PASS | M2 |
| M2-A02 | World 与最小组件存储 | M2-A01 | `cargo test -p engine_core --all-features`、`./scripts/verify.ps1 -KeepLog`、`./scripts/verify.ps1 -Release -KeepLog` | 创建/销毁实体；组件增删改查；单组件与双组件组合查询；删除实体清理组件；无效句柄拒绝访问 | world 单元测试、Clippy、Debug/Release 质量门 | `tasks/done/M2-02-World和最小组件存储.md`；`artifacts/logs/quality/verify-20260820-171636.log`；`artifacts/logs/quality/verify-20260820-171647.log` | PASS | M2 |
| M2-A03 | Transform 与 Velocity 系统 | M2-A02 | `cargo test -p engine_core --all-features`、`./scripts/verify.ps1 -KeepLog`、`./scripts/verify.ps1 -Release -KeepLog` | Transform 默认值；线速度更新位置；角速度更新旋转；缺失组件安全跳过；非法 dt 拒绝；销毁实体不参与更新 | transform 单元测试、Clippy、Debug/Release 质量门 | `tasks/done/M2-03-Transform和Velocity系统.md`；`artifacts/logs/quality/verify-20260820-172853.log`；`artifacts/logs/quality/verify-20260820-172857.log` | PASS | M2 |
| M2-A04 | 系统调度器 | M2-A03 | `cargo test -p engine_core --all-features`、`./scripts/verify.ps1 -KeepLog`、`./scripts/verify.ps1 -Release -KeepLog` | 阶段顺序；依赖拓扑；缺失/重复/非法依赖拒绝；循环依赖报错；系统错误和 panic 包含模块上下文；系统耗时记录 | scheduler 单元测试、Clippy、Debug/Release 质量门 | `tasks/done/M2-04-系统调度器.md`；`artifacts/logs/quality/verify-20260820-174941.log`；`artifacts/logs/quality/verify-20260820-174947.log` | PASS | M2 |
| M2-A05 | 事件队列 | M2-A04 | `cargo test -p engine_core --all-features`、`./scripts/verify.ps1 -KeepLog`、`./scripts/verify.ps1 -Release -KeepLog` | 类型化事件；发布顺序；帧边界隔离；单次消费；未消费事件清理；当前帧清空不影响待发布事件 | events 单元测试、Clippy、Debug/Release 质量门 | `tasks/done/M2-05-事件队列.md`；`artifacts/logs/quality/verify-20260820-175736.log`；`artifacts/logs/quality/verify-20260820-175741.log` | PASS | M2 |
| M2-A06 | Command 队列 | M2-A05 | `cargo test -p engine_core --all-features`、`./scripts/verify.ps1 -KeepLog`、`./scripts/verify.ps1 -Release -KeepLog` | 延迟 spawn/despawn；组件变更；命令顺序；批次只应用一次；错误包含命令上下文 | commands 单元测试、Clippy、Debug/Release 质量门 | `tasks/done/M2-06-Command队列.md`、`artifacts/logs/quality/verify-20260820-180924.log`、`artifacts/logs/quality/verify-20260820-180929.log` | PASS | M2 |
| M2-A07 | ECS 压力测试 | M2-A06 | `cargo run -p engine_core --example ecs_stress --release -- --entities 10000 --seconds 60`、`./scripts/verify.ps1 -KeepLog`、`./scripts/verify.ps1 -Release -KeepLog` | 10000 实体稳定运行；查询/更新/销毁/重建成功；实体数量无异常变化；输出耗时统计 | ecs_stress 示例、workspace 测试、Clippy、Debug/Release 质量门 | `tasks/done/M2-07-ECS压力测试.md`、`artifacts/logs/m2-07-ecs-stress-smoke.json`、`artifacts/logs/quality/verify-20260820-191742.log`、`artifacts/logs/quality/verify-20260820-191746.log` | PASS | M2 |
| M2-A08 | M2 ECS 里程碑验收 | M2-A01 至 M2-A07 | `cargo test --workspace --all-features`、`./scripts/verify.ps1 -KeepLog`、`./scripts/verify.ps1 -Release -KeepLog`、10000 实体 Release 60 秒长跑 | M2-01 至 M2-07 全部 PASS；60 秒无崩溃；实体数稳定；Debug/Release 质量门通过；创建 `v0.2-ecs` 标签 | 里程碑验收记录、workspace 测试、压力报告、质量门 | `tasks/done/M2-milestone-acceptance-v0.2-ecs.md`、`artifacts/logs/m2-07-ecs-stress-60s.json`、`artifacts/logs/quality/verify-20260820-191742.log`、`artifacts/logs/quality/verify-20260820-191746.log` | PASS | M2 |
| M3-A01 | 坐标、颜色和纹理约定 | M2 | `cargo test -p engine_core --all-features`、`./scripts/verify.ps1 -KeepLog`、`./scripts/verify.ps1 -Release -KeepLog` | 世界/屏幕转换往返误差；Y 轴方向；zoom/viewport 校验；sRGB/linear 转换；约定文档完整 | math2d 单元测试、Clippy、Debug/Release 质量门 | `tasks/done/M3-01-坐标约定.md`；`docs/06-coordinate-and-color-conventions.md`；质量门日志 | PASS | M3 |
| M3-A02 | PNG 加载与纹理上传 | M3-A01 | `cargo test --workspace --all-features`、`cargo run -p game_runner --example texture_upload --release`、`./scripts/verify.ps1 -KeepLog`、`./scripts/verify.ps1 -Release -KeepLog` | PNG 解码为 RGBA8；尺寸/行跨度正确；sRGB/Linear 格式规则正确；wgpu 纹理创建和 Queue::write_texture 上传成功 | texture 单元测试、上传示例、Clippy、Debug/Release 质量门 | `tasks/done/M3-02-加载PNG并上传纹理.md`；`docs/07-texture-loading-and-upload.md`；`assets/m3-02/red-1x1.png`；上传输出和质量门日志 | PASS | M3 |

| M3-A03 | Sprite | M3-A02 | cargo test --workspace --all-features、sprite_demo Release、verify Debug/Release | Sprite 顶点/UV/pivot/scale/rotation/tint 正确；真实 GPU 绘制成功；错误输入被拒绝 | sprite 单元测试、Clippy、Debug/Release 质量门 | tasks/done/M3-03-Sprite.md；docs/08-sprite.md；Sprite Demo 输出 | PASS | M3 |

| M3-A04 | SpriteBatch | M3-A03 | cargo test --workspace --all-features、sprite_batch_demo Release、verify Debug/Release | 1000 Sprite 可构建；排序稳定；相同纹理/图层合批；图集 UV 正确；真实 GPU 绘制成功 | SpriteBatch 单元测试、Clippy、Debug/Release 质量门 | tasks/done/M3-04-SpriteBatch.md；docs/09-sprite-batch.md；SpriteBatch Demo 输出 | PASS | M3 |

| M3-A05 | Camera2D | M3-A04 | cargo test --workspace --all-features、camera_demo Release、verify Debug/Release | 平移/缩放/旋转可用；屏幕/世界坐标互转；边界限制正确；不同视口稳定 | Camera2D 单元测试、Clippy、Debug/Release 质量门 | tasks/done/M3-05-Camera2D.md；docs/10-camera2d.md；Camera Demo 输出 | PASS | M3 |


| M3-A06 | 图集 | M3-A05 | `cargo test --workspace --all-features`、`cargo run -p game_runner --example atlas_demo --release`、`verify.ps1 Debug/Release` | 像素矩形转换为正确 UV；命名区域可查询；越界输入被拒绝；四个图集区域通过真实 GPU 单纹理单批次绘制 | atlas 单元测试、SpriteBatch 测试、Clippy、Debug/Release 质量门 | `tasks/done/M3-06-图集.md`、`docs/11-texture-atlas.md`、`atlas_demo_ok regions=4 sprites=4 vertices=24 draw_calls=1` | PASS | M3 |

| M3-A07 | 文本 | M3-A06 | `cargo test --workspace --all-features`、`cargo run -p game_runner --example text_demo --release`、`verify.ps1 Debug/Release` | ASCII 文本可布局；换行/缩放/间距/颜色有效；无效样式被拒绝；字体图集通过真实 GPU 上传并绘制 | text 单元测试、Clippy、Debug/Release 质量门 | `tasks/done/M3-07-文本.md`、`docs/12-text.md`、`text_demo_ok glyphs=17 lines=2 vertices=102` | PASS | M3 |

| M3-A08 | 2D Demo | M3-A07 | `cargo test --workspace --all-features`、`cargo run -p game_runner --example demo_2d --release`、`verify.ps1 Debug/Release` | Camera2D、Sprite、SpriteBatch、图集、文本和两张 GPU 纹理在一个真实 Demo 中联通；7 个 Sprite 生成 2 个批次；文本完成 GPU 绘制 | workspace 测试、Clippy、Debug/Release 质量门、Demo | `tasks/done/M3-08-2D-Demo.md`、`docs/13-2d-demo.md`、`demo_2d_ok sprites=7 sprite_draws=2 text_glyphs=35 text_vertices=210` | PASS | M3 |

| M4-A01 | 资源数据库 | M3 | `cargo test --workspace --all-features`、`cargo run -p game_runner --example resource_db_demo --release`、`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` | UUID 稳定；移动/重命名不丢失引用；重复注册幂等；失败状态持久化；删除提示依赖者；数据库可安全保存和重新加载 | resource_db 单元测试、Release Demo、格式检查、Clippy | `tasks/done/M4-01-资源数据库.md`；`docs/14-resource-database.md`；`resource_db_demo_ok` | PASS | M4 |

| M4-A02 | 导入缓存 | M4-A01 | `cargo test --workspace --all-features`、`cargo run -p game_runner --example import_cache_demo --release`、`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` | 未变化资源复用缓存；源文件或导入器版本变化重新导入；失败不替换有效缓存；清空缓存可重建；首批资源扩展名可识别 | import_cache 单元测试、Release Demo、格式检查、Clippy | `tasks/done/M4-02-导入缓存.md`；`docs/15-import-cache.md`；`import_cache_demo_ok` | PASS | M4 |

| M4-A03 | 场景格式 | M4-A02 | `cargo test --workspace --all-features`、`cargo run -p game_runner --example scene_format_demo --release`、`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` | 场景可保存/加载/再次保存；实体树、组件和资源 UUID 完整；未知记录报告行号；版本不支持时明确拒绝；原子写入 | scene 单元测试、Release Demo、格式检查、Clippy | `tasks/done/M4-03-场景格式.md`；`docs/16-scene-format.md`；`scene_format_demo_ok` | PASS | M4 |


| M4-A04 | 预制体 | M4-A03 | `cargo test -p engine_core --all-features`、`cargo run -p game_runner --example prefab_demo --release`、`cargo fmt --all`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` | 可从场景根实体提取预制体并保存/加载；同一预制体可重复实例化且实例根 Entity ID 唯一；名称、组件和元数据覆盖值生效；保留覆盖值重建实例；非法版本、实体和覆盖字段明确拒绝 | prefab 单元测试、Release Demo、格式检查、Clippy | `tasks/done/M4-04-预制体.md`；`docs/17-prefab.md`；`prefab_demo_ok entities=4 instances=2 unique_roots=true override=25 roundtrip=true` | PASS | M4 |


| M4-A05 | Lua 游戏 API | M4-A04 | `cargo test -p engine_core --all-features`、`cargo run -p game_runner --example lua_api_demo --release`、`cargo fmt --all`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` | Lua 可调用日志、输入、时间、实体、Transform、生命值、物理查询、场景、资源、UI、存档和退出 API；非法/过期 Entity ID 明确报错；API 错误不会导致宿主 panic | script 单元测试、Release Demo、格式检查、Clippy | `tasks/done/M4-05-Lua-API.md`；`docs/18-lua-api.md`；`lua_api_demo_ok update=lua-api-ok quit=false` | PASS | M4 |


| M4-A06 | Lua 热重载 | M4-A05 | `cargo test --workspace --all-features`、`cargo run -p game_runner --example lua_hot_reload_demo --release`、`cargo fmt --all`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` | 脚本未变化返回 Unchanged；修改后可加载新模块；候选脚本加载失败时保留旧 VM；支持 on_create/on_destroy；闭包、协程和 Lua 全局状态不迁移的限制有文档说明 | script 单元测试、Release Demo、格式检查、Clippy | `tasks/done/M4-06-Lua-热重载.md`；`docs/19-lua-hot-reload.md`；`lua_hot_reload_demo_ok unchanged=true reloaded=true rollback=true update=v2` | PASS | M4 |


| M4-A07 | Lua 脚本调试 | M4-A06 | `cargo test --workspace --all-features`、`cargo run -p game_runner --example lua_script_debug_demo --release`、`cargo fmt --all`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` | 语法/运行时错误包含路径和行号或调用栈文本；诊断记录支持过滤和清理；相同错误聚合计数；发布运行路径不自动开启调试控制台 | script 单元测试、Release Demo、格式检查、Clippy | `tasks/done/M4-07-脚本调试.md`；`docs/20-lua-script-debug.md`；`lua_script_debug_demo_ok diagnostics=1 deduplicated=true filtered=true stack=true` | PASS | M4 |

| M4-A08 | 场景脚本 Demo | M4-A07 | `cargo test --workspace --all-features`、`cargo run -p game_runner --example scene_script_demo --release`、`cargo fmt --all`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` | 场景包含玩家、敌人、拾取物、门和触发器；胜利/失败规则全部由 Lua 驱动；场景重新加载结果一致；Lua 文件修改后热更新改变结果；提供脚本教程 | scene/script 单元测试、Release Demo、格式检查、Clippy | `tasks/done/M4-08-场景脚本Demo.md`；`docs/21-scene-script-demo.md`；`scene_script_demo_ok entities=5 rules=lua victory=true reload_consistent=true hot_reload=failure` | PASS | M4 |

## M5 编辑器 MVP（计划）

| ID | 功能 | 前置 | 主要验收 | 自动化证据 | 状态 | 版本 |
|---|---|---|---|---|---|---|
| M5-A01 | 编辑器启动与项目打开 | M4 | 独立 editor 可启动；合法项目检查成功；非法路径清晰失败 | editor 单测、CLI check、Debug/Release 构建、GUI 进程启动 | PASS | M5 |
| M5-A02 | 项目与场景文档模型 | M5-A01 | 新建/打开/保存/另存为；原子写入；dirty 正确 | 临时目录测试、场景 roundtrip、质量门 | PASS | M5 |
| M5-A03 | 场景树与实体编辑 | M5-A02 | 选择、创建、删除、重命名、父子关系一致 | 状态模型测试、GUI 手工记录 | PENDING | M5 |
| M5-A04 | Inspector 第一批组件 | M5-A03 | Transform、Sprite 引用、Lua 字段编辑和校验 | 字段测试、保存重载测试 | PENDING | M5 |
| M5-A05 | 资源浏览器与刷新 | M5-A04 | 资源扫描、筛选、刷新、失败状态可见 | ResourceDatabase 集成测试、GUI 记录 | PENDING | M5 |
| M5-A06 | Lua 绑定与 Console | M5-A04 | 脚本绑定、运行、错误路径/行号/调用栈可见 | Lua 回归测试、诊断测试 | PENDING | M5 |
| M5-A07 | Play/Stop 运行态隔离 | M5-A06 | 运行时修改不污染编辑态；100 次 Play/Stop 稳定 | 快照一致性、循环回归测试 | PENDING | M5 |
| M5-A08 | Undo/Redo | M5-A03 | 属性、创建、删除、重命名、父子关系可逆 | 100 次随机操作一致性测试 | PENDING | M5 |
| M5-A09 | 自动保存与崩溃恢复 | M5-A02 | 恢复有效版本；损坏恢复不覆盖主文件 | 故障注入、临时文件测试 | PENDING | M5 |
| M5-A10 | 编辑器 MVP 综合验收 | M5-A01 至 M5-A09 | 十步工作流完成；无 P0/P1；打 v0.5 标签 | 综合 Demo、全量质量门、验收记录 | PENDING | M5 |
