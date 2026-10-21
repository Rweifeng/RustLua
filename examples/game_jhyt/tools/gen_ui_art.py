# -*- coding: utf-8 -*-
"""《江湖有田》UI 切图生成（按 docs/35-UI资源设计，按模块分目录）。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_ui_art.py            # 生成全部切图 + manifest + 两张预览
    python tools/gen_ui_art.py --check    # 按规格表校验尺寸/存在性

目录结构（`assets/art/ui/<模块>/`，模块按 docs/35 §7/§8 划分）：

    common/     跨模块共用控件：面板 / 按钮 / 槽位 / 输入框 / 标签页 / 勾选 / 滑杆 /
                箭头 / 关闭 / 分隔线 / 面板印章
    hud/        主界面 HUD（§7）：三条状态条与图标、金钱、天气、状态图标、头像框
    backpack/   背包（§8.1）：五档品质框
    dialogue/   对话（§8.3）：头像框、继续箭头、选项引导符
    shop/       商店（§8.2）：数量步进器 −/＋
    quest/      任务（§8.4）：目标状态点（未完成/完成）
    skills/     武学（§8.5）：招式卡片底板
    build/      建筑放置（§8.6）：可放置/冲突的半透明占位格
    save/       存档（§8.7）：存档槽底板、截图占位块
    settings/   设置（§8.8）：开关两态
    touch/      触屏控件：摇杆轨道/杆头、五个玩法按钮盘、面板十字键与关闭
                （直径与 `scripts/ui/layout.lua` 的 `M.TOUCH` 逐项相等，见 TOUCH 节头）

另外产出：
    manifest.json           模块 → 文件清单 + 该模块用到的 common 控件（代码侧只读这个）
    _preview_controls.png   全切图 2× 放大 + 模块分组标签
    _preview_mock.png       按 §7/§8 布局拼出的 HUD/背包/对话/商店效果图 + 9-slice 缩放检查

划分原则：**共用控件只存一份**放 `common/`，模块目录只放该模块专属切图；
模块用到哪些 common 控件写在 `manifest.json` 的 `uses_common` 里，
避免同一张面板切图在 10 个模块里各存一份（改一处要改十处）。

引擎事实：
  * 场景 `Image` 组件支持 `nine_slice=l,r,t,b`（core/ui.rs `UiImage`）→ 静态 UI 可直接用；
  * Lua `ui.image(id,x,y,w,h,texture,r,g,b)` 把 `nine_slice` 写死为 None（core/script.rs）
    → 对话/商店这类运行期浮层要 9-slice 必须先补 Lua 入口，见 docs/38 §5；
  * 图标/框体一律透明背景、1px `ink/900` 描边、光源左上 45°。

依赖：Python 3 标准库 + 同目录 pixel_kit.py。
"""

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import (  # noqa: E402
    Canvas,
    draw_nine_slice,
    draw_text,
    hash_stream,
    load_canvas,
    mix,
    rgb,
    shade,
    text_width,
)
from cjk_text import prewarm, text_canvas  # noqa: E402  门面印章的中文

OUT_DIR = os.path.join("assets", "art", "ui")  # 模块目录直接挂在 ui/ 下（与资源库现状一致）
SCENE_PREVIEW = os.path.join(".temp", "scene_preview_main.png")

# docs/35 §3 色板
INK900 = rgb("#1F2328")
INK800 = rgb("#262C33")
INK700 = rgb("#31383F")
INK600 = rgb("#3D444C")
PAPER50 = rgb("#F2EAD8")
PAPER200 = rgb("#D8CDB2")
PAPER500 = rgb("#A89F8A")
WOOD500 = rgb("#6B4F3A")
WOOD700 = rgb("#4A3628")
CINNABAR500 = rgb("#C3452F")
CINNABAR300 = rgb("#E06B50")
CINNABAR900 = rgb("#8F2E1F")
BAMBOO500 = rgb("#789262")
BAMBOO300 = rgb("#A9C28F")
BAMBOO700 = rgb("#5B7048")
GOLD500 = rgb("#C9A05A")
GOLD300 = rgb("#E4C078")
INDIGO500 = rgb("#4A6FA5")
INDIGO300 = rgb("#7FA3D4")
CLOUD = rgb("#C9CCD6")
CLOUD_DARK = rgb("#8A8F98")

# 批次 4 招式/状态图标用到的补充色（与 gen_manor_art / gen_village_art 同值）
PAPER_DIM = rgb("#D8CDB2")
STONE = rgb("#8E8B80")
STONE_LIGHT = rgb("#A5A197")
IRON = rgb("#C8CCD4")
IRON_DARK = rgb("#7E8391")
BAMBOO_LIGHT = rgb("#A9C28F")
BAMBOO_DARK = rgb("#5B7048")
WOOD_MID = rgb("#6B4F3A")
WOOD_DARK = rgb("#4A3628")
INK = INK900          # 图标描边别名
BAMBOO = BAMBOO500    # 与 gen_manor_art 同名的别名
CINNABAR = CINNABAR500
GOLD = GOLD500

QUALITY = [rgb("#B9BDC4"), BAMBOO500, GOLD500, CINNABAR500, rgb("#8B6BB8")]
TOUCH_VIOLET = rgb("#8B6BB8")   # 任务键（与传世品质同紫，docs/35 §3 之外唯一的新增色）

FONT_LABEL = GOLD300
FONT_DIM = BAMBOO300

# 模块顺序（预览与 manifest 都按这个顺序，稳定可复现）
MODULES = [
    ("common", "COMMON / 共用控件"),
    ("menu", "MENU / 主入口门面（武侠风）"),
    ("hud", "HUD / 主界面 (DOCS35 S7)"),
    ("backpack", "BACKPACK / 背包 (S8.1)"),
    ("dialogue", "DIALOGUE / 对话 (S8.3)"),
    ("shop", "SHOP / 商店 (S8.2)"),
    ("quest", "QUEST / 任务 (S8.4)"),
    ("skills", "SKILLS / 武学 (S8.5)"),
    ("build", "BUILD / 建筑放置 (S8.6)"),
    ("save", "SAVE / 存档 (S8.7)"),
    ("settings", "SETTINGS / 设置 (S8.8)"),
    ("touch", "TOUCH / 触屏控件（摇杆 + 虚拟按钮）"),
]

# 每个模块在 docs/35 对应布局里会用到的 common 控件（写进 manifest，代码侧照抄）
MODULE_USES = {
    "hud": ["ui_panel_main_9s", "ui_slot_9s", "ui_slot_selected_9s", "ui_key_hint_9s"],
    "backpack": ["ui_panel_main_9s", "ui_panel_confirm_9s", "ui_slot_9s", "ui_slot_selected_9s",
                 "ui_btn_primary_9s", "ui_btn_secondary_9s", "ui_btn_danger_9s",
                 "ui_btn_close", "ui_icon_seal", "ui_tab_on_9s", "ui_tab_off_9s"],
    "dialogue": ["ui_panel_paper_9s", "ui_slot_9s"],
    "shop": ["ui_panel_main_9s", "ui_panel_confirm_9s", "ui_slot_9s", "ui_btn_primary_9s",
             "ui_btn_close", "ui_icon_seal", "ui_tab_on_9s", "ui_tab_off_9s",
             "ui_scroll_arrow_up", "ui_scroll_arrow_down"],
    "quest": ["ui_panel_main_9s", "ui_btn_primary_9s", "ui_btn_close", "ui_icon_seal",
              "ui_tab_on_9s", "ui_tab_off_9s"],
    "skills": ["ui_panel_main_9s", "ui_btn_close", "ui_icon_seal", "ui_key_hint_9s"],
    "build": ["ui_panel_main_9s", "ui_btn_primary_9s", "ui_btn_secondary_9s", "ui_key_hint_9s"],
    "save": ["ui_panel_main_9s", "ui_panel_confirm_9s", "ui_btn_primary_9s",
             "ui_btn_secondary_9s", "ui_btn_danger_9s", "ui_btn_close", "ui_icon_seal"],
    "settings": ["ui_panel_main_9s", "ui_slider_track", "ui_slider_handle",
                 "ui_dropdown_arrow", "ui_btn_close", "ui_icon_seal",
                 "ui_tab_on_9s", "ui_tab_off_9s"],
}


# ---------------------------------------------------------------------------
# 通用绘制
# ---------------------------------------------------------------------------


def frame(canvas, x0, y0, x1, y1, color, thickness=1):
    for step in range(thickness):
        canvas.hline(x0 + step, x1 - step, y0 + step, color)
        canvas.hline(x0 + step, x1 - step, y1 - step, color)
        canvas.vline(x0 + step, y0 + step, y1 - step, color)
        canvas.vline(x1 - step, y0 + step, y1 - step, color)


def bevel_box(canvas, x0, y0, x1, y1, base, light=None, dark=None):
    light = light or shade(base, 1.18)
    dark = dark or shade(base, 0.78)
    canvas.rect(x0, y0, x1, y1, base)
    canvas.hline(x0, x1, y0, light)
    canvas.vline(x0, y0, y1, light)
    canvas.hline(x0, x1, y1, dark)
    canvas.vline(x1, y0, y1, dark)


def wood_band_h(canvas, x0, x1, y0, y1):
    canvas.rect(x0, y0, x1, y1, WOOD500)
    for y in range(y0 + 1, y1, 3):
        canvas.hline(x0, x1, y, WOOD700)
    canvas.hline(x0, x1, y0, shade(WOOD500, 1.2))


def wood_band_v(canvas, x0, x1, y0, y1):
    canvas.rect(x0, y0, x1, y1, WOOD500)
    for x in range(x0 + 1, x1, 3):
        canvas.vline(x, y0, y1, WOOD700)
    canvas.vline(x0, y0, y1, shade(WOOD500, 1.2))


def rivet(canvas, x, y, color=GOLD500):
    canvas.set(x, y, GOLD300)
    canvas.set(x + 1, y, color)
    canvas.set(x, y + 1, color)
    canvas.set(x + 1, y + 1, shade(color, 0.8))


def fill_ellipse(canvas, cx, cy, rx, ry, color):
    for y in range(int(cy - ry), int(cy + ry) + 1):
        for x in range(int(cx - rx), int(cx + rx) + 1):
            dx = (x - cx) / max(rx, 0.001)
            dy = (y - cy) / max(ry, 0.001)
            if dx * dx + dy * dy <= 1.0:
                canvas.set(x, y, color)


# ---------------------------------------------------------------------------
# common / 共用控件
# ---------------------------------------------------------------------------


def panel_main():
    """96×96，9-slice 16,16,16,16：ink/900 底 + 木纹边框 + 金角码。"""
    cv = Canvas(96, 96)
    cv.rect(0, 0, 95, 95, INK900)
    wood_band_h(cv, 0, 95, 1, 13)
    wood_band_h(cv, 0, 95, 82, 94)
    wood_band_v(cv, 1, 13, 1, 94)
    wood_band_v(cv, 82, 94, 1, 94)
    frame(cv, 0, 0, 95, 95, INK900, 1)
    frame(cv, 14, 14, 81, 81, WOOD700, 1)
    frame(cv, 15, 15, 80, 80, INK600, 1)
    for cx, cy, sx, sy in ((4, 4, 1, 1), (91, 4, -1, 1), (4, 91, 1, -1), (91, 91, -1, -1)):
        cv.hline(min(cx, cx + sx * 8), max(cx, cx + sx * 8), cy, GOLD500)
        cv.vline(cx, min(cy, cy + sy * 8), max(cy, cy + sy * 8), GOLD500)
        rivet(cv, cx + (1 if sx > 0 else -2), cy + (1 if sy > 0 else -2))
    return cv


