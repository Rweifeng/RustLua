-- data/characters.lua：《江湖有田》角色形象与序列帧动画注册表。
--
-- 本文件由 tools/gen_character_sprites.py 生成，不要手改；
-- 改形象/动作请改生成器里的 CHARACTERS / ACTIONS 表后重新运行：
--     python tools/gen_character_sprites.py
--
-- 引擎事实（engine/crates/engine/src/core/animation.rs + host/runtime_scene.rs）：
--   * Animation 组件串：default=<clip>;clip:<name>:<sheet>:<cols>:<fps>:<looped>
--   * 每个 clip 采样一整张 1×N 横向条带，帧尺寸 = 条带高 × (条带宽/cols)；
--   * 实体带 Animation 时，渲染用 clip 的 sheet 覆盖 Sprite 的 texture，
--     Sprite 只提供尺寸/排序，所以两者要挂在同一实体上；
--   * **NPC 由 entity.spawn 运行时创建，Lua 无"挂 Animation 组件"的 API**，
--     因此 NPC 现在只能用 static_<dir>.png 单帧；缺口记录见 docs/36 第 6 节。
--
-- 主角在场景里是静态实体（main.scene/ village.scene 的 JianghuPlayer），
-- 可以直接在 .scene 里声明 Animation 组件，用 M.player_animation() 取串。

local M = {}

M.version = 1
M.frame_px = 32

