# M6-08：样板游戏增强（BGM / 主菜单 / 方向攻击 / 程序化精灵）

- 状态：ACCEPTED（待 M7 里程碑汇总验收）
- 验收日期：2026-08-22
- 前置任务：M6-06（消除其已知限制中的 4 项）

## 目标

补齐样板游戏体验：循环背景音乐、主菜单、有方向的范围攻击、更精致的程序化精灵。

## 实施

### BGM（循环背景音乐）
- `engine_core::audio`：新增 `play_music`（rodio `repeat_infinite` 循环，替换旧音乐）与 `stop_music`；抽取 `ensure_device` 复用；
- Lua API：`audio.music(name)` / `audio.stop_music()`；
- `scripts/gen-bgm.ps1`：Add-Type C# 合成 24s 循环 chiptune `bgm.wav`（A 小调行进）；
- 游戏启动时 `audio.music("bgm")` 播放一次，跨场景切换保持。

### 主菜单
- `state.menu` 初始为 `true`；`hud.init_menu` 显示标题/开始提示/操作说明；
- 菜单期 SPACE 开始（`state.menu=false`）；开始前敌人不追击、不受伤（`handle_events`/`enemies.chase` 均以 `not menu` 门控）。

### 有方向的范围攻击
- `state.facing` 记录最近非零输入方向（默认向下）；
- `player.attack`：命中需**距离 ≤ attack_range(56)** 且**朝向点积 ≥ cone(0.707=45°)**，不再全向 48 半径；
- 攻击特效粒子朝朝向方向喷发。

### 升级程序化精灵
- `scripts/gen-characters.ps1`：英雄/史莱姆/金币增加暗描边、明暗双色、面部（眼睛/瞳孔/嘴）、剑刃高光、金币内环与反光。

### CJK 文本渲染（主菜单中文"?"修复）
- `engine_core/font.rs`：`FontAtlas` 用 `ab_glyph` 按需栅格化任意字形到动态 RGBA 图集（版本号增量，仅变化时上传）；
- `TextLayout::build_mixed`：ASCII 走内置 5×7 图集、非 ASCII 走字体图集，返回两个 mesh；
- `game_runner`：启动加载 `assets/fonts/NotoSansSC-VF.ttf`，字体图集纹理按版本上传；主菜单中文正常显示；
- 测试：`rasterizes_cjk_glyph_and_reports_uv`、`build_mixed_splits_ascii_and_cjk_glyphs`。

## 验收结果

- 实测：启动 `[audio] music started (loop) name="bgm"`；菜单期零伤害（`player hit` 仅出现在 SPACE 开始后）；开始后伤害/追击正常；F4 切场景正常；
- 精灵 PNG 已重新生成（尺寸更大=细节更多），帧布局不变，动画配置无需改动；
- fmt / clippy / workspace 测试 196 全过。

## 证据

- `tasks/done/M6-08-样板游戏增强（BGM主菜单方向攻击精灵）.md`
- `game/assets/audio/bgm.wav`、`game/assets/textures/{hero-idle,hero-move,hero-attack,slime,coin}.png`

## 建议下一步

- 项目负责人实机确认 BGM/菜单/方向攻击手感后，打 `v0.7` 标签。
