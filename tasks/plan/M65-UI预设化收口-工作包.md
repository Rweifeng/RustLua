
**settings.toml → project.toml 配置整合（✅ 2026-10-07）**：用户要求把 `settings.toml` 的整个 `[input]` 段（map + 死区 + 导航，含 Bug4 的 move padaxis）迁到 `project.toml`，然后删除 `settings.toml`。
- 引擎：`SETTINGS_FILE` 常量 settings.toml → **project.toml**（一处辐射 input/audio 全部读写；upsert 逻辑保留其余段，对 project.toml 的 [project] 段天然兼容）；`user_dir.rs` settings_file() 与 seed_settings 源都改为 project.toml（发布首次运行从 pack 复制 project.toml → 用户目录）；package.ps1 settings 打包段改打包 project.toml（微信改名 project.toml.txt 已有）；微信 wechat-adapter 自检探针删 settings.toml 行；editor「项目设置」面板读写切 project.toml；Lua 注释/用户可见文本（改键说明、main.lua 日志、audio/input/hints/pause_menu）全部同步。
- 门维护：m20 两门的 SETTINGS_TEMPLATE 补完整 [project] 段（main.lua 启动校验窗口尺寸，否则模板覆盖 project.toml 后 [project] 丢 → boot 红）；m20_audio 两处 M71 遗留旧 id 断言（qst:row7/shp:row9，预设化后已不存在）改为数据断言/删除（渲染由 M65/68 黄金门覆盖）；audio.rs 单测路径同步。
- 验证：m19 全绿（clips=16 quest_rows=16 volume=0.5）；m20_audio 全绿（settings=project.toml buses=0.45/0.30/0.70/0.95）；m20_gamepad 14 章全绿（bindings=36 move_up=padaxis:1+）；m22 全绿（keys=194 settings=v1→v2）；rustlua-infra 34 单测 + rustlua-scripting 110 单测全过；game/editor 编译通过；settings.toml 已删除（工程根）。
- 发布产物（builds/）为迁移前旧包，下次 package.ps1 重发自动带 project.toml（含 [input]/[audio] 段）与新 seed 逻辑。

**release.toml 退役 → project.toml `release = true` 标记（✅ 2026-10-07）**：用户问「release.toml 用 project.toml 的 name 取代不」后拍板「project.toml 加个 release = true 标记」——发布包判定从「pack 旁存在 release.toml」改为「pack 目录内 project.toml 带 release = true」（或 `game --pack` 显式）。
- 引擎：`project_config.rs` `publish_name` 改读 project.toml（`publish_name` 键 → 回退 `name`）+ 新增 `release_flag()`；删 `RELEASE_TOML` 常量与读取，单测改写；`user_dir.rs` `is_release_pack` = 目录名 pack **且** pack 内 project.toml `release_flag` 为真（或 explicit_pack），`app_name` 简化为 publish_name → DEFAULT，`install_pack`/`pack_detection`/seed 测试同步（seed 测试曾因改写丢 release 标记而红，已保留标记重写）。
- 发布脚本：`package.ps1` 删 release.toml 生成段，改为「打包 project.toml 后，若 -PublishName 与工程 name 不同则向 pack/project.toml 追加 publish_name = "…"（不污染工程源）」；verify-wechat-minigame.ps1 required 表删 release.toml；wechat project.config.json packOptions.ignore 删 release.toml；scene.rs 注释同步；docs/50、26、34、37 与 wechat README 布局树全部更新。
- 门：`jhyt_m31_pc_release` 全量切 project.toml/release=true 语义（REQUIRED_RELEASE_ENTRIES 11 项、fake_pack publish_name 注入、打包 project.toml 断言 [input]/死区/actions-v2）；rustlua-infra 39 单测全绿、m31/m20_audio/m20_gamepad/m22 全绿；PC 已按新逻辑重发（game.exe 13,213KB、pack/project.toml 含 release=true、无 release.toml）。
- 工程 `examples/game_jhyt/project.toml` [project] 段加 `release = true`（带注释：开发形态不受影响，因判定还要求目录名是 pack）。

**m19_slice 门回归红修复：HUD 预设失效自愈（✅ 2026-10-07）**：场景切换重建 ECS 世界后，旧预设句柄的实体 id 可能被新世界**复用**，`prefab.root` 探活（Bug11 修复）会误判「活着」，只有真正写 `Text` 组件时才发现组件不对（`prefab.set_text: 节点 "HudLine6" 没有 Text 组件`，m19_slice 在青云山道战斗段首帧触发，lua-guard 连续报错停用回调 → 门红）。
- 修复：hud.lua `preset_text`/`preset_visible`/`preset_rect` 三个写入函数统一 pcall 容错——写入失败即置空句柄，下一帧 `preset_spawn` 重新实例化（常驻 HUD 不 despawn，靠这条自愈）；失败帧内后续写入对 nil 句柄幂等跳过，不再向 lua-guard 抛错。
- 验证：m19_slice 全绿（day=2 orders=3 mainline=true money=521）；m70_hud_prefab 全绿（Text 11 条白对齐、跨屏相交 0）；m19_ui 全绿（hud_lines=6 clips=16）；m19_soak 全绿（steps=1800 per_step_ms=10.8）。
- 备注：title/dialogue/shop 等非常驻预设各自 despawn/重开时重新 spawn，不受此问题影响；临时探针 jhyt_tmp_hud_probe.rs 已删。