-- 每个角色：id → 显示名、形象要点、单帧 sprite 串、动画组件串、格子大小。
M.characters = {

  player_male = {
    display = "主角·青衫少侠（默认男）",
    role = "player",
    note = "素色交领短打 + 竹青护腕束腰，背负木剑；米白/竹青便于在药田绿底上读轮廓",
    static_sprite = {
      s = "texture=assets/art/characters/player_male/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/characters/player_male/static_n.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/characters/player_male/static_e.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/characters/player_male/static_w.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/characters/player_male/idle_s.png:4:6:true;clip:idle_n:assets/art/characters/player_male/idle_n.png:4:6:true;clip:idle_e:assets/art/characters/player_male/idle_e.png:4:6:true;clip:idle_w:assets/art/characters/player_male/idle_w.png:4:6:true;clip:walk_s:assets/art/characters/player_male/walk_s.png:6:10:true;clip:walk_n:assets/art/characters/player_male/walk_n.png:6:10:true;clip:walk_e:assets/art/characters/player_male/walk_e.png:6:10:true;clip:walk_w:assets/art/characters/player_male/walk_w.png:6:10:true;clip:hoe_s:assets/art/characters/player_male/hoe_s.png:4:8:false;clip:hoe_n:assets/art/characters/player_male/hoe_n.png:4:8:false;clip:hoe_e:assets/art/characters/player_male/hoe_e.png:4:8:false;clip:hoe_w:assets/art/characters/player_male/hoe_w.png:4:8:false;clip:water_s:assets/art/characters/player_male/water_s.png:4:8:false;clip:water_n:assets/art/characters/player_male/water_n.png:4:8:false;clip:water_e:assets/art/characters/player_male/water_e.png:4:8:false;clip:water_w:assets/art/characters/player_male/water_w.png:4:8:false;clip:attack_s:assets/art/characters/player_male/attack_s.png:4:12:false;clip:attack_n:assets/art/characters/player_male/attack_n.png:4:12:false;clip:attack_e:assets/art/characters/player_male/attack_e.png:4:12:false;clip:attack_w:assets/art/characters/player_male/attack_w.png:4:12:false;clip:hurt_s:assets/art/characters/player_male/hurt_s.png:2:6:false;clip:hurt_n:assets/art/characters/player_male/hurt_n.png:2:6:false;clip:hurt_e:assets/art/characters/player_male/hurt_e.png:2:6:false;clip:hurt_w:assets/art/characters/player_male/hurt_w.png:2:6:false;clip:faint_s:assets/art/characters/player_male/faint_s.png:4:6:false;clip:faint_n:assets/art/characters/player_male/faint_n.png:4:6:false;clip:faint_e:assets/art/characters/player_male/faint_e.png:4:6:false;clip:faint_w:assets/art/characters/player_male/faint_w.png:4:6:false",
    clips = {
      { name = "idle_s", sheet = "assets/art/characters/player_male/idle_s.png", cols = 4, fps = 6, looped = true },
      { name = "idle_n", sheet = "assets/art/characters/player_male/idle_n.png", cols = 4, fps = 6, looped = true },
      { name = "idle_e", sheet = "assets/art/characters/player_male/idle_e.png", cols = 4, fps = 6, looped = true },
      { name = "idle_w", sheet = "assets/art/characters/player_male/idle_w.png", cols = 4, fps = 6, looped = true },
      { name = "walk_s", sheet = "assets/art/characters/player_male/walk_s.png", cols = 6, fps = 10, looped = true },
      { name = "walk_n", sheet = "assets/art/characters/player_male/walk_n.png", cols = 6, fps = 10, looped = true },
      { name = "walk_e", sheet = "assets/art/characters/player_male/walk_e.png", cols = 6, fps = 10, looped = true },
      { name = "walk_w", sheet = "assets/art/characters/player_male/walk_w.png", cols = 6, fps = 10, looped = true },
      { name = "hoe_s", sheet = "assets/art/characters/player_male/hoe_s.png", cols = 4, fps = 8, looped = false },
      { name = "hoe_n", sheet = "assets/art/characters/player_male/hoe_n.png", cols = 4, fps = 8, looped = false },
      { name = "hoe_e", sheet = "assets/art/characters/player_male/hoe_e.png", cols = 4, fps = 8, looped = false },
      { name = "hoe_w", sheet = "assets/art/characters/player_male/hoe_w.png", cols = 4, fps = 8, looped = false },
      { name = "water_s", sheet = "assets/art/characters/player_male/water_s.png", cols = 4, fps = 8, looped = false },
      { name = "water_n", sheet = "assets/art/characters/player_male/water_n.png", cols = 4, fps = 8, looped = false },
      { name = "water_e", sheet = "assets/art/characters/player_male/water_e.png", cols = 4, fps = 8, looped = false },
      { name = "water_w", sheet = "assets/art/characters/player_male/water_w.png", cols = 4, fps = 8, looped = false },
      { name = "attack_s", sheet = "assets/art/characters/player_male/attack_s.png", cols = 4, fps = 12, looped = false },
      { name = "attack_n", sheet = "assets/art/characters/player_male/attack_n.png", cols = 4, fps = 12, looped = false },
      { name = "attack_e", sheet = "assets/art/characters/player_male/attack_e.png", cols = 4, fps = 12, looped = false },
      { name = "attack_w", sheet = "assets/art/characters/player_male/attack_w.png", cols = 4, fps = 12, looped = false },
      { name = "hurt_s", sheet = "assets/art/characters/player_male/hurt_s.png", cols = 2, fps = 6, looped = false },
      { name = "hurt_n", sheet = "assets/art/characters/player_male/hurt_n.png", cols = 2, fps = 6, looped = false },
      { name = "hurt_e", sheet = "assets/art/characters/player_male/hurt_e.png", cols = 2, fps = 6, looped = false },
      { name = "hurt_w", sheet = "assets/art/characters/player_male/hurt_w.png", cols = 2, fps = 6, looped = false },
      { name = "faint_s", sheet = "assets/art/characters/player_male/faint_s.png", cols = 4, fps = 6, looped = false },
      { name = "faint_n", sheet = "assets/art/characters/player_male/faint_n.png", cols = 4, fps = 6, looped = false },
      { name = "faint_e", sheet = "assets/art/characters/player_male/faint_e.png", cols = 4, fps = 6, looped = false },
      { name = "faint_w", sheet = "assets/art/characters/player_male/faint_w.png", cols = 4, fps = 6, looped = false },
    },
  },
  player_female = {
    display = "主角·青衣女侠（可选女）",
    role = "player",
    note = "与男主体共用色板与动作集，仅改轮廓（裙摆 + 马尾），保证换装/自定义不返工",
    static_sprite = {
      s = "texture=assets/art/characters/player_female/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/characters/player_female/static_n.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/characters/player_female/static_e.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/characters/player_female/static_w.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/characters/player_female/idle_s.png:4:6:true;clip:idle_n:assets/art/characters/player_female/idle_n.png:4:6:true;clip:idle_e:assets/art/characters/player_female/idle_e.png:4:6:true;clip:idle_w:assets/art/characters/player_female/idle_w.png:4:6:true;clip:walk_s:assets/art/characters/player_female/walk_s.png:6:10:true;clip:walk_n:assets/art/characters/player_female/walk_n.png:6:10:true;clip:walk_e:assets/art/characters/player_female/walk_e.png:6:10:true;clip:walk_w:assets/art/characters/player_female/walk_w.png:6:10:true;clip:hoe_s:assets/art/characters/player_female/hoe_s.png:4:8:false;clip:hoe_n:assets/art/characters/player_female/hoe_n.png:4:8:false;clip:hoe_e:assets/art/characters/player_female/hoe_e.png:4:8:false;clip:hoe_w:assets/art/characters/player_female/hoe_w.png:4:8:false;clip:water_s:assets/art/characters/player_female/water_s.png:4:8:false;clip:water_n:assets/art/characters/player_female/water_n.png:4:8:false;clip:water_e:assets/art/characters/player_female/water_e.png:4:8:false;clip:water_w:assets/art/characters/player_female/water_w.png:4:8:false;clip:attack_s:assets/art/characters/player_female/attack_s.png:4:12:false;clip:attack_n:assets/art/characters/player_female/attack_n.png:4:12:false;clip:attack_e:assets/art/characters/player_female/attack_e.png:4:12:false;clip:attack_w:assets/art/characters/player_female/attack_w.png:4:12:false;clip:hurt_s:assets/art/characters/player_female/hurt_s.png:2:6:false;clip:hurt_n:assets/art/characters/player_female/hurt_n.png:2:6:false;clip:hurt_e:assets/art/characters/player_female/hurt_e.png:2:6:false;clip:hurt_w:assets/art/characters/player_female/hurt_w.png:2:6:false;clip:faint_s:assets/art/characters/player_female/faint_s.png:4:6:false;clip:faint_n:assets/art/characters/player_female/faint_n.png:4:6:false;clip:faint_e:assets/art/characters/player_female/faint_e.png:4:6:false;clip:faint_w:assets/art/characters/player_female/faint_w.png:4:6:false",
    clips = {
      { name = "idle_s", sheet = "assets/art/characters/player_female/idle_s.png", cols = 4, fps = 6, looped = true },
      { name = "idle_n", sheet = "assets/art/characters/player_female/idle_n.png", cols = 4, fps = 6, looped = true },
      { name = "idle_e", sheet = "assets/art/characters/player_female/idle_e.png", cols = 4, fps = 6, looped = true },
      { name = "idle_w", sheet = "assets/art/characters/player_female/idle_w.png", cols = 4, fps = 6, looped = true },
      { name = "walk_s", sheet = "assets/art/characters/player_female/walk_s.png", cols = 6, fps = 10, looped = true },
      { name = "walk_n", sheet = "assets/art/characters/player_female/walk_n.png", cols = 6, fps = 10, looped = true },
      { name = "walk_e", sheet = "assets/art/characters/player_female/walk_e.png", cols = 6, fps = 10, looped = true },
      { name = "walk_w", sheet = "assets/art/characters/player_female/walk_w.png", cols = 6, fps = 10, looped = true },
      { name = "hoe_s", sheet = "assets/art/characters/player_female/hoe_s.png", cols = 4, fps = 8, looped = false },
      { name = "hoe_n", sheet = "assets/art/characters/player_female/hoe_n.png", cols = 4, fps = 8, looped = false },
      { name = "hoe_e", sheet = "assets/art/characters/player_female/hoe_e.png", cols = 4, fps = 8, looped = false },
      { name = "hoe_w", sheet = "assets/art/characters/player_female/hoe_w.png", cols = 4, fps = 8, looped = false },
      { name = "water_s", sheet = "assets/art/characters/player_female/water_s.png", cols = 4, fps = 8, looped = false },
      { name = "water_n", sheet = "assets/art/characters/player_female/water_n.png", cols = 4, fps = 8, looped = false },
      { name = "water_e", sheet = "assets/art/characters/player_female/water_e.png", cols = 4, fps = 8, looped = false },
      { name = "water_w", sheet = "assets/art/characters/player_female/water_w.png", cols = 4, fps = 8, looped = false },
      { name = "attack_s", sheet = "assets/art/characters/player_female/attack_s.png", cols = 4, fps = 12, looped = false },
      { name = "attack_n", sheet = "assets/art/characters/player_female/attack_n.png", cols = 4, fps = 12, looped = false },
      { name = "attack_e", sheet = "assets/art/characters/player_female/attack_e.png", cols = 4, fps = 12, looped = false },
      { name = "attack_w", sheet = "assets/art/characters/player_female/attack_w.png", cols = 4, fps = 12, looped = false },
      { name = "hurt_s", sheet = "assets/art/characters/player_female/hurt_s.png", cols = 2, fps = 6, looped = false },
      { name = "hurt_n", sheet = "assets/art/characters/player_female/hurt_n.png", cols = 2, fps = 6, looped = false },
      { name = "hurt_e", sheet = "assets/art/characters/player_female/hurt_e.png", cols = 2, fps = 6, looped = false },
      { name = "hurt_w", sheet = "assets/art/characters/player_female/hurt_w.png", cols = 2, fps = 6, looped = false },
      { name = "faint_s", sheet = "assets/art/characters/player_female/faint_s.png", cols = 4, fps = 6, looped = false },
      { name = "faint_n", sheet = "assets/art/characters/player_female/faint_n.png", cols = 4, fps = 6, looped = false },
      { name = "faint_e", sheet = "assets/art/characters/player_female/faint_e.png", cols = 4, fps = 6, looped = false },
      { name = "faint_w", sheet = "assets/art/characters/player_female/faint_w.png", cols = 4, fps = 6, looped = false },
    },
  },
  shen_qingluo = {
    display = "沈青萝·医馆掌柜",
    role = "npc",
    note = "藕荷罗衫 + 米白药围裙 + 木药箱；双螺髻，慈和中年女性轮廓",
    static_sprite = {
      s = "texture=assets/art/characters/shen_qingluo/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/characters/shen_qingluo/static_n.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/characters/shen_qingluo/static_e.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/characters/shen_qingluo/static_w.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/characters/shen_qingluo/idle_s.png:2:6:true;clip:idle_n:assets/art/characters/shen_qingluo/idle_n.png:2:6:true;clip:idle_e:assets/art/characters/shen_qingluo/idle_e.png:2:6:true;clip:idle_w:assets/art/characters/shen_qingluo/idle_w.png:2:6:true;clip:walk_s:assets/art/characters/shen_qingluo/walk_s.png:4:10:true;clip:walk_n:assets/art/characters/shen_qingluo/walk_n.png:4:10:true;clip:walk_e:assets/art/characters/shen_qingluo/walk_e.png:4:10:true;clip:walk_w:assets/art/characters/shen_qingluo/walk_w.png:4:10:true;clip:talk_s:assets/art/characters/shen_qingluo/talk_s.png:2:4:true;clip:talk_n:assets/art/characters/shen_qingluo/talk_n.png:2:4:true;clip:talk_e:assets/art/characters/shen_qingluo/talk_e.png:2:4:true;clip:talk_w:assets/art/characters/shen_qingluo/talk_w.png:2:4:true",
    clips = {
      { name = "idle_s", sheet = "assets/art/characters/shen_qingluo/idle_s.png", cols = 2, fps = 6, looped = true },
      { name = "idle_n", sheet = "assets/art/characters/shen_qingluo/idle_n.png", cols = 2, fps = 6, looped = true },
      { name = "idle_e", sheet = "assets/art/characters/shen_qingluo/idle_e.png", cols = 2, fps = 6, looped = true },
      { name = "idle_w", sheet = "assets/art/characters/shen_qingluo/idle_w.png", cols = 2, fps = 6, looped = true },
      { name = "walk_s", sheet = "assets/art/characters/shen_qingluo/walk_s.png", cols = 4, fps = 10, looped = true },
      { name = "walk_n", sheet = "assets/art/characters/shen_qingluo/walk_n.png", cols = 4, fps = 10, looped = true },
      { name = "walk_e", sheet = "assets/art/characters/shen_qingluo/walk_e.png", cols = 4, fps = 10, looped = true },
      { name = "walk_w", sheet = "assets/art/characters/shen_qingluo/walk_w.png", cols = 4, fps = 10, looped = true },
      { name = "talk_s", sheet = "assets/art/characters/shen_qingluo/talk_s.png", cols = 2, fps = 4, looped = true },
      { name = "talk_n", sheet = "assets/art/characters/shen_qingluo/talk_n.png", cols = 2, fps = 4, looped = true },
      { name = "talk_e", sheet = "assets/art/characters/shen_qingluo/talk_e.png", cols = 2, fps = 4, looped = true },
      { name = "talk_w", sheet = "assets/art/characters/shen_qingluo/talk_w.png", cols = 2, fps = 4, looped = true },
    },
  },
  zhao_tiesuanpan = {
    display = "赵铁算盘·集市商人",
    role = "npc",
    note = "旧金长衫 + 墨色瓜皮帽 + 腰间算盘；体胖（half_w 4），辨识度靠体型与金色",
    static_sprite = {
      s = "texture=assets/art/characters/zhao_tiesuanpan/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/characters/zhao_tiesuanpan/static_n.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/characters/zhao_tiesuanpan/static_e.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/characters/zhao_tiesuanpan/static_w.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/characters/zhao_tiesuanpan/idle_s.png:2:6:true;clip:idle_n:assets/art/characters/zhao_tiesuanpan/idle_n.png:2:6:true;clip:idle_e:assets/art/characters/zhao_tiesuanpan/idle_e.png:2:6:true;clip:idle_w:assets/art/characters/zhao_tiesuanpan/idle_w.png:2:6:true;clip:walk_s:assets/art/characters/zhao_tiesuanpan/walk_s.png:4:10:true;clip:walk_n:assets/art/characters/zhao_tiesuanpan/walk_n.png:4:10:true;clip:walk_e:assets/art/characters/zhao_tiesuanpan/walk_e.png:4:10:true;clip:walk_w:assets/art/characters/zhao_tiesuanpan/walk_w.png:4:10:true;clip:talk_s:assets/art/characters/zhao_tiesuanpan/talk_s.png:2:4:true;clip:talk_n:assets/art/characters/zhao_tiesuanpan/talk_n.png:2:4:true;clip:talk_e:assets/art/characters/zhao_tiesuanpan/talk_e.png:2:4:true;clip:talk_w:assets/art/characters/zhao_tiesuanpan/talk_w.png:2:4:true",
    clips = {
      { name = "idle_s", sheet = "assets/art/characters/zhao_tiesuanpan/idle_s.png", cols = 2, fps = 6, looped = true },
      { name = "idle_n", sheet = "assets/art/characters/zhao_tiesuanpan/idle_n.png", cols = 2, fps = 6, looped = true },
      { name = "idle_e", sheet = "assets/art/characters/zhao_tiesuanpan/idle_e.png", cols = 2, fps = 6, looped = true },
      { name = "idle_w", sheet = "assets/art/characters/zhao_tiesuanpan/idle_w.png", cols = 2, fps = 6, looped = true },
      { name = "walk_s", sheet = "assets/art/characters/zhao_tiesuanpan/walk_s.png", cols = 4, fps = 10, looped = true },
      { name = "walk_n", sheet = "assets/art/characters/zhao_tiesuanpan/walk_n.png", cols = 4, fps = 10, looped = true },
      { name = "walk_e", sheet = "assets/art/characters/zhao_tiesuanpan/walk_e.png", cols = 4, fps = 10, looped = true },
      { name = "walk_w", sheet = "assets/art/characters/zhao_tiesuanpan/walk_w.png", cols = 4, fps = 10, looped = true },
      { name = "talk_s", sheet = "assets/art/characters/zhao_tiesuanpan/talk_s.png", cols = 2, fps = 4, looped = true },
      { name = "talk_n", sheet = "assets/art/characters/zhao_tiesuanpan/talk_n.png", cols = 2, fps = 4, looped = true },
      { name = "talk_e", sheet = "assets/art/characters/zhao_tiesuanpan/talk_e.png", cols = 2, fps = 4, looped = true },
      { name = "talk_w", sheet = "assets/art/characters/zhao_tiesuanpan/talk_w.png", cols = 2, fps = 4, looped = true },
    },
  },
  lin_xiaoman = {
    display = "林小满·茶农",
    role = "npc",
    note = "竹青布衣 + 黛蓝印花头巾 + 竹茶篓；最矮（少女），绿色与药田同色系但靠头巾区分",
    static_sprite = {
      s = "texture=assets/art/characters/lin_xiaoman/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/characters/lin_xiaoman/static_n.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/characters/lin_xiaoman/static_e.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/characters/lin_xiaoman/static_w.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/characters/lin_xiaoman/idle_s.png:2:6:true;clip:idle_n:assets/art/characters/lin_xiaoman/idle_n.png:2:6:true;clip:idle_e:assets/art/characters/lin_xiaoman/idle_e.png:2:6:true;clip:idle_w:assets/art/characters/lin_xiaoman/idle_w.png:2:6:true;clip:walk_s:assets/art/characters/lin_xiaoman/walk_s.png:4:10:true;clip:walk_n:assets/art/characters/lin_xiaoman/walk_n.png:4:10:true;clip:walk_e:assets/art/characters/lin_xiaoman/walk_e.png:4:10:true;clip:walk_w:assets/art/characters/lin_xiaoman/walk_w.png:4:10:true;clip:talk_s:assets/art/characters/lin_xiaoman/talk_s.png:2:4:true;clip:talk_n:assets/art/characters/lin_xiaoman/talk_n.png:2:4:true;clip:talk_e:assets/art/characters/lin_xiaoman/talk_e.png:2:4:true;clip:talk_w:assets/art/characters/lin_xiaoman/talk_w.png:2:4:true",
    clips = {
      { name = "idle_s", sheet = "assets/art/characters/lin_xiaoman/idle_s.png", cols = 2, fps = 6, looped = true },
      { name = "idle_n", sheet = "assets/art/characters/lin_xiaoman/idle_n.png", cols = 2, fps = 6, looped = true },
      { name = "idle_e", sheet = "assets/art/characters/lin_xiaoman/idle_e.png", cols = 2, fps = 6, looped = true },
      { name = "idle_w", sheet = "assets/art/characters/lin_xiaoman/idle_w.png", cols = 2, fps = 6, looped = true },
      { name = "walk_s", sheet = "assets/art/characters/lin_xiaoman/walk_s.png", cols = 4, fps = 10, looped = true },
      { name = "walk_n", sheet = "assets/art/characters/lin_xiaoman/walk_n.png", cols = 4, fps = 10, looped = true },
      { name = "walk_e", sheet = "assets/art/characters/lin_xiaoman/walk_e.png", cols = 4, fps = 10, looped = true },
      { name = "walk_w", sheet = "assets/art/characters/lin_xiaoman/walk_w.png", cols = 4, fps = 10, looped = true },
      { name = "talk_s", sheet = "assets/art/characters/lin_xiaoman/talk_s.png", cols = 2, fps = 4, looped = true },
      { name = "talk_n", sheet = "assets/art/characters/lin_xiaoman/talk_n.png", cols = 2, fps = 4, looped = true },
      { name = "talk_e", sheet = "assets/art/characters/lin_xiaoman/talk_e.png", cols = 2, fps = 4, looped = true },
      { name = "talk_w", sheet = "assets/art/characters/lin_xiaoman/talk_w.png", cols = 2, fps = 4, looped = true },
    },
  },
  han_tieshan = {
    display = "韩铁山·铁匠",
    role = "npc",
    note = "钢灰短褂 + 皮革围裙 + 朱砂发带 + 铁锤；肩最宽（half_w 4），配色最重",
    static_sprite = {
      s = "texture=assets/art/characters/han_tieshan/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/characters/han_tieshan/static_n.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/characters/han_tieshan/static_e.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/characters/han_tieshan/static_w.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/characters/han_tieshan/idle_s.png:2:6:true;clip:idle_n:assets/art/characters/han_tieshan/idle_n.png:2:6:true;clip:idle_e:assets/art/characters/han_tieshan/idle_e.png:2:6:true;clip:idle_w:assets/art/characters/han_tieshan/idle_w.png:2:6:true;clip:walk_s:assets/art/characters/han_tieshan/walk_s.png:4:10:true;clip:walk_n:assets/art/characters/han_tieshan/walk_n.png:4:10:true;clip:walk_e:assets/art/characters/han_tieshan/walk_e.png:4:10:true;clip:walk_w:assets/art/characters/han_tieshan/walk_w.png:4:10:true;clip:talk_s:assets/art/characters/han_tieshan/talk_s.png:2:4:true;clip:talk_n:assets/art/characters/han_tieshan/talk_n.png:2:4:true;clip:talk_e:assets/art/characters/han_tieshan/talk_e.png:2:4:true;clip:talk_w:assets/art/characters/han_tieshan/talk_w.png:2:4:true",
    clips = {
      { name = "idle_s", sheet = "assets/art/characters/han_tieshan/idle_s.png", cols = 2, fps = 6, looped = true },
      { name = "idle_n", sheet = "assets/art/characters/han_tieshan/idle_n.png", cols = 2, fps = 6, looped = true },
      { name = "idle_e", sheet = "assets/art/characters/han_tieshan/idle_e.png", cols = 2, fps = 6, looped = true },
      { name = "idle_w", sheet = "assets/art/characters/han_tieshan/idle_w.png", cols = 2, fps = 6, looped = true },
      { name = "walk_s", sheet = "assets/art/characters/han_tieshan/walk_s.png", cols = 4, fps = 10, looped = true },
      { name = "walk_n", sheet = "assets/art/characters/han_tieshan/walk_n.png", cols = 4, fps = 10, looped = true },
      { name = "walk_e", sheet = "assets/art/characters/han_tieshan/walk_e.png", cols = 4, fps = 10, looped = true },
      { name = "walk_w", sheet = "assets/art/characters/han_tieshan/walk_w.png", cols = 4, fps = 10, looped = true },
      { name = "talk_s", sheet = "assets/art/characters/han_tieshan/talk_s.png", cols = 2, fps = 4, looped = true },
      { name = "talk_n", sheet = "assets/art/characters/han_tieshan/talk_n.png", cols = 2, fps = 4, looped = true },
      { name = "talk_e", sheet = "assets/art/characters/han_tieshan/talk_e.png", cols = 2, fps = 4, looped = true },
      { name = "talk_w", sheet = "assets/art/characters/han_tieshan/talk_w.png", cols = 2, fps = 4, looped = true },
    },
  },
  gu_changfeng = {
    display = "顾长风·退隐镖师",
    role = "npc",
    note = "黛蓝劲装长衫 + 灰短披肩 + 单刀 + 络腮短须；高个（head_top 3），护送/教刀场景主视觉",
    static_sprite = {
      s = "texture=assets/art/characters/gu_changfeng/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/characters/gu_changfeng/static_n.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/characters/gu_changfeng/static_e.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/characters/gu_changfeng/static_w.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/characters/gu_changfeng/idle_s.png:2:6:true;clip:idle_n:assets/art/characters/gu_changfeng/idle_n.png:2:6:true;clip:idle_e:assets/art/characters/gu_changfeng/idle_e.png:2:6:true;clip:idle_w:assets/art/characters/gu_changfeng/idle_w.png:2:6:true;clip:walk_s:assets/art/characters/gu_changfeng/walk_s.png:4:10:true;clip:walk_n:assets/art/characters/gu_changfeng/walk_n.png:4:10:true;clip:walk_e:assets/art/characters/gu_changfeng/walk_e.png:4:10:true;clip:walk_w:assets/art/characters/gu_changfeng/walk_w.png:4:10:true;clip:talk_s:assets/art/characters/gu_changfeng/talk_s.png:2:4:true;clip:talk_n:assets/art/characters/gu_changfeng/talk_n.png:2:4:true;clip:talk_e:assets/art/characters/gu_changfeng/talk_e.png:2:4:true;clip:talk_w:assets/art/characters/gu_changfeng/talk_w.png:2:4:true",
    clips = {
      { name = "idle_s", sheet = "assets/art/characters/gu_changfeng/idle_s.png", cols = 2, fps = 6, looped = true },
      { name = "idle_n", sheet = "assets/art/characters/gu_changfeng/idle_n.png", cols = 2, fps = 6, looped = true },
      { name = "idle_e", sheet = "assets/art/characters/gu_changfeng/idle_e.png", cols = 2, fps = 6, looped = true },
      { name = "idle_w", sheet = "assets/art/characters/gu_changfeng/idle_w.png", cols = 2, fps = 6, looped = true },
      { name = "walk_s", sheet = "assets/art/characters/gu_changfeng/walk_s.png", cols = 4, fps = 10, looped = true },
      { name = "walk_n", sheet = "assets/art/characters/gu_changfeng/walk_n.png", cols = 4, fps = 10, looped = true },
      { name = "walk_e", sheet = "assets/art/characters/gu_changfeng/walk_e.png", cols = 4, fps = 10, looped = true },
      { name = "walk_w", sheet = "assets/art/characters/gu_changfeng/walk_w.png", cols = 4, fps = 10, looped = true },
      { name = "talk_s", sheet = "assets/art/characters/gu_changfeng/talk_s.png", cols = 2, fps = 4, looped = true },
      { name = "talk_n", sheet = "assets/art/characters/gu_changfeng/talk_n.png", cols = 2, fps = 4, looped = true },
      { name = "talk_e", sheet = "assets/art/characters/gu_changfeng/talk_e.png", cols = 2, fps = 4, looped = true },
      { name = "talk_w", sheet = "assets/art/characters/gu_changfeng/talk_w.png", cols = 2, fps = 4, looped = true },
    },
  },
  su_wanqing = {
    display = "苏晚晴·青云门弟子",
    role = "npc",
    note = "青云色门派道服 + 旧金束腰 + 佩剑；高马尾（走路时甩动），与玄鹤同门派但一蓝一白",
    static_sprite = {
      s = "texture=assets/art/characters/su_wanqing/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/characters/su_wanqing/static_n.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/characters/su_wanqing/static_e.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/characters/su_wanqing/static_w.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/characters/su_wanqing/idle_s.png:2:6:true;clip:idle_n:assets/art/characters/su_wanqing/idle_n.png:2:6:true;clip:idle_e:assets/art/characters/su_wanqing/idle_e.png:2:6:true;clip:idle_w:assets/art/characters/su_wanqing/idle_w.png:2:6:true;clip:walk_s:assets/art/characters/su_wanqing/walk_s.png:4:10:true;clip:walk_n:assets/art/characters/su_wanqing/walk_n.png:4:10:true;clip:walk_e:assets/art/characters/su_wanqing/walk_e.png:4:10:true;clip:walk_w:assets/art/characters/su_wanqing/walk_w.png:4:10:true;clip:talk_s:assets/art/characters/su_wanqing/talk_s.png:2:4:true;clip:talk_n:assets/art/characters/su_wanqing/talk_n.png:2:4:true;clip:talk_e:assets/art/characters/su_wanqing/talk_e.png:2:4:true;clip:talk_w:assets/art/characters/su_wanqing/talk_w.png:2:4:true",
    clips = {
      { name = "idle_s", sheet = "assets/art/characters/su_wanqing/idle_s.png", cols = 2, fps = 6, looped = true },
      { name = "idle_n", sheet = "assets/art/characters/su_wanqing/idle_n.png", cols = 2, fps = 6, looped = true },
      { name = "idle_e", sheet = "assets/art/characters/su_wanqing/idle_e.png", cols = 2, fps = 6, looped = true },
      { name = "idle_w", sheet = "assets/art/characters/su_wanqing/idle_w.png", cols = 2, fps = 6, looped = true },
      { name = "walk_s", sheet = "assets/art/characters/su_wanqing/walk_s.png", cols = 4, fps = 10, looped = true },
      { name = "walk_n", sheet = "assets/art/characters/su_wanqing/walk_n.png", cols = 4, fps = 10, looped = true },
      { name = "walk_e", sheet = "assets/art/characters/su_wanqing/walk_e.png", cols = 4, fps = 10, looped = true },
      { name = "walk_w", sheet = "assets/art/characters/su_wanqing/walk_w.png", cols = 4, fps = 10, looped = true },
      { name = "talk_s", sheet = "assets/art/characters/su_wanqing/talk_s.png", cols = 2, fps = 4, looped = true },
      { name = "talk_n", sheet = "assets/art/characters/su_wanqing/talk_n.png", cols = 2, fps = 4, looped = true },
      { name = "talk_e", sheet = "assets/art/characters/su_wanqing/talk_e.png", cols = 2, fps = 4, looped = true },
      { name = "talk_w", sheet = "assets/art/characters/su_wanqing/talk_w.png", cols = 2, fps = 4, looped = true },
    },
  },
  xuanhe = {
    display = "玄鹤·云游道士",
    role = "npc",
    note = "素白道袍 + 灰蓝滚边 + 道髻木簪 + 长须 + 拂尘；最瘦（half_w 2），全白发色最亮",
    static_sprite = {
      s = "texture=assets/art/characters/xuanhe/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/characters/xuanhe/static_n.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/characters/xuanhe/static_e.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/characters/xuanhe/static_w.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/characters/xuanhe/idle_s.png:2:6:true;clip:idle_n:assets/art/characters/xuanhe/idle_n.png:2:6:true;clip:idle_e:assets/art/characters/xuanhe/idle_e.png:2:6:true;clip:idle_w:assets/art/characters/xuanhe/idle_w.png:2:6:true;clip:walk_s:assets/art/characters/xuanhe/walk_s.png:4:10:true;clip:walk_n:assets/art/characters/xuanhe/walk_n.png:4:10:true;clip:walk_e:assets/art/characters/xuanhe/walk_e.png:4:10:true;clip:walk_w:assets/art/characters/xuanhe/walk_w.png:4:10:true;clip:talk_s:assets/art/characters/xuanhe/talk_s.png:2:4:true;clip:talk_n:assets/art/characters/xuanhe/talk_n.png:2:4:true;clip:talk_e:assets/art/characters/xuanhe/talk_e.png:2:4:true;clip:talk_w:assets/art/characters/xuanhe/talk_w.png:2:4:true",
    clips = {
      { name = "idle_s", sheet = "assets/art/characters/xuanhe/idle_s.png", cols = 2, fps = 6, looped = true },
      { name = "idle_n", sheet = "assets/art/characters/xuanhe/idle_n.png", cols = 2, fps = 6, looped = true },
      { name = "idle_e", sheet = "assets/art/characters/xuanhe/idle_e.png", cols = 2, fps = 6, looped = true },
      { name = "idle_w", sheet = "assets/art/characters/xuanhe/idle_w.png", cols = 2, fps = 6, looped = true },
      { name = "walk_s", sheet = "assets/art/characters/xuanhe/walk_s.png", cols = 4, fps = 10, looped = true },
      { name = "walk_n", sheet = "assets/art/characters/xuanhe/walk_n.png", cols = 4, fps = 10, looped = true },
      { name = "walk_e", sheet = "assets/art/characters/xuanhe/walk_e.png", cols = 4, fps = 10, looped = true },
      { name = "walk_w", sheet = "assets/art/characters/xuanhe/walk_w.png", cols = 4, fps = 10, looped = true },
      { name = "talk_s", sheet = "assets/art/characters/xuanhe/talk_s.png", cols = 2, fps = 4, looped = true },
      { name = "talk_n", sheet = "assets/art/characters/xuanhe/talk_n.png", cols = 2, fps = 4, looped = true },
      { name = "talk_e", sheet = "assets/art/characters/xuanhe/talk_e.png", cols = 2, fps = 4, looped = true },
      { name = "talk_w", sheet = "assets/art/characters/xuanhe/talk_w.png", cols = 2, fps = 4, looped = true },
    },
  },
}

-- 朝向：s 南(面向镜头) / n 北(背对) / e 东(朝右) / w 西(朝左)
function M.get(id)
  return M.characters[id]
end

-- 稳定顺序列表（按 id 升序），供诊断与资源校验遍历。
function M.list()
  local out = {}
  for id, entry in pairs(M.characters) do
    out[#out + 1] = {
      id = id,
      display = entry.display,
      role = entry.role,
      sprite = entry.static_sprite.s,
      animation = entry.animation
    }
  end
  table.sort(out, function(a, b)
    return a.id < b.id
  end)
  return out
end

-- 主角动画组件串（写进 .scene 的 Animation 组件行）。
function M.player_animation(variant)
  local id = variant or "player_male"
  local entry = M.characters[id]
  if not entry then
    return nil
  end
  return entry.animation
end

-- NPC 当前可用的单帧 sprite 串（朝向 s/n/e/w）。
function M.npc_sprite(id, facing)
  local entry = M.characters[id]
  if not entry then
    return nil
  end
  return entry.static_sprite[facing or "s"]
end

function M.validate_all()
  local issues = {}
  if M.version ~= 1 then
    issues[#issues + 1] = "data/characters.lua: version: unsupported version " .. tostring(M.version)
  end
  for id, entry in pairs(M.characters) do
    if entry.role ~= "player" and entry.role ~= "npc" then
      issues[#issues + 1] = "data/characters.lua:" .. id .. ".role: expected player|npc"
    end
    if type(entry.animation) ~= "string" or not entry.animation:find("^default=", 1) then
      issues[#issues + 1] = "data/characters.lua:" .. id .. ".animation: expected default=<clip>;clip:... string"
    end
    for _, facing in ipairs({ "s", "n", "e", "w" }) do
      if type(entry.static_sprite) ~= "table" or type(entry.static_sprite[facing]) ~= "string" then
        issues[#issues + 1] = "data/characters.lua:" .. id .. ".static_sprite." .. facing .. ": missing"
      end
    end
    if type(entry.clips) ~= "table" or #entry.clips == 0 then
      issues[#issues + 1] = "data/characters.lua:" .. id .. ".clips: expected non-empty list"
    end
  end
  return #issues == 0, issues
end

-- 别名：与 main.lua 的加载期校验表统一入口名。
M.validate = M.validate_all

return M
