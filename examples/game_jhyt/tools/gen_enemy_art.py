# -*- coding: utf-8 -*-
"""《江湖有田》敌人序列帧生成（docs/31 §3.2：敌人待机/移动/攻击/受击/死亡）。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_enemy_art.py            # 生成全部敌人 + manifest + 预览
    python tools/gen_enemy_art.py --check    # 只校验尺寸/列数/存在性

产出（目录 `assets/art/enemies/`）：

    <id>/<action>_<dir>.png    1×N 横向条带（引擎 Animation clip 采样约定）
    <id>/static_<dir>.png      单帧兜底（引擎不给 entity.spawn 的实体挂 Animation，见下）
    <id>/preview.png           该敌人全部条带 2× 预览
    manifest.json              机器可读清单（含 Animation 组件串）
    _preview.png               三个敌人 idle_s/walk_s 总览

三种骨架：

  1. `bandit_grunt` 山贼喽啰 —— **直接复用主角的人形骨架**
     （`gen_character_sprites.render_frame` + 同一套 POSE 表），只换色板/发型/配件/武器；
     所以敌人的动作帧数、帧尺寸、朝向与主角完全一致，战斗接动画不需要第二套约定。
  2. `gray_wolf` 灰狼 —— 四足骨架（本文件实现）：身体 / 头 / 四肢 / 尾，4 方向 + 行走循环。
  3. `training_dummy` 训练木人 —— 静态标靶（speed=0，只需要待机与受击）。

引擎事实（为什么必须出 static_<dir>.png）：
  NPC/敌人由 `entity.spawn` 在运行期创建，Lua 侧**没有挂 Animation 组件的 API**
  （`animation.set` 要求组件已存在），所以现在只能用单帧；条带已经按 Animation 约定出好，
  等引擎补 `animation.attach(id, str)`（docs/36 §8.3）就能直接播。

依赖：Python 3 标准库 + pixel_kit.py + gen_character_sprites.py（人形骨架）。
"""

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, draw_text, rgb, shade  # noqa: E402
from gen_manor_art import fill_ellipse  # noqa: E402  椭圆填充
from gen_character_sprites import (  # noqa: E402  复用人形骨架与姿态表
    ART_SCALE,
    FRAME,
    FRAME_PX,
    INK,
    MIRROR_DIR,
    POSE_ATTACK,
    POSE_FAINT,
    POSE_HURT,
    POSE_IDLE,
    POSE_WALK6,
    render_frame,
    stamp_shadow,
)

OUT_DIR = os.path.join("assets", "art", "enemies")
DIRECTIONS = ("s", "n", "e", "w", "se", "sw", "ne", "nw")

# 灰狼色板
FUR = rgb("#8A8F98")
FUR_DARK = rgb("#5E6470")
FUR_LIGHT = rgb("#B9BDC4")
BELLY = rgb("#D8CDB2")
MUZZLE = rgb("#E8E4DC")
EYE = rgb("#C3452F")

# 训练木人色板
POST = rgb("#8A6A46")
POST_DARK = rgb("#4A3628")
STRAW = rgb("#C9A05A")
STRAW_DARK = rgb("#A8813F")
ROPE = rgb("#6B4F3A")
TARGET = rgb("#C3452F")


# ---------------------------------------------------------------------------
# 1) 山贼喽啰：复用主角人形骨架
# ---------------------------------------------------------------------------

BANDIT = {
    "id": "bandit_grunt",
    "display": "山贼喽啰",
    "role": "enemy",
    "frame_px": FRAME,
    "sprite_size": f"{FRAME_PX}x{FRAME_PX}",
    "kind": "humanoid",
    "note": "短打 + 朱砂头巾 + 单刀；与主角同骨架，只换色板/发型/配件",
    "build": {"head_top": 5, "head_h": 7, "head_w": 7, "half_w": 3, "torso_bottom": 22},
    "outfit": "shorts",
    "hair": "kerchief",
    "beard": "stubble",
    "accessory": "saber",
    "tool": {"attack": "blade"},
    "palette": {
        "skin": rgb("#D9A87C"),
        "hair": rgb("#241F1C"),
        "cloth": rgb("#4A4038"),        # 脏褐短打
        "cloth_alt": rgb("#6B4F3A"),    # 皮护腕/裤
        "trim": rgb("#C3452F"),         # 朱砂头巾
        "belt": rgb("#2B2B33"),
        "shoe": rgb("#31383F"),
    },
}