def panel_paper():
    """96×96，9-slice 16,16,16,16：宣纸底（对话/tooltip 共用）。"""
    cv = Canvas(96, 96)
    cv.rect(0, 0, 95, 95, PAPER50)
    frame(cv, 0, 0, 95, 95, INK900, 1)
    frame(cv, 1, 1, 94, 94, WOOD700, 2)
    frame(cv, 3, 3, 92, 92, PAPER200, 1)
    for x in range(4, 92, 7):
        cv.set(x, 5, PAPER200)
        cv.set(x + 3, 90, PAPER200)
    for y in range(6, 90, 9):
        cv.set(5, y, PAPER200)
        cv.set(90, y + 4, PAPER200)
    return cv


def panel_confirm():
    """64×64，9-slice 12,12,12,12：二次确认小框。"""
    cv = Canvas(64, 64)
    cv.rect(0, 0, 63, 63, INK800)
    wood_band_h(cv, 0, 63, 1, 9)
    wood_band_h(cv, 0, 63, 54, 62)
    wood_band_v(cv, 1, 9, 1, 62)
    wood_band_v(cv, 54, 62, 1, 62)
    frame(cv, 0, 0, 63, 63, INK900, 1)
    frame(cv, 10, 10, 53, 53, GOLD500, 1)
    frame(cv, 11, 11, 52, 52, INK600, 1)
    return cv


def button_primary(pressed=False):
    """64×48，9-slice 8,8,8,8：朱砂底主按钮。

    pressed 态按 §6.1：内容下移 1px（顶端留 1px 暗缝）+ 底色压暗一档；
    只把颜色调暗一点在小尺寸下分不出来（实测过）。
    """
    cv = Canvas(64, 48)
    cv.rect(0, 0, 63, 47, INK900)
    frame(cv, 1, 1, 62, 46, shade(GOLD500, 0.8) if pressed else GOLD500, 1)
    if pressed:
        cv.rect(2, 3, 61, 45, CINNABAR900)
        cv.hline(2, 61, 2, INK900)
        cv.hline(2, 61, 44, shade(CINNABAR900, 0.8))
        cv.hline(3, 60, 45, CINNABAR500)
    else:
        bevel_box(cv, 2, 2, 61, 45, CINNABAR500)
        cv.hline(3, 60, 3, CINNABAR300)
        cv.hline(3, 60, 4, shade(CINNABAR300, 0.92))
        cv.hline(3, 60, 44, CINNABAR900)
    for x, y in ((4, 4), (57, 4), (4, 41), (57, 41)):
        rivet(cv, x, y, GOLD500)
    return cv


def button_secondary():
    """64×48，9-slice 8,8,8,8：木纹框次按钮。"""
    cv = Canvas(64, 48)
    cv.rect(0, 0, 63, 47, INK900)
    frame(cv, 1, 1, 62, 46, GOLD500, 1)
    wood_band_h(cv, 2, 61, 2, 8)
    wood_band_h(cv, 2, 61, 39, 45)
    wood_band_v(cv, 2, 8, 2, 45)
    wood_band_v(cv, 55, 61, 2, 45)
    cv.rect(9, 9, 54, 38, INK800)
    frame(cv, 9, 9, 54, 38, INK600, 1)
    return cv


def button_danger():
    """64×48，9-slice 8,8,8,8：朱砂框危险按钮。"""
    cv = Canvas(64, 48)
    cv.rect(0, 0, 63, 47, INK900)
    frame(cv, 1, 1, 62, 46, CINNABAR500, 2)
    cv.rect(3, 3, 60, 44, INK800)
    frame(cv, 3, 3, 60, 44, INK600, 1)
    cv.hline(4, 59, 4, CINNABAR900)
    return cv


def slot(selected=False):
    """48×48，9-slice 6,6,6,6：物品槽。"""
    cv = Canvas(48, 48)
    frame(cv, 0, 0, 47, 47, INK900, 1)
    ring = GOLD500 if selected else INK700
    frame(cv, 1, 1, 46, 46, ring, 2 if selected else 1)
    bevel_box(cv, 3, 3, 44, 44, INK800, light=INK700, dark=INK900)
    if selected:
        frame(cv, 5, 5, 42, 42, shade(GOLD500, 0.7), 1)
    return cv


def input_box():
    """48×28，9-slice 6,6,6,6：输入框。"""
    cv = Canvas(48, 28)
    frame(cv, 0, 0, 47, 27, INK900, 1)
    cv.rect(1, 1, 46, 26, INK800)
    cv.hline(1, 46, 1, INK900)
    cv.hline(1, 46, 26, INK700)
    frame(cv, 2, 2, 45, 25, INK600, 1)
    return cv


def key_hint():
    """24×24，9-slice 6,6,6,6：交互提示键位框。"""
    cv = Canvas(24, 24)
    frame(cv, 0, 0, 23, 23, INK900, 1)
    cv.rect(1, 1, 22, 22, PAPER50)
    cv.rect(1, 17, 22, 22, PAPER200)
    cv.hline(1, 22, 1, rgb("#FFFFFF"))
    cv.hline(1, 22, 23, INK900)
    return cv


def tab(on):
    """64×28，9-slice 6,6,6,6：标签页（选中项底部 2px gold/500，§6.2）。"""
    cv = Canvas(64, 28)
    frame(cv, 0, 0, 63, 27, INK900, 1)
    cv.rect(1, 1, 62, 26, INK700 if on else INK800)
    cv.hline(1, 62, 1, INK600)
    if on:
        cv.hline(2, 61, 24, GOLD500)
        cv.hline(2, 61, 25, shade(GOLD500, 0.8))
    else:
        cv.hline(2, 61, 24, INK600)
    return cv


def checkbox(on):
    cv = Canvas(16, 16)
    frame(cv, 0, 0, 15, 15, GOLD500 if on else INK900, 1)
    cv.rect(1, 1, 14, 14, INK800)
    if on:
        for step in range(4):
            cv.set(4 + step, 9 - step, PAPER50)
        for step in range(5):
            cv.set(7 + step, 6 + step, PAPER50)
    return cv


def slider_track():
    """4×12：横向滑轨（竖直剖面每 4 行循环，横向拉伸不糊）。"""
    cv = Canvas(4, 12)
    for y in range(12):
        phase = y % 4
        color = (INK900, INK600, INK600, INK900)[phase]
        cv.hline(0, 3, y, color)
    return cv


def slider_handle():
    cv = Canvas(12, 12)
    fill_ellipse(cv, 5.5, 5.5, 5, 5, GOLD500)
    fill_ellipse(cv, 4.5, 4.5, 2.4, 2.0, GOLD300)
    cv.outline(INK900)
    return cv


def dropdown_arrow():
    cv = Canvas(8, 8)
    for y in range(4):
        cv.hline(1 + y, 6 - y, 2 + y, PAPER200 if y < 3 else PAPER500)
    cv.outline(INK900)
    return cv


def scroll_arrow(direction):
    cv = Canvas(8, 8)
    for y in range(4):
        row = y if direction == "down" else 3 - y
        cv.hline(1 + row, 6 - row, 2 + y, PAPER500)
    cv.outline(INK900)
    return cv


def close_button(hover):
    cv = Canvas(16, 16)
    color = CINNABAR300 if hover else PAPER500
    if hover:
        fill_ellipse(cv, 7.5, 7.5, 7, 7, INK800)
    for step in range(8):
        cv.set(4 + step, 4 + step, color)
        cv.set(5 + step, 4 + step, color)
        cv.set(11 - step, 4 + step, color)
        cv.set(10 - step, 4 + step, color)
    if hover:
        cv.outline(INK900)
    return cv


def divider():
    """4×4：分隔线（第 0 行为线，其余透明）。"""
    cv = Canvas(4, 4)
    cv.hline(0, 3, 0, INK600)
    return cv


def icon_seal():
    """24×24：朱砂印章（各面板标题栏共用，文字一律走 UiText）。"""
    cv = Canvas(24, 24)
    cv.rect(2, 2, 21, 21, CINNABAR500)
    frame(cv, 2, 2, 21, 21, CINNABAR900, 1)
    frame(cv, 5, 5, 18, 18, PAPER50, 1)
    cv.hline(7, 16, 7, shade(CINNABAR500, 0.9))
    cv.vline(7, 7, 16, shade(CINNABAR500, 0.9))
    for x, y in ((3, 3), (20, 4), (4, 20), (19, 19), (12, 21)):
        cv.set(x, y, shade(CINNABAR900, 0.9))
    return cv


# ---------------------------------------------------------------------------
# hud / 主界面
# ---------------------------------------------------------------------------


def bar_frame():
    """48×16，9-slice 4,4,4,4：进度条外框（中心透明，填充透出）。"""
    cv = Canvas(48, 16)
    frame(cv, 0, 0, 47, 15, INK900, 1)
    frame(cv, 1, 1, 46, 14, INK700, 2)
    cv.hline(1, 46, 1, INK600)
    cv.hline(3, 44, 3, INK800)
    return cv


def bar_fill(color, highlight):
    """24×8，9-slice 4,0,4,0：进度条填充（横向拉伸，两端圆头）。"""
    cv = Canvas(24, 8)
    cv.rect(1, 0, 22, 7, color)
    cv.hline(1, 22, 0, highlight)
    cv.hline(1, 22, 7, shade(color, 0.72))
    cv.rect(0, 1, 0, 6, shade(color, 0.85))
    cv.rect(23, 1, 23, 6, shade(color, 0.85))
    cv.hline(1, 22, 1, shade(highlight, 1.05))
    return cv


def icon_hp():
    """20×20：心（生命）。"""
    cv = Canvas(20, 20)
    fill_ellipse(cv, 6.5, 7.5, 4.5, 4.5, CINNABAR500)
    fill_ellipse(cv, 13.5, 7.5, 4.5, 4.5, CINNABAR500)
    for y in range(8, 18):
        t = (y - 8) / 9.0
        half = int(round(8 * (1 - t)))
        cv.hline(10 - half, 10 + half, y, CINNABAR500)
    fill_ellipse(cv, 6.5, 6.5, 2.0, 1.6, CINNABAR300)
    cv.outline(INK900)
    return cv


def icon_stamina():
    """20×20：麦穗（体力，对应 bamboo/500）。"""
    cv = Canvas(20, 20)
    cv.vline(10, 4, 18, BAMBOO700)
    for y in (5, 8, 11, 14):
        cv.hline(6, 9, y, BAMBOO500)
        cv.hline(11, 14, y, BAMBOO500)
        cv.set(5, y + 1, BAMBOO300)
        cv.set(15, y + 1, BAMBOO300)
    cv.vline(10, 3, 17, BAMBOO500)
    for y in (4, 7, 10, 13):
        cv.set(9, y, GOLD300)
        cv.set(11, y, GOLD300)
    cv.outline(INK900)
    return cv


def icon_neili():
    """20×20：太极（内力，对应 indigo/500）。"""
    cv = Canvas(20, 20)
    fill_ellipse(cv, 10, 10, 8, 8, INDIGO500)
    for y in range(2, 19):
        for x in range(10, 19):
            if cv.get(x, y)[:3] == INDIGO500[:3]:
                cv.set(x, y, PAPER50)
    fill_ellipse(cv, 10, 6.5, 3.2, 3.2, PAPER50)
    fill_ellipse(cv, 10, 13.5, 3.2, 3.2, INDIGO500)
    cv.set(10, 6, INDIGO500)
    cv.set(10, 14, PAPER50)
    cv.outline(INK900)
    return cv


def icon_coin():
    """16×16：铜钱（方孔）。"""
    cv = Canvas(16, 16)
    fill_ellipse(cv, 8, 8, 7, 7, GOLD500)
    fill_ellipse(cv, 8, 8, 5, 5, GOLD300)
    cv.rect(6, 6, 9, 9, (0, 0, 0, 0))
    frame(cv, 6, 6, 9, 9, shade(GOLD500, 0.8), 1)
    cv.outline(INK900)
    return cv


