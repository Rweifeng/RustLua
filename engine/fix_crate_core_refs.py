"""Fix crate::core:: references in the split rustlua-* crates.

In the monolithic engine crate, everything was accessible as crate::core::module::Item.
After the split, each rustlua-* crate has its own namespace:
  - Modules within the same crate: crate::core::X -> crate::X
  - Modules in other crates: crate::core::X -> rustlua_OTHER_CRATE::X
  - Free functions: crate::core::games_dir() -> rustlua_infra::games_dir()
"""

import os
import re

CRATES_DIR = r"C:\GitHub\RustLua\engine\crates"

# Module -> which rustlua-* crate it lives in
MOD_TO_CRATE = {
    # rustlua-math
    "math3d": "rustlua-math",
    "math2d": "rustlua-math",
    "geom3d": "rustlua-math",
    # rustlua-ecs
    "entity": "rustlua-ecs",
    "dense_storage": "rustlua-ecs",
    "world": "rustlua-ecs",
    "commands": "rustlua-ecs",
    "component_schema": "rustlua-ecs",
    "data_schema": "rustlua-ecs",
    "property_path": "rustlua-ecs",
    "hierarchy": "rustlua-ecs",
    "spatial_transform": "rustlua-ecs",
    "transform": "rustlua-ecs",
    "scheduler": "rustlua-ecs",
    "events": "rustlua-ecs",
    "entity_script": "rustlua-ecs",
    "particle": "rustlua-ecs",
    "tilemap": "rustlua-ecs",
    # rustlua-scene
    "scene": "rustlua-scene",
    "scene_extract": "rustlua-scene",
    "scene_space": "rustlua-scene",
    "scene_migration": "rustlua-scene",
    "typed_scene": "rustlua-scene",
    "prefab": "rustlua-scene",
    "world_text": "rustlua-scene",
    "char_collect": "rustlua-scene",
    # rustlua-camera
    "camera": "rustlua-camera",
    # rustlua-animation
    "animation": "rustlua-animation",
    "animation3d": "rustlua-animation",
    "animation_track": "rustlua-animation",
    # rustlua-render
    "render_mode": "rustlua-render",
    "render_plan": "rustlua-render",
    "renderable": "rustlua-render",
    "quad": "rustlua-render",
    "sprite": "rustlua-render",
    "sprite_batch": "rustlua-render",
    "layer": "rustlua-render",
    "visibility": "rustlua-render",
    "spatial_index": "rustlua-render",
    "picking": "rustlua-render",
    "post_process": "rustlua-render",
    "light": "rustlua-render",
    "lod": "rustlua-render",
    "mesh3d": "rustlua-render",
    "texture": "rustlua-render",
    "atlas": "rustlua-render",
    "font": "rustlua-render",
    "font_subset": "rustlua-render",
    "text": "rustlua-render",
    # rustlua-ui
    "ui": "rustlua-ui",
    "ui_animation": "rustlua-ui",
    "ui_binding": "rustlua-ui",
    "ui_events": "rustlua-ui",
    "ui_layout": "rustlua-ui",
    "ui_layout_cache": "rustlua-ui",
    "ui_navigation": "rustlua-ui",
    "ui_scene_layout": "rustlua-ui",
    "ui_states": "rustlua-ui",
    # rustlua-physics
    "physics": "rustlua-physics",
    "physics3d": "rustlua-physics",
    # rustlua-assets
    "asset_handle": "rustlua-assets",
    "async_loader": "rustlua-assets",
    "import_cache": "rustlua-assets",
    "importer": "rustlua-assets",
    "resource_db": "rustlua-assets",
    "resource_kind": "rustlua-assets",
    "pack": "rustlua-assets",
    "gltf_asset": "rustlua-assets",
    "hash": "rustlua-assets",
    "dependency": "rustlua-assets",
    # rustlua-audio
    "audio": "rustlua-audio",
    # rustlua-input
    "input": "rustlua-input",
    # rustlua-gameplay
    "gameplay": "rustlua-gameplay",
    "grid": "rustlua-gameplay",
    "save": "rustlua-gameplay",
    "settings": "rustlua-gameplay",
    # rustlua-scripting
    "script": "rustlua-scripting",
    # rustlua-infra
    "logging": "rustlua-infra",
    "performance": "rustlua-infra",
    "paths": "rustlua-infra",
    "project_config": "rustlua-infra",
    "user_dir": "rustlua-infra",
    "vfs": "rustlua-infra",
    "window": "rustlua-infra",
    "time_step": "rustlua-infra",
    "editor_plugins": "rustlua-infra",
    "preview_diagnostics": "rustlua-infra",
    "par": "rustlua-infra",
}

