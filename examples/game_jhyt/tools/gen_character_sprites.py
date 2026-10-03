# -*- coding: utf-8 -*-
"""《江湖有田》角色序列帧生成器（主角 + NPC）。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_character_sprites.py                     # 生成全部角色
    python tools/gen_character_sprites.py --only player_male   # 只生成指定角色
    python tools/gen_character_sprites.py --check              # 只校验已生成资源

产出：

    assets/art/characters/<char_id>/<action>_<dir>.png   1×N 横向帧条带（引擎 Animation clip 采样）
    assets/art/characters/<char_id>/static_<dir>.png     单帧静态图（无 Animation 组件时的 Sprite 兜底）
    assets/art/characters/<char_id>/preview.png          单角色形象总览（带 ASCII 标签）
    assets/art/characters/_overview.png                  全角色总览
    assets/art/characters/manifest.json                  机器可读清单（引擎组件串在此）
    scripts/data/characters.lua                          游戏侧角色形象/动画注册表

引擎事实（engine/crates/engine/src/core/animation.rs）：

  * Animation 组件串 = `default=<clip>;clip:<name>:<sheet>:<cols>:<fps>:<looped>`；
  * 每个 clip 采样 **一整张 1×cols 横向条带**（UV: u=frame/cols..(frame+1)/cols, v=0..1），
    所以「一个动作 + 一个朝向」必须是一张独立条带，不能拼成多行图集；
  * 帧尺寸 = 条带高度 × (条带宽度 / cols)；
  * 有 Animation 组件时渲染器用 clip 的 sheet 覆盖 Sprite 的 texture 并裁 UV，
    Sprite 只负责尺寸/对齐/排序层（host/runtime_scene.rs vertices()）。

注意：NPC 由 `entity.spawn` 运行时创建，Lua 侧没有挂 Animation 组件的 API，
因此 NPC 目前只有 static_<dir>.png 可用；缺口见 docs/36。

依赖：Python 3 标准库（zlib/struct），不需要 Pillow。
"""

import argparse
import json
import os
import struct
import zlib

# ---------------------------------------------------------------------------
# 常量
# ---------------------------------------------------------------------------

FRAME = 32  # 绘制画布尺寸（像素）：与世界 tile 32 单位 1:1（绘制逻辑坐标）
ART_SCALE = 2  # 导出放大倍数：角色帧 = FRAME × ART_SCALE = 64×64（docs/51 §2.2 标准）
FRAME_PX = FRAME * ART_SCALE  # 导出帧尺寸：引擎 Sprite size 与 Animation 条带采样都用它
FOOT_Y = 29  # 脚底基线（画布 y 向下，绘制坐标）
CX = 16  # 角色中轴（绘制坐标）

# 八向：四正（s/n/e/w）+ 四斜（se/sw/ne/nw）。
# 对角帧采用"侧身布局 + 正脸头"（像素游戏 8 向通用做法），见 layout()/draw_head()。
DIRECTIONS = ("s", "n", "e", "w", "se", "sw", "ne", "nw")
DIR_TITLE = {
    "s": "南(面向镜头)", "n": "北(背对)", "e": "东(朝右)", "w": "西(朝左)",
    "se": "东南(右下)", "sw": "西南(左下)", "ne": "东北(右上)", "nw": "西北(左上)",
}
# 负 x 朝向 → 正 x 基准朝向（渲染后整体水平镜像），复用既有 w 的镜像逻辑。
MIRROR_DIR = {"w": "e", "sw": "se", "nw": "ne"}

# 动作 → 每方向帧数 / FPS / 是否循环（对应 docs/31 §3.2 的帧预算）
ACTIONS = {
    "idle": {"frames": 4, "fps": 6, "looped": True, "player": True, "npc": True},
    "walk": {"frames": 6, "fps": 10, "looped": True, "player": True, "npc": True},
    "talk": {"frames": 2, "fps": 4, "looped": True, "player": False, "npc": True},
    "hoe": {"frames": 4, "fps": 8, "looped": False, "player": True, "npc": False},
    "water": {"frames": 4, "fps": 8, "looped": False, "player": True, "npc": False},
    "attack": {"frames": 4, "fps": 12, "looped": False, "player": True, "npc": False},
    "hurt": {"frames": 2, "fps": 6, "looped": False, "player": True, "npc": False},
    "faint": {"frames": 4, "fps": 6, "looped": False, "player": True, "npc": False},
}

# 色板锚点（与 docs/35-UI资源设计 §3 同源）
INK = (0x1F, 0x23, 0x28, 255)
SHADOW = (0x1F, 0x23, 0x28, 70)
WOOD = (0x8A, 0x6A, 0x46, 255)
WOOD_DARK = (0x4A, 0x36, 0x28, 255)
IRON = (0xC8, 0xCC, 0xD4, 255)
IRON_DARK = (0x7E, 0x83, 0x91, 255)
WATER = (0x7F, 0xA3, 0xD4, 255)
HILIGHT = (0xFF, 0xFF, 0xFF, 210)


# ---------------------------------------------------------------------------
# 画布与 PNG 编码
# ---------------------------------------------------------------------------


class Canvas:
    """极简 RGBA8 画布：像素级绘制 + 轮廓描边 + 水平镜像。"""

    __slots__ = ("w", "h", "px")

    def __init__(self, w=FRAME, h=FRAME):
        self.w = w
        self.h = h
        self.px = bytearray(w * h * 4)

    def set(self, x, y, color):
        x = int(x)
        y = int(y)
        if 0 <= x < self.w and 0 <= y < self.h:
            i = (y * self.w + x) * 4
            self.px[i:i + 4] = bytes(color)

    def get(self, x, y):
        if 0 <= x < self.w and 0 <= y < self.h:
            i = (y * self.w + x) * 4
            return tuple(self.px[i:i + 4])
        return (0, 0, 0, 0)

    def opaque(self, x, y):
        return self.get(x, y)[3] > 0

    def rect(self, x0, y0, x1, y1, color):
        for y in range(int(y0), int(y1) + 1):
            for x in range(int(x0), int(x1) + 1):
                self.set(x, y, color)

    def hline(self, x0, x1, y, color):
        self.rect(x0, y, x1, y, color)

    def vline(self, x, y0, y1, color):
        self.rect(x, y0, x, y1, color)

    def blit(self, other):
        for y in range(other.h):
            for x in range(other.w):
                c = other.get(x, y)
                if c[3] > 0:
                    self.set(x, y, c)

    def outline(self, color=INK):
        """给不透明区域外描一圈 1px 轮廓（4 邻域），像素画可读性关键。"""
        edges = []
        for y in range(self.h):
            for x in range(self.w):
                if self.get(x, y)[3] > 0:
                    continue
                for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    if self.opaque(x + dx, y + dy):
                        edges.append((x, y))
                        break
        for x, y in edges:
            self.set(x, y, color)

    def mirrored(self):
        out = Canvas(self.w, self.h)
        for y in range(self.h):
            for x in range(self.w):
                out.set(self.w - 1 - x, y, self.get(x, y))
        return out

    def scaled(self, factor):
        """最近邻整数放大（预览用；与 pixel_kit.Canvas.scaled 同语义）。"""
        out = Canvas(self.w * factor, self.h * factor)
        for y in range(self.h):
            for x in range(self.w):
                color = self.get(x, y)
                if color[3] == 0:
                    continue
                for sy in range(factor):
                    for sx in range(factor):
                        out.set(x * factor + sx, y * factor + sy, color)
        return out

    def to_png(self):
        raw = bytearray()
        stride = self.w * 4
        for y in range(self.h):
            raw.append(0)  # filter type 0
            raw += self.px[y * stride:(y + 1) * stride]
        return encode_png(self.w, self.h, bytes(raw))


def _chunk(tag, data):
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def encode_png(width, height, raw_rgba):
    """RGBA8 (color type 6) PNG 编码，不需要 Pillow。"""
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    body = _chunk(b"IHDR", ihdr)
    body += _chunk(b"IDAT", zlib.compress(raw_rgba, 9))
    body += _chunk(b"IEND", b"")
    return b"\x89PNG\r\n\x1a\n" + body


def png_size(path):
    """读 IHDR 返回 (width, height, bit_depth, color_type)。"""
    with open(path, "rb") as handle:
        data = handle.read(26)
    if len(data) < 26 or data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG: " + path)
    width, height = struct.unpack(">II", data[16:24])
    return width, height, data[24], data[25]


# ---------------------------------------------------------------------------
# 颜色工具
# ---------------------------------------------------------------------------


def rgb(value):
    value = value.lstrip("#")
    return (int(value[0:2], 16), int(value[2:4], 16), int(value[4:6], 16), 255)


def shade(color, factor):
    return (
        max(0, min(255, int(color[0] * factor))),
        max(0, min(255, int(color[1] * factor))),
        max(0, min(255, int(color[2] * factor))),
        color[3] if len(color) > 3 else 255,
    )


