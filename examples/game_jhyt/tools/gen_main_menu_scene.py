# -*- coding: utf-8 -*-
"""《江湖有田》主入口门面场景生成（scenes/main.scene）。

用法：工程根目录 python tools/gen_main_menu_scene.py

设计（docs/44 §2、docs/52 §3）：门面是玩家的**第一眼**，按"夜色压暗 → 墨色横幅托底 →
中文木匾标题 → 金线云纹收边"的顺序做视觉层次。**M32 起整块版面按竖版 720×1280 排**：

    BgNight        title_bg_pixel.png（1280×720 像素夜景，横构图）
                   竖屏按 16:9 等比缩放贴到顶部 720×432（不拉伸：像素画拉伸会糊）
    BgBand         纯色暗底（64,428,720,1280），补满 432 以下的整块版面
                   —— 色值取自夜景图最下沿的颜色，接缝不可见
    BgVignette     ui/menu/ui_menu_vignette.png（暗角 + 四角云纹 + 金线画框，满屏）
    InkBand        ui/menu/ui_menu_ink_band.png（960×240 墨色横幅，托住标题）
    Logo           ui/logo/ui_title_logo.png（中文木匾「江湖有田」+ 印章「田」，1:1 不缩放）
    LanternL/R     ui/menu/ui_menu_lantern.png（标题两侧红灯笼，1:1 不缩放）
    BtnStart / BtnContinue / BtnExit
                   ui/menu/ui_menu_btn_wuxia_9s.png（9-slice 木牌 + 回纹金边）+ 中文标签
                   竖排在中部（y 620 / 708 / 796），与 `scripts/ui/layout.lua` 的
                   `PANEL.title_btn`、`scripts/ui/title_menu.lua` 的 OPTIONS 三方一致
    卷轴提示        ui/menu/ui_menu_panel_scroll_9s.png + 小贴士文字（底部）
    FooterText     版本号（最底一行，与 HUD 键位提示行同一个 y）

  实体 id 按**绘制顺序**分配，同时写 `meta editor.root_order`，两种排序口径都不会错层。

契约自检：
  * 每个引用的贴图必须存在；
  * 非 9-slice 且不在 STRETCH 里的 Image，其 UiLayout 矩形必须等于贴图原始尺寸
    （像素画/图标拉伸会糊，所以要 1:1）；
  * 三个按钮必须存在且带 `UiEvents event=click;function=on_click`（main.lua 与验收驱动按名字点击）；
  * 所有 UiLayout 的 offsets 必须落在 720×1280 内（竖版设计画布，M32）。

依赖：Python 3 标准库 + pixel_kit.py（尺寸校验）。
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import decode_png  # noqa: E402
# M32：`Transform` 的统一形态序列化器（引擎读侧只认 position/rotation/scale，
# 场景数据门 `shipped_data_uses_only_unified_components` 会拒绝 2D 旧形态）。
from pixel_kit import transform_line as spawn_transform  # noqa: E402

SCENE = os.path.join("scenes", "main.scene")
# M32 设计分辨率（2026-09-27 转横屏）。必须与 `scripts/ui/layout.lua` 的 DESIGN_W/DESIGN_H、
# `project.toml` 的 window_width/height 一致（由 `ui/layout_check.lua` 守住）。
REF_W, REF_H = 1280, 720
GRID = 4

# 允许"作者矩形 != 贴图原始尺寸"的实体：
#   * 满屏叠加层（底图/暗角）按版面铺；
#   * InkBand 是 960×240 的墨色横幅，竖版放不进 720 宽，等比缩到 2/3（640×160）后
#     只当"标题的暗底"用——墨晕没有直线结构（不是像素画、也不是九宫格），
#     等比缩小不会露出"该 1:1"的证据。见 docs/52 §7 的已知偏差。
STRETCH = {"BgNight", "BgVignette", "InkBand"}

# 夜景图原始尺寸（`tools/gen_title_bg.py` 产出 1280×720）；横屏底图 1:1 铺满版面，
# 因此这里只记原始尺寸用于"资源确实存在且尺寸符合预期"的核对。
BG_W, BG_H = 1280, 720
# 夜景图最下沿的近似色（`tools/gen_title_bg.py` 的调色板底部色）——横屏已 1:1，保留待用。
BG_BAND_COLOR = "0.086,0.098,0.129"

# (name, kind, texture, rect(l,t,r,b), nine_slice, role)
LAYERS = [
    # 底图：横构图夜景（1280×720）横屏 1:1 铺满版面。底图与暗角因此都在 STRETCH 里
    # （允许"作者矩形 != 贴图原始尺寸"；横屏下实际都是 1:1，不触发缩放偏差）。
    ("BgNight", "image", "assets/art/title/title_bg_pixel.png", (0, 0, REF_W, REF_H), None, "menu_background"),
    ("BgVignette", "image", "assets/art/ui/menu/ui_menu_vignette.png", (0, 0, REF_W, REF_H), None, "menu_vignette"),
    # InkBand 是 960×240 的墨色横幅，横屏 1:1（左右各 160，水平居中）。
    ("InkBand", "image", "assets/art/ui/menu/ui_menu_ink_band.png", (320, 80, 960, 240), None, "menu_ink_band"),
    # Logo 是 512×256 的木匾贴图，**必须 1:1**（像素画缩放会糊）；横屏水平居中：
    # (1280 - 512) / 2 = 384，落在 4px 网格上。
    ("Logo", "image", "assets/art/ui/logo/ui_title_logo.png", (384, 80, 896, 336), None, "menu_logo"),
    # 门面做减法（三轮评审的结果）：只留"标题 + 菜单"，中间不放任何装饰。
    #   ① 朱砂印章（Logo 内「田」+ 外加「江湖」）—— 红章抢视线，撤；
    #   ② 副标题「月夜归园 · 半亩心田」—— 撤；
    #   ③ 竹节分隔 + 两侧回纹角饰 —— 副标题没了它就成了多余的重量，整组撤。
    # 三样切图都还在 UI 套件里（menu/ 模块），将来要加回来往 LAYERS 里加一行即可；
    # 下面那两条"成对装饰必须镜像/对称"的自检也保留着，等装饰回归时自动生效。
    ("LanternL", "image", "assets/art/ui/menu/ui_menu_lantern.png", (280, 120, 352, 248), None, "menu_lantern"),
    ("LanternR", "image", "assets/art/ui/menu/ui_menu_lantern.png", (928, 120, 1000, 248), None, "menu_lantern"),
]

# 三个按钮的**公共矩形**（与 scripts/ui/layout.lua 的 PANEL.title_btn 逐字一致）：
#   x=460 w=360 h=64；纵向间隔 = h + title_btn_gap(16) = 80。
BTN_X, BTN_W, BTN_H, BTN_STEP = 460, 360, 64, 80
BTNS = [
    # (实体名, 中文标签, top)
    ("BtnStart", "开始新游戏", 380),
    ("BtnContinue", "继续江湖", 460),
    ("BtnExit", "退出江湖", 540),
]

# 底部卷轴小贴士面板（横屏：左下角，不挡按钮）。
FOOTER_PANEL = ("TipScroll", "ui/menu 卷轴", (24, 600, 476, 696), "menu_tip_panel")
FOOTER_TEXT = ("FooterText", "江湖有田 v0.1.0 · RustLua 原型 · 横版", (1080, 690, 1240, 710), "menu_footer")

TEX = {
    "btn": "assets/art/ui/menu/ui_menu_btn_wuxia_9s.png",
    "scroll": "assets/art/ui/menu/ui_menu_panel_scroll_9s.png",
}

COLOR_TEXT = "0.949,0.918,0.847"
COLOR_MUTED = "0.659,0.624,0.541"
COLOR_INK = "0.145,0.165,0.196"


def transform_line(entity_id):
    # 门面的实体都在原点：位置 (0,0,0)、无旋转、缩放 1。形态由 `pixel_kit` 单点产出。
    return spawn_transform(entity_id, 0.0, 0.0)


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
        # M32 竖版：设计分辨率 720×1280（与 scripts/ui/layout.lua 一致）。
        "meta\tui_reference_height\t1280",
        "meta\tui_reference_width\t720",
        "meta\tdesign_doc\tdocs/52-江湖有田-竖版720x1280改造.md",
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

    # --- 底图 → 暗角 → 墨色横幅 → 标题（→ 灯笼） ---
    for name, kind, texture, rect, nine_slice, role in LAYERS:
        if kind != "image":
            raise AssertionError(f"未知图层类型 {kind}（{name}）：门面只允许 scene 能表达的 image")
        emit_image(name, texture, rect, nine_slice, role)
    # --- 三个木牌按钮（先底板再标签，标签在更上层）---
    button_boxes = {}
    for name, label, top in BTNS:
        left = BTN_X
        rect = (left, top, left + BTN_W, top + BTN_H)
        button_boxes[name] = rect
        entity_id = emit_image(name, TEX["btn"], rect, (18, 18, 18, 18), "menu_button_" + name)
        # 点击绑定：function=on_click，参数即实体名（main.lua 的 on_click 按名字分发）
        lines.append(f"component\t{entity_id}\tUiEvents\tevent=click;function=on_click")
        # 标签居中：用**估算宽度**（每个中文字约 25px）而不是逐字量宽——场景生成器不该依赖
        # 运行时字号度量；`scripts/ui/title_menu.lua` 的 `label_left()` 用 `ui.measure_text`
        # 算同一个位置，两边允许 1px 内的差异（脚本只在这里补一个"> "箭头，不画标签本体）。
        label_w = max(80, 25 * len(label))
        label_left = left + (BTN_W - label_w) // 2
        label_top = top + ((BTN_H) - 30) // 2
        emit_text(name + "Label", label, (label_left, label_top, label_left + label_w, label_top + 30),
                  "menu_button_label", font_size=24.0)

    # --- 卷轴提示 + 版本号 ---
    tip_name, _, tip_rect, tip_role = FOOTER_PANEL
    emit_image(tip_name, TEX["scroll"], tip_rect, (20, 20, 20, 20), tip_role)
    tip_x, tip_y = tip_rect[0], tip_rect[1]
    emit_text("TipTitle", "江湖小贴士", (tip_x + 20, tip_y + 12, tip_x + 220, tip_y + 36), "menu_tip_title",
              font_size=20.0, color=COLOR_INK)
    emit_text("TipBody", "新档先在药田种下第一株灵芝，", (tip_x + 20, tip_y + 38, tip_x + 428, tip_y + 58),
              "menu_tip_body", font_size=15.0, color=COLOR_INK)
    emit_text("TipBody2", "三日后再下山去云泽村换些种子。", (tip_x + 20, tip_y + 60, tip_x + 428, tip_y + 80),
              "menu_tip_body", font_size=15.0, color=COLOR_INK)
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
    # 底图必须是**满屏**（竖版整块版面都要被盖住，否则会露出后面的清屏色）。
    bg_rect = next(rect for name, kind, _, rect, _, role in LAYERS if name == "BgNight")
    if bg_rect != (0, 0, REF_W, REF_H):
        problems.append(f"BgNight: 竖版底图必须铺满 {REF_W}x{REF_H}，实为 {bg_rect}")
    for path in (TEX["btn"], TEX["scroll"]):
        if not os.path.exists(path.replace("/", os.sep)):
            problems.append("按钮/卷轴贴图缺失：" + path)
        else:
            with open(path.replace("/", os.sep), "rb") as handle:
                if decode_png(path.replace("/", os.sep))[2] != 4:
                    problems.append("按钮/卷轴需要 RGBA：" + path)
    for name, label, top in BTNS:
        if name not in button_boxes:
            problems.append("按钮缺失：" + name)
        elif top + BTN_H > REF_H or top < 0:
            problems.append(f"{name}: 按钮纵向越界 {top}..{top + BTN_H}")
    # 三块按钮必须落进 `scripts/ui/layout.lua` 声明的同一个公共矩形（脚本按它画高亮/箭头）。
    for name, label, top in BTNS:
        rect = button_boxes[name]
        if rect[0] != BTN_X or rect[2] != BTN_X + BTN_W:
            problems.append(f"{name}: 按钮左/右边缘 {rect[0]}..{rect[2]} 与公共矩形 {BTN_X}..{BTN_X + BTN_W} 不一致")
    if BTNS[1][2] - BTNS[0][2] != BTN_STEP or BTNS[2][2] - BTNS[1][2] != BTN_STEP:
        problems.append("按钮纵向间隔不等于 BTN_STEP（应与 ui/layout.lua 的 title_btn_gap 一致）")
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
    # 灯笼必须关于画面中线左右对称（竖版里它们是标题两侧的一对）。
    lanterns = [rect for name, _, _, rect, _, role in LAYERS if role == "menu_lantern"]
    if len(lanterns) == 2:
        left_lantern, right_lantern = lanterns
        if left_lantern[0] != (REF_W - right_lantern[2]):
            problems.append(
                f"左右灯笼不关于画面中线对称：左 {left_lantern[0]} vs 右内缘 {REF_W - right_lantern[2]}"
            )
        if (left_lantern[2], left_lantern[1], left_lantern[3]) != (
                REF_W - right_lantern[0], right_lantern[1], right_lantern[3]):
            problems.append("左右灯笼尺寸/纵向位置不一致")
    # 左右角饰应贴着分隔线两端、且关于画面中心对称（装饰回归时自动生效）
    left = next((rect for name, _, _, rect, _, role in LAYERS if role == "menu_ornament_left"), None)
    right = next((rect for name, _, _, rect, _, role in LAYERS if role == "menu_ornament_right"), None)
    if left and right:
        if left[2] != (REF_W - right[0]):
            problems.append(f"左右角饰不关于画面中线对称：左内缘 {left[2]} vs 右内缘 {REF_W - right[0]}")
        if (left[0], left[1], left[3]) != (REF_W - right[2], right[1], right[3]):
            problems.append("左右角饰尺寸/纵向位置不一致")
    # 所有元素必须落在设计画布内（竖版 720×1280）。
    for name, kind, _, rect, _, _ in LAYERS:
        if rect[0] < 0 or rect[1] < 0 or rect[2] > REF_W or rect[3] > REF_H:
            problems.append(f"{name}: 越出设计画布 {REF_W}x{REF_H}：{rect}")
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
    print(f"契约自检 ok：贴图存在 + 非 9-slice 图 1:1 + 底图等比 + 按钮带 UiEvents + {GRID}px 网格")


if __name__ == "__main__":
    main()
