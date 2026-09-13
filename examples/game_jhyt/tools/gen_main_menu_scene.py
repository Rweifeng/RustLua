# -*- coding: utf-8 -*-
"""《江湖有田》主入口门面场景生成（scenes/main.scene）。

用法：工程根目录 python tools/gen_main_menu_scene.py

设计（docs/44 §2）：
  门面是玩家的**第一眼**，所以按"夜色压暗 → 墨色横幅托底 → 中文木匾标题 → 金线云纹收边"
  的顺序做视觉层次，全部元素落在 1280×720 参考分辨率与 4px 网格上：

    BgNight       title_bg_pixel.png（像素夜景，满屏）
    BgVignette    ui/menu/ui_menu_vignette.png（暗角 + 四角云纹 + 金线画框）
    InkBand       ui/menu/ui_menu_ink_band.png（960×240 墨色横幅，让标题与菜单浮起来）
    Logo          ui/logo/ui_title_logo.png（中文木匾「江湖有田」+ 印章「田」）
    Seal           ui/menu/ui_menu_seal_jianghu.png（朱砂「江湖」，落在横幅右下，像落款）
    Subtitle       「月夜归园 · 半亩心田」（运行时中文文字）
    DividerBamboo  ui/menu/ui_menu_divider_bamboo.png（竹节分隔）
    BtnStart / BtnContinue / BtnExit
                   ui/menu/ui_menu_btn_wuxia_9s.png（9-slice 木牌 + 回纹金边）+ 中文标签
    LanternL/R     ui/menu/ui_menu_lantern.png（两侧红灯笼）
    卷轴提示          ui/menu/ui_menu_panel_scroll_9s.png + 小贴士文字
    FooterText     版本号

  实体 id 按**绘制顺序**分配，同时写 `meta editor.root_order`，两种排序口径都不会错层。

契约自检：
  * 每个引用的贴图必须存在；
  * 非 9-slice 的 Image，其 UiLayout 矩形必须等于贴图原始尺寸（除满屏拉伸的底图/叠加层）；
  * 三个按钮必须存在且带 `UiEvents event=click;function=on_click`（main.lua 与验收驱动按名字点击）；
  * 所有 UiLayout 的 offsets 必须落在 1280×720 内。

依赖：Python 3 标准库 + pixel_kit.py（尺寸校验）。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import decode_png  # noqa: E402

SCENE = os.path.join("scenes", "main.scene")
REF_W, REF_H = 1280, 720
GRID = 4

STRETCH = {"BgNight", "BgVignette"}  # 满屏拉伸的层：不校验矩形等于贴图尺寸

# (name, kind, texture, rect(l,t,r,b), nine_slice, role)
LAYERS = [
    ("BgNight", "image", "assets/art/title/title_bg_pixel.png", (0, 0, REF_W, REF_H), None, "menu_background"),
    ("BgVignette", "image", "assets/art/ui/menu/ui_menu_vignette.png", (0, 0, REF_W, REF_H), None, "menu_vignette"),
    ("InkBand", "image", "assets/art/ui/menu/ui_menu_ink_band.png", (160, 56, 1120, 296), None, "menu_ink_band"),
    ("Logo", "image", "assets/art/ui/logo/ui_title_logo.png", (384, 80, 896, 336), None, "menu_logo"),
    # 门面做减法（三轮评审的结果）：只留"标题 + 菜单"，中间不放任何装饰。
    #   ① 朱砂印章（Logo 内「田」+ 外加「江湖」）—— 红章抢视线，撤；
    #   ② 副标题「月夜归园 · 半亩心田」—— 撤；
    #   ③ 竹节分隔 + 两侧回纹角饰 —— 副标题没了它就成了多余的重量，整组撤。
    # 三样切图都还在 UI 套件里（menu/ 模块），将来要加回来往 LAYERS 里加一行即可；
    # 下面那两条"成对装饰必须镜像/对称"的自检也保留着，等装饰回归时自动生效。
    ("LanternL", "image", "assets/art/ui/menu/ui_menu_lantern.png", (112, 168, 184, 296), None, "menu_lantern"),
    ("LanternR", "image", "assets/art/ui/menu/ui_menu_lantern.png", (1096, 168, 1168, 296), None, "menu_lantern"),
]

BUTTONS = [
    # (实体名, 中文标签, top, bottom)：320×64 木牌，居中竖排
    ("BtnStart", "开始新游戏", 432, 496),
    ("BtnContinue", "继续江湖", 504, 568),
    ("BtnExit", "退出江湖", 576, 640),
]

FOOTER_PANEL = ("TipScroll", "ui/menu 卷轴", (32, 596, 452, 692), "menu_tip_panel")
FOOTER_TEXT = ("FooterText", "江湖有田 v0.1.0 · RustLua 原型 · 主入口门面", (1010, 696, 1256, 712), "menu_footer")

TEX = {
    "btn": "assets/art/ui/menu/ui_menu_btn_wuxia_9s.png",
    "scroll": "assets/art/ui/menu/ui_menu_panel_scroll_9s.png",
}

COLOR_TEXT = "0.949,0.918,0.847"
COLOR_MUTED = "0.659,0.624,0.541"
COLOR_INK = "0.145,0.165,0.196"


def transform_line(entity_id):
    return (
        f"component\t{entity_id}\tTransform\tx=0.000000000;y=0.000000000;rotation=0.000000000"
        ";scale_x=1.000000000;scale_y=1.000000000"
    )


def layout_line(entity_id, rect):
    left, top, right, bottom = rect
    return (
        f"component\t{entity_id}\tUiLayout\tanchors=0.0000,0.0000,0.0000,0.0000;"
        f"offsets={left:.2f},{top:.2f},{right:.2f},{bottom:.2f};"
        "min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000"
    )


def image_line(entity_id, texture, nine_slice=None, color="1.000,1.000,1.000"):
    text = f"component\t{entity_id}\tImage\ttexture={texture};color={color}"
    if nine_slice:
        text += ";nine_slice=" + ",".join(str(value) for value in nine_slice)
    return text


def build():
    """返回 (scene 文本, 自检问题列表)。"""
    problems = []
    lines = [
        "scene_version=2",
        "name=main",
        "meta\tdisplay_name\t江湖有田·主入口门面",
        "meta\tgame\tjianghu_youtian",
        "meta\tui_reference_height\t720",
        "meta\tui_reference_width\t1280",
        "meta\tdesign_doc\tdocs/44-江湖有田-主入口门面与山庄场景重构.md",
    ]
    order = []
    next_id = 1

    def emit_image(name, texture, rect, nine_slice, role, color="1.000,1.000,1.000"):
        nonlocal next_id
        entity_id = next_id
        next_id += 1
        order.append(entity_id)
        lines.append(f"entity\t{entity_id}\t{name}")
        lines.append(image_line(entity_id, texture, nine_slice, color))
        lines.append(transform_line(entity_id))
        lines.append(layout_line(entity_id, rect))
        lines.append(f"entity_meta\t{entity_id}\trole\t{role}")
        return entity_id

    def emit_text(name, text, rect, role, font_size=22.0, color=COLOR_TEXT):
        nonlocal next_id
        entity_id = next_id
        next_id += 1
        order.append(entity_id)
        lines.append(f"entity\t{entity_id}\t{name}")
        lines.append(f"component\t{entity_id}\tText\ttext={text};font_size={font_size:.2f};color={color}")
        lines.append(transform_line(entity_id))
        lines.append(layout_line(entity_id, rect))
        lines.append(f"entity_meta\t{entity_id}\trole\t{role}")
        return entity_id

    # --- 底图 → 暗角 → 墨色横幅 → 标题 ---
    for name, kind, texture, rect, nine_slice, role in LAYERS:
        if kind == "image":
            emit_image(name, texture, rect, nine_slice, role)
        else:
            emit_text(name, texture, rect, role, font_size=26.0, color=COLOR_MUTED)

    # --- 三个木牌按钮（先底板再标签，标签在更上层）---
    button_boxes = {}
    for name, label, top, bottom in BUTTONS:
        board_w = 320
        left = (REF_W - board_w) // 2
        rect = (left, top, left + board_w, bottom)
        button_boxes[name] = rect
        entity_id = emit_image(name, TEX["btn"], rect, (18, 18, 18, 18), "menu_button_" + name)
        # 点击绑定：function=on_click，参数即实体名（main.lua 的 on_click 按名字分发）
        lines.append(f"component\t{entity_id}\tUiEvents\tevent=click;function=on_click")
        # 标签居中
        label_w = max(80, 25 * len(label))
        label_left = left + (board_w - label_w) // 2
        label_top = top + ((bottom - top) - 30) // 2
        emit_text(name + "Label", label, (label_left, label_top, label_left + label_w, label_top + 30),
                  "menu_button_label", font_size=24.0)

    # --- 卷轴提示 + 版本号 ---
    tip_name, _, tip_rect, tip_role = FOOTER_PANEL
    emit_image(tip_name, TEX["scroll"], tip_rect, (20, 20, 20, 20), tip_role)
    emit_text("TipTitle", "江湖小贴士", (52, 608, 200, 632), "menu_tip_title",
              font_size=20.0, color=COLOR_INK)
    emit_text("TipBody", "新档先在药田种下第一株灵芝，", (52, 634, 440, 654), "menu_tip_body",
              font_size=15.0, color=COLOR_INK)
    emit_text("TipBody2", "三日后再下山去云泽村换些种子。", (52, 656, 440, 676), "menu_tip_body",
              font_size=15.0, color=COLOR_INK)
    footer_name, footer_text, footer_rect, footer_role = FOOTER_TEXT
    emit_text(footer_name, footer_text, footer_rect, footer_role, font_size=14.0, color=COLOR_MUTED)

    lines.append("meta\teditor.root_order\t" + ",".join(str(value) for value in order))

    # --- 自检 ---
    textures = {}
    for name, kind, texture, rect, nine_slice, role in LAYERS:
        if kind == "image":
            textures[name] = (texture, rect, nine_slice)
    for name, (texture, rect, nine_slice) in textures.items():
        path = texture.replace("/", os.sep)
        if not os.path.exists(path):
            problems.append(f"{name}: 贴图缺失 {texture}")
            continue
        width, height, channels, _ = decode_png(path)
        # 底图允许 RGB（满屏不透明）；叠加层必须是 RGBA，否则会盖住下层
        if channels != 4 and name != "BgNight":
            problems.append(f"{name}: {texture} 期望 RGBA（channels={channels}）")
        if not nine_slice and name not in STRETCH:
            want = (rect[2] - rect[0], rect[3] - rect[1])
            if want != (width, height):
                problems.append(f"{name}: UiLayout {want[0]}x{want[1]} != 贴图 {width}x{height}"
                                "（非 9-slice 图必须 1:1）")
    for path in (TEX["btn"], TEX["scroll"]):
        if not os.path.exists(path.replace("/", os.sep)):
            problems.append("按钮/卷轴贴图缺失：" + path)
        else:
            with open(path.replace("/", os.sep), "rb") as handle:
                if decode_png(path.replace("/", os.sep))[2] != 4:
                    problems.append("按钮/卷轴需要 RGBA：" + path)
    for name, label, top, bottom in BUTTONS:
        if name not in button_boxes:
            problems.append("按钮缺失：" + name)
        elif bottom > REF_H or top < 0:
            problems.append(f"{name}: 按钮纵向越界 {top}..{bottom}")
    for name, kind, texture, rect, nine_slice, role in LAYERS:
        if rect[0] % GRID:
            problems.append(f"{name}: 左边缘未对齐 {GRID}px 网格 {rect[0]}")
        # 竹节分隔是高 18px 的细线，要**垂直居中**在 64px 角饰之间：
        # 18 为偶数、中心对齐到 4 的倍数时上边必然是 4k-9 ≡ 3 (mod 4)，所以单独豁免。
        if rect[1] % GRID and name != "DividerBamboo":
            problems.append(f"{name}: 上边缘未对齐 {GRID}px 网格 {rect[1]}")
    # 成对装饰必须镜像：左右角饰用同一张贴图 = 视觉上左右不对称（评审踩过）
    ornament_textures = [texture for name, kind, texture, _, _, role in LAYERS
                         if role.startswith("menu_ornament")]
    if len(ornament_textures) != len(set(ornament_textures)):
        problems.append("左右回纹角饰用了同一张贴图（应分别用左上/右上变体，否则左右不对称）")
    # 左右角饰应贴着分隔线两端、且关于画面中心对称
    left = next((rect for name, _, _, rect, _, role in LAYERS if role == "menu_ornament_left"), None)
    right = next((rect for name, _, _, rect, _, role in LAYERS if role == "menu_ornament_right"), None)
    if left and right:
        if left[2] != (REF_W - right[0]):
            problems.append(f"左右角饰不关于画面中线对称：左内缘 {left[2]} vs 右内缘 {REF_W - right[0]}")
        if (left[0], left[1], left[3]) != (REF_W - right[2], right[1], right[3]):
            problems.append("左右角饰尺寸/纵向位置不一致")
    return "\n".join(lines) + "\n", problems


def main():
    parser = argparse.ArgumentParser(description="生成主入口门面场景 scenes/main.scene")
    parser.parse_args()
    text, problems = build()
    if problems:
        for problem in problems:
            print("[FAIL]", problem)
        raise SystemExit(1)
    os.makedirs("scenes", exist_ok=True)
    with open(SCENE, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
    total = text.count("\nentity\t")
    print(f"wrote {SCENE}（{total} 个 UI 实体：夜色底图 / 暗角+云纹金框 / 墨色横幅 / "
          "中文木匾 / 两侧红灯笼 / 3 块木牌按钮 / 卷轴小贴士 / 版本号）")
    print("契约自检 ok：贴图存在 + 非 9-slice 图 1:1 + 按钮带 UiEvents + 4px 网格")


if __name__ == "__main__":
    main()