def mix(a, b, t):
    return (
        int(a[0] + (b[0] - a[0]) * t),
        int(a[1] + (b[1] - a[1]) * t),
        int(a[2] + (b[2] - a[2]) * t),
        255,
    )


# ---------------------------------------------------------------------------
# 角色设定表（形象设计的唯一来源）
# ---------------------------------------------------------------------------
#
# build：head_top 越小越高；half_w 越大越壮。
# outfit：robe 长衫 / skirt 上衣+裙 / shorts 短打+裤 / apron 短褂+皮围裙
# hair：topknot 束发髻 / bun 双螺髻 / ponytail 高马尾 / kerchief 头巾 /
#       cap 瓜皮帽 / dao 道髻木簪 / band 发带短发
# beard：none / stubble 胡茬 / goatee 山羊胡 / long 长须
# accessory：sword_back 背剑 / med_box 药箱 / abacus 算盘 / tea_basket 茶篓 /
#            hammer 铁锤 / saber 单刀 / sword_hip 佩剑 / whisk 拂尘
#
# palette 主色沿用 data/npcs.lua 的 tint 意图（灰盒期的颜色约定），
# 再落到 docs/35 的色板 token 上，保证 HUD/角色同一套色。

CHARACTERS = [
    {
        "id": "player_male",
        "display": "主角·青衫少侠（默认男）",
        "role": "player",
        "note": "素色交领短打 + 竹青护腕束腰，背负木剑；米白/竹青便于在药田绿底上读轮廓",
        "build": {"head_top": 5, "head_h": 7, "head_w": 7, "half_w": 3, "torso_bottom": 22},
        "outfit": "shorts",
        "hair": "topknot",
        "beard": "none",
        "accessory": "sword_back",
        "tool": {"hoe": "hoe", "water": "pot", "attack": "blade"},
        "palette": {
            "skin": rgb("#E8C49A"),
            "hair": rgb("#2B2B33"),
            "cloth": rgb("#F2EAD8"),        # paper/50 米白上衣
            "cloth_alt": rgb("#789262"),    # bamboo/500 竹青裤/护腕
            "trim": rgb("#4A6FA5"),         # indigo/500 交领
            "belt": rgb("#4A3628"),         # wood/700
            "shoe": rgb("#4A3628"),
        },
    },
    {
        "id": "player_female",
        "display": "主角·青衣女侠（可选女）",
        "role": "player",
        "note": "与男主体共用色板与动作集，仅改轮廓（裙摆 + 马尾），保证换装/自定义不返工",
        "build": {"head_top": 5, "head_h": 7, "head_w": 7, "half_w": 3, "torso_bottom": 22},
        "outfit": "skirt",
        "hair": "ponytail",
        "beard": "none",
        "accessory": "sword_back",
        "tool": {"hoe": "hoe", "water": "pot", "attack": "blade"},
        "palette": {
            "skin": rgb("#F0CFA8"),
            "hair": rgb("#241F1C"),
            "cloth": rgb("#F2EAD8"),
            "cloth_alt": rgb("#789262"),
            "trim": rgb("#4A6FA5"),
            "belt": rgb("#C9A05A"),         # gold/500
            "shoe": rgb("#4A3628"),
        },
    },
    {
        "id": "shen_qingluo",
        "display": "沈青萝·医馆掌柜",
        "role": "npc",
        "note": "藕荷罗衫 + 米白药围裙 + 木药箱；双螺髻，慈和中年女性轮廓",
        "build": {"head_top": 5, "head_h": 7, "head_w": 7, "half_w": 3, "torso_bottom": 21},
        "outfit": "skirt",
        "hair": "bun",
        "beard": "none",
        "accessory": "med_box",
        "palette": {
            "skin": rgb("#F0CFA8"),
            "hair": rgb("#33272A"),
            "cloth": rgb("#C97B8E"),        # 原 tint(0.86,0.42,0.55) 的藕荷
            "cloth_alt": rgb("#A85F73"),
            "trim": rgb("#F2EAD8"),
            "belt": rgb("#789262"),
            "shoe": rgb("#4A3628"),
        },
    },
    {
        "id": "zhao_tiesuanpan",
        "display": "赵铁算盘·集市商人",
        "role": "npc",
        "note": "旧金长衫 + 墨色瓜皮帽 + 腰间算盘；体胖（half_w 4），辨识度靠体型与金色",
        "build": {"head_top": 6, "head_h": 7, "head_w": 7, "half_w": 4, "torso_bottom": 22},
        "outfit": "robe",
        "hair": "cap",
        "beard": "goatee",
        "accessory": "abacus",
        "palette": {
            "skin": rgb("#E8C49A"),
            "hair": rgb("#2B2B33"),
            "cloth": rgb("#C9A05A"),        # gold/500
            "cloth_alt": rgb("#A8813F"),
            "trim": rgb("#4A3628"),
            "belt": rgb("#4A3628"),
            "shoe": rgb("#31383F"),
        },
    },
    {
        "id": "lin_xiaoman",
        "display": "林小满·茶农",
        "role": "npc",
        "note": "竹青布衣 + 黛蓝印花头巾 + 竹茶篓；最矮（少女），绿色与药田同色系但靠头巾区分",
        "build": {"head_top": 6, "head_h": 7, "head_w": 7, "half_w": 3, "torso_bottom": 22},
        "outfit": "shorts",
        "hair": "kerchief",
        "beard": "none",
        "accessory": "tea_basket",
        "palette": {
            "skin": rgb("#F2D3B0"),
            "hair": rgb("#2E2622"),
            "cloth": rgb("#789262"),        # 原 tint(0.42,0.72,0.45) 的竹青
            "cloth_alt": rgb("#A9C28F"),
            "trim": rgb("#4A6FA5"),         # indigo/500 头巾
            "belt": rgb("#4A3628"),
            "shoe": rgb("#6B4F3A"),
        },
    },
    {
        "id": "han_tieshan",
        "display": "韩铁山·铁匠",
        "role": "npc",
        "note": "钢灰短褂 + 皮革围裙 + 朱砂发带 + 铁锤；肩最宽（half_w 4），配色最重",
        "build": {"head_top": 5, "head_h": 7, "head_w": 7, "half_w": 4, "torso_bottom": 22},
        "outfit": "apron",
        "hair": "band",
        "beard": "stubble",
        "accessory": "hammer",
        "palette": {
            "skin": rgb("#D9A87C"),
            "hair": rgb("#241F1C"),
            "cloth": rgb("#7E8391"),        # 原 tint(0.55,0.55,0.62) 的钢灰
            "cloth_alt": rgb("#5E6470"),
            "trim": rgb("#C3452F"),         # cinnabar/500 发带
            "belt": rgb("#4A3628"),
            "shoe": rgb("#31383F"),
        },
    },
    {
        "id": "gu_changfeng",
        "display": "顾长风·退隐镖师",
        "role": "npc",
        "note": "黛蓝劲装长衫 + 灰短披肩 + 单刀 + 络腮短须；高个（head_top 3），护送/教刀场景主视觉",
        "build": {"head_top": 3, "head_h": 7, "head_w": 7, "half_w": 3, "torso_bottom": 22},
        "outfit": "robe",
        "hair": "topknot",
        "beard": "stubble",
        "accessory": "saber",
        "palette": {
            "skin": rgb("#E0B489"),
            "hair": rgb("#2B2B33"),
            "cloth": rgb("#2E4A6B"),        # 原 tint(0.28,0.45,0.62) 的黛蓝
            "cloth_alt": rgb("#4A6FA5"),
            "trim": rgb("#F2EAD8"),
            "belt": rgb("#C9A05A"),
            "shoe": rgb("#31383F"),
        },
    },
    {
        "id": "su_wanqing",
        "display": "苏晚晴·青云门弟子",
        "role": "npc",
        "note": "青云色门派道服 + 旧金束腰 + 佩剑；高马尾（走路时甩动），与玄鹤同门派但一蓝一白",
        "build": {"head_top": 5, "head_h": 7, "head_w": 7, "half_w": 3, "torso_bottom": 21},
        "outfit": "robe",
        "hair": "ponytail",
        "beard": "none",
        "accessory": "sword_hip",
        "palette": {
            "skin": rgb("#F2D3B0"),
            "hair": rgb("#241F1C"),
            "cloth": rgb("#7FA3D4"),        # 原 tint(0.55,0.72,0.85) 的青云
            "cloth_alt": rgb("#5C82B4"),
            "trim": rgb("#F2EAD8"),
            "belt": rgb("#C9A05A"),
            "shoe": rgb("#4A3628"),
        },
    },
    {
        "id": "xuanhe",
        "display": "玄鹤·云游道士",
        "role": "npc",
        "note": "素白道袍 + 灰蓝滚边 + 道髻木簪 + 长须 + 拂尘；最瘦（half_w 2），全白发色最亮",
        "build": {"head_top": 3, "head_h": 8, "head_w": 7, "half_w": 2, "torso_bottom": 22},
        "outfit": "robe",
        "hair": "dao",
        "beard": "long",
        "accessory": "whisk",
        "palette": {
            "skin": rgb("#E8D2B8"),
            "hair": rgb("#E4E4EA"),         # 原 tint(0.92,0.92,0.96) 的鹤白
            "cloth": rgb("#EAEAF0"),
            "cloth_alt": rgb("#C9CCD6"),
            "trim": rgb("#6E7A8A"),
            "belt": rgb("#6E7A8A"),
            "shoe": rgb("#8A8F98"),
        },
    },
    # ---- 批次 4：首发 NPC 扩充（docs/30 §4.2）----
    {
        "id": "a_qi",
        "display": "阿七·年轻镖师",
        "role": "npc",
        "note": "护安镖局短打 + 镖旗纹样护腕 + 腰刀；最年轻、动作最利落（偏矮 + 标准体型）",
        "build": {"head_top": 6, "head_h": 7, "head_w": 7, "half_w": 3, "torso_bottom": 22},
        "outfit": "shorts",
        "hair": "band",
        "beard": "none",
        "accessory": "saber",
        "palette": {
            "skin": rgb("#E8C49A"),
            "hair": rgb("#2E2622"),
            "cloth": rgb("#B25A3C"),        # 镖局赭红短打
            "cloth_alt": rgb("#4A3628"),
            "trim": rgb("#C9A05A"),         # 旧金镖局纹样
            "belt": rgb("#3D444C"),
            "shoe": rgb("#4A3628"),
        },
    },
    {
        "id": "situ_kui",
        "display": "司徒魁·玄冥会执事",
        "role": "npc",
        "note": "玄冥会墨黑长袍 + 暗紫滚边 + 面罩半遮；反派辨识度靠全黑配紫",
        "build": {"head_top": 3, "head_h": 7, "head_w": 7, "half_w": 3, "torso_bottom": 22},
        "outfit": "robe",
        "hair": "cap",
        "beard": "goatee",
        "accessory": "sword_hip",
        "palette": {
            "skin": rgb("#D9B08C"),
            "hair": rgb("#1E1A1C"),
            "cloth": rgb("#2A2430"),        # 墨黑
            "cloth_alt": rgb("#4A3A56"),    # 暗紫
            "trim": rgb("#8B6BB8"),         # 传世紫
            "belt": rgb("#8B6BB8"),
            "shoe": rgb("#1E1A1C"),
        },
    },
    {
        "id": "lu_guizang",
        "display": "陆归藏·师父（已失踪）",
        "role": "npc",
        "note": "旧青灰布袍 + 灰白发 + 药篓；轮廓与玄鹤区分：更旧的配色、带背篓",
        "build": {"head_top": 4, "head_h": 7, "head_w": 7, "half_w": 3, "torso_bottom": 22},
        "outfit": "robe",
        "hair": "dao",
        "beard": "long",
        "accessory": "tea_basket",
        "palette": {
            "skin": rgb("#DCC0A0"),
            "hair": rgb("#B9BDC4"),         # 灰白
            "cloth": rgb("#6E7A72"),        # 旧青灰
            "cloth_alt": rgb("#57635C"),
            "trim": rgb("#D8CDB2"),
            "belt": rgb("#4A3628"),
            "shoe": rgb("#57635C"),
        },
    },
]


