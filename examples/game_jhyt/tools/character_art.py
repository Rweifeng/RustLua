# -*- coding: utf-8 -*-
"""场景生成器共用：从角色美术 manifest 取主角的 Sprite / Animation 组件串。

为什么不把串直接抄进生成器：`assets/art/characters/manifest.json` 是
`tools/gen_character_sprites.py` 的产物，是"主角长什么样"的唯一事实来源；
场景里写死纹理路径，一旦美术换帧尺寸/换目录就会与数据层脱节
（docs/36 §8.2 要求场景与 characters.lua 用同一套串）。

用法：
    from character_art import player_art
    sprite, animation = player_art("player_male")
    # sprite    -> "texture=assets/art/characters/player_male/static_s.png;size=32x32;..."
    # animation -> "default=idle_s;clip:idle_s:...;clip:walk_s:..."（manifest 缺失时为 None）

manifest 缺失/条目不存在时回退到旧的灰盒 sprite（保证生成器仍可运行，
并在 stderr 说明原因），而不是直接崩掉整条美术链。
"""

import json
import os
import sys

MANIFEST = os.path.join("assets", "art", "characters", "manifest.json")
FALLBACK_SPRITE = (
    "texture=__white__;size=28x28;tint=0.949,0.918,0.847,1.000;sorting_layer=actor"
)


def load_manifest(path=MANIFEST):
    if not os.path.exists(path):
        return None
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def character_entry(character_id, path=MANIFEST):
    manifest = load_manifest(path)
    if not manifest:
        return None
    for entry in manifest.get("characters", []):
        if entry.get("id") == character_id:
            return entry
    return None


def player_art(character_id="player_male", path=MANIFEST):
    """返回 (sprite_component_str, animation_component_str_or_None)。

    角色不存在时回退灰盒 sprite + None（场景不带 Animation，运行期
    player.apply() 仍会补上正确的形象）。
    """
    entry = character_entry(character_id, path)
    if not entry:
        print(
            f"[character_art] manifest 里找不到角色 {character_id}"
            f"（{path}），场景回退灰盒 sprite；先跑 tools/gen_character_sprites.py",
            file=sys.stderr,
        )
        return FALLBACK_SPRITE, None
    sprite = entry.get("static_sprite") or FALLBACK_SPRITE
    animation = entry.get("animation")
    return sprite, animation
