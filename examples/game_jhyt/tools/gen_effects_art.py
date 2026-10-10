# -*- coding: utf-8 -*-
"""《江湖有田》基础特效序列帧生成（docs/31 §3.5 的 MVP 9 项，收获跳字是文字不在此列）。

用法（工程根目录 examples/game_jhyt 下运行）：

    python tools/gen_effects_art.py            # 生成全部特效 + manifest + 预览
    python tools/gen_effects_art.py --check    # 只校验尺寸/列数/存在性

产出：`assets/art/effects/<id>.png`（1×N 横向条带）+ `manifest.json` + `_preview.png`

| id | 名称 | 帧 | 帧尺寸 | FPS | 循环 | 触发点 |
|---|---|---:|---:|---:|---|---|
| `hoe_dust` | 锄地尘土 | 6 | 32 | 12 | 否 | farm.lua 翻土成功 |
| `water_splash` | 浇水水花 | 6 | 32 | 12 | 否 | farm.lua 浇水成功 |
| `blade_arc` | 刀光 | 5 | 48 | 18 | 否 | skills.lua 普攻命中前摇结束 |
| `hit_spark` | 命中火花 | 5 | 32 | 18 | 否 | combat.lua 产生 AttackHit |
| `hurt_flash` | 受击闪白 | 3 | 32 | 20 | 否 | 敌人/玩家受击 |
| `heal_motes` | 治疗提示 | 6 | 32 | 8 | 否 | 使用回复药 / 睡觉恢复 |
| `poison_motes` | 中毒提示 | 6 | 32 | 6 | 是 | 中毒状态持续期间 |
| `dash_streak` | 轻功残影 | 4 | 32 | 16 | 否 | 追风步/轻功位移 |
| `harvest_sparkle` | 收成闪光 | 6 | 32 | 10 | 否 | farm.lua 收获成功（M72-08） |

引擎事实：特效实体同样由 `entity.spawn` 运行期创建，而 Lua 没有挂 Animation 组件的 API
（docs/36 §8.3），所以条带先按 Animation 约定出好；在引擎补 `animation.attach` 之前，
可以先退化成单帧（`frames/f0` 见 manifest 的 `static_frame` 字段说明）。

依赖：Python 3 标准库 + pixel_kit.py。
"""

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pixel_kit import Canvas, draw_text, hash_stream, rgb, shade  # noqa: E402

OUT_DIR = os.path.join("assets", "art", "effects")

INK = rgb("#1F2328")
PAPER = rgb("#F2EAD8")
GOLD = rgb("#C9A05A")
GOLD_LIGHT = rgb("#E4C078")
CINNABAR = rgb("#C3452F")
CINNABAR_LIGHT = rgb("#E06B50")
BAMBOO = rgb("#789262")
BAMBOO_LIGHT = rgb("#A9C28F")
BAMBOO_DARK = rgb("#5B7048")
WATER = rgb("#7FA3D4")
WATER_LIGHT = rgb("#B8D2F0")
DUST = rgb("#8F6E4A")
DUST_DARK = rgb("#63482F")
DUST_LIGHT = rgb("#B08F66")


def ring(cv, cx, cy, radius, color, thickness=1):
    """圆环（用 mid-point 圆算法，逐像素点）。"""
    x = radius
    y = 0
    err = 0
    while x >= y:
        for dx, dy in ((x, y), (y, x), (-x, y), (-y, x), (-x, -y), (-y, -x), (x, -y), (y, -x)):
            for step in range(thickness):
                cv.set(cx + dx + step, cy + dy, color)
        y += 1
        if err <= 0:
            err += 2 * y + 1
        if err > 0:
            x -= 1
            err -= 2 * x + 1


def puff(cv, cx, cy, radius, base, light, dark):
    """一团烟/土：带左上高光的圆块。"""
    from gen_manor_art import fill_ellipse

    fill_ellipse(cv, cx, cy, radius, radius * 0.8, base)
    fill_ellipse(cv, cx - radius * 0.3, cy - radius * 0.35, radius * 0.5, radius * 0.4, light)
    fill_ellipse(cv, cx + radius * 0.35, cy + radius * 0.35, radius * 0.4, radius * 0.3, dark)


# ---------------------------------------------------------------------------
# 单个特效：每个函数返回 [Canvas, ...]（帧序列）
# ---------------------------------------------------------------------------


