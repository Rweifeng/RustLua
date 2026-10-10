# -*- coding: utf-8 -*-
"""Read-only art audit, Python 3.9+ standard library only.

python -B -X utf8 tools/audit_art_assets.py [--json] [--self-test]
Exit: 0 no FAIL (NOT release approval), 1 known gaps, 2 audit/input error.
Static Lua table/field inspection only, not a Lua interpreter. Computed paths,
execution reachability, PNG pixel quality, and runtime rendering are not verified.
"""
from __future__ import annotations

import argparse
import json
import re
import struct
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
# Preserve quoted strings while removing comments (including Lua long comments).
LEX = re.compile(r'--\[(=*)\[.*?\]\1\]|--[^\n]*|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'', re.S)


def clean(text):
    return LEX.sub(lambda m: "\n" * m[0].count("\n") if m[0].startswith("--") else m[0], text)


def table(text, name):
    """Extract a named literal table; fail closed when the expected shape changes."""
    match = re.search(r'(?<![\w.])' + re.escape(name) + r'\s*=\s*\{', text)
    if not match:
        raise ValueError("expected literal table: " + name)
    return balanced(text, match.end() - 1)[0]


def balanced(text, start):
    depth = 0
    quote = None
    escape = False
    for i in range(start, len(text)):
        char = text[i]
        if quote:
            if escape:
                escape = False
            elif char == "\\":
                escape = True
            elif char == quote:
                quote = None
        elif char in "\"'":
            quote = char
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return text[start + 1:i], i + 1
    raise ValueError("unclosed literal table")


def records(body):
    """Top-level literal table children, keyed or array entries; nested braces safe."""
    result = []
    pos = 0
    while pos < len(body):
        match = re.match(r'\s*,?\s*(?:([\w]+)\s*=\s*)?\{', body[pos:])
        if not match:
            if body[pos:].strip(" \t\r\n,"):
                raise ValueError("unsupported registry shape near " + body[pos:pos + 60])
            break
        child, end = balanced(body, pos + match.end() - 1)
        result.append((match[1], child))
        pos = end
    return result


def field(body, name):
    match = re.search(r'\b' + re.escape(name) + r'\s*=\s*"([^"\n]*)"', body)
    return match[1] if match else None


def source(path):
    return clean((ROOT / path).read_text(encoding="utf-8"))


def png_info(path):
    with path.open("rb") as stream:
        data = stream.read(33)
    if len(data) < 33 or data[:8] != b"\x89PNG\r\n\x1a\n" or data[8:16] != b"\x00\x00\x00\rIHDR":
        raise ValueError("invalid/truncated PNG IHDR")
    width, height, depth, kind = struct.unpack(">IIBB", data[16:26])
    if not width or not height:
        raise ValueError("zero PNG dimension")
    return width, height, depth, kind


def audit():
    findings = []

    def add(level, kind, subject, detail):
        findings.append(dict(level=level, kind=kind, subject=subject, detail=detail))

    def png(ref, expected=None):
        path = ROOT / ref
        if not path.is_file():
            add("FAIL", "missing_asset", ref, "File absent; a reference is not a delivered asset")
            return
        info = png_info(path)
        if expected and info[:2] != expected:
            add("FAIL", "asset_spec", ref, f"IHDR size {info[:2]}, expected {expected}")
        if info[2:] != (8, 6):
            add("FAIL", "asset_spec", ref, f"IHDR depth/type {info[2:]}, expected RGBA8 (8,6)")

    chars = source("scripts/data/characters.lua")
    entries = dict(records(table(chars, "M.characters")))
    refs = sorted(set(re.findall(r'assets/art/characters/[\w/-]+\.png', chars)))
    clips = re.findall(r'sheet\s*=\s*"([^"]+)"\s*,\s*cols\s*=\s*(\d+)', chars)
    expected = {ref: (int(cols) * 64, 64) for ref, cols in clips}
    for ref in refs:
        png(ref, (64, 64) if Path(ref).name.startswith("static_") else expected.get(ref))
    add("INFO", "wiring_fact", "characters", f"{len(entries)} entries; {len(refs)} explicit PNG paths; {len(clips)} clip size declarations")
    npcs = records(table(source("scripts/data/npcs.lua"), "M.npcs"))
    for _, body in npcs:
        npc_id = field(body, "persist_id")
        character = field(body, "character")
        if not npc_id:
            raise ValueError("NPC without literal persist_id")
        if not character:
            add("FAIL", "missing_wiring", npc_id, "No literal character field; world/npc.lua falls back to sprite")
            conventional = "assets/art/characters/" + npc_id.removeprefix("npc_")
            if not (ROOT / conventional).is_dir():
                add("FAIL", "missing_asset", conventional, "Identity-specific directory prescribed by docs/58 absent; reuse/alias remains a separate choice")
        elif character not in entries:
            add("FAIL", "missing_wiring", npc_id, "Unknown character target: " + character)
    add("INFO", "registry_fact", "npcs", f"{len(npcs)} records inspected individually; aliases allowed")

    crops_text = source("scripts/data/crops.lua")
    crop_body = balanced(crops_text, crops_text.index("{", crops_text.index("return")))[0]
    crops = records(crop_body)
    add("INFO", "registry_fact", "crops", f"{len(crops)} registered crops; runtime phase mapping NOT checked")
    for crop, body in crops:
        stages = re.search(r'\bstages\s*=\s*(\d+)', body)
        if not stages:
            raise ValueError("crop lacks literal stages: " + str(crop))
        base = "assets/art/crops/" + crop
        expected_stages = [f"{base}/stage_{i}.png" for i in range(1, int(stages[1]) + 1)]
        absent = [ref for ref in expected_stages if not (ROOT / ref).is_file()]
        if absent:
            add("FAIL", "missing_asset", base, f"Missing {len(absent)}/{len(expected_stages)} runtime-count stage files (1-based docs/57 convention)")
        for ref in expected_stages:
            if ref not in absent:
                png(ref, (32, 32))
    add("INFO", "not_checked", "crop art target", "4-6 target stages, wither/quality/harvest animation, and runtime crop renderer not validated")

    item_paths = sorted((ROOT / "scripts/data").glob("items*.lua")) + [ROOT / "scripts/data/tools.lua"]
    icon_refs = []
    for path in item_paths:
        text = clean(path.read_text(encoding="utf-8"))
        icon_refs.extend(re.findall(r'\bicon\s*=\s*"(assets/art/items/[\w/-]+\.png)"', text))
    fish = dict(records(table(source("scripts/data/fish.lua"), "M.fish")))
    fish_text = source("scripts/data/items_fish.lua")
    fish_map = dict(re.findall(r'(\w+)\s*=\s*"(assets/art/items/[\w/-]+\.png)"', table(fish_text, "fish_icons")))
    for ref in sorted(set(icon_refs) | set(fish_map.values())):
        png(ref, (32, 32))
    add("INFO", "wiring_fact", "item icons", f"{len(icon_refs)} literal icon fields; {len(set(icon_refs))} unique direct paths; {len(fish_map)} explicit fish-map paths")
    add("INFO", "registry_fact", "fish", f"{len(fish)} fish; map has {len(fish_map)} keys; no explicit fish_icons keys: {sorted(set(fish) - set(fish_map))}; items_fish.lua base-item collision guard requires runtime review")
    item_files = sorted((ROOT / "assets/art/items").glob("*.png"))
    previews = [x for x in item_files if x.name.startswith("_") or x.stem == "preview"]
    add("INFO", "asset_fact", "item inventory", f"{len(item_files)} PNG files, {len(previews)} preview files, {len(item_files)-len(previews)} other files; count is NOT originality/readability approval")

    seasons = records(table(source("scripts/data/season_art.lua"), "M.ground"))
    season_count = 0
    for map_id, body in seasons:
        pattern = field(body, "pattern")
        if not pattern or pattern.count("%s") != 1:
            raise ValueError("unsupported season pattern for " + map_id)
        names = re.findall(r'"(spring|summer|autumn|winter)"', table(body, "seasons"))
        if not names:
            raise ValueError("empty season list for " + map_id)
        for name in names:
            png(pattern % name)
            season_count += 1
    add("INFO", "wiring_fact", "seasons", f"{len(seasons)} registered maps, {season_count} expanded texture paths; runtime switching NOT checked")

    scenes = sorted((ROOT / "scenes").glob("*.scene"))
    if not scenes:
        raise ValueError("no scene inputs")
    for path in scenes:
        lines = [line for line in path.read_text(encoding="utf-8").splitlines() if line.startswith("component\t")]
        text = "\n".join(lines)
        refs = sorted(set(re.findall(r'(?:texture|sheet)=(assets/[^;\s]+)', text)))
        for ref in refs:
            if not (ROOT / ref).is_file():
                add("FAIL", "missing_asset", ref, "Explicit scene reference in " + path.name)
        tilemaps = [line for line in lines if "\tTilemap\t" in line]
        add("INFO", "scene_fact", path.name, f"{len(refs)} texture/sheet paths, {len(tilemaps)} Tilemap components; no tile encoding or draw-order completion inferred")
    hud = (ROOT / "assets/prefabs/hud.prefab").read_text(encoding="utf-8")
    white = sum("\tImage\ttexture=__white__" in line for line in hud.splitlines() if line.startswith("component\t"))
    add("INFO", "wiring_fact", "HUD", f"{white} Image components use __white__; intentional bars/effects must be reviewed, not all are missing art")
    config = (ROOT / "project.toml").read_text(encoding="utf-8")
    add("INFO", "wiring_fact", "font", f"font={field(config, 'font')}; text_render_mode={field(config, 'text_render_mode')}; fonts on disk do not prove activation")
    return findings


def self_test():
    fixture = clean('M.npcs = { -- character = "fake"\n{persist_id="npc_one", character="alias"}, {persist_id="npc_two", sprite="x{y}"}}')
    rows = records(table(fixture, "M.npcs"))
    assert len(rows) == 2 and field(rows[0][1], "character") == "alias"
    assert field(rows[1][1], "character") is None
    assert len(records(table('M.ground={a={pattern="a%s", seasons={"spring"}},b={pattern="b%s",seasons={"winter"}}}', 'M.ground'))) == 2
    assert clean('x="a--b" -- ignored').strip() == 'x="a--b"'
    assert len(records(table('M.fish={a={hours={from=1,to=2}}}', 'M.fish'))) == 1
    print("PASS: static parser fixtures (comments, alias, nested braces, last entry without comma)")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    try:
        if args.self_test:
            self_test()
            return 0
        findings = audit()
    except (OSError, ValueError, UnicodeDecodeError, struct.error) as exc:
        print("AUDIT ERROR: " + str(exc), file=sys.stderr)
        return 2
    counts = dict(sorted(Counter(x["level"] for x in findings).items()))
    if args.json:
        print(json.dumps(dict(schema_version=1, counts=counts, findings=findings), ensure_ascii=False, indent=2))
    else:
        for item in findings:
            print("[{level}] {kind}: {subject} | {detail}".format(**item))
        print("SUMMARY " + json.dumps(counts, sort_keys=True))
    return 1 if counts.get("FAIL", 0) else 0


if __name__ == "__main__":
    sys.exit(main())