def draw_scorpion(facing, pose):
    """32×32 毒蝎：小身体 + 双螯 + 三对足 + 分节上翘毒尾。

    第一版身体椭圆画得太大（rx=7），加上双螯变成一个扁褐团；现在收窄身体、
    把尾巴画成"逐节偏移"的曲线、螯单独成钳，轮廓才像蝎子。
    """
    shell = rgb("#6B4A3A")
    shell_dark = rgb("#4A3226")
    shell_light = rgb("#8A6248")
    sting = rgb("#C9A05A")
    cv = Canvas(FRAME, FRAME)
    bob = int(pose.get("bob", 0))
    leg = float(pose.get("leg", 0.0))
    lunge = int(pose.get("lunge", 0))
    hurt = int(pose.get("hurt", 0))
    y0 = 19 + bob

    cv.hline(8, 23, 30, (0x1F, 0x23, 0x28, 70))

    # 三对足（细，交替摆动）
    for index, base_x in enumerate((10, 15, 20)):
        for side in (-1, 1):
            offset = int(round(leg * 1.5)) if (index + (side > 0)) % 2 == 0 else -int(round(leg * 1.5))
            lift = 1 if abs(leg) > 0.4 and (index % 2 == 0) else 0
            x = base_x + offset
            cv.rect(x, y0 + 2, x + 1, 27 - lift, shell_dark)
            cv.set(x + side, 27 - lift, shell_dark)

    # 尾：从背部向上逐节偏移（蝎子的标志）
    tail_x = 16
    for step in range(6):
        tx = tail_x + (-1 if step % 2 else 0) + (1 if (facing == "e" and step > 2) else 0)
        cv.rect(tx, y0 - 2 - step * 2, tx + 1, y0 - 1 - step * 2, shell_dark)
        cv.set(tx, y0 - 3 - step * 2, shell)
    tip_y = y0 - 14
    cv.set(tail_x, tip_y, sting)
    cv.set(tail_x, tip_y - 1, sting)
    if lunge:  # 攻击时尾巴前压
        cv.set(tail_x + 1, tip_y, sting)
        cv.set(tail_x + 2, tip_y + 1, shell_light)

    if facing == "e":
        fill_ellipse(cv, 15, y0, 6, 3.5, shell)
        cv.hline(10, 20, y0 - 3, shell_light)
        fill_ellipse(cv, 22 + lunge, y0, 3, 3, shell_light)
        for dy in (-2, 2):  # 双螯
            cv.rect(24 + lunge, y0 + dy - 1, 27 + lunge, y0 + dy + 1, shell)
            cv.set(27 + lunge, y0 + dy - 1, shell_dark)
            cv.set(27 + lunge, y0 + dy + 1, shell_dark)
        cv.set(24 + lunge, y0, rgb("#1F2328"))
    elif facing == "n":
        fill_ellipse(cv, 16, y0, 6, 3.5, shell_dark)
        cv.hline(10, 22, y0 - 3, shell)
        for dx in (-1, 1):  # 从背后看到的两只螯
            fill_ellipse(cv, 16 + dx * 8, y0 + 1, 3, 2.5, shell_dark)
            cv.set(16 + dx * 10, y0, shell)
    else:
        fill_ellipse(cv, 16, y0, 6, 3.5, shell)
        cv.hline(10, 22, y0 - 3, shell_light)
        fill_ellipse(cv, 16, y0 - 3, 3, 2.5, shell_light)   # 头胸
        cv.set(14, y0 - 4, rgb("#1F2328"))
        cv.set(18, y0 - 4, rgb("#1F2328"))
        for dx in (-1, 1):  # 双螯前伸
            ax = 16 + dx * 8
            cv.rect(ax, y0 - 1, ax + dx, y0 + 2, shell)
            fill_ellipse(cv, ax + dx * 2, y0 - 2 - lunge, 3, 2, shell)
            cv.set(ax + dx * 3, y0 - 3 - lunge, shell_dark)
    if hurt:
        cv.hline(10, 22, y0 + 4, shell_dark)
    cv.outline(INK)
    if facing == "w":
        cv = cv.mirrored()
    return cv


