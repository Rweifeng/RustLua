"""Rewrite `use crate::core::X::...` imports after splitting core into separate crates."""
import os, re, sys
from pathlib import Path
from collections import defaultdict

# Module name -> crate name mapping
MOD_TO_CRATE = {
    "math3d": "rustlua_math", "math2d": "rustlua_math", "geom3d": "rustlua_math",
    "entity": "rustlua_ecs", "dense_storage": "rustlua_ecs", "world": "rustlua_ecs",
    "commands": "rustlua_ecs", "component_schema": "rustlua_ecs", "data_schema": "rustlua_ecs",
    "property_path": "rustlua_ecs", "hierarchy": "rustlua_ecs", "spatial_transform": "rustlua_ecs",
    "transform": "rustlua_ecs", "scheduler": "rustlua_ecs", "events": "rustlua_ecs",
    "entity_script": "rustlua_ecs",
    "scene": "rustlua_scene", "scene_extract": "rustlua_scene", "scene_space": "rustlua_scene",
    "scene_migration": "rustlua_scene", "typed_scene": "rustlua_scene", "prefab": "rustlua_scene",
    "world_text": "rustlua_scene",
    "camera": "rustlua_camera",
    "animation": "rustlua_animation", "animation3d": "rustlua_animation",
    "animation_track": "rustlua_animation",
    "render_mode": "rustlua_render", "render_plan": "rustlua_render", "renderable": "rustlua_render",
    "quad": "rustlua_render", "sprite": "rustlua_render", "sprite_batch": "rustlua_render",
    "layer": "rustlua_render", "visibility": "rustlua_render", "spatial_index": "rustlua_render",
    "picking": "rustlua_render", "post_process": "rustlua_render", "light": "rustlua_render",
    "lod": "rustlua_render", "mesh3d": "rustlua_render", "texture": "rustlua_render",
    "atlas": "rustlua_render", "font": "rustlua_render", "font_subset": "rustlua_render",
    "text": "rustlua_render",
    "ui": "rustlua_ui", "ui_animation": "rustlua_ui", "ui_binding": "rustlua_ui",
    "ui_events": "rustlua_ui", "ui_layout": "rustlua_ui", "ui_layout_cache": "rustlua_ui",
    "ui_navigation": "rustlua_ui", "ui_scene_layout": "rustlua_ui", "ui_states": "rustlua_ui",
    "physics": "rustlua_physics", "physics3d": "rustlua_physics",
    "asset_handle": "rustlua_assets", "async_loader": "rustlua_assets",
    "import_cache": "rustlua_assets", "importer": "rustlua_assets",
    "resource_db": "rustlua_assets", "resource_kind": "rustlua_assets",
    "pack": "rustlua_assets", "gltf_asset": "rustlua_assets", "hash": "rustlua_assets",
    "dependency": "rustlua_assets",
    "audio": "rustlua_audio",
    "input": "rustlua_input",
    "gameplay": "rustlua_gameplay", "particle": "rustlua_gameplay", "tilemap": "rustlua_gameplay",
    "grid": "rustlua_gameplay", "save": "rustlua_gameplay", "settings": "rustlua_gameplay",
    "char_collect": "rustlua_gameplay",
    "script": "rustlua_scripting",
    "logging": "rustlua_infra", "performance": "rustlua_infra", "paths": "rustlua_infra",
    "project_config": "rustlua_infra", "user_dir": "rustlua_infra", "vfs": "rustlua_infra",
    "window": "rustlua_infra", "time_step": "rustlua_infra", "editor_plugins": "rustlua_infra",
    "preview_diagnostics": "rustlua_infra", "par": "rustlua_infra",
}

# Directory name -> crate name (for files inside a new crate)
DIR_TO_CRATE = {
    "rustlua-math": "rustlua_math", "rustlua-ecs": "rustlua_ecs",
    "rustlua-scene": "rustlua_scene", "rustlua-camera": "rustlua_camera",
    "rustlua-animation": "rustlua_animation", "rustlua-render": "rustlua_render",
    "rustlua-ui": "rustlua_ui", "rustlua-physics": "rustlua_physics",
    "rustlua-assets": "rustlua_assets", "rustlua-audio": "rustlua_audio",
    "rustlua-input": "rustlua_input", "rustlua-gameplay": "rustlua_gameplay",
    "rustlua-scripting": "rustlua_scripting", "rustlua-infra": "rustlua_infra",
}

def get_crate_for_file(filepath):
    """Determine which crate a file belongs to based on its directory."""
    parts = filepath.parts
    for part in parts:
        if part in DIR_TO_CRATE:
            return DIR_TO_CRATE[part]
    return None

def parse_use_items(rest):
    """Parse the items after `use crate::core::` into (module, sub_path) pairs.

    Handles:
    - `module::sub::path;`
    - `module::{A, B, C};`
    - `module::{A, sub::{B, C}};`  (nested)
    - Multi-line versions of the above
    """
    rest = rest.strip()
    if rest.endswith(";"):
        rest = rest[:-1].strip()

    results = []  # list of (module_name, full_path_after_module)

    # Find the first module name
    m = re.match(r'(\w+)(.*)', rest, re.DOTALL)
    if not m:
        return results

    mod_name = m.group(1)
    remaining = m.group(2).strip()

    if remaining.startswith("::{"):
        # Brace expansion: module::{A, B, C}
        brace_content = extract_brace(remaining[2:])  # skip ::
        if brace_content:
            items, after = brace_content
            for item in split_top_level(items):
                item = item.strip()
                if item:
                    results.append((mod_name, item))
        else:
            # Fallback: treat as single path
            results.append((mod_name, remaining))
    elif remaining.startswith("::"):
        remaining = remaining[2:]
        results.append((mod_name, remaining))
    elif remaining == "" or remaining == ";":
        results.append((mod_name, ""))
    else:
        results.append((mod_name, remaining))

    return results