def by_id(char_id):
    for ch in CHARACTERS:
        if ch["id"] == char_id:
            return ch
    raise KeyError(char_id)


# ---------------------------------------------------------------------------
# 骨架布局
# ---------------------------------------------------------------------------


def layout(ch, facing):
    build = ch["build"]
    side = facing in ("e", "w", "se", "sw", "ne", "nw")
    diag = facing in ("se", "sw", "ne", "nw")
    half = build["half_w"] - (1 if side else 0)
    head_top = build["head_top"]
    head_h = build["head_h"]
    return {
        "side": side,
        "diag": diag,
        "half": max(1, half),
        "head_top": head_top,
        "head_h": head_h,
        "head_w": build["head_w"],
        "chin": head_top + head_h - 1,
        "torso_top": head_top + head_h + 1,
        "torso_bottom": build["torso_bottom"],
    }


def skin_of(pal):
    return pal["skin"]


def skin_shade(pal):
    return shade(pal["skin"], 0.82)


def cloth_dark(ch):
    return shade(ch["palette"]["cloth"], 0.78)


def cloth_light(ch):
    return mix(ch["palette"]["cloth"], (255, 255, 255, 255), 0.18)


# ---------------------------------------------------------------------------
# 各部件绘制
# ---------------------------------------------------------------------------


def draw_shadow(cv, fallen=False):
    if fallen:
        cv.hline(CX - 10, CX + 8, 31, SHADOW)
        cv.hline(CX - 11, CX + 9, 30, SHADOW)
        cv.hline(CX - 9, CX + 7, 29, SHADOW)
        return
    cv.hline(CX - 6, CX + 6, 31, SHADOW)
    cv.hline(CX - 7, CX + 7, 30, SHADOW)
    cv.hline(CX - 5, CX + 5, 29, SHADOW)


def draw_legs(cv, ch, facing, lay, o):
    pal = ch["palette"]
    outfit = ch["outfit"]
    leg = float(o.get("leg", 0.0))
    squat = int(o.get("crouch", 0) or 0)
    half = lay["half"]
    bottom = FOOT_Y - squat  # 下蹲时脚仍在地面：改为身体下沉
    if squat:
        bottom = FOOT_Y
    trouser = pal["cloth_alt"]
    shoe = pal["shoe"]
    leg_top = lay["torso_bottom"] + 1

    if outfit in ("robe", "skirt"):
        # 长衫/裙摆盖到大腿，只露鞋与一小截裤脚
        leg_top = max(leg_top, FOOT_Y - 3)

    if lay["side"]:
        front_x = CX - 1 + int(round(leg * 2))
        back_x = CX - 2 - int(round(leg * 1.2))
        front_lift = 2 if abs(leg) > 0.8 else (1 if abs(leg) > 0.2 else 0)
        for x, lift in ((back_x, 0), (front_x, front_lift)):
            cv.rect(x, leg_top, x + 1, bottom - 2 - lift, trouser)
            cv.rect(x - 1, bottom - 1 - lift, x + 2, bottom - lift, shoe)
    else:
        lx = CX - 2
        rx = CX + 1
        lift_l = 2 if leg > 0.8 else (1 if leg > 0.3 else 0)
        lift_r = 2 if leg < -0.8 else (1 if leg < -0.3 else 0)
        if leg > 0.3:
            lx -= 1
        if leg < -0.3:
            rx += 1
        if lay["half"] >= 4:
            lx -= 1
            rx += 1
        cv.rect(lx, leg_top, lx + 1, bottom - 2 - lift_l, trouser)
        cv.rect(rx, leg_top, rx + 1, bottom - 2 - lift_r, trouser)
        cv.rect(lx - 1, bottom - 1 - lift_l, lx + 2, bottom - lift_l, shoe)
        cv.rect(rx - 1, bottom - 1 - lift_r, rx + 2, bottom - lift_r, shoe)