# Crate name -> module names it contains
CRATE_TO_MODS = {}
for mod, crate_name in MOD_TO_CRATE.items():
    CRATE_TO_MODS.setdefault(crate_name, set()).add(mod)

# Free functions/constants that live in rustlua-infra
INFRA_FUNCTIONS = {
    "games_dir",
    "fixture_games_dir",
    "version",
    "PROJECT_WORK_DIR",
}


def get_crate_name(dir_name):
    """Extract crate name from directory name like 'rustlua-animation'."""
    return dir_name


def fix_file(filepath, current_crate):
    """Fix crate::core:: references in a single file."""
    with open(filepath, "r", encoding="utf-8") as f:
        content = f.read()

    original = content

    # Pattern: crate::core::MODULE::rest or crate::core::MODULE
    # Also: crate::core::FUNCTION() for free functions
    def replace_crate_core(m):
        full = m.group(0)
        after_core = m.group(1)  # everything after "crate::core::"

        # Check if it's a free function/constant from infra
        for func in INFRA_FUNCTIONS:
            if after_core.startswith(func):
                remainder = after_core[len(func):]
                return f"rustlua_infra::{func}{remainder}"

        # Extract the module name (first path segment)
        parts = after_core.split("::", 1)
        mod_name = parts[0]
        rest = "::" + parts[1] if len(parts) > 1 else ""

        if mod_name not in MOD_TO_CRATE:
            # Unknown module - leave as-is (will be a compile error)
            return full

        target_crate = MOD_TO_CRATE[mod_name]

        if target_crate == current_crate:
            # Same crate: crate::core::X -> crate::X
            return f"crate::{mod_name}{rest}"
        else:
            # Different crate: crate::core::X -> rustlua_other::X
            return f"{target_crate}::{mod_name}{rest}"

    # Match crate::core:: followed by something (not in doc-comment-only context)
    content = re.sub(r"crate::core::(\w[\w:]*)", replace_crate_core, content)

    if content != original:
        with open(filepath, "w", encoding="utf-8") as f:
            f.write(content)
        return True
    return False


def process_crate(crate_dir):
    """Process all .rs files in a crate."""
    crate_name = os.path.basename(crate_dir)
    src_dir = os.path.join(crate_dir, "src")
    if not os.path.isdir(src_dir):
        return 0

    changed = 0
    for root, dirs, files in os.walk(src_dir):
        for fname in files:
            if fname.endswith(".rs"):
                filepath = os.path.join(root, fname)
                if fix_file(filepath, crate_name):
                    changed += 1
    return changed


def main():
    total_changed = 0
    # Only process rustlua-* crates (not engine, game, editor, etc.)
    for entry in sorted(os.listdir(CRATES_DIR)):
        if not entry.startswith("rustlua-"):
            continue
        crate_dir = os.path.join(CRATES_DIR, entry)
        if not os.path.isdir(crate_dir):
            continue
        changed = process_crate(crate_dir)
        if changed:
            print(f"  {entry}: {changed} files changed")
            total_changed += changed
    print(f"\nTotal: {total_changed} files changed")


if __name__ == "__main__":
    main()
