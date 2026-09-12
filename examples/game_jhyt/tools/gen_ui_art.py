# -*- coding: utf-8 -*-
"""《江湖有田》UI 切图生成（按 docs/35-UI资源设计，按模块分目录）。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_ui_art.py            # 生成全部切图 + manifest + 两张预览
    python tools/gen_ui_art.py --check    # 按规格表校验尺寸/存在性

目录结构（`assets/art/ui/jianghu/<模块>/`，模块按 docs/35 §7/§8 划分）：

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
    load_canvas,
    rgb,
    shade,
    text_width,
)

OUT_DIR = os.path.join("assets", "art", "ui", "jianghu")
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

QUALITY = [rgb("#B9BDC4"), BAMBOO500, GOLD500, CINNABAR500, rgb("#8B6BB8")]

FONT_LABEL = GOLD300
FONT_DIM = BAMBOO300

# 模块顺序（预览与 manifest 都按这个顺序，稳定可复现）
MODULES = [
    ("common", "COMMON / 共用控件"),
    ("hud", "HUD / 主界面 (DOCS35 S7)"),
    ("backpack", "BACKPACK / 背包 (S8.1)"),
    ("dialogue", "DIALOGUE / 对话 (S8.3)"),
    ("shop", "SHOP / 商店 (S8.2)"),
    ("quest", "QUEST / 任务 (S8.4)"),
    ("skills", "SKILLS / 武学 (S8.5)"),
    ("build", "BUILD / 建筑放置 (S8.6)"),
    ("save", "SAVE / 存档 (S8.7)"),
    ("settings", "SETTINGS / 设置 (S8.8)"),
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
    """16×16：中毒（朱砂框）/ 增益（旧金框 + 上箭头）。"""
    cv = Canvas(16, 16)
    border = CINNABAR500 if kind == "poison" else GOLD500
    frame(cv, 0, 0, 15, 15, INK900, 1)
    cv.rect(1, 1, 14, 14, INK800)
    frame(cv, 1, 1, 14, 14, border, 1)
    if kind == "poison":
        fill_ellipse(cv, 8, 9, 3, 4, BAMBOO700)
        cv.set(7, 5, BAMBOO300)
        cv.set(8, 4, BAMBOO300)
    else:
        for y in range(4, 9):
            half = y - 3
            cv.hline(8 - half, 7 + half, y, GOLD300)
        cv.rect(6, 9, 9, 13, GOLD500)
        cv.hline(6, 9, 13, shade(GOLD500, 0.8))
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
# 切图清单：模块 → [(文件名, 画布, 规格标签, 9-slice 边框)]
# ---------------------------------------------------------------------------


def slice_table():
    table = {
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
            ("ui_hud_avatar_frame", hud_avatar_frame(), "48x48", None),
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
    return f"assets/art/ui/jianghu/{module}/{name}.png"


# ---------------------------------------------------------------------------
# 预览一：全切图（按模块分组）
# ---------------------------------------------------------------------------


def build_controls_preview(items, scale=2):
    cols = 6
    pad = 12
    label_h = 22
    group_gap = 14
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
        "root": "assets/art/ui/jianghu",
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