def draw_boss(facing, pose):
    """48×48 山魈首领：厚肩长臂 + 獠牙 + 朱砂眼（大体型 Boss 骨架）。"""
    fur = rgb("#4E4238")
    fur_dark = rgb("#332B24")
    fur_light = rgb("#6A5A4A")
    tusk = rgb("#F2EAD8")
    eye = rgb("#C3452F")
    cv = Canvas(48, 48)
    bob = int(pose.get("bob", 0))
    leg = float(pose.get("leg", 0.0))
    lunge = int(pose.get("lunge", 0))
    hurt = int(pose.get("hurt", 0))
    slam = int(pose.get("slam", 0))

    cv.hline(10, 37, 45, (0x1F, 0x23, 0x28, 80))
    cv.hline(8, 39, 46, (0x1F, 0x23, 0x28, 60))

    y0 = 20 + bob
    # 腿
    for index, base_x in enumerate((16, 28)):
        offset = int(round(leg * 2)) if index == 0 else -int(round(leg * 2))
        lift = 1 if (index == 0 and leg > 0.4) or (index == 1 and leg < -0.4) else 0
        x = base_x + offset
        cv.rect(x, y0 + 16, x + 4, 43 - lift, fur_dark)
        cv.rect(x - 1, 42 - lift, x + 5, 44 - lift, fur)
    # 躯干
    fill_ellipse(cv, 24, y0 + 6, 12, 11, fur)
    cv.hline(12, 36, y0 - 4, fur_light)
    fill_ellipse(cv, 24, y0 + 12, 8, 5, fur_light)   # 胸腹浅色
    # 长臂（攻击时抬起砸下）
    for side in (-1, 1):
        ax = 24 + side * 15
        ay = y0 + 2
        if side == 1 and slam:
            ay = y0 - 6 + slam * 2
        cv.rect(ax - 3, ay, ax + 2, ay + 18, fur)
        cv.rect(ax - 4, ay + 18, ax + 3, ay + 23, fur_dark)  # 拳
        cv.vline(ax - 3, ay, ay + 18, fur_light)
    # 头
    head_y = y0 - 16
    fill_ellipse(cv, 24, head_y + 4, 9, 8, fur)
    cv.hline(16, 32, head_y - 3, fur_dark)      # 鬃毛
    cv.rect(14, head_y - 4, 34, head_y - 1, fur_dark)
    fill_ellipse(cv, 24, head_y + 8, 6, 4, fur_light)     # 面部
    if facing != "n":
        cv.set(20, head_y + 3, eye)
        cv.set(28, head_y + 3, eye)
        cv.set(20, head_y + 2, rgb("#E07B3A"))
        cv.set(28, head_y + 2, rgb("#E07B3A"))
        cv.rect(21, head_y + 8, 27, head_y + 10, fur_dark)  # 口
        cv.set(21, head_y + 7, tusk)
        cv.set(27, head_y + 7, tusk)
    else:
        cv.rect(16, head_y + 1, 32, head_y + 9, fur_dark)
    if hurt:
        cv.hline(14, 34, y0 + 9, fur_dark)
    cv.outline(INK)
    if facing == "w":
        cv = cv.mirrored()
    return cv


# 动作 → (姿态表, FPS, 是否循环)
HUMANOID_ACTIONS = {
    "idle": (POSE_IDLE, 6, True),
    "walk": (POSE_WALK6, 10, True),
    "attack": (POSE_ATTACK, 12, False),
    "hurt": (POSE_HURT, 6, False),
    "faint": (POSE_FAINT, 6, False),
}
QUADRUPED_ACTIONS = {
    "idle": ([{"bob": 0}, {"bob": 0}, {"bob": 1}, {"bob": 0}], 6, True),
    "walk": ([{"leg": 1.0, "bob": 0}, {"leg": 0.5, "bob": 1}, {"leg": 0.0, "bob": 0},
              {"leg": -1.0, "bob": 0}, {"leg": -0.5, "bob": 1}, {"leg": 0.0, "bob": 0}], 10, True),
    "attack": ([{"lunge": 0, "jaw": 0}, {"lunge": 2, "jaw": 1},
                {"lunge": 3, "jaw": 1}, {"lunge": 0, "jaw": 0}], 12, False),
    "hurt": ([{"hurt": 1}, {"hurt": 0}], 6, False),
}
DUMMY_ACTIONS = {
    "idle": ([{"bob": 0}, {"bob": 1}], 4, True),
    "hurt": ([{"tilt": 1}, {"tilt": 0}], 6, False),
}
SCORPION_ACTIONS = {
    "idle": ([{"bob": 0}, {"bob": 0}, {"bob": 1}, {"bob": 0}], 6, True),
    "walk": ([{"leg": 1.0, "bob": 0}, {"leg": 0.5, "bob": 1}, {"leg": 0.0, "bob": 0},
              {"leg": -1.0, "bob": 0}, {"leg": -0.5, "bob": 1}, {"leg": 0.0, "bob": 0}], 10, True),
    "attack": ([{"lunge": 0}, {"lunge": 3}, {"lunge": 4}, {"lunge": 0}], 12, False),
    "hurt": ([{"hurt": 1}, {"hurt": 0}], 6, False),
}
BOSS_ACTIONS = {
    "idle": ([{"bob": 0}, {"bob": 0}, {"bob": 1}, {"bob": 0}], 5, True),
    "walk": ([{"leg": 1.0, "bob": 0}, {"leg": 0.5, "bob": 1}, {"leg": 0.0, "bob": 0},
              {"leg": -1.0, "bob": 0}, {"leg": -0.5, "bob": 1}, {"leg": 0.0, "bob": 0}], 8, True),
    "attack": ([{"slam": 0}, {"slam": 1}, {"slam": 3}, {"slam": 0}], 10, False),
    "hurt": ([{"hurt": 1}, {"hurt": 0}], 6, False),
}


# ---------------------------------------------------------------------------
# 2) 灰狼：四足骨架
# ---------------------------------------------------------------------------