def extract_brace(s):
    """Extract content within { }, handling nesting. Returns (content, remaining) or None."""
    s = s.strip()
    if not s.startswith("{"):
        return None
    depth = 0
    for i, c in enumerate(s):
        if c == '{':
            depth += 1
        elif c == '}':
            depth -= 1
            if depth == 0:
                return (s[1:i], s[i+1:])
    return None

def split_top_level(s):
    """Split comma-separated items at top level (not inside nested braces)."""
    items = []
    depth = 0
    current = []
    for c in s:
        if c == '{':
            depth += 1
            current.append(c)
        elif c == '}':
            depth -= 1
            current.append(c)
        elif c == ',' and depth == 0:
            items.append(''.join(current))
            current = []
        else:
            current.append(c)
    if current:
        items.append(''.join(current))
    return items

def rewrite_file(filepath):
    """Rewrite all `use crate::core::` imports in a single file."""
    text = filepath.read_text(encoding="utf-8")
    original = text
    my_crate = get_crate_for_file(filepath)

    # Strategy: find all use statements that reference crate::core
    # Handle both single-line and multi-line use statements

    # First, join multi-line use statements into single lines for parsing
    # We'll work with the full text using regex that spans lines

    # Pattern to match `use crate::core::...;` potentially spanning multiple lines
    # This handles brace groups that span multiple lines
    use_pattern = re.compile(
        r'use\s+crate::core::'  # the prefix
        r'('
        r'(?:\w+::)*\w+'        # simple path like `module::sub::item`
        r'|'
        r'\w+::\{[^}]*\}'       # brace group like `module::{A, B}` (non-greedy, no nesting)
        r'|'
        r'\w+'                   # just module name
        r')'
        r'\s*;',
        re.DOTALL
    )

    # More robust: manually find and parse use statements
    new_use_lines = []
    replacements = []  # (start, end, replacement_text)

    pos = 0
    while pos < len(text):
        # Find next `use crate::core::`
        idx = text.find("use crate::core::", pos)
        if idx == -1:
            break

        # Find the end of this use statement (the semicolon)
        # Need to handle nested braces
        start = idx
        i = idx + len("use crate::core::")
        brace_depth = 0
        while i < len(text):
            c = text[i]
            if c == '{':
                brace_depth += 1
            elif c == '}':
                brace_depth -= 1
            elif c == ';' and brace_depth == 0:
                break
            i += 1

        if i >= len(text):
            break

        end = i + 1  # include the semicolon
        use_stmt = text[start:end]

        # Extract the part after `use crate::core::`
        after_prefix = use_stmt[len("use crate::core::"):]

        # Parse the items
        items = parse_use_items(after_prefix)

        # Group by target crate
        by_crate = defaultdict(list)
        for mod_name, sub_path in items:
            target_crate = MOD_TO_CRATE.get(mod_name)
            if target_crate is None:
                # Unknown module, keep as-is (shouldn't happen)
                print(f"  WARNING: unknown module '{mod_name}' in {filepath}")
                by_crate[None].append(f"{mod_name}::{sub_path}" if sub_path else mod_name)
                continue

            if target_crate == my_crate:
                # Same crate: use crate::sub_path
                if sub_path:
                    by_crate["__local__"].append(f"{mod_name}::{sub_path}")
                else:
                    by_crate["__local__"].append(mod_name)
            else:
                # Different crate: use rustlua_X::mod_name::sub_path
                if sub_path:
                    by_crate[target_crate].append(f"{mod_name}::{sub_path}")
                else:
                    by_crate[target_crate].append(mod_name)

        # Build replacement use statements
        new_stmts = []

        # Local imports first
        if "__local__" in by_crate:
            for item in by_crate["__local__"]:
                new_stmts.append(f"use crate::{item};")

        # External imports (sorted by crate name for determinism)
        for crate_name in sorted(by_crate.keys()):
            if crate_name in ("__local__", None):
                continue
            for item in by_crate[crate_name]:
                new_stmts.append(f"use {crate_name}::{item};")

        # Handle unknown modules
        if None in by_crate:
            for item in by_crate[None]:
                new_stmts.append(f"use crate::core::{item};")

        replacement = "\n".join(new_stmts)
        replacements.append((start, end, replacement))
        pos = end

    # Apply replacements in reverse order to preserve positions
    for start, end, replacement in reversed(replacements):
        text = text[:start] + replacement + text[end:]

    if text != original:
        filepath.write_text(text, encoding="utf-8")
        return len(replacements)
    return 0

def main():
    base = Path("crates")
    total = 0
    for crate_dir in base.iterdir():
        if not crate_dir.name.startswith("rustlua-"):
            continue
        src_dir = crate_dir / "src"
        if not src_dir.exists():
            continue
        for rs_file in src_dir.rglob("*.rs"):
            count = rewrite_file(rs_file)
            if count > 0:
                print(f"  {rs_file.relative_to(base.parent)}: {count} use statements rewritten")
                total += count
    print(f"\nTotal: {total} use statements rewritten")

if __name__ == "__main__":
    main()