def draw_torso(cv, ch, facing, lay, o):
    pal = ch["palette"]
    outfit = ch["outfit"]
    top = lay["torso_top"] + int(o.get("bob", 0)) - int(o.get("crouch", 0) or 0)
    bottom = lay["torso_bottom"] + int(o.get("bob", 0)) - int(o.get("crouch", 0) or 0)
    half = lay["half"]
    lean = int(o.get("lean", 0))
    x0 = CX - half + lean
    x1 = CX + half + lean
    cloth = pal["cloth"]
    dark = cloth_dark(ch)
    light = cloth_light(ch)
    trim = pal["trim"]
    belt = pal["belt"]

    def body_row(y, xa, xb, color):
        cv.hline(xa, xb, y, color)

    # 脖子先画，随后被交领/衣身覆盖：保证领口不被脖子顶掉
    cv.rect(CX - 1 + lean, top - 1, CX + lean, top, skin_shade(pal))

    if outfit == "robe":
        for y in range(top, bottom + 5):
            grow = 1 if y >= bottom - 1 else 0
            body_row(y, x0 - grow, x1 + grow, cloth)
        body_row(top, x0, x1, cloth)
        # 交领
        cv.vline(CX + lean, top, top + 2, trim)
        cv.vline(CX + lean - 1, top + 1, top + 2, trim)
        cv.vline(CX + lean + 1, top + 1, top + 2, trim)
        cv.hline(x0, x1, bottom, belt)
        cv.hline(x0 - 1, x1 + 1, bottom + 1, belt)
        # 下摆开衩：左右各一条暗色竖线
        cv.vline(x0 - 1, bottom + 2, bottom + 4, dark)
        cv.vline(x1 + 1, bottom + 2, bottom + 4, dark)
    elif outfit == "skirt":
        for y in range(top, bottom):
            body_row(y, x0, x1, cloth)
        cv.vline(CX + lean, top, top + 2, trim)
        cv.vline(CX + lean - 1, top + 1, top + 2, trim)
        cv.vline(CX + lean + 1, top + 1, top + 2, trim)
        cv.hline(x0, x1, bottom - 1, belt)
        # 裙：外扩两段
        for y in range(bottom, bottom + 3):
            grow = 1 if y < bottom + 2 else 2
            body_row(y, x0 - grow, x1 + grow, pal["cloth_alt"])
        cv.hline(x0 - 2, x1 + 2, bottom + 3, shade(pal["cloth_alt"], 0.8))
    elif outfit == "shorts":
        for y in range(top, bottom + 1):
            body_row(y, x0, x1, cloth)
        # 交领 + 短打系带
        cv.vline(CX + lean, top, top + 2, trim)
        cv.vline(CX + lean - 1, top + 1, top + 2, trim)
        cv.vline(CX + lean + 1, top + 1, top + 2, trim)
        cv.hline(x0, x1, bottom - 1, belt)
    else:  # apron：短褂 + 皮围裙
        for y in range(top, bottom + 2):
            body_row(y, x0, x1, cloth)
        cv.hline(x0, x1, top, cloth)
        cv.hline(x0, x1, bottom - 1, belt)
        # 围裙前片（皮革）
        ap0 = CX - 2 + lean
        ap1 = CX + 1 + lean
        cv.rect(ap0, top + 2, ap1, bottom + 3, WOOD_DARK)
        cv.hline(ap0, ap1, top + 2, WOOD)
        cv.hline(ap0 - 1, ap0 - 1, top + 1, WOOD_DARK)

    # 体积感：侧边一列暗 + 一列亮；只描到该套服装的下摆，避免盖住裤腿/鞋
    shade_to = {
        "robe": bottom + 4,
        "skirt": bottom + 3,
        "apron": bottom + 3,
        "shorts": bottom - 1,
    }[outfit]
    shade_to = min(shade_to, FOOT_Y - 3)
    if not lay["side"]:
        cv.vline(x1, top, shade_to, dark)
        cv.vline(x0, top, shade_to, light)
    else:
        cv.vline(x0, top, shade_to, dark)
    return x0, x1, top, bottom


def draw_arms(cv, ch, facing, lay, o, x0, x1, top, bottom):
    pal = ch["palette"]
    cloth = pal["cloth"]
    dark = cloth_dark(ch)
    sleeve_end = pal["cloth_alt"] if ch["outfit"] in ("shorts", "apron") else cloth
    hand = pal["skin"]
    swing = float(o.get("arm", 0.0))
    raise_arm = int(o.get("arm_raise", 0) or 0)
    lean = int(o.get("lean", 0))

    if lay["side"]:
        ax = CX + 1 + lean + int(round(swing * 2))
        ay0 = top + 1
        ay1 = bottom - 1 - int(round(abs(swing) * 2))
        cv.rect(ax, ay0, ax + 1, ay1, sleeve_end)
        cv.vline(ax, ay0, ay1, dark)
        cv.rect(ax, ay1 + 1, ax + 1, ay1 + 2, hand)
        if raise_arm:
            cv.rect(ax + 1, ay0 - 2, ax + 2, ay0, sleeve_end)
            cv.rect(ax + 2, ay0 - 3, ax + 3, ay0 - 2, hand)
        return

    arms = [(x0 - 2, x0 - 1), (x1 + 1, x1 + 2)]
    for index, (ax0, ax1) in enumerate(arms):
        lift = int(round(swing * 2)) if index == 1 else int(round(-swing * 2))
        ay0 = top + 1
        ay1 = bottom - 1 + lift
        cv.rect(ax0, ay0, ax1, ay1, sleeve_end)
        cv.vline(ax0 if index == 0 else ax1, ay0, ay1, dark)
        # 手
        cv.rect(ax0, ay1 + 1, ax1, ay1 + 2, hand)
        if index == 1 and raise_arm == 2:
            cv.rect(ax0, ay1 + 3, ax1, ay1 + 4, hand)
        if index == 1 and raise_arm == 3:
            cv.rect(ax0 - 1, ay1 + 3, ax1 + 1, ay1 + 5, hand)


def draw_head(cv, ch, facing, lay, o):
    pal = ch["palette"]
    head_top = lay["head_top"] + int(o.get("bob", 0)) - int(o.get("crouch", 0) or 0)
    head_h = lay["head_h"]
    chin = head_top + head_h - 1
    lean = int(o.get("lean", 0))
    side = lay["side"]
    skin = pal["skin"]
    sdark = skin_shade(pal)
    hair = pal["hair"]
    hair_hi = mix(hair, (255, 255, 255, 255), 0.22)
    hurt = int(o.get("hurt", 0) or 0)

    hx0 = CX - 3 + lean
    hx1 = CX + 3 + lean

    # 头（面向东时把整颗头右移 1px，做出侧脸）
    if side:
        hx0 += 1
        hx1 += 1
    cv.rect(hx0, head_top, hx1, chin, skin)

    # 发型的公共部分：顶发 + 两鬓
    style = ch["hair"]
    top_rows = 3 if style in ("topknot", "band", "dao") else 3
    cv.hline(hx0, hx1, head_top, hair)
    cv.hline(hx0 - (0 if side else 1), hx1 + (0 if side else 1), head_top + 1, hair)
    for y in range(head_top + 2, head_top + 2 + (1 if style != "cap" else 0)):
        cv.hline(hx0 - (0 if side else 1), hx1 + (0 if side else 1), y, hair)
    # 鬓角
    cv.vline(hx0, head_top + 2, chin - 2, hair)
    cv.vline(hx1, head_top + 2, chin - 2, hair)

    if style == "topknot":
        cv.rect(CX - 1 + lean, head_top - 3, CX + 1 + lean, head_top - 1, hair)
        cv.set(CX + lean, head_top - 3, hair_hi)
    elif style == "band":
        cv.hline(hx0, hx1, head_top + 2, pal["trim"])
        cv.rect(CX - 1 + lean, head_top - 2, CX + 1 + lean, head_top - 1, hair)
        # 发带尾
        cv.rect(hx1 - 1, head_top + 3, hx1, head_top + 5, pal["trim"])
    elif style == "bun":
        cv.rect(CX - 3 + lean, head_top - 2, CX - 1 + lean, head_top - 1, hair)
        cv.rect(CX + 1 + lean, head_top - 2, CX + 3 + lean, head_top - 1, hair)
        cv.hline(hx0, hx1, head_top + 1, hair)
        cv.set(CX - 2 + lean, head_top - 2, hair_hi)
        cv.set(CX + 2 + lean, head_top - 2, hair_hi)
    elif style == "ponytail":
        cv.rect(CX - 2 + lean, head_top - 2, CX + 1 + lean, head_top - 1, hair)
        tail_x = hx1 - 1
        cv.rect(tail_x, head_top + 2, tail_x + 1, head_top + 7 - int(round(abs(float(o.get("leg", 0.0))) * 2)), hair)
        cv.set(tail_x + 1, head_top + 3, hair_hi)
    elif style == "kerchief":
        cv.rect(hx0 - 1, head_top - 1, hx1 + 1, head_top + 2, pal["trim"])
        cv.hline(hx0 - 1, hx1 + 1, head_top + 3, shade(pal["trim"], 0.82))
        cv.rect(hx1, head_top + 1, hx1 + 2, head_top + 2, pal["trim"])
    elif style == "cap":
        cv.rect(hx0, head_top - 2, hx1, head_top + 2, shade(hair, 0.9))
        cv.hline(hx0 - 1, hx1 + 1, head_top + 2, INK)
        cv.set(CX + lean, head_top - 2, mix(hair, (255, 255, 255, 255), 0.3))
    elif style == "dao":
        cv.rect(CX - 1 + lean, head_top - 3, CX + 1 + lean, head_top - 1, hair)
        cv.hline(CX - 2 + lean, CX + 2 + lean, head_top - 2, WOOD)  # 木簪
        cv.set(CX + lean, head_top + 2, hair)
        cv.vline(CX - 2 + lean, head_top + 2, head_top + 3, hair)
        cv.vline(CX + 2 + lean, head_top + 2, head_top + 3, hair)

    # 五官：背面无脸
    if facing == "n":
        cv.rect(hx0 + 1, head_top + 2, hx1 - 1, chin - 1, hair)
    else:
        eye = INK
        diag = lay.get("diag", False)
        if side and not diag:
            cv.rect(hx1 - 2, head_top + 3, hx1 - 2, head_top + 3, eye)
            cv.set(hx1 + 1, head_top + 2, hair)  # 额发
            if hurt:
                cv.hline(hx1 - 3, hx1 - 1, head_top + 3, eye)
        elif diag:
            # 斜向：头保持侧身偏移（hx0/hx1 已 +1），五官用正脸，做出 3/4 侧视角
            cv.set(hx1 + 1, head_top + 2, hair)  # 额发
            if hurt:
                cv.hline(hx0 + 1, hx0 + 2, head_top + 3, eye)
                cv.hline(hx1 - 2, hx1 - 1, head_top + 3, eye)
            else:
                cv.set(hx0 + 1, head_top + 3, eye)
                cv.set(hx0 + 2, head_top + 3, eye)
                cv.set(hx1 - 2, head_top + 3, eye)
                cv.set(hx1 - 1, head_top + 3, eye)
            cv.hline(CX - 1 + lean, CX + lean, head_top + 5, sdark)
        else:
            if hurt:
                cv.hline(hx0 + 1, hx0 + 2, head_top + 3, eye)
                cv.hline(hx1 - 2, hx1 - 1, head_top + 3, eye)
            else:
                cv.set(hx0 + 1, head_top + 3, eye)
                cv.set(hx0 + 2, head_top + 3, eye)
                cv.set(hx1 - 2, head_top + 3, eye)
                cv.set(hx1 - 1, head_top + 3, eye)
            cv.hline(CX - 1 + lean, CX + lean, head_top + 5, sdark)

    # 胡须
    beard = ch["beard"]
    if beard != "none" and facing != "n":
        bcol = hair if beard != "stubble" else shade(hair, 0.85)
        if beard == "stubble":
            cv.hline(hx0 + 1, hx1 - 1, chin, mix(bcol, skin, 0.35))
        elif beard == "goatee":
            cv.rect(CX - 1 + lean, chin + 1, CX + 1 + lean, chin + 3, bcol)
            cv.hline(hx0 + 1, hx1 - 1, chin, mix(bcol, skin, 0.3))
        elif beard == "long":
            cv.rect(CX - 1 + lean, chin + 1, CX + 1 + lean, chin + 6, bcol)
            cv.hline(hx0 + 1, hx1 - 1, chin, bcol)
            cv.set(CX + lean, chin + 3, mix(bcol, (255, 255, 255, 255), 0.4))