def draw_wolf(facing, pose):
    """32×32 四足狼：s 面向镜头 / n 背对 / e 朝右（w 由 e 镜像）。

    早期版本用矩形拼身体和头，描边之后并成一个灰方块（预览里像箱子）；
    改成椭圆 + 独立的腿/耳/尾，轮廓才有四足的辨识度。
    """
    cv = Canvas(FRAME, FRAME)
    bob = int(pose.get("bob", 0))
    leg = float(pose.get("leg", 0.0))
    lunge = int(pose.get("lunge", 0))
    hurt = int(pose.get("hurt", 0))
    jaw = int(pose.get("jaw", 0))

    # 影子
    cv.hline(9, 23, 30, (0x1F, 0x23, 0x28, 70))
    cv.hline(8, 24, 31, (0x1F, 0x23, 0x28, 50))

    y0 = 2 + bob                       # 整体垂直基准
    sway = int(round(leg * 1.5))
    lift_a = 1 if leg > 0.4 else 0
    lift_b = 1 if leg < -0.4 else 0

    def legs(positions):
        for index, x in enumerate(positions):
            offset = sway if index % 2 == 0 else -sway
            lift = lift_a if index % 2 == 0 else lift_b
            px = x + offset
            cv.rect(px, 22 + y0 - 2, px + 1, 28 - lift, FUR_DARK)
            cv.rect(px - 1, 28 - lift, px + 2, 29 - lift, shade(FUR_DARK, 0.78))

    if facing == "e":
        # 侧向站立：身体横向椭圆 + 前伸的头 + 四条腿 + 后扬的尾
        fill_ellipse(cv, 15, 18 + y0, 9, 5, FUR)
        cv.hline(8, 22, 14 + y0, FUR_LIGHT)
        fill_ellipse(cv, 16, 21 + y0, 7, 2, BELLY)
        legs((9, 13, 19, 23))
        # 头（朝右，带尖吻）
        head_x = 23 + lunge
        fill_ellipse(cv, head_x, 13 + y0, 5, 4, FUR)
        cv.rect(head_x + 3, 12 + y0, head_x + 7, 15 + y0, MUZZLE)
        cv.set(head_x + 7, 12 + y0, INK)
        cv.set(head_x + 2, 10 + y0, EYE)
        cv.set(head_x - 1, 7 + y0, FUR_DARK)   # 耳
        cv.set(head_x, 6 + y0, FUR_DARK)
        if jaw:
            cv.hline(head_x + 4, head_x + 7, 16 + y0, INK)
            cv.set(head_x + 5, 16 + y0, rgb("#FFFFFF"))
        # 尾
        cv.rect(5, 12 + y0, 8, 14 + y0, FUR_DARK)
        cv.set(4, 11 + y0, FUR_DARK)
        cv.set(3, 10 + y0, FUR)
    elif facing == "n":
        # 背对：圆背 + 后脑 + 朝玩家的尾
        fill_ellipse(cv, 16, 18 + y0, 8, 5, FUR_DARK)
        cv.hline(9, 23, 14 + y0, FUR)
        for x in range(10, 23, 3):
            cv.vline(x, 15 + y0, 21 + y0, FUR)
        legs((10, 14, 18, 22))
        fill_ellipse(cv, 16, 11 + y0, 5, 4, FUR)
        cv.hline(12, 20, 8 + y0, FUR_LIGHT)
        cv.set(12, 6 + y0, FUR_DARK)
        cv.set(13, 7 + y0, FUR_DARK)
        cv.set(20, 6 + y0, FUR_DARK)
        cv.set(19, 7 + y0, FUR_DARK)
        cv.rect(15, 21 + y0, 17, 26 + y0, FUR_DARK)
        cv.set(16, 27 + y0, FUR)
    else:
        # 面向镜头：圆身 + 大一点的正面头 + 立耳
        fill_ellipse(cv, 16, 18 + y0, 8, 5, FUR)
        cv.hline(9, 23, 14 + y0, FUR_LIGHT)
        fill_ellipse(cv, 16, 21 + y0, 6, 2, BELLY)
        legs((10, 14, 18, 22))
        head_y = 11 + y0 + lunge // 2
        fill_ellipse(cv, 16, head_y, 6, 5, FUR)
        cv.hline(11, 21, head_y - 4, FUR_LIGHT)
        fill_ellipse(cv, 16, head_y + 3, 3, 3, MUZZLE)
        cv.set(15, head_y + 3, INK)
        cv.set(17, head_y + 3, INK)
        cv.set(13, head_y - 1, EYE)
        cv.set(19, head_y - 1, EYE)
        if jaw:
            cv.hline(14, 18, head_y + 6, INK)
            cv.set(15, head_y + 6, rgb("#FFFFFF"))
            cv.set(17, head_y + 6, rgb("#FFFFFF"))
        for ex in (11, 20):
            cv.set(ex, head_y - 6, FUR_DARK)
            cv.set(ex + 1, head_y - 5, FUR_DARK)
    if hurt:
        cv.hline(9, 23, 26 + y0, shade(FUR_DARK, 0.75))
    cv.outline(INK)
    if facing == "w":
        cv = cv.mirrored()
    return cv