def fx_hoe_dust():
    """锄地尘土：贴地向外扩散的土团，末帧散尽。"""
    frames = []
    for index in range(6):
        cv = Canvas(32, 32)
        t = index / 5.0
        spread = 3 + int(round(t * 9))
        for side in (-1, 1):
            cx = 16 + side * spread
            radius = max(1, 4 - int(round(t * 3)))
            puff(cv, cx, 25 - int(round(t * 3)), radius, DUST, DUST_LIGHT, DUST_DARK)
        if index < 4:
            for offset in range(-2, 3):
                cv.set(16 + offset * 2, 27 - (index % 2), DUST_DARK)
        frames.append(cv)
    return frames


def fx_water_splash():
    """浇水水花：水珠向外抛 + 地面水环。"""
    frames = []
    for index in range(6):
        cv = Canvas(32, 32)
        t = index / 5.0
        ring(cv, 16, 26, 3 + int(round(t * 9)), WATER, 1)
        for step in range(5):
            angle = (step / 5.0) * 3.14159
            x = 16 + int(round((4 + t * 10) * (1 if step % 2 else -1) * 0.9))
            y = 24 - int(round((6 + t * 4) * (0.4 + 0.6 * (step / 5.0))))
            cv.set(x, y, WATER_LIGHT if step % 2 else WATER)
        if index < 3:
            cv.hline(13, 19, 26, WATER_LIGHT)
        frames.append(cv)
    return frames


def fx_blade_arc():
    """刀光：48×48，从右上扫到左下的月牙。"""
    frames = []
    for index in range(5):
        cv = Canvas(48, 48)
        t = index / 4.0
        radius = 14 + int(round(t * 6))
        cx, cy = 16, 30
        thickness = 3 if index < 3 else 2
        # 月牙：只画 0..150° 的弧
        for step in range(70):
            angle = (step / 69.0) * 2.6 - 0.2
            import math

            x = int(round(cx + radius * math.cos(angle)))
            y = int(round(cy - radius * math.sin(angle) * 0.9))
            for offset in range(thickness):
                cv.set(x + offset, y, PAPER if offset == 0 else rgb("#CFE0F2"))
        if index >= 3:
            cv.hline(6, 40, 31, rgb("#9FB6D0"))
        frames.append(cv)
    return frames


def fx_hit_spark():
    """命中火花：金/朱砂的星形爆点。"""
    frames = []
    for index in range(5):
        cv = Canvas(32, 32)
        length = 4 + index * 3
        color = GOLD_LIGHT if index < 2 else (GOLD if index < 4 else CINNABAR)
        for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            for step in range(2, length):
                cv.set(16 + dx * step, 16 + dy * step, color)
                if step < length - 1:
                    cv.set(16 + dx * step + dy, 16 + dy * step + dx, shade(color, 0.8))
        for dx, dy in ((1, 1), (-1, 1), (1, -1), (-1, -1)):
            for step in range(2, max(2, length - 3)):
                cv.set(16 + dx * step, 16 + dy * step, color)
        if index < 3:
            cv.set(16, 16, PAPER)
        frames.append(cv)
    return frames


def fx_hurt_flash():
    """受击闪白：一圈白环 + 中心白闪，覆盖在受击者身上。"""
    frames = []
    for index in range(3):
        cv = Canvas(32, 32)
        alpha = 200 - index * 60
        radius = 8 + index * 5
        for step in range(radius * 2):
            cv.set(16 + step - radius, 16, (255, 255, 255, alpha))
            cv.set(16, 16 + step - radius, (255, 255, 255, alpha))
        ring(cv, 16, 16, radius, (255, 255, 255, max(40, alpha - 40)), 1)
        frames.append(cv)
    return frames


def fx_heal_motes():
    """治疗提示：竹青光点上浮 + 十字微光。"""
    frames = []
    rnd = hash_stream(707)
    seeds = [(6 + rnd(22), 24 - rnd(10)) for _ in range(7)]
    for index in range(6):
        cv = Canvas(32, 32)
        rise = index * 3
        for base_x, base_y in seeds:
            y = base_y - rise
            if y < 2:
                continue
            cv.set(base_x, y, BAMBOO_LIGHT)
            cv.set(base_x, y + 1, BAMBOO)
        if index < 3:
            cv.hline(14, 18, 16, PAPER)
            cv.vline(16, 13, 19, PAPER)
        frames.append(cv)
    return frames


def fx_poison_motes():
    """中毒提示：暗绿气泡上浮（循环播放）。"""
    frames = []
    rnd = hash_stream(808)
    seeds = [(7 + rnd(20), 26 - rnd(12)) for _ in range(6)]
    for index in range(6):
        cv = Canvas(32, 32)
        rise = index * 2
        for base_x, base_y in seeds:
            y = base_y - rise
            if y < 2:
                y += 20
            cv.set(base_x, y, BAMBOO_DARK)
            cv.set(base_x + 1, y, BAMBOO)
            cv.set(base_x, y - 1, BAMBOO_LIGHT)
        frames.append(cv)
    return frames