# ---------------------------------------------------------------------------
# 随身道具（体现身份的配件）
# ---------------------------------------------------------------------------


def draw_accessory(cv, ch, facing, lay, o, back_layer):
    """back_layer=True 画在身体后面（先画），False 画在身体前面（后画）。"""
    kind = ch["accessory"]
    pal = ch["palette"]
    lean = int(o.get("lean", 0))
    bob = int(o.get("bob", 0))
    top = lay["torso_top"] + bob
    bottom = lay["torso_bottom"] + bob
    facing_n = facing == "n"

    if kind == "sword_back":
        if back_layer:
            # 背负木剑：鞘身斜跨背部
            if facing_n:
                return
            cv.rect(CX + 1, top - 2, CX + 2, top + 4, WOOD_DARK)
            cv.set(CX + 2, top - 3, WOOD)
            return
        if facing_n:
            cv.rect(CX - 1, top - 3, CX + 1, top + 6, WOOD_DARK)
            cv.hline(CX - 1, CX + 1, top - 3, WOOD)
            cv.hline(CX - 1, CX + 1, top + 1, WOOD)
    elif kind == "med_box":
        if back_layer:
            return
        bx = CX + lay["half"] - 1 if not lay["side"] else CX + 2
        cv.rect(bx, bottom - 1, bx + 3, bottom + 3, WOOD)
        cv.hline(bx, bx + 3, bottom - 1, WOOD_DARK)
        cv.rect(bx + 1, bottom - 3, bx + 2, bottom - 2, WOOD_DARK)
        if facing_n:
            cv.set(bx + 1, bottom + 1, pal["trim"])
    elif kind == "abacus":
        if back_layer:
            return
        bx = CX - 4 if not lay["side"] else CX - 4
        cv.rect(bx, bottom - 1, bx + 3, bottom + 3, WOOD_DARK)
        cv.hline(bx, bx + 3, bottom, WOOD)
        cv.hline(bx, bx + 3, bottom + 2, WOOD)
        cv.set(bx + 1, bottom + 1, (0xE4, 0xC0, 0x78, 255))
        cv.set(bx + 2, bottom + 1, (0xE4, 0xC0, 0x78, 255))
    elif kind == "tea_basket":
        if facing_n:
            if back_layer:
                return
            cv.rect(CX - 3, top - 1, CX + 2, top + 7, WOOD)
            cv.hline(CX - 3, CX + 2, top + 1, WOOD_DARK)
            cv.hline(CX - 3, CX + 2, top + 4, WOOD_DARK)
            cv.rect(CX - 2, top - 3, CX + 1, top - 2, shade(pal["cloth_alt"], 0.8))
        else:
            if back_layer:
                cv.rect(CX + 2, top, CX + 4, top + 7, WOOD)
                cv.hline(CX + 2, CX + 4, top + 2, WOOD_DARK)
                cv.hline(CX + 2, CX + 4, top + 5, WOOD_DARK)
    elif kind == "hammer":
        if back_layer:
            return
        bx = CX - 5 if not lay["side"] else CX - 4
        cv.vline(bx + 1, bottom - 2, bottom + 4, WOOD)
        cv.rect(bx, bottom + 3, bx + 3, bottom + 4, IRON_DARK)
    elif kind == "saber":
        if back_layer:
            return
        bx = CX - 5 if not lay["side"] else CX - 4
        cv.rect(bx, bottom - 2, bx + 1, bottom + 4, IRON)
        cv.hline(bx, bx + 1, bottom + 4, IRON_DARK)
        cv.set(bx, bottom - 3, WOOD_DARK)
    elif kind == "sword_hip":
        if back_layer:
            return
        bx = CX + lay["half"] if not lay["side"] else CX + 2
        cv.rect(bx, bottom - 1, bx + 1, bottom + 5, IRON)
        cv.set(bx, bottom - 2, (0xE4, 0xC0, 0x78, 255))
    elif kind == "whisk":
        if back_layer:
            return
        bx = CX + lay["half"] + 1 if not lay["side"] else CX + 3
        cv.vline(bx, bottom - 2, bottom + 2, WOOD)
        cv.rect(bx - 1, bottom - 6, bx + 1, bottom - 3, (0xF4, 0xF4, 0xF8, 255))
        cv.set(bx, bottom - 7, (0xF4, 0xF4, 0xF8, 255))


# ---------------------------------------------------------------------------
# 劳作/战斗道具（仅主角动作帧使用）
# ---------------------------------------------------------------------------


def draw_tool(cv, ch, facing, lay, o):
    kind = (ch.get("tool") or {}).get(o.get("tool_action", ""), None)
    if not kind:
        return
    stage = int(o.get("tool_stage", 0))
    bob = int(o.get("bob", 0))
    top = lay["torso_top"] + bob
    bottom = lay["torso_bottom"] + bob
    side = lay["side"]
    flip = -1 if facing == "w" else 1

    if kind == "hoe":
        if stage <= 0:  # 举起
            hx = CX + flip * (lay["half"] + 3)
            hy = bottom - 1
            cv.vline(hx, top - 6, hy, WOOD)
            cv.rect(hx - 1, top - 8, hx + 1, top - 6, IRON)
        elif stage == 1:  # 挥下
            hx = CX + flip * (lay["half"] + 2)
            hy = bottom + 1
            cv.vline(hx, top - 3, hy, WOOD)
            cv.rect(hx - 1, top - 5, hx + 1, top - 3, IRON)
        else:  # 触地
            hx = CX + flip * (lay["half"] + 4)
            cv.vline(hx, bottom - 2, bottom + 3, WOOD)
            cv.rect(hx - 2, bottom + 2, hx + 2, bottom + 3, IRON)
            cv.hline(hx - 2, hx + 2, bottom + 4, (0x6A, 0x4C, 0x30, 255))
    elif kind == "pot":
        hx = CX + flip * (lay["half"] + 2)
        if stage <= 0:
            cv.rect(hx - 1, bottom - 3, hx + 2, bottom + 1, WOOD)
            cv.hline(hx - 1, hx + 2, bottom - 4, WOOD_DARK)
        elif stage == 1:
            cv.rect(hx - 1, bottom - 2, hx + 2, bottom + 1, WOOD)
            cv.rect(hx + flip * 2, bottom - 1, hx + flip * 3, bottom - 1, WOOD_DARK)
            cv.set(hx + flip * 3, bottom + 1, WATER)
        else:
            cv.rect(hx - 1, bottom - 2, hx + 2, bottom + 1, WOOD)
            cv.rect(hx + flip * 2, bottom, hx + flip * 4, bottom, WATER)
            cv.set(hx + flip * 3, bottom + 2, WATER)
            cv.set(hx + flip * 4, bottom + 3, WATER)
            cv.set(hx + flip * 2, bottom + 3, WATER)
    elif kind == "blade":
        if stage <= 0:  # 蓄力
            hx = CX + flip * (lay["half"] + 2)
            cv.vline(hx, top - 8, bottom - 2, IRON)
            cv.hline(hx - 1, hx + 1, bottom - 1, WOOD_DARK)
            cv.set(hx, top - 9, IRON)
        elif stage == 1:  # 挥击 + 刀光
            hx = CX + flip * (lay["half"] + 3)
            cv.vline(hx, top - 2, bottom + 3, IRON)
            for i in range(4):
                cv.set(hx + flip * (1 + i), top - 1 + i, HILIGHT)
                cv.set(hx + flip * (1 + i), top - 2 + i, HILIGHT)
        else:  # 收势
            hx = CX + flip * (lay["half"] + 2)
            cv.vline(hx, bottom - 4, bottom + 3, IRON)