# ---------------------------------------------------------------------------
# 3) 训练木人：静态标靶
# ---------------------------------------------------------------------------


def draw_dummy(pose):
    cv = Canvas(FRAME, FRAME)
    tilt = int(pose.get("tilt", 0))
    bob = int(pose.get("bob", 0))
    # 影子
    cv.hline(9, 22, 30, (0x1F, 0x23, 0x28, 70))
    cv.hline(8, 23, 31, (0x1F, 0x23, 0x28, 55))
    # 立杆
    cv.rect(14 + tilt, 8 + bob, 17 + tilt, 29, POST)
    cv.vline(14 + tilt, 8 + bob, 29, shade(POST, 1.2))
    cv.vline(17 + tilt, 8 + bob, 29, POST_DARK)
    # 横臂
    cv.rect(7 + tilt, 12 + bob, 24 + tilt, 15 + bob, POST)
    cv.hline(7 + tilt, 24 + tilt, 12 + bob, shade(POST, 1.2))
    cv.hline(7 + tilt, 24 + tilt, 15 + bob, POST_DARK)
    # 草身
    for y in range(16, 26):
        half = 7 - abs(y - 21) // 2
        cv.hline(16 + tilt - half, 16 + tilt + half, y + bob, STRAW if (y // 2) % 2 else STRAW_DARK)
    # 绳箍
    for y in (17, 24):
        cv.hline(9 + tilt, 23 + tilt, y + bob, ROPE)
    # 靶心
    fill_ellipse(cv, 16 + tilt, 21 + bob, 4, 4, TARGET)
    fill_ellipse(cv, 16 + tilt, 21 + bob, 2, 2, rgb("#F2EAD8"))
    cv.set(16 + tilt, 21 + bob, TARGET)
    cv.outline(INK)
    return cv


# ---------------------------------------------------------------------------
# 渲染 / 写出
# ---------------------------------------------------------------------------


def render_action(entry, action, facing):
    if entry["kind"] == "humanoid":
        poses, _, _ = HUMANOID_ACTIONS[action]
        frames = []
        # 人形复用主角骨架（gen_character_sprites）：负 x 朝向镜像，对角有独立条带
        base = MIRROR_DIR.get(facing, facing)
        for pose in poses:
            frame = render_frame(entry, base, pose)
            frame = frame.mirrored() if base != facing else frame
            frames.append(stamp_shadow(frame, pose))
        return frames
    # 非人形骨架只画 s/n/e/w：斜向归一到侧身朝向（se/ne→e，sw/nw→w）
    side = {"se": "e", "ne": "e", "sw": "w", "nw": "w"}.get(facing, facing)
    if entry["kind"] == "quadruped":
        poses, _, _ = QUADRUPED_ACTIONS[action]
        return [draw_wolf(side, pose) for pose in poses]
    if entry["kind"] == "scorpion":
        poses, _, _ = SCORPION_ACTIONS[action]
        return [draw_scorpion(side, pose) for pose in poses]
    if entry["kind"] == "boss":
        poses, _, _ = BOSS_ACTIONS[action]
        return [draw_boss(side, pose) for pose in poses]
    poses, _, _ = DUMMY_ACTIONS[action]
    frames = [draw_dummy(pose) for pose in poses]
    return [frame.mirrored() for frame in frames] if facing == "w" else frames


def actions_for(entry):
    kind = entry["kind"]
    if kind == "humanoid":
        return HUMANOID_ACTIONS
    if kind == "quadruped":
        return QUADRUPED_ACTIONS
    if kind == "scorpion":
        return SCORPION_ACTIONS
    if kind == "boss":
        return BOSS_ACTIONS
    return DUMMY_ACTIONS


def directions_for(entry):
    # 训练木人不分朝向（speed=0，永远面向玩家也不影响观感），只出 s
    return ("s",) if entry["kind"] == "dummy" else DIRECTIONS


def enemy_entries():
    return [
        BANDIT,
        {"id": "gray_wolf", "display": "灰狼", "frame_px": FRAME, "sprite_size": f"{FRAME_PX}x{FRAME_PX}",
         "kind": "quadruped", "note": "四足骨架：追击型近战，攻击带前扑",
         "palette": {"fur": "#8A8F98"}},
        {"id": "training_dummy", "display": "训练木人", "frame_px": FRAME, "sprite_size": f"{FRAME_PX}x{FRAME_PX}",
         "kind": "dummy", "note": "静态标靶：不移动不攻击，只有待机与受击",
         "palette": {"post": "#8A6A46", "straw": "#C9A05A"}},
        # ---- 批次 4：首发敌人扩充（docs/30 §8.4）----
        {"id": "xuanming_adept", "display": "玄冥会徒", "frame_px": FRAME, "sprite_size": f"{FRAME_PX}x{FRAME_PX}",
         "kind": "humanoid", "note": "墨黑长袍 + 紫滚边 + 面罩，与司徒魁同色系；复用主角骨架",
         "build": {"head_top": 5, "head_h": 7, "head_w": 7, "half_w": 3, "torso_bottom": 22},
         "outfit": "robe", "hair": "cap", "beard": "none", "accessory": "sword_hip",
         "tool": {"attack": "blade"},
         "palette": {"skin": rgb("#D9B08C"), "hair": rgb("#1E1A1C"), "cloth": rgb("#2A2430"),
                     "cloth_alt": rgb("#4A3A56"), "trim": rgb("#8B6BB8"), "belt": rgb("#8B6BB8"),
                     "shoe": rgb("#1E1A1C")}},
        {"id": "poison_scorpion", "display": "毒蝎", "frame_px": FRAME, "sprite_size": f"{FRAME_PX}x{FRAME_PX}",
         "kind": "scorpion", "note": "低伏甲壳骨架：双螯 + 六足 + 上翘毒尾，攻击带突刺",
         "palette": {"shell": "#6B4A3A"}},
        {"id": "mountain_ape_boss", "display": "山魈首领（第一章 Boss）", "frame_px": 48,
         "sprite_size": f"{48 * ART_SCALE}x{48 * ART_SCALE}",
         "kind": "boss",
         "note": "48×48 绘制帧（导出 96×96）大体型骨架：厚肩长臂 + 獠牙 + 朱砂眼；重击/冲撞用同一套攻击帧",
         "palette": {"fur": "#4E4238"}},
    ]


def build_enemy(entry, out_root, preview_scale=2, quiet=False):
    enemy_dir = os.path.join(out_root, entry["id"])
    os.makedirs(enemy_dir, exist_ok=True)
    frame_px = entry.get("frame_px", FRAME)
    export_px = frame_px * ART_SCALE  # 导出帧尺寸（与 sprite_size 一致，放大 ART_SCALE 倍）
    clips = []
    rows = []
    for action, (poses, fps, looped) in actions_for(entry).items():
        for facing in directions_for(entry):
            frames = render_action(entry, action, facing)
            assert len(frames) == len(poses), (entry["id"], action, facing)
            strip = Canvas(export_px * len(frames), export_px)
            for index, frame in enumerate(frames):
                scaled = frame.scaled(ART_SCALE)
                for y in range(export_px):
                    for x in range(export_px):
                        strip.set(index * export_px + x, y, scaled.get(x, y))
            name = f"{action}_{facing}"
            rel = f"assets/art/enemies/{entry['id']}/{name}.png"
            with open(os.path.join(enemy_dir, f"{name}.png"), "wb") as handle:
                handle.write(strip.to_png())
            clips.append({"name": name, "sheet": rel, "cols": len(frames),
                          "fps": fps, "looped": looped, "action": action, "dir": facing})
            rows.append((name, frames))
            if action == "idle":
                with open(os.path.join(enemy_dir, f"static_{facing}.png"), "wb") as handle:
                    handle.write(frames[0].scaled(ART_SCALE).to_png())
    with open(os.path.join(enemy_dir, "preview.png"), "wb") as handle:
        handle.write(build_enemy_preview(entry, rows, preview_scale).to_png())
    if not quiet:
        print(f"  {entry['id']}: {len(clips)} clips / {sum(c['cols'] for c in clips)} frames")
    return clips


def build_enemy_preview(entry, rows, scale=2):
    tile = FRAME * scale
    label_w = 46
    body_w = max(len(frames) for _, frames in rows) * (tile + 1)
    width = label_w + body_w + 8
    row_h = tile + 3
    cv = Canvas(width, 20 + len(rows) * row_h + 6)
    cv.rect(0, 0, cv.w - 1, cv.h - 1, rgb("#14171A"))
    draw_text(cv, 4, 4, entry["id"], (0xE4, 0xC0, 0x78, 255), 2)
    draw_text(cv, 4 + len(entry["id"]) * 8 + 8, 6, f"FRAME {FRAME} X{scale}", (0x8A, 0x8F, 0x98, 255), 1)
    for index, (label, frames) in enumerate(rows):
        y = 20 + index * row_h
        draw_text(cv, 4, y + tile // 2 - 2, label, (0xA9, 0xC2, 0x8F, 255), 1)
        for col, frame in enumerate(frames):
            x = label_w + col * (tile + 1)
            cv.rect(x - 1, y - 1, x + tile, y + tile, rgb("#262C33"))
            cv.blit(frame.scaled(scale), x, y)
    return cv


def build_overview(entries, out_root, scale=2):
    tile = FRAME * scale
    label_w = 150
    rows = []
    for entry in entries:
        actions = actions_for(entry)
        second = "walk" if "walk" in actions else "hurt"  # 训练木人没有 walk
        rows.append((f"{entry['id']}  IDLE_S", entry, "idle", "s"))
        rows.append((f"{entry['id']}  {second.upper()}_S", entry, second, "s"))
    body_w = 6 * (tile + 1)
    cv = Canvas(label_w + body_w + 8, 22 + len(rows) * (tile + 3) + 8)
    cv.rect(0, 0, cv.w - 1, cv.h - 1, rgb("#14171A"))
    draw_text(cv, 4, 5, "ENEMIES / IDLE + MOVE / FRAME 32 X2", (0xE4, 0xC0, 0x78, 255), 1)
    for index, (label, entry, action, facing) in enumerate(rows):
        y = 22 + index * (tile + 3)
        draw_text(cv, 4, y + tile // 2 - 2, label, (0xA9, 0xC2, 0x8F, 255), 1)
        for col, frame in enumerate(render_action(entry, action, facing)):
            x = label_w + col * (tile + 1)
            cv.rect(x - 1, y - 1, x + tile, y + tile, rgb("#262C33"))
            cv.blit(frame.scaled(scale), x, y)
    return cv


def animation_component(clips, default="idle_s"):
    parts = [f"default={default}"]
    for clip in clips:
        parts.append(
            f"clip:{clip['name']}:{clip['sheet']}:{clip['cols']}:{clip['fps']}:{str(clip['looped']).lower()}"
        )
    return ";".join(parts)


def static_sprite(entry, facing="s"):
    return (
        f"texture=assets/art/enemies/{entry['id']}/static_{facing}.png"
        f";size={entry['sprite_size']};tint=1.000,1.000,1.000,1.000;sorting_layer=actor"
    )


def emit_manifest(entries, clips_by_enemy, out_root):
    payload = {
        "version": 1,
        "generated_by": "tools/gen_enemy_art.py",
        "design_doc": "docs/31-江湖有田-美术资源方案.md",
        "frame_px": FRAME_PX,
        "engine": {
            "animation_component": "default=<clip>;clip:<name>:<sheet>:<cols>:<fps>:<looped>",
            "note": "敌人由 entity.spawn 运行期创建，Lua 目前无法挂 Animation，只能用 static_<dir>.png",
        },
        "enemies": [],
    }
    for entry in entries:
        clips = clips_by_enemy[entry["id"]]
        payload["enemies"].append({
            "id": entry["id"],
            "display": entry["display"],
            "kind": entry["kind"],
            "note": entry["note"],
            "sprite_size": entry["sprite_size"],
            "static_sprite": static_sprite(entry, "s"),
            "static_sprites": {d: static_sprite(entry, d) for d in directions_for(entry)},
            "animation": animation_component(clips),
            "preview": f"assets/art/enemies/{entry['id']}/preview.png",
            "clips": clips,
        })
    path = os.path.join(out_root, "manifest.json")
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(payload, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    return path


def emit_lua(entries, clips_by_enemy, path):
    """生成 scripts/data/enemy_art.lua：敌人 id → 单帧 sprite 串 + 动画组件串。

    enemy.lua 在运行期 entity.spawn 后调 `animation.attach`（引擎 M18+ 新增），
    所以这里把"美术侧"的字符串集中生成，敌人数据表只写 id，不再手抄长串。
    """
    lines = [
        "-- data/enemy_art.lua：《江湖有田》敌人美术注册表（由 tools/gen_enemy_art.py 生成，勿手改）。",
        "--",
        "-- 字段：",
        "--   display        显示名",
        "--   sprite_size    世界尺寸（与帧尺寸一致）",
        "--   static_sprite  单帧 SpriteComponent 串（八向：s/n/e/w/se/sw/ne/nw）",
        "--   animation      Animation 组件串（default + clip:name:sheet:cols:fps:looped）",
        "--   clips          条带清单（调试/验收用）",
        "local M = {}",
        "",
        "M.version = 1",
        "M.enemies = {",
    ]
    for entry in entries:
        clips = clips_by_enemy[entry["id"]]
        lines.append(f"  {entry['id']} = {{")
        lines.append(f"    display = {lua_quote(entry['display'])},")
        lines.append(f"    sprite_size = {lua_quote(entry['sprite_size'])},")
        lines.append("    static_sprite = {")
        for facing in directions_for(entry):
            lines.append(f"      {facing} = {lua_quote(static_sprite(entry, facing))},")
        lines.append("    },")
        lines.append(f"    animation = {lua_quote(animation_component(clips))},")
        lines.append("  },")
    lines += [
        "}",
        "",
        "function M.get(id)",
        "  return M.enemies[id]",
        "end",
        "",
        "-- 运行期把美术挂到敌人实体上：先设单帧（立刻可见），再 attach 动画。",
        "function M.apply(entity_id, id, facing)",
        "  local art = M.enemies[id]",
        "  if not art or not entity_id then",
        "    return false",
        "  end",
        "  local ok = pcall(sprite.set, entity_id, art.static_sprite[facing or 's'] or art.static_sprite.s)",
        "  if not ok then",
        "    return false",
        "  end",
        "  local attached = pcall(animation.attach, entity_id, art.animation)",
        "  return attached == true",
        "end",
        "",
        "function M.validate_all()",
        "  local issues = {}",
        "  if M.version ~= 1 then",
        "    issues[#issues + 1] = 'data/enemy_art.lua: version: unsupported version ' .. tostring(M.version)",
        "  end",
        "  for id, art in pairs(M.enemies) do",
        "    if type(art.animation) ~= 'string' or not art.animation:find('^default=', 1) then",
        "      issues[#issues + 1] = 'data/enemy_art.lua:' .. id .. '.animation: expected default=..;clip:..'",
        "    end",
        "    if type(art.static_sprite) ~= 'table' or type(art.static_sprite.s) ~= 'string' then",
        "      issues[#issues + 1] = 'data/enemy_art.lua:' .. id .. '.static_sprite.s: missing'",
        "    end",
        "  end",
        "  return #issues == 0, issues",
        "end",
        "",
        "return M",
    ]
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write("\n".join(lines) + "\n")


def lua_quote(value):
    return '"' + str(value).replace("\\", "\\\\").replace('"', '\\"') + '"'


def generate(quiet=False):
    entries = enemy_entries()
    clips_by_enemy = {}
    if not quiet:
        print(f"生成敌人序列帧 → {OUT_DIR}")
    for entry in entries:
        clips_by_enemy[entry["id"]] = build_enemy(entry, OUT_DIR, quiet=quiet)
    with open(os.path.join(OUT_DIR, "_preview.png"), "wb") as handle:
        handle.write(build_overview(entries, OUT_DIR).to_png())
    manifest = emit_manifest(entries, clips_by_enemy, OUT_DIR)
    emit_lua(entries, clips_by_enemy, os.path.join("scripts", "data", "enemy_art.lua"))
    if not quiet:
        print("  _preview.png")
        print("  manifest.json:", manifest)
        print("  lua: scripts/data/enemy_art.lua")


def check():
    from pixel_kit import decode_png

    issues = []
    manifest_path = os.path.join(OUT_DIR, "manifest.json")
    if not os.path.exists(manifest_path):
        return ["manifest.json 缺失，请先运行 tools/gen_enemy_art.py"]
    with open(manifest_path, encoding="utf-8") as handle:
        manifest = json.load(handle)
    for entry in manifest["enemies"]:
        frame_px = int(entry.get("sprite_size", f"{FRAME}x{FRAME}").split("x")[0])
        for clip in entry["clips"]:
            path = clip["sheet"].replace("/", os.sep)
            if not os.path.exists(path):
                issues.append("missing sheet: " + clip["sheet"])
                continue
            width, height, channels, _ = decode_png(path)
            if height != frame_px:
                issues.append(f"{clip['sheet']}: height {height} != {frame_px}")
            if width != frame_px * clip["cols"]:
                issues.append(f"{clip['sheet']}: width {width} != {frame_px}*{clip['cols']}")
            if channels != 4:
                issues.append(f"{clip['sheet']}: 期望 RGBA（channels={channels}）")
        for facing, _ in entry["static_sprites"].items():
            path = os.path.join(OUT_DIR, entry["id"], f"static_{facing}.png")
            if not os.path.exists(path):
                issues.append("missing static: " + path)
                continue
            width, height, _, _ = decode_png(path)
            if (width, height) != (frame_px, frame_px):
                issues.append(f"{path}: {width}x{height} != {frame_px}x{frame_px}")
    if not os.path.exists(os.path.join(OUT_DIR, "_preview.png")):
        issues.append("missing " + os.path.join(OUT_DIR, "_preview.png"))
    # 交叉校验：data/enemies.lua 里写的 texture 路径必须真实存在（打错字不会报错、只会白块）
    data_path = os.path.join("scripts", "data", "enemies.lua")
    if os.path.exists(data_path):
        import re

        with open(data_path, encoding="utf-8") as handle:
            lines = [line for line in handle if not line.lstrip().startswith("--")]
        for match in re.finditer(r"texture=([^;\"]+\.png)", "\n".join(lines)):
            referenced = match.group(1)
            if referenced.startswith("__") or "<" in referenced:
                continue  # 内建贴图 / 注释里的占位写法
            if not os.path.exists(referenced.replace("/", os.sep)):
                issues.append(f"data/enemies.lua 引用了不存在的贴图：{referenced}")
    return issues


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》敌人序列帧生成")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        problems = check()
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        print("[OK] 敌人序列帧校验通过")
        return
    generate()


if __name__ == "__main__":
    main()
