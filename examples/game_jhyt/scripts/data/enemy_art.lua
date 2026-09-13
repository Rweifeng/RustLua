-- data/enemy_art.lua：《江湖有田》敌人美术注册表（由 tools/gen_enemy_art.py 生成，勿手改）。
--
-- 字段：
--   display        显示名
--   sprite_size    世界尺寸（与帧尺寸一致）
--   static_sprite  单帧 SpriteComponent 串（朝向 s/n/e/w）
--   animation      Animation 组件串（default + clip:name:sheet:cols:fps:looped）
--   clips          条带清单（调试/验收用）
local M = {}

M.version = 1
M.enemies = {
  bandit_grunt = {
    display = "山贼喽啰",
    sprite_size = "32x32",
    static_sprite = {
      s = "texture=assets/art/enemies/bandit_grunt/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/enemies/bandit_grunt/static_n.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/enemies/bandit_grunt/static_e.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/enemies/bandit_grunt/static_w.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/enemies/bandit_grunt/idle_s.png:4:6:true;clip:idle_n:assets/art/enemies/bandit_grunt/idle_n.png:4:6:true;clip:idle_e:assets/art/enemies/bandit_grunt/idle_e.png:4:6:true;clip:idle_w:assets/art/enemies/bandit_grunt/idle_w.png:4:6:true;clip:walk_s:assets/art/enemies/bandit_grunt/walk_s.png:6:10:true;clip:walk_n:assets/art/enemies/bandit_grunt/walk_n.png:6:10:true;clip:walk_e:assets/art/enemies/bandit_grunt/walk_e.png:6:10:true;clip:walk_w:assets/art/enemies/bandit_grunt/walk_w.png:6:10:true;clip:attack_s:assets/art/enemies/bandit_grunt/attack_s.png:4:12:false;clip:attack_n:assets/art/enemies/bandit_grunt/attack_n.png:4:12:false;clip:attack_e:assets/art/enemies/bandit_grunt/attack_e.png:4:12:false;clip:attack_w:assets/art/enemies/bandit_grunt/attack_w.png:4:12:false;clip:hurt_s:assets/art/enemies/bandit_grunt/hurt_s.png:2:6:false;clip:hurt_n:assets/art/enemies/bandit_grunt/hurt_n.png:2:6:false;clip:hurt_e:assets/art/enemies/bandit_grunt/hurt_e.png:2:6:false;clip:hurt_w:assets/art/enemies/bandit_grunt/hurt_w.png:2:6:false;clip:faint_s:assets/art/enemies/bandit_grunt/faint_s.png:4:6:false;clip:faint_n:assets/art/enemies/bandit_grunt/faint_n.png:4:6:false;clip:faint_e:assets/art/enemies/bandit_grunt/faint_e.png:4:6:false;clip:faint_w:assets/art/enemies/bandit_grunt/faint_w.png:4:6:false",
  },
  gray_wolf = {
    display = "灰狼",
    sprite_size = "32x32",
    static_sprite = {
      s = "texture=assets/art/enemies/gray_wolf/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/enemies/gray_wolf/static_n.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/enemies/gray_wolf/static_e.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/enemies/gray_wolf/static_w.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/enemies/gray_wolf/idle_s.png:4:6:true;clip:idle_n:assets/art/enemies/gray_wolf/idle_n.png:4:6:true;clip:idle_e:assets/art/enemies/gray_wolf/idle_e.png:4:6:true;clip:idle_w:assets/art/enemies/gray_wolf/idle_w.png:4:6:true;clip:walk_s:assets/art/enemies/gray_wolf/walk_s.png:6:10:true;clip:walk_n:assets/art/enemies/gray_wolf/walk_n.png:6:10:true;clip:walk_e:assets/art/enemies/gray_wolf/walk_e.png:6:10:true;clip:walk_w:assets/art/enemies/gray_wolf/walk_w.png:6:10:true;clip:attack_s:assets/art/enemies/gray_wolf/attack_s.png:4:12:false;clip:attack_n:assets/art/enemies/gray_wolf/attack_n.png:4:12:false;clip:attack_e:assets/art/enemies/gray_wolf/attack_e.png:4:12:false;clip:attack_w:assets/art/enemies/gray_wolf/attack_w.png:4:12:false;clip:hurt_s:assets/art/enemies/gray_wolf/hurt_s.png:2:6:false;clip:hurt_n:assets/art/enemies/gray_wolf/hurt_n.png:2:6:false;clip:hurt_e:assets/art/enemies/gray_wolf/hurt_e.png:2:6:false;clip:hurt_w:assets/art/enemies/gray_wolf/hurt_w.png:2:6:false",
  },
  training_dummy = {
    display = "训练木人",
    sprite_size = "32x32",
    static_sprite = {
      s = "texture=assets/art/enemies/training_dummy/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/enemies/training_dummy/idle_s.png:2:4:true;clip:hurt_s:assets/art/enemies/training_dummy/hurt_s.png:2:6:false",
  },
  xuanming_adept = {
    display = "玄冥会徒",
    sprite_size = "32x32",
    static_sprite = {
      s = "texture=assets/art/enemies/xuanming_adept/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/enemies/xuanming_adept/static_n.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/enemies/xuanming_adept/static_e.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/enemies/xuanming_adept/static_w.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/enemies/xuanming_adept/idle_s.png:4:6:true;clip:idle_n:assets/art/enemies/xuanming_adept/idle_n.png:4:6:true;clip:idle_e:assets/art/enemies/xuanming_adept/idle_e.png:4:6:true;clip:idle_w:assets/art/enemies/xuanming_adept/idle_w.png:4:6:true;clip:walk_s:assets/art/enemies/xuanming_adept/walk_s.png:6:10:true;clip:walk_n:assets/art/enemies/xuanming_adept/walk_n.png:6:10:true;clip:walk_e:assets/art/enemies/xuanming_adept/walk_e.png:6:10:true;clip:walk_w:assets/art/enemies/xuanming_adept/walk_w.png:6:10:true;clip:attack_s:assets/art/enemies/xuanming_adept/attack_s.png:4:12:false;clip:attack_n:assets/art/enemies/xuanming_adept/attack_n.png:4:12:false;clip:attack_e:assets/art/enemies/xuanming_adept/attack_e.png:4:12:false;clip:attack_w:assets/art/enemies/xuanming_adept/attack_w.png:4:12:false;clip:hurt_s:assets/art/enemies/xuanming_adept/hurt_s.png:2:6:false;clip:hurt_n:assets/art/enemies/xuanming_adept/hurt_n.png:2:6:false;clip:hurt_e:assets/art/enemies/xuanming_adept/hurt_e.png:2:6:false;clip:hurt_w:assets/art/enemies/xuanming_adept/hurt_w.png:2:6:false;clip:faint_s:assets/art/enemies/xuanming_adept/faint_s.png:4:6:false;clip:faint_n:assets/art/enemies/xuanming_adept/faint_n.png:4:6:false;clip:faint_e:assets/art/enemies/xuanming_adept/faint_e.png:4:6:false;clip:faint_w:assets/art/enemies/xuanming_adept/faint_w.png:4:6:false",
  },
  poison_scorpion = {
    display = "毒蝎",
    sprite_size = "32x32",
    static_sprite = {
      s = "texture=assets/art/enemies/poison_scorpion/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/enemies/poison_scorpion/static_n.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/enemies/poison_scorpion/static_e.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/enemies/poison_scorpion/static_w.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/enemies/poison_scorpion/idle_s.png:4:6:true;clip:idle_n:assets/art/enemies/poison_scorpion/idle_n.png:4:6:true;clip:idle_e:assets/art/enemies/poison_scorpion/idle_e.png:4:6:true;clip:idle_w:assets/art/enemies/poison_scorpion/idle_w.png:4:6:true;clip:walk_s:assets/art/enemies/poison_scorpion/walk_s.png:6:10:true;clip:walk_n:assets/art/enemies/poison_scorpion/walk_n.png:6:10:true;clip:walk_e:assets/art/enemies/poison_scorpion/walk_e.png:6:10:true;clip:walk_w:assets/art/enemies/poison_scorpion/walk_w.png:6:10:true;clip:attack_s:assets/art/enemies/poison_scorpion/attack_s.png:4:12:false;clip:attack_n:assets/art/enemies/poison_scorpion/attack_n.png:4:12:false;clip:attack_e:assets/art/enemies/poison_scorpion/attack_e.png:4:12:false;clip:attack_w:assets/art/enemies/poison_scorpion/attack_w.png:4:12:false;clip:hurt_s:assets/art/enemies/poison_scorpion/hurt_s.png:2:6:false;clip:hurt_n:assets/art/enemies/poison_scorpion/hurt_n.png:2:6:false;clip:hurt_e:assets/art/enemies/poison_scorpion/hurt_e.png:2:6:false;clip:hurt_w:assets/art/enemies/poison_scorpion/hurt_w.png:2:6:false",
  },
  mountain_ape_boss = {
    display = "山魈首领（第一章 Boss）",
    sprite_size = "48x48",
    static_sprite = {
      s = "texture=assets/art/enemies/mountain_ape_boss/static_s.png;size=48x48;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      n = "texture=assets/art/enemies/mountain_ape_boss/static_n.png;size=48x48;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      e = "texture=assets/art/enemies/mountain_ape_boss/static_e.png;size=48x48;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
      w = "texture=assets/art/enemies/mountain_ape_boss/static_w.png;size=48x48;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    },
    animation = "default=idle_s;clip:idle_s:assets/art/enemies/mountain_ape_boss/idle_s.png:4:5:true;clip:idle_n:assets/art/enemies/mountain_ape_boss/idle_n.png:4:5:true;clip:idle_e:assets/art/enemies/mountain_ape_boss/idle_e.png:4:5:true;clip:idle_w:assets/art/enemies/mountain_ape_boss/idle_w.png:4:5:true;clip:walk_s:assets/art/enemies/mountain_ape_boss/walk_s.png:6:8:true;clip:walk_n:assets/art/enemies/mountain_ape_boss/walk_n.png:6:8:true;clip:walk_e:assets/art/enemies/mountain_ape_boss/walk_e.png:6:8:true;clip:walk_w:assets/art/enemies/mountain_ape_boss/walk_w.png:6:8:true;clip:attack_s:assets/art/enemies/mountain_ape_boss/attack_s.png:4:10:false;clip:attack_n:assets/art/enemies/mountain_ape_boss/attack_n.png:4:10:false;clip:attack_e:assets/art/enemies/mountain_ape_boss/attack_e.png:4:10:false;clip:attack_w:assets/art/enemies/mountain_ape_boss/attack_w.png:4:10:false;clip:hurt_s:assets/art/enemies/mountain_ape_boss/hurt_s.png:2:6:false;clip:hurt_n:assets/art/enemies/mountain_ape_boss/hurt_n.png:2:6:false;clip:hurt_e:assets/art/enemies/mountain_ape_boss/hurt_e.png:2:6:false;clip:hurt_w:assets/art/enemies/mountain_ape_boss/hurt_w.png:2:6:false",
  },
}

function M.get(id)
  return M.enemies[id]
end

-- 运行期把美术挂到敌人实体上：先设单帧（立刻可见），再 attach 动画。
function M.apply(entity_id, id, facing)
  local art = M.enemies[id]
  if not art or not entity_id then
    return false
  end
  local ok = pcall(sprite.set, entity_id, art.static_sprite[facing or 's'] or art.static_sprite.s)
  if not ok then
    return false
  end
  local attached = pcall(animation.attach, entity_id, art.animation)
  return attached == true
end

function M.validate_all()
  local issues = {}
  if M.version ~= 1 then
    issues[#issues + 1] = 'data/enemy_art.lua: version: unsupported version ' .. tostring(M.version)
  end
  for id, art in pairs(M.enemies) do
    if type(art.animation) ~= 'string' or not art.animation:find('^default=', 1) then
      issues[#issues + 1] = 'data/enemy_art.lua:' .. id .. '.animation: expected default=..;clip:..'
    end
    if type(art.static_sprite) ~= 'table' or type(art.static_sprite.s) ~= 'string' then
      issues[#issues + 1] = 'data/enemy_art.lua:' .. id .. '.static_sprite.s: missing'
    end
  end
  return #issues == 0, issues
end

return M