# ---------------------------------------------------------------------------
# 倒地（faint 末帧）
# ---------------------------------------------------------------------------


def draw_fallen(cv, ch, facing):
    pal = ch["palette"]
    hair = pal["hair"]
    cloth = pal["cloth"]
    dark = cloth_dark(ch)
    # 腿
    cv.rect(6, 25, 12, 28, pal["cloth_alt"])
    cv.rect(4, 26, 6, 28, pal["shoe"])
    # 躯干
    cv.rect(12, 23, 22, 28, cloth)
    cv.vline(12, 23, 28, dark)
    cv.hline(12, 22, 23, cloth_light(ch))
    # 头
    cv.rect(21, 21, 27, 27, pal["skin"])
    cv.rect(20, 20, 27, 22, hair)
    cv.vline(20, 23, 26, hair)
    cv.hline(22, 23, 24, INK)  # 闭眼


# ---------------------------------------------------------------------------
# 姿态 → 帧
# ---------------------------------------------------------------------------

POSE_IDLE = [dict(bob=0), dict(bob=0), dict(bob=1), dict(bob=0)]
POSE_IDLE_SHORT = [dict(bob=0), dict(bob=1)]
POSE_WALK6 = [
    dict(leg=1.0, arm=-1.0, bob=0),
    dict(leg=0.5, arm=-0.5, bob=1),
    dict(leg=0.0, arm=0.0, bob=0),
    dict(leg=-1.0, arm=1.0, bob=0),
    dict(leg=-0.5, arm=0.5, bob=1),
    dict(leg=0.0, arm=0.0, bob=0),
]
POSE_WALK4 = [POSE_WALK6[0], POSE_WALK6[2], POSE_WALK6[3], POSE_WALK6[5]]
POSE_TALK = [
    dict(arm_raise=1, bob=0),
    dict(arm_raise=1, bob=1),
]
POSE_HOE = [
    dict(tool_action="hoe", tool_stage=0, arm_raise=2, bob=0, lean=-1 if False else 0),
    dict(tool_action="hoe", tool_stage=1, arm_raise=1, bob=1),
    dict(tool_action="hoe", tool_stage=1, arm_raise=1, bob=1),
    dict(tool_action="hoe", tool_stage=2, arm_raise=0, bob=0),
]
POSE_WATER = [
    dict(tool_action="water", tool_stage=0, arm_raise=1, bob=0),
    dict(tool_action="water", tool_stage=1, arm_raise=1, bob=1),
    dict(tool_action="water", tool_stage=2, arm_raise=0, bob=1),
    dict(tool_action="water", tool_stage=0, arm_raise=0, bob=0),
]
POSE_ATTACK = [
    dict(tool_action="attack", tool_stage=0, arm_raise=2, bob=0),
    dict(tool_action="attack", tool_stage=1, arm_raise=0, bob=0, lean=1),
    dict(tool_action="attack", tool_stage=2, arm_raise=0, bob=0, lean=1),
    dict(tool_action="attack", tool_stage=0, arm_raise=0, bob=0),
]
POSE_HURT = [
    dict(hurt=1, bob=0, arm=-1.0),
    dict(hurt=0, bob=0, arm=0.0),
]
POSE_FAINT = [
    dict(crouch=1, bob=0),
    dict(crouch=2, bob=0),
    dict(crouch=3, bob=0),
    dict(lying=True),
]

POSES = {
    "idle": {"player": POSE_IDLE, "npc": POSE_IDLE_SHORT},
    "walk": {"player": POSE_WALK6, "npc": POSE_WALK4},
    "talk": {"player": POSE_TALK, "npc": POSE_TALK},
    "hoe": {"player": POSE_HOE, "npc": POSE_HOE},
    "water": {"player": POSE_WATER, "npc": POSE_WATER},
    "attack": {"player": POSE_ATTACK, "npc": POSE_ATTACK},
    "hurt": {"player": POSE_HURT, "npc": POSE_HURT},
    "faint": {"player": POSE_FAINT, "npc": POSE_FAINT},
}


def render_frame(ch, facing, pose):
    """渲染单帧：阴影 → 后景配件 → 身体 → 轮廓 → 前景配件/道具。"""
    final = Canvas()
    draw_shadow(final, fallen=bool(pose.get("lying")))
    if pose.get("lying"):
        body = Canvas()
        draw_fallen(body, ch, facing)
        body.outline(INK)
        final.blit(body)
        return final

    lay = layout(ch, facing)
    body = Canvas()
    draw_accessory(body, ch, facing, lay, pose, back_layer=True)
    draw_legs(body, ch, facing, lay, pose)
    x0, x1, top, bottom = draw_torso(body, ch, facing, lay, pose)
    draw_arms(body, ch, facing, lay, pose, x0, x1, top, bottom)
    draw_head(body, ch, facing, lay, pose)
    draw_accessory(body, ch, facing, lay, pose, back_layer=False)
    draw_tool(body, ch, facing, lay, pose)
    body.outline(INK)
    final.blit(body)
    return final


def clip_frames(action, role):
    """该角色该动作的帧数（主角/NPC 帧数不同，见 POSES）。"""
    return len(POSES[action][role])


def render_action(ch, action, facing):
    poses = POSES[action][ch["role"]]
    # 负 x 朝向（w/sw/nw）以对应正 x 朝向为基准，渲染后整体水平镜像
    base = MIRROR_DIR.get(facing, facing)
    frames = []
    for pose in poses:
        frame = render_frame(ch, base, pose)
        if base != facing:
            frame = frame.mirrored()
        frames.append(frame)
    return frames


# ---------------------------------------------------------------------------
# 文本贴图（极简 3×5 点阵，仅用于预览图标签）
# ---------------------------------------------------------------------------

FONT = {
    "A": ["010", "101", "111", "101", "101"],
    "B": ["110", "101", "110", "101", "110"],
    "C": ["011", "100", "100", "100", "011"],
    "D": ["110", "101", "101", "101", "110"],
    "E": ["111", "100", "110", "100", "111"],
    "F": ["111", "100", "110", "100", "100"],
    "G": ["011", "100", "101", "101", "011"],
    "H": ["101", "101", "111", "101", "101"],
    "I": ["111", "010", "010", "010", "111"],
    "J": ["001", "001", "001", "101", "010"],
    "K": ["101", "101", "110", "101", "101"],
    "L": ["100", "100", "100", "100", "111"],
    "M": ["101", "111", "111", "101", "101"],
    "N": ["101", "111", "111", "111", "101"],
    "O": ["010", "101", "101", "101", "010"],
    "P": ["110", "101", "110", "100", "100"],
    "Q": ["010", "101", "101", "111", "011"],
    "R": ["110", "101", "110", "101", "101"],
    "S": ["011", "100", "010", "001", "110"],
    "T": ["111", "010", "010", "010", "010"],
    "U": ["101", "101", "101", "101", "011"],
    "V": ["101", "101", "101", "101", "010"],
    "W": ["101", "101", "111", "111", "101"],
    "X": ["101", "101", "010", "101", "101"],
    "Y": ["101", "101", "010", "010", "010"],
    "Z": ["111", "001", "010", "100", "111"],
    "0": ["111", "101", "101", "101", "111"],
    "1": ["010", "110", "010", "010", "111"],
    "2": ["111", "001", "111", "100", "111"],
    "3": ["111", "001", "111", "001", "111"],
    "4": ["101", "101", "111", "001", "001"],
    "5": ["111", "100", "111", "001", "111"],
    "6": ["111", "100", "111", "101", "111"],
    "7": ["111", "001", "001", "001", "001"],
    "8": ["111", "101", "111", "101", "111"],
    "9": ["111", "101", "111", "001", "111"],
    "_": ["000", "000", "000", "000", "111"],
    "-": ["000", "000", "111", "000", "000"],
    ".": ["000", "000", "000", "000", "010"],
    " ": ["000", "000", "000", "000", "000"],
}


def text_width(text, scale=1):
    return max(0, len(text) * (3 * scale + scale) - scale)


def draw_text(cv, x, y, text, color=(0xE4, 0xC0, 0x78, 255), scale=1):
    cursor = x
    for char in text.upper():
        glyph = FONT.get(char, FONT[" "])
        for gy, row in enumerate(glyph):
            for gx, bit in enumerate(row):
                if bit == "1":
                    for sy in range(scale):
                        for sx in range(scale):
                            cv.set(cursor + gx * scale + sx, y + gy * scale + sy, color)
        cursor += 3 * scale + scale
    return cursor