def fx_dash_streak():
    """轻功残影：横向速度线 + 渐隐的冲刺影。"""
    frames = []
    for index in range(4):
        cv = Canvas(32, 32)
        alpha = 190 - index * 45
        for row in (13, 16, 19, 22):
            length = 20 - index * 4
            for x in range(6, 6 + length):
                cv.set(x, row, (232, 240, 255, alpha))
        # 冲刺影
        for y in range(10, 26):
            half = 4 - abs(y - 18) // 4
            for x in range(20, 20 + half):
                cv.set(x, y, (200, 216, 240, max(40, alpha - 80)))
        frames.append(cv)
    return frames


# ---------------------------------------------------------------------------
# 清单
# ---------------------------------------------------------------------------

def fx_harvest_sparkle():
    """收成闪光：金色四尖星炸开 + 谷壳碎屑上抛后消散（docs/51 §2.3 收成反馈）。

    与 hit_spark 的分别：命中火花是**横向**的急爆（5 帧 / 18fps），收成是**向上**的
    散落（6 帧 / 10fps）——玩家在田里连收十格时，两种节奏一眼就能分出来。
    """
    frames = []
    rnd = hash_stream(919)
    chaff = [(10 + rnd(12), rnd(2) - 1, 1 + rnd(2)) for _ in range(6)]
    for index in range(6):
        cv = Canvas(32, 32)
        rise = index * 2
        color = PAPER if index < 2 else (GOLD_LIGHT if index < 4 else GOLD)
        # 四尖星：横竖两条主焰 + 斜向短焰，第 4 帧后只余焰心
        span = [10, 12, 11, 8, 5, 2][index]
        for step in range(-span, span + 1):
            cv.set(16 + step, 22 - rise, color)
            cv.set(16, 22 - rise + step, color)
        diagonal = max(1, span // 2)
        for step in range(-diagonal, diagonal + 1):
            cv.set(16 + step, 22 - rise + step, GOLD if index < 3 else GOLD_LIGHT)
        if index == 0:
            cv.set(16, 22, PAPER)
        # 谷壳碎屑：从焰心散开，越飞越高、越飞越稀
        for base_x, drift, size in chaff:
            y = 22 - rise - 3 - index * 2 + drift
            if y < 2 or index > 4:
                continue
            for width in range(size):
                cv.set(base_x + width - index, y, DUST_LIGHT if index < 3 else DUST)
        frames.append(cv)
    return frames


EFFECTS = [
    ("hoe_dust", "锄地尘土", fx_hoe_dust, 32, 12, False, "farm.lua 翻土成功"),
    ("water_splash", "浇水水花", fx_water_splash, 32, 12, False, "farm.lua 浇水成功"),
    ("blade_arc", "刀光", fx_blade_arc, 48, 18, False, "普攻前摇结束/hitbox 生成"),
    ("hit_spark", "命中火花", fx_hit_spark, 32, 18, False, "combat.lua 产生 AttackHit"),
    ("hurt_flash", "受击闪白", fx_hurt_flash, 32, 20, False, "玩家/敌人受击"),
    ("heal_motes", "治疗提示", fx_heal_motes, 32, 8, False, "回复药/睡觉恢复"),
    ("poison_motes", "中毒提示", fx_poison_motes, 32, 6, True, "中毒状态持续期间"),
    ("dash_streak", "轻功残影", fx_dash_streak, 32, 16, False, "追风步/轻功位移"),
    ("harvest_sparkle", "收成闪光", fx_harvest_sparkle, 32, 10, False, "farm.lua 收获成功"),
]

# 特效在世界里的显示尺寸（世界单位）：刀光比格子略大
WORLD_SIZE = {
    "hoe_dust": "32x32",
    "water_splash": "32x32",
    "blade_arc": "48x48",
    "hit_spark": "32x32",
    "hurt_flash": "32x32",
    "heal_motes": "32x32",
    "poison_motes": "32x32",
    "dash_streak": "32x32",
    "harvest_sparkle": "32x32",
}


def build_preview(rendered, scale=3):
    pad = 10
    label_w = 130
    cell_h = max(max(frame.h for frame in frames) for _, _, frames, _ in rendered) * scale + pad
    width = label_w + max(len(frames) for _, _, frames, _ in rendered) * (48 * scale + 4) + 20
    height = 22 + len(rendered) * (cell_h + 6) + 8
    cv = Canvas(width, height)
    cv.rect(0, 0, cv.w - 1, cv.h - 1, rgb("#14171A"))
    draw_text(cv, 6, 6, "EFFECTS / 1xN STRIPS X3 (DARK BG = TRANSPARENT)", GOLD_LIGHT, 1)
    y = 22
    for name, display, frames, _ in rendered:
        draw_text(cv, 6, y + cell_h // 2 - 2, name, BAMBOO_LIGHT, 1)
        for index, frame in enumerate(frames):
            x = label_w + index * (48 * scale + 4)
            cv.rect(x - 1, y - 1, x + 48 * scale, y + cell_h - 2, rgb("#1B2026"))
            cv.blit(frame.scaled(scale), x, y)
        y += cell_h + 6
    return cv


def animation_component(entry_id, cols, fps, looped, path):
    return f"default={entry_id};clip:{entry_id}:{path}:{cols}:{fps}:{str(looped).lower()}"


def generate(quiet=False):
    os.makedirs(OUT_DIR, exist_ok=True)
    payload = {
        "version": 1,
        "generated_by": "tools/gen_effects_art.py",
        "design_doc": "docs/31-江湖有田-美术资源方案.md",
        "engine": {
            "sheet_layout": "1xN 横向条带；uv u=frame/cols..(frame+1)/cols, v=0..1",
            "note": "特效实体由 entity.spawn 创建，Lua 暂无挂 Animation 的 API；"
                    "补 animation.attach 后可直接播，之前可只用第 0 帧",
        },
        "effects": [],
    }
    rendered = []
    for entry_id, display, factory, frame_px, fps, looped, trigger in EFFECTS:
        frames = factory()
        strip = Canvas(frame_px * len(frames), frame_px)
        for index, frame in enumerate(frames):
            for y in range(frame_px):
                for x in range(frame_px):
                    strip.set(index * frame_px + x, y, frame.get(x, y))
        rel = f"assets/art/effects/{entry_id}.png"
        with open(os.path.join(OUT_DIR, entry_id + ".png"), "wb") as handle:
            handle.write(strip.to_png())
        rendered.append((entry_id, display, frames, trigger))
        payload["effects"].append({
            "id": entry_id,
            "display": display,
            "sheet": rel,
            "cols": len(frames),
            "fps": fps,
            "looped": looped,
            "frame_px": frame_px,
            "world_size": WORLD_SIZE[entry_id],
            "trigger": trigger,
            "animation": animation_component(entry_id, len(frames), fps, looped, rel),
        })
        if not quiet:
            print(f"  {entry_id}: {len(frames)} 帧 / {frame_px}px  {display}")
    with open(os.path.join(OUT_DIR, "_preview.png"), "wb") as handle:
        handle.write(build_preview(rendered).to_png())
    manifest_path = os.path.join(OUT_DIR, "manifest.json")
    with open(manifest_path, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(payload, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    if not quiet:
        print("  _preview.png")
        print("  manifest.json:", manifest_path)


def check():
    from pixel_kit import decode_png

    issues = []
    manifest_path = os.path.join(OUT_DIR, "manifest.json")
    if not os.path.exists(manifest_path):
        return ["manifest.json 缺失，请先运行 tools/gen_effects_art.py"]
    with open(manifest_path, encoding="utf-8") as handle:
        manifest = json.load(handle)
    for entry in manifest["effects"]:
        path = entry["sheet"].replace("/", os.sep)
        if not os.path.exists(path):
            issues.append("missing sheet: " + entry["sheet"])
            continue
        width, height, channels, _ = decode_png(path)
        if height != entry["frame_px"]:
            issues.append(f"{entry['sheet']}: height {height} != {entry['frame_px']}")
        if width != entry["frame_px"] * entry["cols"]:
            issues.append(f"{entry['sheet']}: width {width} != {entry['frame_px']}*{entry['cols']}")
        if channels != 4:
            issues.append(f"{entry['sheet']}: 期望 RGBA（channels={channels}）")
    if not os.path.exists(os.path.join(OUT_DIR, "_preview.png")):
        issues.append("missing " + os.path.join(OUT_DIR, "_preview.png"))
    return issues


def main():
    parser = argparse.ArgumentParser(description="《江湖有田》基础特效序列帧生成")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        problems = check()
        if problems:
            for problem in problems:
                print("[FAIL]", problem)
            raise SystemExit(1)
        print(f"[OK] 特效校验通过（{len(EFFECTS)} 组）")
        return
    print(f"生成基础特效 → {OUT_DIR}")
    generate()


if __name__ == "__main__":
    main()
