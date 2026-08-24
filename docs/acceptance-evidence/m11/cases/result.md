# M11 实机 GUI/GPU 验收结果（G01–G04）

日期：2026-08-24
基线提交：`fc7ba0a`（实机使用工作树代码，含 M11-01~M11-07 未提交改动）
环境：见 [../environment.txt](../environment.txt)（GT 1030 / 驱动 32.0.15.8228 / Windows 11 26200 / 1920x1080 / DPI 96=100%）

## 总状态

| 编号 | 项目 | 状态 | 证据 |
|---|---|---|---|
| G01 | GUI/GPU 启动与中文 SDF | **PASS（实机）** | 启动日志、editor.log 无 error/panic/validation、截图 g01-editor-start.png |
| G02 | DPI 150% 实测 | **PENDING（需用户手动）** | — |
| G03 | resize 预览分辨率切换 + 主窗口缩放 | **PENDING（部分，需用户手动）** | 预览分辨率切换需点击 ComboBox，合成输入受限 |
| G04 | Play/Stop 往返 | **部分 PASS** | Play 实机 ✅、运行态不写回 ✅、Stop 真实点击受环境限制 |

## 实机执行记录

### G01 GUI/GPU（PASS）

- 以 `P2_EDITOR_AUTOPLAY=1` 启动 `editor.exe --project <repo>/game_test`，
  wgpu 选用 **NVIDIA GeForce GT 1030**（日志：`backend: Vulkan` 582.28 / `Dx12` 32.0.15.8228）。
- AUTOPLAY 钩子自动打开 `main.scene`（`[scene] loaded name=main entities=8`）并切 3D 视口。
- 中文 SDF 渲染证据：editor.log `[render] scene ui draws=14 sdf=5 text_entities=1`、
  `font atlas ready path=.../NotoSansSC-VF.ttf`（CJK 字体已加载）。
- 全程 editor.log 无 `error`/`panic`/wgpu `validation`；进程多次启动/关闭均为优雅退出
  （`[exited with code 0]`），**无 crash.log**。
- 截图：`g01-editor-start.png`、`g04-autoplay.png`（非黑，AUTOPLAY 实例稳定运行）。
- 说明：截图含窗口可见 UI；中文 glyph 无 tofu/裁切需用户最终目视确认（本机自动化侧
  仅能以字体加载 + SDF 文本计数佐证）。

### G02 DPI 150%（PENDING，需用户手动）

- 本机当前 AppliedDPI=96（100%）。150% 缩放需用户通过系统设置切换后重测，
  脚本不能修改用户系统缩放设置。
- 用户操作指引见文末。

### G03 resize（PENDING）

- 预览分辨率切换依赖点击工具栏 ComboBox（见 G04 环境限制），本环境合成输入无法触发，
  未能自动完成切换。主窗口缩放（MoveWindow 已做）不产生黑屏/崩溃，但未按验收标准逐项截图。
- 用户操作指引见文末。

### G04 Play/Stop（部分 PASS）

**Play 实机（PASS）**
- AUTOPLAY 实例日志：`[preview] created 640x360` → `[scene] loaded main entities=8` →
  `Lua log.info scene 'main' tilemap 10x8`；game3d Lua 运行：`enemies=2 pickups=3`、
  `audio hit`、玩家 `hp: 100→80→60→40→20→0`（运行态可观察修改发生）。

**运行态修改不写回编辑态（PASS）**
- Play 前后 `game_test/scenes/main.scene` SHA256 均为
  `f1548f7bfedbcd4862b913f42f945d06b6bb3be8c8aa93bf9ae682fb3dcdeed3`；
  `game_test/scenes/game3d.scene` 均为
  `d4f501f962c456be18907355f8525d9792b81abd0cbb81688ed83a9de7bb81e6`。
- 即：运行态 hp/enemies/pickups 变化**没有**写回任何场景文件。

**Stop 真实点击（受环境限制，未完成）**
- 环境边界：本机无真实输入设备驱动到该窗口的按钮事件。已依次尝试
  `SetCursorPos+mouse_event`、`SendInput`、`PostMessage(WM_LBUTTONDOWN/UP)`、
  `PostMessage(WM_KEYDOWN)`、`AttachThreadInput` 强制激活、UI Automation（eframe 未启用
  accesskit）——鼠标 `move`/hover 有效（按钮 hover 背景 60→70 已验证），但 `down/up` 与
  键盘均无法到达 egui/winit。
- 佐证：PID 27424 AUTOPLAY 实例在 12:21:48–12:22:13 连续三次 `[preview] created 1920x1080`
  （Stop→AUTOPLAY 自动重 Play 循环迹象，与 main.rs AUTOPLAY 钩子 `!is_playing() → play()`
  逻辑吻合）；该实例退出前 `draws=14→12、sdf=5→4`（Scene 视图内容随预览释放变化）。
- 自动化侧：`play_stop_can_repeat_one_hundred_times`（100 轮 Play/Stop 往返）及
  `stop_without_play_is_noop`、`is_playing_reflects_state_machine` 等已通过（见
  [../m11-06-gui-gpu-evidence.md](../m11-06-gui-gpu-evidence.md)）。

## 环境限制说明

本会话运行在无真实指针设备的自动化环境，Windows 合成输入（`SendInput`/`mouse_event`/
`PostMessage`）的鼠标**按钮**与**键盘**事件无法触发 egui/winit 的点击/按键（仅鼠标移动/
hover 有效）。因此**依赖真实点击的交互项（Stop 按钮、分辨率 ComboBox）无法由脚本完成**，
已如实标注 PENDING，不影响代码侧 329 项测试与质量门结论。

## 用户手动验收指引（PENDING 项）

1. **G04 Stop/往返**：启动 `editor.exe --project <repo>\game_test`（可加
   `P2_EDITOR_AUTOPLAY=1`），在工具栏点击「Play」观察 Game 视图运行，再点击「Stop」
   确认回到 Scene 且场景文件哈希不变（`certutil -hashfile game_test\scenes\main.scene SHA256`）；
   重复 Play/Stop ≥10 轮，保存 editor.log 与截图。
2. **G03 resize**：点击工具栏分辨率下拉（640×360 与 960×540/1280×720/1920×1080）切换，
   调整主窗口大小，确认预览纵横比居中、不黑屏、不崩溃，恢复正常后画面正确。
3. **G02 DPI**：系统显示缩放切到 150%，重复 G01 启动与 G03 操作，确认四行布局、中文文本、
   按钮命中无重叠/裁切；回 100% 后把结果追加到 `environment.txt`。