# ---------------------------------------------------------------------------
# 资源写出
# ---------------------------------------------------------------------------


def action_list(ch):
    return [name for name, spec in ACTIONS.items() if spec[ch["role"]]]


def write_png(path, canvas):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(canvas.to_png())


def build_character(ch, out_root, preview_scale=2, quiet=False):
    char_dir = os.path.join(out_root, ch["id"])
    os.makedirs(char_dir, exist_ok=True)
    clips = []
    for action in action_list(ch):
        spec = ACTIONS[action]
        count = clip_frames(action, ch["role"])
        for facing in DIRECTIONS:
            frames = render_action(ch, action, facing)
            assert len(frames) == count, (action, facing, len(frames))
            # 导出 = 绘制帧 × ART_SCALE（最近邻放大），条带帧尺寸 FRAME_PX×FRAME_PX
            strip = Canvas(FRAME_PX * len(frames), FRAME_PX)
            for index, frame in enumerate(frames):
                scaled = frame.scaled(ART_SCALE)
                for y in range(FRAME_PX):
                    for x in range(FRAME_PX):
                        strip.set(index * FRAME_PX + x, y, scaled.get(x, y))
            name = f"{action}_{facing}"
            rel = f"assets/art/characters/{ch['id']}/{name}.png"
            write_png(os.path.join(out_root, ch["id"], f"{name}.png"), strip)
            clips.append(
                {
                    "name": name,
                    "sheet": rel,
                    "cols": len(frames),
                    "fps": spec["fps"],
                    "looped": spec["looped"],
                    "action": action,
                    "dir": facing,
                }
            )
            # 每个朝向的第一帧空闲姿态另存单帧静态图（没有 Animation 组件时的兜底）
            if action == "idle":
                write_png(
                    os.path.join(out_root, ch["id"], f"static_{facing}.png"),
                    frames[0].scaled(ART_SCALE),
                )
    write_png(os.path.join(out_root, ch["id"], "preview.png"), build_character_preview(ch, preview_scale))
    if not quiet:
        print(f"  {ch['id']}: {len(clips)} clips / {sum(c['cols'] for c in clips)} frames")
    return clips