def weather_sun():
    cv = Canvas(32, 32)
    fill_ellipse(cv, 16, 16, 7, 7, GOLD300)
    for dx, dy in ((0, -12), (0, 12), (-12, 0), (12, 0), (-9, -9), (9, -9), (-9, 9), (9, 9)):
        for step in range(3, 6):
            cv.set(16 + dx * step // 6, 16 + dy * step // 6, GOLD500)
    cv.outline(INK900)
    return cv


def weather_cloud():
    cv = Canvas(32, 32)
    fill_ellipse(cv, 12, 18, 7, 6, CLOUD)
    fill_ellipse(cv, 21, 18, 6, 5, CLOUD)
    fill_ellipse(cv, 16, 13, 6, 5, CLOUD)
    cv.hline(6, 26, 22, CLOUD_DARK)
    fill_ellipse(cv, 14, 12, 3, 2, rgb("#FFFFFF"))
    cv.outline(INK900)
    return cv


def weather_rain():
    cv = weather_cloud()
    for x in (9, 16, 23):
        cv.vline(x, 24, 27, INDIGO300)
        cv.set(x, 28, INDIGO500)
    cv.outline(INK900)
    return cv


def status_icon(kind):
    """16×16 状态图标：中毒/增益为 MVP，其余 6 个在批次 4 补齐（docs/35 §5）。"""
    cv = Canvas(16, 16)
    border = CINNABAR500 if kind in ("poison", "bleed", "debuff") else GOLD500
    if kind in ("stealth", "sleep"):
        border = INDIGO500
    frame(cv, 0, 0, 15, 15, INK900, 1)
    cv.rect(1, 1, 14, 14, INK800)
    frame(cv, 1, 1, 14, 14, border, 1)
    if kind == "poison":
        fill_ellipse(cv, 8, 9, 3, 4, BAMBOO700)
        cv.set(7, 5, BAMBOO300)
        cv.set(8, 4, BAMBOO300)
    elif kind == "buff":
        for y in range(4, 9):
            half = y - 3
            cv.hline(8 - half, 7 + half, y, GOLD300)
        cv.rect(6, 9, 9, 13, GOLD500)
        cv.hline(6, 9, 13, shade(GOLD500, 0.8))
    elif kind == "bleed":
        for index, (x, y) in enumerate(((6, 5), (9, 4), (7, 8), (10, 9))):
            cv.set(x, y, CINNABAR300 if index % 2 else CINNABAR500)
            cv.set(x, y + 1, CINNABAR500)
            cv.set(x, y + 2, CINNABAR900)
    elif kind == "debuff":
        purple = rgb("#8B6BB8")
        for y in range(5, 13):
            half = 12 - y
            cv.hline(8 - half, 7 + half, y, purple)
        cv.rect(6, 5, 9, 9, purple)
        cv.set(7, 6, rgb("#B79CD8"))
    elif kind == "sleep":
        for index, (x, y) in enumerate(((4, 6), (8, 4), (10, 9))):
            size = 3 + index
            cv.hline(x, x + size, y, INDIGO300)
            cv.hline(x, x + size, y + size, INDIGO300)
            cv.set(x, y + size, INDIGO300)
    elif kind == "stealth":
        fill_ellipse(cv, 8, 10, 4, 5, INK700)
        cv.set(6, 8, INDIGO300)
        cv.set(10, 8, INDIGO300)
        cv.hline(5, 11, 13, INK600)
    elif kind == "weather":
        fill_ellipse(cv, 6, 7, 3, 3, GOLD300)
        fill_ellipse(cv, 10, 10, 3, 2, PAPER200)
        cv.hline(6, 13, 12, PAPER500)
    elif kind == "festival":
        cv.rect(5, 5, 11, 12, CINNABAR500)
        cv.hline(5, 11, 5, GOLD500)
        cv.hline(5, 11, 12, GOLD500)
        cv.vline(8, 2, 5, WOOD_DARK)
        cv.vline(8, 12, 14, CINNABAR500)
    return cv


def skill_icon(kind):
    """32×32 招式图标（docs/30 §8.2 的五式）。"""
    cv = Canvas(32, 32)
    if kind == "basic_blade":
        for step in range(14):
            cv.set(8 + step, 24 - step, IRON)
            cv.set(9 + step, 24 - step, IRON_DARK)
        cv.rect(6, 22, 10, 26, WOOD_MID)
        for step in range(6):
            cv.set(20 + step, 8 + step, PAPER50)
            cv.set(19 + step, 8 + step, PAPER_DIM)
    elif kind == "chase_wind":
        for y in (10, 16, 22):
            cv.hline(4, 18 - (y % 4), y, PAPER_DIM)
        fill_ellipse(cv, 22, 16, 5, 7, BAMBOO_LIGHT)
        fill_ellipse(cv, 22, 22, 4, 3, BAMBOO)
        cv.set(24, 20, BAMBOO_DARK)
    elif kind == "guard_heart":
        for y in range(6, 27):
            half = 9 - abs(y - 16) // 2
            cv.hline(16 - half, 16 + half, y, STONE)
        cv.hline(8, 24, 6, STONE_LIGHT)
        fill_ellipse(cv, 13, 14, 4, 3, CINNABAR)
        fill_ellipse(cv, 19, 14, 4, 3, CINNABAR)
        for y in range(15, 24):
            half = int((23 - y) * 0.9)
            cv.hline(16 - half, 16 + half, y, CINNABAR)
    elif kind == "return_palm":
        fill_ellipse(cv, 16, 19, 8, 7, rgb("#E8C49A"))
        for x in (10, 13, 16, 19, 22):
            cv.vline(x, 9, 16, rgb("#E8C49A"))
        for step in range(10):
            cv.set(24 - step // 2, 6 + step % 3, INDIGO300 if step % 2 else INDIGO500)
        cv.set(8, 10, INDIGO300)
    elif kind == "breath_qi":
        import math

        for radius, color in ((11, INDIGO500), (8, INDIGO300), (5, PAPER50)):
            for step in range(28):
                angle = step / 28.0 * 6.28318
                cv.set(16 + int(radius * math.cos(angle)), 16 + int(radius * math.sin(angle)), color)
        cv.set(16, 16, INDIGO500)
    elif kind == "palm_gust":
        import math

        skin = rgb("#E8C49A")
        # 千风掌：右侧一掌推出，左侧四道同心风弧逐层收短——"千风"读的是弧的层数，
        # 不画五指，免得与回风掌/铁砂掌同骨架、三张并排看像同一式。
        fill_ellipse(cv, 25, 16, 5, 7, skin)
        for index, radius in enumerate((20, 16, 12, 8)):
            color = INDIGO500 if index % 2 == 0 else INDIGO300
            for step in range(16):
                angle = -0.85 + step / 15.0 * 1.7
                x = 30 - int(radius * math.cos(angle))
                y = 16 - int(radius * math.sin(angle) * 0.9)
                if 0 <= x <= 31 and 0 <= y <= 31 and not cv.opaque(x, y):
                    cv.set(x, y, color)
    elif kind == "crescent_slash":
        import math

        # 两段同心圆弧相减 = 月牙：外刃 IRON、内缘 IRON_DARK，两端各留一粒火花。
        for step in range(30):
            angle = -1.15 + step / 29.0 * 2.6
            cos, sin = math.cos(angle), math.sin(angle)
            cv.set(16 + int(11 * cos), 17 - int(11 * sin), IRON)
            cv.set(16 + int(8 * cos), 17 - int(8 * sin), IRON_DARK)
        cv.set(5, 27, PAPER50)
        cv.set(26, 7, PAPER50)
    elif kind == "iron_palm":
        # 正掌（与回风掌同骨架：五指各留一缝，靠描边分开）：铁色掌面 + 两粒铁砂 + 朱色腕带。
        fill_ellipse(cv, 16, 20, 7, 6, IRON)
        for x in (10, 13, 16, 19, 22):
            cv.vline(x, 8, 17, IRON)
            cv.set(x, 8, IRON_DARK)
        cv.set(13, 20, IRON_DARK)
        cv.set(18, 22, IRON_DARK)
        cv.rect(12, 27, 20, 29, CINNABAR)
    elif kind == "leopard_step":
        # 豹掌印：掌垫在下、四趾各留缝排成弧；左侧两道 GOLD 横线是"跃"的来路。
        fill_ellipse(cv, 18, 21, 7, 6, GOLD500)
        for toe_x, toe_y in ((9, 15), (13, 9), (20, 7), (26, 12)):
            fill_ellipse(cv, toe_x, toe_y, 3, 3, GOLD500)
        for spot_x, spot_y in ((16, 20), (21, 22), (18, 25)):
            cv.set(spot_x, spot_y, CINNABAR900)
        cv.hline(2, 8, 22, GOLD300)
        cv.hline(1, 6, 27, GOLD300)
    cv.outline(INK)
    return cv


def hud_avatar_frame():
    """48×48：头像框（透明中心，覆盖在 48² 头像上）。"""
    cv = Canvas(48, 48)
    frame(cv, 0, 0, 47, 47, INK900, 1)
    frame(cv, 1, 1, 46, 46, WOOD500, 2)
    frame(cv, 3, 3, 44, 44, GOLD500, 1)
    for x, y in ((2, 2), (45, 2), (2, 45), (45, 45)):
        rivet(cv, x - (1 if x > 20 else 0), y - (1 if y > 20 else 0))
    return cv


def hud_chip():
    """8×8，9-slice 2,2,2,2：行分类色标（HUD 状态面板左缘的 6×16 竖条）。

    **近纯白的底色是这张切图的存在理由**：实体层 Image 的 `color` 是乘法染色
    （docs/68），白底 × 分类色 = 分类色本色，而描边与明暗用灰阶，染完自动变成
    同色系的深浅 ⇒ 六行各染一色仍读得出分组，但不再是一个纯色矩形。
    边距取 2px 而不是 common 那批的 6px：目标只有 6px 宽，6+6>6 会让整块塌成
    边框（`nine_slice_cells` 按比例压角，中心的分类色一格都不剩）。
    """
    cv = Canvas(8, 8)
    core = rgb("#EDEDED")   # ×0.93：比本色暗一档
    light = rgb("#FFFFFF")  # 左上受光 = 本色
    dark = rgb("#9E9E9E")   # ×0.62：右下背光
    cv.rect(0, 0, 7, 7, core)
    cv.hline(1, 6, 1, light)
    cv.vline(1, 1, 6, light)
    cv.hline(1, 6, 6, dark)
    cv.vline(6, 1, 6, dark)
    frame(cv, 0, 0, 7, 7, INK900, 1)
    return cv


def hud_accent():
    """16×3，9-slice 4,4,1,1：金色收口轨（面板顶沿）。

    行方向 1:1（目标高 3px 正好吃下上高光/中本色/下暗影三行，不会被竖向拉伸糊掉），
    列方向中间平色横拉、两端 4px 是带端帽的收头。金值取自 GOLD300/GOLD500/WOOD700
    （docs/35 §3 色板，与 panel_main 的金角码同源），所以预设里这一条不再靠染色。
    """
    cv = Canvas(16, 3)
    cv.rect(0, 0, 15, 2, GOLD500)
    cv.hline(0, 15, 0, GOLD300)
    cv.hline(0, 15, 2, WOOD700)
    for x in (0, 15):
        cv.vline(x, 0, 2, INK900)
    return cv


# ---------------------------------------------------------------------------
# backpack / 背包
# ---------------------------------------------------------------------------


def quality_frame(index):
    """40×40：品质内描边 + 左上 8×8 角标（中心透明）。"""
    cv = Canvas(40, 40)
    color = QUALITY[index]
    frame(cv, 0, 0, 39, 39, INK900, 1)
    frame(cv, 1, 1, 38, 38, color, 2)
    for y in range(0, 8):
        for x in range(0, 8 - y):
            cv.set(x, y, color)
    cv.hline(0, 7, 0, INK900)
    cv.vline(0, 0, 7, INK900)
    return cv


# ---------------------------------------------------------------------------
# dialogue / 对话
# ---------------------------------------------------------------------------


def portrait_frame():
    """128×128：对话头像框（透明中心 + 木框 + 金线）。"""
    cv = Canvas(128, 128)
    frame(cv, 0, 0, 127, 127, INK900, 1)
    frame(cv, 1, 1, 126, 126, WOOD700, 3)
    wood_band_h(cv, 4, 123, 4, 6)
    wood_band_h(cv, 4, 123, 121, 123)
    wood_band_v(cv, 4, 6, 4, 123)
    wood_band_v(cv, 121, 123, 4, 123)
    frame(cv, 7, 7, 120, 120, GOLD500, 1)
    return cv


def dialogue_continue():
    """16×16：继续箭头（运行时 500ms 闪烁）。"""
    cv = Canvas(16, 16)
    for y in range(6):
        cv.hline(2 + y, 13 - y, 4 + y, GOLD300 if y % 2 == 0 else GOLD500)
    cv.outline(INK900)
    return cv


def dialogue_marker():
    """8×8：选项引导符（§8.3 的金色 >）。"""
    cv = Canvas(8, 8)
    for step in range(4):
        cv.set(2 + step, 4 - step, GOLD300)
        cv.set(2 + step, 3 + step, GOLD300)
    cv.outline(INK900)
    return cv


# ---------------------------------------------------------------------------
# shop / 商店
# ---------------------------------------------------------------------------


def step_button(plus):
    """24×24：数量步进器 −/＋（§8.2，行内 24×24 按钮）。"""
    cv = Canvas(24, 24)
    frame(cv, 0, 0, 23, 23, INK900, 1)
    frame(cv, 1, 1, 22, 22, WOOD500, 1)
    cv.rect(2, 2, 21, 21, INK800)
    cv.hline(6, 17, 12, PAPER50)
    cv.hline(6, 17, 11, PAPER200)
    if plus:
        cv.vline(11, 7, 16, PAPER50)
        cv.vline(12, 7, 16, PAPER200)
    return cv


# ---------------------------------------------------------------------------
# quest / 任务
# ---------------------------------------------------------------------------


def quest_bullet(done):
    """12×12：目标状态点（未完成空心圈 / 完成实心 + 勾）。

    未完成态用 paper/500 圆环——一开始用 ink/800 实心挖空，在深色面板上几乎看不见。
    """
    cv = Canvas(12, 12)
    if done:
        fill_ellipse(cv, 5.5, 5.5, 5, 5, BAMBOO500)
        for step in range(3):
            cv.set(3 + step, 6 + step, PAPER50)
        for step in range(4):
            cv.set(6 + step, 5 + step, PAPER50)
        cv.outline(shade(BAMBOO500, 0.7))
    else:
        fill_ellipse(cv, 5.5, 5.5, 5, 5, PAPER500)
        fill_ellipse(cv, 5.5, 5.5, 3, 3, (0, 0, 0, 0))
        cv.outline(INK900)
    return cv


# ---------------------------------------------------------------------------
# skills / 武学
# ---------------------------------------------------------------------------


def skill_card():
    """128×64，9-slice 8,8,8,8：招式卡片底板（左侧金色强调条 + 底部分隔）。"""
    cv = Canvas(128, 64)
    frame(cv, 0, 0, 127, 63, INK900, 1)
    cv.rect(1, 1, 126, 62, INK800)
    cv.rect(1, 1, 3, 62, GOLD500)
    cv.vline(4, 1, 62, shade(GOLD500, 0.75))
    cv.hline(1, 126, 1, INK700)
    cv.hline(5, 126, 62, INK900)
    cv.hline(5, 126, 61, INK600)
    return cv


# ---------------------------------------------------------------------------
# build / 建筑放置
# ---------------------------------------------------------------------------


def ghost_cell(ok):
    """32×32：可放置/冲突的占位格（§8.6，20% 半透明填充 + 脚手架描边）。"""
    color = BAMBOO500 if ok else CINNABAR500
    cv = Canvas(32, 32)
    cv.rect(0, 0, 31, 31, (color[0], color[1], color[2], 64))
    frame(cv, 0, 0, 31, 31, (color[0], color[1], color[2], 190), 1)
    for corner_x, corner_y in ((2, 2), (29, 2), (2, 29), (29, 29)):
        cv.set(corner_x, corner_y, (color[0], color[1], color[2], 255))
    return cv


# ---------------------------------------------------------------------------
# save / 存档
# ---------------------------------------------------------------------------


def save_slot():
    """128×64，9-slice 8,8,8,8：存档槽底板。"""
    cv = Canvas(128, 64)
    frame(cv, 0, 0, 127, 63, INK900, 1)
    wood_band_h(cv, 1, 126, 1, 6)
    wood_band_h(cv, 1, 126, 57, 62)
    wood_band_v(cv, 1, 6, 1, 62)
    wood_band_v(cv, 121, 126, 1, 62)
    cv.rect(7, 7, 120, 56, INK800)
    frame(cv, 7, 7, 120, 56, INK600, 1)
    return cv


def save_shot_placeholder():
    """96×54：无截图时的占位块（§8.7，不写字）。"""
    cv = Canvas(96, 54)
    cv.rect(0, 0, 95, 53, INK800)
    frame(cv, 0, 0, 95, 53, INK600, 1)
    # 简笔"山 + 月"占位图形，避免出现文字
    for y in range(30, 48):
        half = (y - 30) * 2
        cv.hline(48 - half, 48 + half, y, INK700)
    fill_ellipse(cv, 70, 16, 6, 6, INK700)
    cv.hline(8, 88, 48, INK600)
    return cv


# ---------------------------------------------------------------------------
# settings / 设置
# ---------------------------------------------------------------------------


def toggle(on):
    """44×20，9-slice 8,8,8,8：开关（§8.8 的 [开/关] 行控件）。"""
    cv = Canvas(44, 20)
    frame(cv, 0, 0, 43, 19, INK900, 1)
    track = BAMBOO500 if on else INK700
    cv.rect(1, 1, 42, 18, track)
    cv.hline(1, 42, 1, shade(track, 1.2))
    cv.hline(1, 42, 18, shade(track, 0.75))
    knob_x0 = 24 if on else 2
    knob = GOLD300 if on else PAPER200
    cv.rect(knob_x0, 2, knob_x0 + 16, 17, knob)
    cv.vline(knob_x0, 2, 17, rgb("#FFFFFF"))
    cv.vline(knob_x0 + 16, 2, 17, shade(knob, 0.7))
    frame(cv, knob_x0, 2, knob_x0 + 16, 17, shade(knob, 0.55), 1)
    return cv


# ---------------------------------------------------------------------------
# TOUCH / 触屏控件（虚拟摇杆 + 虚拟按钮）
#
# 为什么要出切图而不是继续用 `ui.circle` 叠：圆片是"纯色圆盘 + 白边"，与全游戏的
# 墨底金线 + 朱砂点睛（docs/44）不是同一套语言，实机截图里触屏控件是唯一的
# "灰占位符岛"。同时它必须**半透明**（覆盖层控件要透出地面）。
#
# 尺寸与 `scripts/ui/layout.lua` 的 `M.TOUCH` 一一对应（1:1 出图，避免非整数缩放糊边）：
#   stick base 112 = (base_r 52 + 外圈 4) × 2、knob 42 = knob_r 21 × 2、
#   玩法按钮 112（交互，右下主键，与摇杆轨道同径）/68×4（攻击·背包·任务·暂停，等分绕在
#     主键左上象限）= buttons[i].size（68 < 72 ⇒ `touch_button` 的铜钉按既有守卫自动不画）、
#   dpad 64 / confirm 80 / close 44。
# 改 layout 的 size 要回来同步改这里；对不上的表现是"贴图被缩放"（软边），不会报错。
# ---------------------------------------------------------------------------


def with_a(color, value):
    return (color[0], color[1], color[2], value)


def over_px(cv, x, y, color):
    """source-over 叠色。`Canvas.set` 是直接覆盖，叠第二层时上层 alpha 会变成最终 alpha。"""
    dst = cv.get(x, y)
    src_a = color[3] / 255.0
    inv = 1.0 - src_a
    cv.set(x, y, (
        round(color[0] * src_a + dst[0] * inv),
        round(color[1] * src_a + dst[1] * inv),
        round(color[2] * src_a + dst[2] * inv),
        round(color[3] + dst[3] * inv),
    ))


def disc_over(cv, cx, cy, radius, color):
    for y in range(int(cy - radius) - 1, int(cy + radius) + 2):
        for x in range(int(cx - radius) - 1, int(cx + radius) + 2):
            if (x - cx) ** 2 + (y - cy) ** 2 <= radius * radius:
                over_px(cv, x, y, color)


def ring_over(cv, cx, cy, inner, outer, color):
    for y in range(int(cy - outer) - 1, int(cy + outer) + 2):
        for x in range(int(cx - outer) - 1, int(cx + outer) + 2):
            dist = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5
            if inner < dist <= outer:
                over_px(cv, x, y, color)


def rect_over(cv, x0, y0, x1, y1, color):
    for y in range(int(y0), int(y1) + 1):
        for x in range(int(x0), int(x1) + 1):
            over_px(cv, x, y, color)


def bevel_ring(cv, cx, cy, inner, outer, light, dark):
    """外圈斜面：法线朝左上提亮、朝右下压暗（全项目统一的"光源左上 45°"）。"""
    for y in range(int(cy - outer) - 1, int(cy + outer) + 2):
        for x in range(int(cx - outer) - 1, int(cx + outer) + 2):
            dist = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5
            if not (inner < dist <= outer) or dist <= 0:
                continue
            aim = (x - cx) / dist + (y - cy) / dist
            if aim < -0.3:
                over_px(cv, x, y, light)
            elif aim > 0.3:
                over_px(cv, x, y, dark)


def touch_button(size, accent, studs=True):
    """圆形按钮盘：墨漆面 + 彩色漆圈 + 描金 + 铜钉。

    面子必须压暗：`ui.text` 在两个宿主都固定白色（web-runtime.js drawUi 里
    `fillStyle = '#ffffff'`，没有颜色参数），浅色盘会让"交互/攻击"糊成一片。
    """
    cv = Canvas(size, size)
    cx = cy = (size - 1) / 2.0
    radius = size / 2.0 - 2                # 视觉半径留 1px + 描边，落在命中圆内
    band = max(3.0, size * 0.10)           # 彩圈厚度随直径走
    disc_over(cv, cx, cy, radius, with_a(INK800, 178))
    ring_over(cv, cx, cy, radius - band, radius, with_a(accent, 205))
    bevel_ring(cv, cx, cy, radius - band, radius,
               with_a(shade(accent, 1.45), 130), with_a(shade(accent, 0.55), 150))
    ring_over(cv, cx, cy, radius - band - 1.4, radius - band, with_a(GOLD500, 165))
    # 中心再压暗一档：白色标签的对比度全靠这一步
    disc_over(cv, cx, cy, max(4.0, radius - band - 3.0), with_a(INK900, 60))
    if studs and size >= 72:
        mid = radius - band / 2.0
        for dx, dy in ((0, -1), (1, 0), (0, 1), (-1, 0)):
            px, py = cx + dx * mid, cy + dy * mid
            disc_over(cv, px, py, 1.7, with_a(GOLD300, 235))
            disc_over(cv, px + 0.9, py + 0.9, 1.0, with_a(shade(GOLD500, 0.6), 210))
    cv.outline(with_a(INK900, 235))
    return cv


def touch_stick_base(size=112):
    """摇杆底盘：一圈**中空轨道** + 四向金刻度。

    中心必须是全透明的：`ui.image` 的多条记录在 `__rustlua_export_ui` 里走 Lua
    `pairs` 遍历（web-runtime.js:1935），**没有绘制顺序保证**。底盘与杆头一旦重叠，
    谁盖谁取决于哈希序，杆头可能某一帧被轨道埋掉。中心留孔 + Lua 侧把杆头的**视觉**
    行程夹在孔内（`layout.TOUCH.stick.hole_r - knob_r`），两者几何上不相交，
    顺序就无关了。这也是底盘画成"轨道"而不是"实心圆盘"的真正原因。
    """
    cv = Canvas(size, size)
    cx = cy = (size - 1) / 2.0
    radius = size / 2.0 - 2                # 54：与 layout 的 base_r 52 + 外圈 4 对齐
    hole = 45.0                            # = layout.TOUCH.stick.hole_r
    band = radius - hole                   # 9px 轨道
    ring_over(cv, cx, cy, hole, radius, with_a(INK800, 186))
    bevel_ring(cv, cx, cy, hole, radius, with_a(PAPER500, 70), with_a(INK900, 120))
    # 描金两道：外缘一道、孔缘一道，轨道就成了"铜圈"而不是灰饼
    ring_over(cv, cx, cy, radius - 1.4, radius, with_a(GOLD500, 175))
    ring_over(cv, cx, cy, hole, hole + 1.4, with_a(GOLD500, 150))
    # 四向刻度：跨轨道的短金条 + 指向圆心的朱砂点（静止时也要看得出这是方向杆）
    for dx, dy in ((0, -1), (1, 0), (0, 1), (-1, 0)):
        x0, x1 = sorted((cx + dx * hole, cx + dx * radius))
        y0, y1 = sorted((cy + dy * hole, cy + dy * radius))
        rect_over(cv, x0 - 1, y0 - 1, x1 + 1, y1 + 1, with_a(GOLD300, 215))
        px, py = cx + dx * (hole + band / 2.0), cy + dy * (hole + band / 2.0)
        disc_over(cv, px, py, 1.2, with_a(CINNABAR300, 225))
    cv.outline(with_a(INK900, 235))
    return cv


def touch_stick_knob(size=42):
    """摇杆头：墨漆圆头 + 金圈 + 左上高光（跟着拇指走，要最亮）。"""
    cv = Canvas(size, size)
    cx = cy = (size - 1) / 2.0
    radius = size / 2.0 - 2
    disc_over(cv, cx, cy, radius, with_a(INK700, 235))
    bevel_ring(cv, cx, cy, radius - 3.0, radius, with_a(PAPER200, 120), with_a(INK900, 150))
    ring_over(cv, cx, cy, radius - 2.4, radius - 0.6, with_a(GOLD500, 205))
    disc_over(cv, cx - radius * 0.28, cy - radius * 0.3, radius * 0.34, with_a(PAPER50, 95))
    disc_over(cv, cx, cy, 2.4, with_a(PAPER200, 225))
    cv.outline(with_a(INK900, 245))
    return cv


# ---------------------------------------------------------------------------
# 切图清单：模块 → [(文件名, 画布, 规格标签, 9-slice 边框)]
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# MENU / 主入口门面：中国武侠风（木牌按钮 + 卷轴 + 云纹 + 灯笼 + 印章）
#
# 设计取向（docs/44）：门面是"第一眼"，所以走**墨底金线 + 朱砂点睛**：
#   底色压暗（夜色 + 暗角）把视线收到中间，主视觉用木匾中文标题，
#   两侧挂红灯笼，四角云纹，进度/说明用卷轴纸面，按钮做成木牌 + 回纹金边。
# 所有元素都在 1280×720 参考分辨率下按 4px 网格对齐。
# ---------------------------------------------------------------------------


def meander_corner(cv, x0, y0, size, color, flip_x=False, flip_y=False):
    """回纹（雷纹）角饰：从外角 (x0,y0) 出发的**方形螺旋**，只走横竖直线。

    flip_x=True → 角在外框右侧；flip_y=True → 角在下侧。两种翻转天然给出镜像对称的螺旋。

    两版踩坑记录：
      1. 第一版把 x0 当"方框参照角"、竖笔画在横笔**内端** → 左角回纹朝里拐，看着是反的；
      2. 第二版改成三横三竖的平行格 → 四个角对称了，但成了"井字格"，不像回纹。
    现在用真正的方形螺旋：横↔竖交替、每两段缩一格，方向随 flip 镜像。
    """
    sx = -1 if flip_x else 1
    sy = -1 if flip_y else 1
    step = max(2, size // 4)
    segments = (
        (sx, 0, size),
        (0, sy, size),
        (-sx, 0, size - step),
        (0, -sy, size - step),
        (sx, 0, size - 2 * step),
        (0, sy, size - 2 * step),
    )
    x, y = x0, y0
    cv.set(x, y, color)
    for dx, dy, length in segments:
        for _ in range(length):
            x += dx
            y += dy
            cv.set(x, y, color)


def cloud_motif(cv, cx, cy, scale, color):
    """如意云纹：三个卷勾 + 一条拖尾，用像素弧线拼，风格与像素 UI 一致。"""
    r = scale
    fill_ellipse(cv, cx, cy, r, max(2, r // 2), color)
    fill_ellipse(cv, cx - r, cy + r // 3, max(2, r // 2), max(1, r // 3), color)
    fill_ellipse(cv, cx + r, cy + r // 3, max(2, r // 2), max(1, r // 3), color)
    for step in range(r * 2):
        cv.set(cx - r * 2 + step, cy + r, color)
        cv.set(cx + r * 2 - step, cy + r, color)


def menu_button_wuxia(hover=False, pressed=False):
    """木牌按钮 256×72（9-slice 18）：墨底 + 回纹金边 + 顶部受光。"""
    cv = Canvas(256, 72)
    board = INK800 if not hover else rgb("#333C46")
    if pressed:
        board = INK900
    cv.rect(0, 0, 255, 71, board)
    # 木纹底（横向细纹）
    for y in range(4, 68, 3):
        cv.hline(6, 249, y, mix(board, INK900, 0.35))
    # 金边（外粗内细）+ 四角回纹（传**外角**坐标：左 3 / 右 252 / 上 3 / 下 68）
    frame(cv, 0, 0, 255, 71, INK900, 1)
    frame(cv, 1, 1, 254, 70, GOLD500 if not hover else GOLD300, 2)
    frame(cv, 5, 5, 250, 66, mix(GOLD500, board, 0.45), 1)
    corner = GOLD300 if hover else GOLD500
    meander_corner(cv, 3, 3, 16, corner)
    meander_corner(cv, 252, 3, 16, corner, flip_x=True)
    meander_corner(cv, 3, 68, 16, corner, flip_y=True)
    meander_corner(cv, 252, 68, 16, corner, True, True)
    # 顶部受光 / 底部压暗
    cv.hline(2, 253, 2, shade(board, 1.5))
    cv.hline(2, 253, 69, INK900)
    if hover:  # 悬停：中央暖光
        for radius in range(30, 0, -6):
            cv.rect(128 - radius * 3, 36 - radius // 2, 128 + radius * 3, 36 + radius // 2,
                    mix(board, GOLD500, 0.10))
    if pressed:  # 按下：整体下沉一格
        cv.hline(2, 253, 3, INK900)
    return cv


def menu_panel_scroll():
    """卷轴面板 192×128（9-slice 20）：上下木轴 + 中间宣纸。"""
    cv = Canvas(192, 128)
    paper = rgb("#E6DCC4")
    cv.rect(0, 0, 191, 127, paper)
    for y in range(0, 128, 4):  # 纸纹
        cv.hline(4, 187, y, mix(paper, WOOD_MID, 0.08))
    for x in range(0, 192, 11):  # 纤维
        cv.vline(x, 4, 123, mix(paper, WOOD_MID, 0.05))
    frame(cv, 8, 8, 183, 119, mix(GOLD500, paper, 0.55), 1)
    # 木轴
    for y0, y1 in ((0, 12), (116, 127)):
        cv.rect(0, y0, 191, y1, WOOD_MID)
        cv.hline(0, 191, y0, WOOD_DARK)
        cv.hline(0, 191, y1, WOOD_DARK)
        for x in range(2, 190, 6):
            cv.vline(x, y0 + 2, y1 - 2, mix(WOOD_MID, WOOD_DARK, 0.4))
    # 轴头金环
    for x0 in (0, 176):
        cv.rect(x0, 1, x0 + 15, 11, GOLD500)
        cv.hline(x0, x0 + 15, 1, GOLD300)
        cv.rect(x0, 117, x0 + 15, 126, GOLD500)
        cv.hline(x0, x0 + 15, 126, shade(GOLD500, 0.7))
    frame(cv, 0, 0, 191, 127, INK900, 1)
    return cv


def menu_vignette():
    """暗角 + 云纹 + 金线画框（1280×720 透明叠加层）。"""
    cv = Canvas(1280, 720)
    # 四周压暗（越靠边越暗）
    for step in range(150):
        alpha = int(190 * (1 - step / 150.0) ** 1.6)
        if alpha <= 0:
            continue
        color = (0x0B, 0x0E, 0x14, alpha)
        cv.hline(step, 1279 - step, step, color)
        cv.hline(step, 1279 - step, 719 - step, color)
        cv.vline(step, step, 719 - step, color)
        cv.vline(1279 - step, step, 719 - step, color)
    # 顶部与底部的墨色压边
    for y in range(90):
        alpha = int(120 * (1 - y / 90.0))
        cv.hline(0, 1279, y, (0x0B, 0x0E, 0x14, alpha))
        cv.hline(0, 1279, 719 - y, (0x0B, 0x0E, 0x14, alpha))
    # 金线画框（内缩 22px，四角回纹；同样传外角坐标）
    frame(cv, 22, 22, 1257, 697, (0xC9, 0xA0, 0x5A, 150), 2)
    for x, y, fx, fy in ((22, 22, False, False), (1257, 22, True, False),
                         (22, 697, False, True), (1257, 697, True, True)):
        meander_corner(cv, x, y, 24, (0xE4, 0xC0, 0x78, 210), fx, fy)
    # 四角云纹（金色要够亮才读得出是云纹，压太暗会变成一坨灰）
    for cx, cy, scale in ((150, 150, 18), (1130, 150, 18), (150, 570, 18), (1130, 570, 18)):
        cloud_motif(cv, cx, cy, scale, (0xE4, 0xC0, 0x78, 120))
        cloud_motif(cv, cx, cy - 22, scale // 2, (0xF2, 0xDC, 0xA8, 90))
        cv.hline(cx - scale, cx + scale, cy + scale + 2, (0xC9, 0xA0, 0x5A, 70))
    return cv


def menu_ink_band():
    """墨色横幅 960×240：托住标题与菜单（中间实、两端化开）。"""
    cv = Canvas(960, 240)
    for y in range(240):
        t = abs(y - 120) / 120.0
        half = int(480 * (1 - t ** 2.2))
        for x in range(480 - half, 480 + half):
            edge = abs(x - 480) / max(1.0, half)
            alpha = int(150 * (1 - edge ** 3) * (1 - t ** 1.6))
            if alpha > 0:
                cv.set(x, y, (0x0B, 0x0E, 0x14, alpha))
    # 两端化开的笔触
    for y in range(20, 220, 7):
        length = 40 + (y * 7) % 70
        alpha = 60 + (y * 13) % 60
        cv.hline(0, length, y, (0x0B, 0x0E, 0x14, alpha))
        cv.hline(959 - length, 959, y, (0x0B, 0x0E, 0x14, alpha))
    return cv


def menu_ornament_corner(flip_x=False, flip_y=False):
    """回纹角饰 64×64：四向变体（外角 + 云纹都跟着镜像）。

    只出一种朝向会立刻出问题：门面里用同一张图左右各摆一个，看着就是"左右不对称"。
    所以四个方向各出一张，场景按"外角朝哪边"选：
      ui_menu_ornament_corner      外角在左上（臂向右下）
      _tr                          外角在右上（臂向左下）
      _bl                          外角在左下（臂向右上）
      _br                          外角在右下（臂向左上）
    """
    cv = Canvas(64, 64)
    outer_x = 54 if flip_x else 2
    outer_y = 54 if flip_y else 2
    meander_corner(cv, outer_x, outer_y, 52, GOLD500, flip_x, flip_y)
    meander_corner(cv, outer_x + (-8 if flip_x else 8), outer_y + (-8 if flip_y else 8),
                   34, GOLD300, flip_x, flip_y)
    cloud_x = 62 - 46 if flip_x else 46
    cloud_y = 62 - 46 if flip_y else 46
    cloud_motif(cv, cloud_x, cloud_y, 8, (0xC9, 0xA0, 0x5A, 190))
    return cv


def menu_divider_bamboo():
    """竹节分隔线 320×18。"""
    cv = Canvas(320, 18)
    cv.rect(0, 8, 319, 12, BAMBOO_DARK)
    cv.hline(0, 319, 8, BAMBOO)
    cv.hline(0, 319, 12, shade(BAMBOO_DARK, 0.8))
    for x in (0, 106, 212, 316):  # 竹节
        cv.rect(x, 6, x + 3, 14, BAMBOO_DARK)
        cv.vline(x + 1, 6, 14, BAMBOO_LIGHT)
    for x, flip in ((60, False), (170, True), (270, False)):  # 竹叶
        for step in range(9):
            y = 6 - step // 2
            cv.set(x + (step if flip else -step), y, BAMBOO_LIGHT if step % 2 else BAMBOO)
    return cv


def menu_lantern():
    """红灯笼 72×128：暖光 + 灯身 + 金盖 + 流苏。"""
    cv = Canvas(72, 128)
    # 暖光halo
    for radius in range(30, 0, -2):
        alpha = int(70 * (1 - radius / 30.0))
        fill_ellipse(cv, 36, 54, radius + 6, radius + 10, (0xE0, 0x7B, 0x3A, alpha))
    cv.vline(36, 0, 18, WOOD_DARK)          # 挂绳
    cv.rect(28, 16, 44, 22, GOLD500)        # 上盖
    cv.hline(28, 44, 16, GOLD300)
    cv.rect(20, 22, 51, 88, CINNABAR500)    # 灯身
    for x in range(22, 51, 4):              # 灯骨
        cv.vline(x, 23, 87, mix(CINNABAR500, INK900, 0.35))
    cv.hline(20, 51, 23, CINNABAR300)
    cv.hline(20, 51, 87, CINNABAR900)
    fill_ellipse(cv, 36, 55, 12, 26, mix(CINNABAR500, (255, 240, 200, 255), 0.25))
    cv.rect(28, 88, 44, 94, GOLD500)        # 下盖
    for x in range(31, 42, 3):              # 流苏
        cv.vline(x, 95, 116 + (x % 5), CINNABAR500)
    cv.rect(30, 95, 42, 99, GOLD500)
    return cv


def menu_seal():
    """朱砂印章 72×72：内嵌「江湖」（中文走 cjk_text 栅格化）。"""
    cv = Canvas(72, 72)
    cv.rect(2, 2, 69, 69, CINNABAR500)
    cv.rect(4, 4, 67, 67, CINNABAR900)
    cv.rect(7, 7, 64, 64, CINNABAR500)
    glyph = text_canvas("江湖", size=26, mode="pixel", color=(0xF6, 0xE6, 0xCE, 255))
    if glyph.w > 1:
        cv.blit(glyph, 36 - glyph.w // 2, 36 - glyph.h // 2)
    # 做旧缺口（固定伪随机，保证可复现）
    rnd = hash_stream(2971)
    for _ in range(26):
        x = 4 + rnd(64)
        y = 4 + rnd(64)
        cv.set(x, y, (0x00, 0x00, 0x00, 0))
    frame(cv, 2, 2, 69, 69, (0x8F, 0x2E, 0x1F, 255), 1)
    return cv


def slice_table():
    # 门面印章要先有中文字形（GDI+ 批量渲染一次）
    prewarm([("江湖", 26, "pixel", True)])
    table = {
        "menu": [
            ("ui_menu_vignette", menu_vignette(), "1280x720 暗角+云纹+金框", None),
            ("ui_menu_ink_band", menu_ink_band(), "960x240 墨色横幅", None),
            ("ui_menu_btn_wuxia_9s", menu_button_wuxia(False), "256x72 9S 18 木牌按钮", (18, 18, 18, 18)),
            ("ui_menu_btn_wuxia_hover_9s", menu_button_wuxia(True), "256x72 9S 18 悬停", (18, 18, 18, 18)),
            ("ui_menu_btn_wuxia_pressed_9s", menu_button_wuxia(False, True), "256x72 9S 18 按下", (18, 18, 18, 18)),
            ("ui_menu_panel_scroll_9s", menu_panel_scroll(), "192x128 9S 20 卷轴", (20, 20, 20, 20)),
            ("ui_menu_ornament_corner", menu_ornament_corner(), "64x64 回纹角饰 左上", None),
            ("ui_menu_ornament_corner_tr", menu_ornament_corner(True, False), "64x64 回纹角饰 右上", None),
            ("ui_menu_ornament_corner_bl", menu_ornament_corner(False, True), "64x64 回纹角饰 左下", None),
            ("ui_menu_ornament_corner_br", menu_ornament_corner(True, True), "64x64 回纹角饰 右下", None),
            ("ui_menu_divider_bamboo", menu_divider_bamboo(), "320x18 竹节分隔", None),
            ("ui_menu_lantern", menu_lantern(), "72x128 红灯笼", None),
            ("ui_menu_seal_jianghu", menu_seal(), "72x72 印章 江湖", None),
        ],
        "common": [
            ("ui_panel_main_9s", panel_main(), "96x96 9S 16", (16, 16, 16, 16)),
            ("ui_panel_paper_9s", panel_paper(), "96x96 9S 16", (16, 16, 16, 16)),
            ("ui_panel_confirm_9s", panel_confirm(), "64x64 9S 12", (12, 12, 12, 12)),
            ("ui_btn_primary_9s", button_primary(False), "64x48 9S 8", (8, 8, 8, 8)),
            ("ui_btn_primary_pressed_9s", button_primary(True), "64x48 9S 8", (8, 8, 8, 8)),
            ("ui_btn_secondary_9s", button_secondary(), "64x48 9S 8", (8, 8, 8, 8)),
            ("ui_btn_danger_9s", button_danger(), "64x48 9S 8", (8, 8, 8, 8)),
            ("ui_slot_9s", slot(False), "48x48 9S 6", (6, 6, 6, 6)),
            ("ui_slot_selected_9s", slot(True), "48x48 9S 6", (6, 6, 6, 6)),
            ("ui_input_9s", input_box(), "48x28 9S 6", (6, 6, 6, 6)),
            ("ui_tab_on_9s", tab(True), "64x28 9S 6", (6, 6, 6, 6)),
            ("ui_tab_off_9s", tab(False), "64x28 9S 6", (6, 6, 6, 6)),
            ("ui_key_hint_9s", key_hint(), "24x24 9S 6", (6, 6, 6, 6)),
            ("ui_checkbox_off", checkbox(False), "16x16", None),
            ("ui_checkbox_on", checkbox(True), "16x16", None),
            ("ui_slider_track", slider_track(), "4x12", None),
            ("ui_slider_handle", slider_handle(), "12x12", None),
            ("ui_dropdown_arrow", dropdown_arrow(), "8x8", None),
            ("ui_scroll_arrow_up", scroll_arrow("up"), "8x8", None),
            ("ui_scroll_arrow_down", scroll_arrow("down"), "8x8", None),
            ("ui_btn_close", close_button(False), "16x16", None),
            ("ui_btn_close_hover", close_button(True), "16x16", None),
            ("ui_divider", divider(), "4x4", None),
            ("ui_icon_seal", icon_seal(), "24x24", None),
        ],
        "hud": [
            ("ui_bar_frame_9s", bar_frame(), "48x16 9S 4", (4, 4, 4, 4)),
            ("ui_bar_fill_hp", bar_fill(CINNABAR500, CINNABAR300), "24x8 9S 4,0", (4, 0, 4, 0)),
            ("ui_bar_fill_stamina", bar_fill(BAMBOO500, BAMBOO300), "24x8 9S 4,0", (4, 0, 4, 0)),
            ("ui_bar_fill_neili", bar_fill(INDIGO500, INDIGO300), "24x8 9S 4,0", (4, 0, 4, 0)),
            ("ui_icon_hp", icon_hp(), "20x20", None),
            ("ui_icon_stamina", icon_stamina(), "20x20", None),
            ("ui_icon_neili", icon_neili(), "20x20", None),
            ("ui_icon_coin", icon_coin(), "16x16", None),
            ("ui_weather_sun", weather_sun(), "32x32", None),
            ("ui_weather_cloud", weather_cloud(), "32x32", None),
            ("ui_weather_rain", weather_rain(), "32x32", None),
            ("ui_status_poison", status_icon("poison"), "16x16", None),
            ("ui_status_buff", status_icon("buff"), "16x16", None),
            # 批次 4：首发状态图标补齐（docs/35 §5）
            ("ui_status_bleed", status_icon("bleed"), "16x16 流血", None),
            ("ui_status_debuff", status_icon("debuff"), "16x16 减益", None),
            ("ui_status_sleep", status_icon("sleep"), "16x16 睡眠", None),
            ("ui_status_stealth", status_icon("stealth"), "16x16 隐身", None),
            ("ui_status_weather", status_icon("weather"), "16x16 天气", None),
            ("ui_status_festival", status_icon("festival"), "16x16 节日", None),
            ("ui_hud_avatar_frame", hud_avatar_frame(), "48x48", None),
            ("ui_hud_chip_9s", hud_chip(), "8x8 9S 2 可染色分类色标", (2, 2, 2, 2)),
            ("ui_hud_accent_9s", hud_accent(), "16x3 9S 4,4,1,1 金色收口轨", (4, 4, 1, 1)),
        ],
        "backpack": [
            ("ui_quality_frame_0", quality_frame(0), "40x40 凡品", None),
            ("ui_quality_frame_1", quality_frame(1), "40x40 良品", None),
            ("ui_quality_frame_2", quality_frame(2), "40x40 上品", None),
            ("ui_quality_frame_3", quality_frame(3), "40x40 珍品", None),
            ("ui_quality_frame_4", quality_frame(4), "40x40 传世", None),
        ],
        "dialogue": [
            ("ui_portrait_frame", portrait_frame(), "128x128", None),
            ("ui_dialogue_continue", dialogue_continue(), "16x16", None),
            ("ui_dialogue_marker", dialogue_marker(), "8x8", None),
        ],
        "shop": [
            ("ui_btn_step_minus", step_button(False), "24x24", None),
            ("ui_btn_step_plus", step_button(True), "24x24", None),
        ],
        "quest": [
            ("ui_quest_bullet_todo", quest_bullet(False), "12x12", None),
            ("ui_quest_bullet_done", quest_bullet(True), "12x12", None),
        ],
        "skills": [
            ("ui_skill_card_9s", skill_card(), "128x64 9S 8", (8, 8, 8, 8)),
            # 批次 4：五式招式图标（docs/30 §8.2）；后四式跟 `data/skills.lua` 的现阵容对齐
            # （`palm_gust`/`crescent_slash`/`iron_palm`/`leopard_step`，HUD 卡底要显示它们）。
            ("ui_skill_basic_blade", skill_icon("basic_blade"), "32x32 基础刀法", None),
            ("ui_skill_chase_wind", skill_icon("chase_wind"), "32x32 追风步", None),
            ("ui_skill_guard_heart", skill_icon("guard_heart"), "32x32 守心诀", None),
            ("ui_skill_return_palm", skill_icon("return_palm"), "32x32 回风掌", None),
            ("ui_skill_breath_qi", skill_icon("breath_qi"), "32x32 养气诀", None),
            ("ui_skill_palm_gust", skill_icon("palm_gust"), "32x32 千风掌", None),
            ("ui_skill_crescent_slash", skill_icon("crescent_slash"), "32x32 弦月斩", None),
            ("ui_skill_iron_palm", skill_icon("iron_palm"), "32x32 铁砂掌", None),
            ("ui_skill_leopard_step", skill_icon("leopard_step"), "32x32 豹跃", None),
        ],
        "build": [
            ("ui_build_ghost_ok", ghost_cell(True), "32x32", None),
            ("ui_build_ghost_bad", ghost_cell(False), "32x32", None),
        ],
        "save": [
            ("ui_save_slot_9s", save_slot(), "128x64 9S 8", (8, 8, 8, 8)),
            ("ui_save_shot_placeholder", save_shot_placeholder(), "96x54", None),
        ],
        "settings": [
            ("ui_toggle_on_9s", toggle(True), "44x20 9S 8", (8, 8, 8, 8)),
            ("ui_toggle_off_9s", toggle(False), "44x20 9S 8", (8, 8, 8, 8)),
        ],
        # 触屏控件：直径一律等于 `layout.TOUCH` 里的 size（1:1 出图，见本节头注释）
        "touch": [
            ("ui_touch_stick_base", touch_stick_base(), "112x112 摇杆轨道(中空)", None),
            ("ui_touch_stick_knob", touch_stick_knob(), "42x42 摇杆头", None),
            ("ui_touch_btn_interact", touch_button(112, BAMBOO500), "112x112 交互(右下主键,与摇杆同径)", None),
            ("ui_touch_btn_attack", touch_button(68, CINNABAR500), "68x68 攻击", None),
            ("ui_touch_btn_bag", touch_button(68, INDIGO500), "68x68 背包", None),
            ("ui_touch_btn_quest", touch_button(68, TOUCH_VIOLET), "68x68 任务", None),
            ("ui_touch_btn_pause", touch_button(68, CLOUD_DARK), "68x68 暂停", None),
            ("ui_touch_dpad", touch_button(64, CLOUD_DARK), "64x64 面板十字键", None),
            ("ui_touch_confirm", touch_button(80, BAMBOO500), "80x80 面板确定", None),
            ("ui_touch_close", touch_button(44, CINNABAR500, studs=False), "44x44 面板关闭", None),
            # 动作盘（M34-06 批 2）：9 个生活/建造动作共用一张盘，标签各自不同（睡觉/建造…），
            # 所以这里只需要"第 6 种语义色"而不是 9 张图。漆色取木作棕（建造/放置/升级是工造系），
            # 与簇内已有的竹青/朱砂/靛青/紫/云灰不撞；直径必须 = layout.TOUCH.action.size（68）。
            ("ui_touch_btn_action", touch_button(68, WOOD_MID), "68x68 动作盘（睡觉/建造/放置…）", None),
        ],
    }
    return table


def all_items():
    """展平为 [(module, name, canvas, spec, nine_slice)]，模块顺序固定。"""
    table = slice_table()
    items = []
    for module, _ in MODULES:
        for name, canvas, spec, nine in table[module]:
            items.append((module, name, canvas, spec, nine))
    return items


def rel_path(module, name):
    return f"{module}/{name}.png"


def asset_path(module, name):
    return f"assets/art/ui/{module}/{name}.png"


# ---------------------------------------------------------------------------
# 预览一：全切图（按模块分组）
# ---------------------------------------------------------------------------


def build_controls_preview(items, scale=1):
    """控件总览（1×）：78 张切图用 2× 排版会到 4000×5800 ≈ 2300 万像素，
    超过引擎读图的显存/内存上限（导入会报 Memory limit exceeded），所以固定 1×。"""
    cols = 6
    pad = 12
    label_h = 22
    group_gap = 14
    # 满屏/大尺寸底衬（门面的暗角、墨色横幅等）不进"控件总览"：
    # 一是它们本来就不是控件，二是把 1280×720 放进缩放排版会让成图巨大，
    # 引擎侧读图直接 "Memory limit exceeded"（导入 failed=1 就是这么来的）。
    big = [item for item in items if item[2].w > 480 or item[2].h > 320]
    items = [item for item in items if item not in big]
    if big:
        print(f"  控件总览跳过 {len(big)} 张大幅底衬：" +
              "、".join(item[1] for item in big))
    cell_w = 0
    cell_h = 0
    for _, _, canvas, _, _ in items:
        cell_w = max(cell_w, canvas.w * scale + pad * 2)
        cell_h = max(cell_h, canvas.h * scale + pad * 2 + label_h)
    width = cell_w * cols + 8
    # 先量高度
    height = 24
    for module, title in MODULES:
        count = sum(1 for item in items if item[0] == module)
        rows = (count + cols - 1) // cols
        height += 16 + rows * cell_h + group_gap
    cv = Canvas(width, height)
    cv.rect(0, 0, width - 1, height - 1, rgb("#14171A"))
    draw_text(cv, 6, 6, "JIANGHU UI KIT / BY MODULE / X2", FONT_LABEL, 1)
    y = 24
    for module, title in MODULES:
        group = [item for item in items if item[0] == module]
        draw_text(cv, 6, y, title, GOLD300, 1)
        cv.hline(6, width - 8, y + 10, INK600)
        y += 16
        for index, (_, name, canvas, spec, nine) in enumerate(group):
            row, col = divmod(index, cols)
            x0 = 4 + col * cell_w
            y0 = y + row * cell_h
            short = name.replace("ui_", "")
            draw_text(cv, x0 + pad, y0 + 2, short[:26], FONT_DIM, 1)
            draw_text(cv, x0 + pad, y0 + 10, spec[:26], PAPER500, 1)
            top = y0 + label_h
            for by in range(0, canvas.h * scale, 8):
                for bx in range(0, canvas.w * scale, 8):
                    tone = rgb("#262C33") if ((bx // 8 + by // 8) % 2 == 0) else rgb("#31383F")
                    cv.rect(x0 + pad + bx, top + by, x0 + pad + bx + 7, top + by + 7, tone)
            cv.blit(canvas.scaled(scale), x0 + pad, top)
        rows = (len(group) + cols - 1) // cols
        y += rows * cell_h + group_gap
    return cv


# ---------------------------------------------------------------------------
# 预览二：按 §7/§8 布局拼效果图
# ---------------------------------------------------------------------------


def draw_texture(canvas, texture, x, y, w=None, h=None, nine_slice=None):
    w = texture.w if w is None else w
    h = texture.h if h is None else h
    if nine_slice:
        draw_nine_slice(canvas, texture, x, y, w, h, nine_slice)
    else:
        from pixel_kit import blit_scaled

        blit_scaled(canvas, texture, 0, 0, texture.w, texture.h, x, y, w, h)


def build_mock_preview(items):
    tex = {name: canvas for _, name, canvas, _, _ in items}
    nine = {name: nine for _, name, _, _, nine in items}
    width = 1280
    hud_h = 720
    gap = 16
    height = hud_h + gap + 360 + gap + 160 + gap + 440 + gap + 200 + 16
    cv = Canvas(width, height)
    cv.rect(0, 0, width - 1, height - 1, rgb("#0B0E11"))

    def text(x, y, s, color=PAPER50, scale=1):
        draw_text(cv, x, y, s, color, scale)

    # --- A. HUD（背景用真实场景预览）---
    if os.path.exists(SCENE_PREVIEW):
        draw_texture(cv, load_canvas(SCENE_PREVIEW), 0, 0, width, hud_h)
    else:
        cv.rect(0, 0, width - 1, hud_h - 1, rgb("#26331F"))
    panel = tex["ui_panel_main_9s"]
    panel9 = nine["ui_panel_main_9s"]
    draw_texture(cv, panel, 16, 16, 256, 100, panel9)
    draw_texture(cv, tex["ui_slot_9s"], 26, 30, 48, 48, nine["ui_slot_9s"])
    draw_texture(cv, tex["ui_hud_avatar_frame"], 26, 30, 48, 48)
    text(32, 50, "AVATAR", PAPER500, 1)
    for index, (icon, fill, label, ratio) in enumerate((
        ("ui_icon_hp", "ui_bar_fill_hp", "HP", 0.72),
        ("ui_icon_stamina", "ui_bar_fill_stamina", "SP", 0.54),
        ("ui_icon_neili", "ui_bar_fill_neili", "MP", 0.33),
    )):
        y = 30 + index * 22
        draw_texture(cv, tex[icon], 84, y, 20, 20)
        fill_w = int(144 * ratio)
        if fill_w > 0:
            draw_texture(cv, tex[fill], 110, y - 1, fill_w, 8, nine[fill])
        draw_texture(cv, tex["ui_bar_frame_9s"], 110, y - 5, 150, 16, nine["ui_bar_frame_9s"])
        text(112, y + 14, label, PAPER200, 1)
    draw_texture(cv, panel, 1040, 16, 224, 40, panel9)
    text(1052, 28, "SPRING DAY 3  08:24", PAPER50, 1)
    draw_texture(cv, tex["ui_weather_sun"], 1228, 20, 32, 32)
    draw_texture(cv, panel, 1104, 62, 160, 32, panel9)
    draw_texture(cv, tex["ui_icon_coin"], 1112, 70, 16, 16)
    text(1136, 74, "1250", GOLD300, 2)
    draw_texture(cv, panel, 1024, 104, 240, 108, panel9)
    text(1036, 114, "QUESTS", GOLD300, 1)
    draw_texture(cv, tex["ui_quest_bullet_todo"], 1036, 132, 12, 12)
    text(1054, 134, "HERB ORDER 0/3", PAPER200, 1)
    draw_texture(cv, tex["ui_quest_bullet_done"], 1036, 156, 12, 12)
    text(1054, 158, "FIX THE MANOR 1/4", BAMBOO300, 1)
    draw_texture(cv, tex["ui_quest_bullet_todo"], 1036, 180, 12, 12)
    text(1054, 182, "MEET GU CHANGFENG", PAPER200, 1)
    quick_x = (width - 352) // 2
    quick_y = hud_h - 64
    for index in range(8):
        x = quick_x + index * 44
        draw_texture(cv, tex["ui_slot_selected_9s" if index == 0 else "ui_slot_9s"], x, quick_y, 40, 40,
                     nine["ui_slot_9s"])
        text(x + 4, quick_y + 30, str(index + 1), GOLD300 if index == 0 else PAPER500, 1)
    prompt_x, prompt_y = 520, hud_h - 140
    draw_texture(cv, panel, prompt_x, prompt_y, 240, 40, panel9)
    draw_texture(cv, tex["ui_key_hint_9s"], prompt_x + 10, prompt_y + 8, 24, 24, nine["ui_key_hint_9s"])
    text(prompt_x + 21, prompt_y + 16, "E", INK900, 1)
    text(prompt_x + 46, prompt_y + 16, "TALK TO SHEN QINGLUO", PAPER50, 1)

    # --- B. 背包 + 二次确认 ---
    bx, by = 40, hud_h + gap
    draw_texture(cv, panel, bx, by, 480, 360, panel9)
    draw_texture(cv, tex["ui_icon_seal"], bx + 16, by + 12, 24, 24)
    text(bx + 48, by + 20, "BACKPACK", GOLD300, 2)
    draw_texture(cv, tex["ui_btn_close"], bx + 448, by + 16, 16, 16)
    draw_texture(cv, tex["ui_tab_on_9s"], bx + 16, by + 44, 96, 28, nine["ui_tab_on_9s"])
    text(bx + 44, by + 54, "ITEMS", GOLD300, 1)
    draw_texture(cv, tex["ui_tab_off_9s"], bx + 116, by + 44, 96, 28, nine["ui_tab_off_9s"])
    text(bx + 132, by + 54, "TOOLS", PAPER500, 1)
    for row in range(4):
        for col in range(5):
            draw_texture(cv, tex["ui_slot_selected_9s" if (row, col) == (0, 0) else "ui_slot_9s"],
                         bx + 24 + col * 44, by + 84 + row * 44, 40, 40, nine["ui_slot_9s"])
    dx = bx + 264
    draw_texture(cv, tex["ui_slot_9s"], dx, by + 56, 48, 48, nine["ui_slot_9s"])
    draw_texture(cv, tex["ui_quality_frame_2"], dx - 4, by + 52, 40, 40)
    text(dx + 60, by + 60, "RICE", PAPER50, 2)
    text(dx + 60, by + 80, "GOOD / CROP", BAMBOO300, 1)
    text(dx + 4, by + 120, "PLUMP GRAIN FROM THE", PAPER200, 1)
    text(dx + 4, by + 132, "MANOR PADDY.", PAPER200, 1)
    text(dx + 4, by + 156, "COUNT 12/99", GOLD300, 1)
    draw_texture(cv, tex["ui_btn_primary_9s"], dx + 4, by + 180, 116, 32, nine["ui_btn_primary_9s"])
    text(dx + 42, by + 190, "USE", PAPER50, 1)
    draw_texture(cv, tex["ui_btn_danger_9s"], dx + 128, by + 180, 116, 32, nine["ui_btn_danger_9s"])
    text(dx + 156, by + 190, "DROP", CINNABAR300, 1)
    text(dx + 4, by + 232, "17/20 SLOTS", PAPER500, 1)
    cx, cy = bx + 560, by + 40
    draw_texture(cv, tex["ui_panel_confirm_9s"], cx, cy, 260, 140, nine["ui_panel_confirm_9s"])
    text(cx + 76, cy + 24, "DROP ITEM ?", PAPER50, 2)
    text(cx + 24, cy + 56, "THIS CANNOT BE UNDONE.", PAPER200, 1)
    draw_texture(cv, tex["ui_btn_primary_9s"], cx + 24, cy + 88, 100, 32, nine["ui_btn_primary_9s"])
    text(cx + 54, cy + 98, "YES", PAPER50, 1)
    draw_texture(cv, tex["ui_btn_secondary_9s"], cx + 136, cy + 88, 100, 32, nine["ui_btn_secondary_9s"])
    text(cx + 164, cy + 98, "NO", PAPER50, 1)

    # --- C. 对话 ---
    dx, dy = 160, hud_h + gap + 360 + gap
    draw_texture(cv, tex["ui_panel_paper_9s"], dx, dy, 960, 160, nine["ui_panel_paper_9s"])
    draw_texture(cv, tex["ui_slot_9s"], dx + 16, dy + 16, 128, 128, nine["ui_slot_9s"])
    draw_texture(cv, tex["ui_portrait_frame"], dx + 16, dy + 16, 128, 128)
    text(dx + 56, dy + 76, "PORTRAIT", INK900, 1)
    text(dx + 164, dy + 16, "SHEN QINGLUO", INK900, 2)
    text(dx + 164, dy + 48, "THE HERBS YOU BROUGHT ARE FRESH.", INK900, 1)
    text(dx + 164, dy + 64, "TAKE THIS RECIPE, AND MIND THE RAIN.", INK900, 1)
    draw_texture(cv, tex["ui_dialogue_marker"], dx + 164, dy + 92, 8, 8)
    text(dx + 178, dy + 92, "1 ASK ABOUT LINGZHI", GOLD500, 1)
    draw_texture(cv, tex["ui_dialogue_marker"], dx + 164, dy + 110, 8, 8)
    text(dx + 178, dy + 110, "2 SAY GOODBYE", GOLD500, 1)
    draw_texture(cv, tex["ui_dialogue_continue"], dx + 924, dy + 134, 16, 16)

    # --- D. 商店 ---
    sx, sy = 40, hud_h + gap + 360 + gap + 160 + gap
    draw_texture(cv, panel, sx, sy, 640, 440, panel9)
    draw_texture(cv, tex["ui_icon_seal"], sx + 16, sy + 12, 24, 24)
    text(sx + 48, sy + 20, "BLACKSMITH", GOLD300, 2)
    draw_texture(cv, tex["ui_tab_on_9s"], sx + 240, sy + 16, 80, 28, nine["ui_tab_on_9s"])
    text(sx + 262, sy + 26, "BUY", GOLD300, 1)
    draw_texture(cv, tex["ui_tab_off_9s"], sx + 328, sy + 16, 80, 28, nine["ui_tab_off_9s"])
    text(sx + 350, sy + 26, "SELL", PAPER500, 1)
    draw_texture(cv, tex["ui_icon_coin"], sx + 520, sy + 22, 16, 16)
    text(sx + 542, sy + 26, "3000", GOLD300, 1)
    for row in range(5):
        row_y = sy + 60 + row * 44
        draw_texture(cv, tex["ui_slot_9s"], sx + 16, row_y, 40, 40, nine["ui_slot_9s"])
        text(sx + 68, row_y + 16, f"ITEM {row + 1}", PAPER50, 1)
        text(sx + 300, row_y + 16, f"{8 + row * 3}", GOLD300, 1)
        draw_texture(cv, tex["ui_btn_step_minus"], sx + 380, row_y + 8, 24, 24)
        text(sx + 418, row_y + 16, "1", PAPER50, 1)
        draw_texture(cv, tex["ui_btn_step_plus"], sx + 436, row_y + 8, 24, 24)
    draw_texture(cv, tex["ui_skill_card_9s"], sx + 380, sy + 320, 240, 44, nine["ui_skill_card_9s"])
    text(sx + 392, sy + 330, "BASKET", GOLD300, 1)
    text(sx + 392, sy + 346, "TOTAL 1250", PAPER50, 1)
    draw_texture(cv, tex["ui_btn_primary_9s"], sx + 392, sy + 384, 200, 32, nine["ui_btn_primary_9s"])
    text(sx + 460, sy + 394, "CONFIRM", PAPER50, 1)

    # --- E. 更多模块 + 9-slice 缩放检查 ---
    zy = sy + 440 + gap
    text(40, zy, "SAVE / SETTINGS / BUILD + NINE-SLICE SCALE 0.75X 1.0X 1.5X", GOLD300, 1)
    y = zy + 18
    draw_texture(cv, tex["ui_save_slot_9s"], 40, y, 260, 88, nine["ui_save_slot_9s"])
    draw_texture(cv, tex["ui_save_shot_placeholder"], 52, y + 16, 96, 54)
    text(160, y + 22, "MANOR DAY 3", PAPER50, 1)
    text(160, y + 40, "2026-03-01 08:24", PAPER200, 1)
    draw_texture(cv, tex["ui_btn_primary_9s"], 160, y + 56, 60, 24, nine["ui_btn_primary_9s"])
    text(176, y + 62, "SAVE", PAPER50, 1)
    draw_texture(cv, tex["ui_toggle_on_9s"], 320, y + 16, 44, 20, nine["ui_toggle_on_9s"])
    text(374, y + 20, "FULLSCREEN", PAPER200, 1)
    draw_texture(cv, tex["ui_toggle_off_9s"], 320, y + 44, 44, 20, nine["ui_toggle_off_9s"])
    text(374, y + 48, "SCREEN SHAKE", PAPER200, 1)
    draw_texture(cv, tex["ui_slider_track"], 320, y + 74, 120, 4)
    draw_texture(cv, tex["ui_slider_handle"], 370, y + 66, 12, 12)
    text(452, y + 70, "VOLUME", PAPER200, 1)
    draw_texture(cv, tex["ui_build_ghost_ok"], 560, y + 16, 32, 32)
    draw_texture(cv, tex["ui_build_ghost_ok"], 592, y + 16, 32, 32)
    draw_texture(cv, tex["ui_build_ghost_bad"], 624, y + 16, 32, 32)
    text(560, y + 54, "BUILD PLACEMENT", PAPER200, 1)
    for index, factor in enumerate((0.75, 1.0, 1.5)):
        box_w = int(200 * factor)
        box_h = int(120 * factor)
        x = 720 + index * 190
        yy = zy + 18
        draw_texture(cv, panel, x, yy, box_w, box_h, panel9)
        text(x + 12, yy + 12, f"{factor:g}X", PAPER200, 1)
        draw_texture(cv, tex["ui_btn_primary_9s"], x + 12, yy + 34, int(120 * factor), int(32 * factor),
                     nine["ui_btn_primary_9s"])
        draw_texture(cv, tex["ui_slot_9s"], x + int(150 * factor), yy + 34, int(40 * factor),
                     int(40 * factor), nine["ui_slot_9s"])
    return cv


# ---------------------------------------------------------------------------
# manifest / 输出 / 校验
# ---------------------------------------------------------------------------


def build_manifest(items):
    modules = {}
    for module, title in MODULES:
        group = [item for item in items if item[0] == module]
        modules[module] = {
            "title": title,
            "files": [rel_path(module, name) for _, name, _, _, _ in group],
            "uses_common": [rel_path("common", name) for name in MODULE_USES.get(module, [])],
        }
    return {
        "version": 1,
        "generated_by": "tools/gen_ui_art.py",
        "design_doc": "docs/35-江湖有田-UI资源设计.md",
        "impl_doc": "docs/38-江湖有田-UI资源实现与接入.md",
        "root": "assets/art/ui",
        "nine_slice_field": "Image 组件 ninth 参数顺序 [left, right, top, bottom]（纹理像素）",
        "engine_note": "场景 Image 支持 nine_slice；Lua ui.image 目前不支持（nine_slice 写死 None）",
        "modules": modules,
        "assets": [
            {
                "module": module,
                "name": name,
                "path": asset_path(module, name),
                "size": f"{canvas.w}x{canvas.h}",
                "spec": spec,
                "nine_slice": list(nine) if nine else None,
            }
            for module, name, canvas, spec, nine in items
        ],
    }


def write_all():
    items = all_items()
    for module, _ in MODULES:
        os.makedirs(os.path.join(OUT_DIR, module), exist_ok=True)
    for module, name, canvas, _, _ in items:
        with open(os.path.join(OUT_DIR, module, name + ".png"), "wb") as handle:
            handle.write(canvas.to_png())
    with open(os.path.join(OUT_DIR, "_preview_controls.png"), "wb") as handle:
        handle.write(build_controls_preview(items).to_png())
    with open(os.path.join(OUT_DIR, "_preview_mock.png"), "wb") as handle:
        handle.write(build_mock_preview(items).to_png())
    manifest = build_manifest(items)
    with open(os.path.join(OUT_DIR, "manifest.json"), "w", encoding="utf-8", newline="\n") as handle:
        json.dump(manifest, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    print(f"wrote {len(items)} 切图 / {len(MODULES)} 个模块 → {OUT_DIR}")
    for module, title in MODULES:
        count = sum(1 for item in items if item[0] == module)
        print(f"  {module + '/':12s} {count:2d} 个   {title}")
    print("  manifest.json / _preview_controls.png / _preview_mock.png")


def check():
    from pixel_kit import decode_png

    issues = []
    items = all_items()
    seen = set()
    for module, name, canvas, spec, _ in items:
        rel = rel_path(module, name)
        if rel in seen:
            issues.append("重复文件名 " + rel)
        seen.add(rel)
        path = os.path.join(OUT_DIR, module, name + ".png")
        if not os.path.exists(path):
            issues.append("missing " + path)
            continue
        width, height, _, _ = decode_png(path)
        if (width, height) != (canvas.w, canvas.h):
            issues.append(f"{path}: {width}x{height} != {canvas.w}x{canvas.h}")
    # 旧版扁平目录残留会让代码引用到过期路径，必须为 0
    if os.path.isdir(OUT_DIR):
        for entry in os.listdir(OUT_DIR):
            full = os.path.join(OUT_DIR, entry)
            if os.path.isfile(full) and entry.endswith(".png") and not entry.startswith("_preview"):
                issues.append(f"根目录残留旧扁平切图：{entry}（应移入模块目录）")
    for extra in ("manifest.json", "_preview_controls.png", "_preview_mock.png"):
        if not os.path.exists(os.path.join(OUT_DIR, extra)):
            issues.append("missing " + os.path.join(OUT_DIR, extra))
    return issues


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》UI 切图生成（按模块分目录）")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        problems = check()
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        print(f"[OK] UI 切图校验通过（{len(all_items())} 个文件，{len(MODULES)} 个模块，尺寸与 docs/35 §10 一致）")
        return
    write_all()


if __name__ == "__main__":
    main()
