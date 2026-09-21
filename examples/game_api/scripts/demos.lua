-- 演示清单：**场景 ↔ 演示模块 ↔ 菜单文案**的唯一真源。
--
-- `scripts/router.lua` 用它做场景到模块的分发，`scripts/demo_index.lua` 用它做菜单点击，
-- `scenes/index.scene` 里的按钮实体名与这里的 `scene` 字段一一对应（名字就是场景名，
-- 因此点击回调可以直接把实体名喂给 `scene.load`）。
return {
    {
        scene = "main",
        module = "demo_main",
        label = "2D Core - sprite / quad / tilemap / physics",
    },
    {
        scene = "api_core",
        module = "demo_core",
        label = "Core - log / time / entity / event / save",
    },
    {
        scene = "api_gameplay",
        module = "demo_gameplay",
        label = "Gameplay - grid / clock / item / inv / shop / map",
    },
    {
        scene = "api_input",
        module = "demo_input",
        label = "Input - keys / actions / pad / rumble / settings",
    },
    {
        scene = "api_audio",
        module = "demo_audio",
        label = "Audio - buses / sfx / bgm / settings",
    },
    {
        scene = "api_ui",
        module = "demo_ui",
        label = "Runtime UI - panel / grid / modal / drag / focus",
    },
    {
        scene = "ui_test",
        module = "demo_ui_events",
        label = "Scene UI - UiEvents / ButtonStates / Container",
    },
    {
        scene = "api_camera",
        module = "demo_camera",
        label = "Cameras - camera2d / camera3d / picking",
    },
    {
        scene = "api_3d",
        module = "demo_3d",
        label = "3D - mesh / light / skin / physics3d",
    },
    {
        scene = "api_script",
        module = "demo_script",
        label = "Scripts - per-entity hooks / persist_id",
    },
    {
        scene = "api_tilemap",
        module = "demo_tilemap",
        label = "Tilemap - grid / cell meta / A* pathfinding",
    },
    {
        scene = "api_prefab",
        module = "demo_prefab",
        label = "Prefabs - instances are plain entities",
    },
    {
        scene = "unified",
        module = "demo_unified",
        label = "Unified - every renderable category in one scene",
    },
    {
        scene = "demo3d",
        module = "demo_fixture",
        label = "3D fixture - pinned by wp0_3d_golden",
    },
    {
        scene = "mixed",
        module = "demo_fixture",
        label = "Mixed fixture - pinned by wp0_mixed_golden",
    },
}