def build_character_preview(ch, scale=2):
    rows = []
    for action in action_list(ch):
        for facing in DIRECTIONS:
            rows.append((f"{action}_{facing}", render_action(ch, action, facing)))
    tile = FRAME * scale
    label_w = text_width("attack_s", 1) + 8
    body_w = max(len(frames) for _, frames in rows) * (tile + 1)
    width = label_w + body_w + 6
    row_h = tile + 3
    header = 20
    height = header + len(rows) * row_h + 6
    cv = Canvas(width, height)
    cv.rect(0, 0, width - 1, height - 1, (0x14, 0x17, 0x1A, 255))
    draw_text(cv, 4, 4, ch["id"], (0xE4, 0xC0, 0x78, 255), 2)
    draw_text(cv, 4 + text_width(ch["id"], 2) + 6, 6,
              f"FRAME {FRAME_PX} X{scale} {len(rows)} ROWS", (0x8A, 0x8F, 0x98, 255), 1)
    for index, (label, frames) in enumerate(rows):
        y = header + index * row_h
        draw_text(cv, 4, y + tile // 2 - 2, label, (0xA9, 0xC2, 0x8F, 255), 1)
        for col, frame in enumerate(frames):
            x = label_w + col * (tile + 1)
            draw_cell(cv, x, y, tile)
            paste_frame(cv, x, y, frame, scale)
    return cv


def draw_cell(cv, x, y, tile):
    """预览格子：深色底 + 1px 边框，保证相邻帧可分辨。"""
    cv.rect(x - 1, y - 1, x + tile, y + tile, (0x26, 0x2C, 0x33, 255))
    border = (0x3D, 0x44, 0x4C, 255)
    cv.hline(x - 1, x + tile, y - 1, border)
    cv.hline(x - 1, x + tile, y + tile, border)
    cv.vline(x - 1, y - 1, y + tile, border)
    cv.vline(x + tile, y - 1, y + tile, border)


def paste_frame(cv, x, y, frame, scale):
    for py in range(FRAME):
        for px in range(FRAME):
            color = frame.get(px, py)
            if color[3] == 0:
                continue
            for sy in range(scale):
                for sx in range(scale):
                    cv.set(x + px * scale + sx, y + py * scale + sy, color)


def build_overview(chars, out_root, scale=2):
    """全角色总览：每人 2 行（idle_s / walk_s），放大 scale 倍便于验收。"""
    tile = FRAME * scale
    rows = []
    for ch in chars:
        rows.append((ch["id"] + "  IDLE_S", ch, "idle_s"))
        rows.append((ch["id"] + "  WALK_S", ch, "walk_s"))
    label_w = max(text_width(label, 1) for label, _, _ in rows) + 8
    body_w = max(ACTIONS["walk"]["frames"], ACTIONS["idle"]["frames"], 6) * (tile + 1)
    width = label_w + body_w + 8
    row_h = tile + 3
    height = 22 + len(rows) * row_h + 8
    cv = Canvas(width, height)
    cv.rect(0, 0, width - 1, height - 1, (0x14, 0x17, 0x1A, 255))
    draw_text(cv, 4, 5, f"JIANGHU YOUTIAN / CHARACTER OVERVIEW / FRAME {FRAME_PX} X{scale}",
              (0xE4, 0xC0, 0x78, 255), 1)
    for index, (label, ch, clip) in enumerate(rows):
        y = 22 + index * row_h
        draw_text(cv, 4, y + tile // 2 - 2, label, (0xA9, 0xC2, 0x8F, 255), 1)
        action, facing = clip.split("_")
        for col, frame in enumerate(render_action(ch, action, facing)):
            x = label_w + col * (tile + 1)
            draw_cell(cv, x, y, tile)
            paste_frame(cv, x, y, frame, scale)
    return cv


def ascii_frames(ch, action, facing):
    """把帧按字符打印到终端：像素画调参/验收用（每像素一字符，按色板分配符号）。"""
    frames = render_action(ch, action, facing)
    symbols = "#*+xoO0@%$&aeiou"
    legend = {}
    for frame in frames:
        for y in range(FRAME):
            for x in range(FRAME):
                color = frame.get(x, y)
                if color[3] == 0:
                    continue
                key = color[:3]
                if key not in legend:
                    legend[key] = symbols[len(legend) % len(symbols)]
    print(f"# {ch['id']} {action}_{facing}  frames={len(frames)}")
    for index, frame in enumerate(frames):
        print(f"--- frame {index} ---")
        for y in range(FRAME):
            row = "".join(
                legend[frame.get(x, y)[:3]] if frame.get(x, y)[3] > 0 else "." for x in range(FRAME)
            )
            print(row)
    print("legend: " + "  ".join(f"{sym}=#{r:02X}{g:02X}{b:02X}" for (r, g, b), sym in legend.items()))


def animation_component(clips, default="idle_s"):
    parts = [f"default={default}"]
    for clip in clips:
        parts.append(
            f"clip:{clip['name']}:{clip['sheet']}:{clip['cols']}:{_fps(clip['fps'])}:{str(clip['looped']).lower()}"
        )
    return ";".join(parts)


def _fps(value):
    return str(int(value)) if float(value).is_integer() else f"{value:g}"


def static_sprite(ch, facing="s"):
    return (
        f"texture=assets/art/characters/{ch['id']}/static_{facing}.png"
        f";size={FRAME_PX}x{FRAME_PX};tint=1.000,1.000,1.000,1.000;sorting_layer=actor"
    )


def emit_manifest(chars, clips_by_char, out_root):
    payload = {
        "version": 1,
        "generated_by": "tools/gen_character_sprites.py",
        "design_doc": "docs/36-江湖有田-角色序列帧设计.md",
        "frame_px": FRAME_PX,
        "directions": {key: DIR_TITLE[key] for key in DIRECTIONS},
        "engine": {
            "animation_component": "default=<clip>;clip:<name>:<sheet>:<cols>:<fps>:<looped>",
            "sheet_layout": "1xN horizontal strip; uv u=frame/cols..(frame+1)/cols, v=0..1",
            "note": "NPC 由 entity.spawn 运行时创建，当前 Lua 无挂 Animation 组件的 API，只能用 static_<dir>.png",
        },
        "characters": [],
    }
    for ch in chars:
        clips = clips_by_char[ch["id"]]
        payload["characters"].append(
            {
                "id": ch["id"],
                "display": ch["display"],
                "role": ch["role"],
                "note": ch["note"],
                "sprite_size": f"{FRAME_PX}x{FRAME_PX}",
                "sorting_layer": "actor",
                "static_sprite": static_sprite(ch, "s"),
                "animation": animation_component(clips),
                "preview": f"assets/art/characters/{ch['id']}/preview.png",
                "palette": {key: "#%02X%02X%02X" % value[:3] for key, value in ch["palette"].items()},
                "clips": clips,
            }
        )
    path = os.path.join(out_root, "manifest.json")
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(payload, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    return path


LUA_HEADER = '''-- data/characters.lua：《江湖有田》角色形象与序列帧动画注册表。
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
-- 主角在场景里是静态实体（manor.scene / village.scene 的 JianghuPlayer），
-- 可以直接在 .scene 里声明 Animation 组件，用 M.player_animation() 取串。

local M = {}

M.version = 1
M.frame_px = {FRAME_PX}

-- 每个角色：id → 显示名、形象要点、单帧 sprite 串、动画组件串、格子大小。
M.characters = {
'''

LUA_FOOTER = '''}

-- 朝向：s 南(面向镜头) / n 北(背对) / e 东(朝右) / w 西(朝左)
--       se 东南 / sw 西南 / ne 东北 / nw 西北（斜向帧：侧身布局 + 正脸头）
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

-- NPC 当前可用的单帧 sprite 串（八向：s/n/e/w/se/sw/ne/nw）。
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
    for _, facing in ipairs({ "s", "n", "e", "w", "se", "sw", "ne", "nw" }) do
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
'''


def lua_quote(value):
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def emit_lua(chars, clips_by_char, path):
    lines = [LUA_HEADER.replace("{FRAME_PX}", str(FRAME_PX))]
    for ch in chars:
        clips = clips_by_char[ch["id"]]
        lines.append(f"  {ch['id']} = {{")
        lines.append(f"    display = {lua_quote(ch['display'])},")
        lines.append(f"    role = {lua_quote(ch['role'])},")
        lines.append(f"    note = {lua_quote(ch['note'])},")
        lines.append("    static_sprite = {")
        for facing in DIRECTIONS:
            lines.append(
                f"      {facing} = {lua_quote(static_sprite(ch, facing))},"
            )
        lines.append("    },")
        lines.append(f"    animation = {lua_quote(animation_component(clips))},")
        lines.append("    clips = {")
        for clip in clips:
            lines.append(
                "      { name = %s, sheet = %s, cols = %d, fps = %s, looped = %s },"
                % (
                    lua_quote(clip["name"]),
                    lua_quote(clip["sheet"]),
                    clip["cols"],
                    _fps(clip["fps"]),
                    str(clip["looped"]).lower(),
                )
            )
        lines.append("    },")
        lines.append("  },")
    lines.append(LUA_FOOTER)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write("\n".join(lines))


# ---------------------------------------------------------------------------
# 校验
# ---------------------------------------------------------------------------


def strip_lua_strings(source):
    """去掉 Lua 字符串字面量与注释，便于做括号配平检查。"""
    out = []
    index = 0
    length = len(source)
    while index < length:
        char = source[index]
        if char == "-" and source.startswith("--", index):
            end = source.find("\n", index)
            index = length if end < 0 else end
            continue
        if char == '"' or char == "'":
            quote = char
            index += 1
            while index < length:
                if source[index] == "\\":
                    index += 2
                    continue
                if source[index] == quote:
                    index += 1
                    break
                index += 1
            out.append('""')
            continue
        out.append(char)
        index += 1
    return "".join(out)


def lint_lua(path, manifest):
    """校验生成的 data/characters.lua：括号配平 + 每条 sheet 路径真实存在。"""
    issues = []
    if not os.path.exists(path):
        return ["lua 缺失: " + path]
    with open(path, encoding="utf-8") as handle:
        source = handle.read()
    stripped = strip_lua_strings(source)
    for open_char, close_char in (("{", "}"), ("(", ")"), ("[", "]")):
        if stripped.count(open_char) != stripped.count(close_char):
            issues.append(
                f"{path}: {open_char}{close_char} 不配平 "
                f"({stripped.count(open_char)} vs {stripped.count(close_char)})"
            )
    if not source.rstrip().endswith("return M"):
        issues.append(f"{path}: 结尾缺少 return M")
    if "function M.validate_all()" not in source:
        issues.append(f"{path}: 缺少 M.validate_all（main.lua 加载期校验入口）")
    for entry in manifest["characters"]:
        for clip in entry["clips"]:
            if f'"{clip["sheet"]}"' not in source:
                issues.append(f"{path}: 缺少 clip sheet {clip['sheet']}")
        for facing in DIRECTIONS:
            if f"static_{facing}.png" not in source:
                issues.append(f"{path}: 缺少 static_{facing} 引用")
    return issues


def check(out_root, chars, lua_path=None):
    issues = []
    manifest_path = os.path.join(out_root, "manifest.json")
    if not os.path.exists(manifest_path):
        return ["manifest.json 缺失，请先运行生成器"]
    with open(manifest_path, encoding="utf-8") as handle:
        manifest = json.load(handle)
    for entry in manifest["characters"]:
        for clip in entry["clips"]:
            path = clip["sheet"].replace("/", os.sep)
            if not os.path.exists(path):
                issues.append("missing sheet: " + clip["sheet"])
                continue
            width, height, depth, color_type = png_size(path)
            if height != FRAME_PX:
                issues.append(f"{clip['sheet']}: height {height} != {FRAME_PX}")
            if width != FRAME_PX * clip["cols"]:
                issues.append(
                    f"{clip['sheet']}: width {width} != {FRAME_PX * clip['cols']} (cols={clip['cols']})"
                )
            if (depth, color_type) != (8, 6):
                issues.append(f"{clip['sheet']}: expected RGBA8, got depth={depth} color_type={color_type}")
        for facing in DIRECTIONS:
            path = os.path.join(out_root, entry["id"], f"static_{facing}.png")
            if not os.path.exists(path):
                issues.append(f"missing static frame: {entry['id']}/static_{facing}.png")
                continue
            width, height, _, _ = png_size(path)
            if (width, height) != (FRAME_PX, FRAME_PX):
                issues.append(f"{entry['id']}/static_{facing}.png: {width}x{height} != {FRAME_PX}x{FRAME_PX}")
        if not os.path.exists(os.path.join(out_root, entry["id"], "preview.png")):
            issues.append(f"missing preview: {entry['id']}/preview.png")
    if lua_path:
        issues.extend(lint_lua(lua_path, manifest))
    return issues


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》角色序列帧生成器")
    parser.add_argument("--out", default=os.path.join("assets", "art", "characters"))
    parser.add_argument("--data", default=os.path.join("scripts", "data", "characters.lua"))
    parser.add_argument("--only", nargs="*", help="只生成指定角色 id")
    parser.add_argument("--preview-scale", type=int, default=2, help="预览图放大倍数（默认 2）")
    parser.add_argument("--check", action="store_true", help="只校验已生成资源")
    parser.add_argument("--ascii", nargs=3, metavar=("CHAR", "ACTION", "DIR"),
                        help="把指定角色的某动作某朝向打印成字符画（调参用）")
    args = parser.parse_args()

    if args.ascii:
        char_id, action, facing = args.ascii
        ascii_frames(by_id(char_id), action, facing)
        return

    chars = CHARACTERS
    if args.only:
        chars = [by_id(char_id) for char_id in args.only]

    if args.check:
        issues = check(args.out, chars, args.data)
        if issues:
            for issue in issues:
                print("[FAIL]", issue)
            raise SystemExit(1)
        print("[OK] 资源校验通过：", args.out)
        return

    clips_by_char = {}
    print(f"生成角色序列帧 → {args.out}")
    for ch in chars:
        clips_by_char[ch["id"]] = build_character(ch, args.out, args.preview_scale)
    all_chars = CHARACTERS if args.only else chars
    for ch in all_chars:
        if ch["id"] not in clips_by_char:
            clips_by_char[ch["id"]] = []
            for action in action_list(ch):
                spec = ACTIONS[action]
                for facing in DIRECTIONS:
                    clips_by_char[ch["id"]].append(
                        {
                            "name": f"{action}_{facing}",
                            "sheet": f"assets/art/characters/{ch['id']}/{action}_{facing}.png",
                            "cols": clip_frames(action, ch["role"]),
                            "fps": spec["fps"],
                            "looped": spec["looped"],
                            "action": action,
                            "dir": facing,
                        }
                    )
    overview = build_overview(all_chars, args.out, args.preview_scale)
    write_png(os.path.join(args.out, "_overview.png"), overview)
    print("  _overview.png")
    manifest = emit_manifest(all_chars, clips_by_char, args.out)
    print("  manifest.json:", manifest)
    emit_lua(all_chars, clips_by_char, args.data)
    print("  lua:", args.data)
    issues = check(args.out, all_chars, args.data)
    if issues:
        for issue in issues:
            print("[FAIL]", issue)
        raise SystemExit(1)
    total = sum(len(clips) for clips in clips_by_char.values())
    frames = sum(c["cols"] for clips in clips_by_char.values() for c in clips)
    print(f"完成：{len(all_chars)} 个角色 / {total} 条条带 / {frames} 帧")


if __name__ == "__main__":
    main()
