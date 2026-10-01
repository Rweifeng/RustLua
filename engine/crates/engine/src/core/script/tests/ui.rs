use super::*;

#[test]
fn ui_text_at_accepts_optional_scale_and_font_size() {
    let mut runtime = LuaRuntime::new().unwrap();
    let path = script(
        "ui.text_at('small', 10, 10, 'a')\nui.text_at('big', 10, 30, 'b', 3)\nui.text_at('huge', 10, 50, 'c', nil, 24)\nfunction update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    runtime.call_update().unwrap();
    let texts = runtime.with_ui(|texts, _| texts.clone());
    assert_eq!(
        texts.get("small").expect("small text stored").scale,
        1.0,
        "no scale argument defaults to 1.0"
    );
    assert_eq!(
        texts.get("small").expect("small text stored").font_size,
        None,
        "no font_size argument stays None"
    );
    assert_eq!(
        texts.get("big").expect("big text stored").scale,
        3.0,
        "optional scale argument is honored"
    );
    let huge = texts.get("huge").expect("huge text stored");
    assert_eq!(huge.scale, 1.0, "scale stays default when font_size given");
    assert_eq!(
        huge.font_size,
        Some(24.0),
        "optional font_size argument is honored"
    );
    let _ = fs::remove_file(path);
}

/// `ui.scene_text_visible(name, visible)`：false 登记进隐藏集、非 false 移除；
/// 缺失（nil）与 true 均视为显示。语义与 web 兼容层 `__rustlua_scene_text_hidden` 对齐。
#[test]
fn ui_scene_text_visible_toggles_hidden_set() {
    let mut runtime = LuaRuntime::new().unwrap();
    let path =
        script("ui.scene_text_visible('BtnStartLabel', false)\nfunction update() return 'ok' end");
    runtime.execute_project_script(&path).unwrap();
    runtime.call_update().unwrap();
    assert!(
        runtime.scene_text_hidden().contains("BtnStartLabel"),
        "false registers the name as hidden"
    );
    let _ = fs::remove_file(path);
    let mut runtime = LuaRuntime::new().unwrap();
    let path = script(
        "ui.scene_text_visible('BtnStartLabel', true)\nui.scene_text_visible('BtnExitLabel', nil)\nfunction update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    runtime.call_update().unwrap();
    let hidden = runtime.scene_text_hidden();
    assert!(
        !hidden.contains("BtnStartLabel"),
        "true removes the name from the hidden set"
    );
    assert!(
        !hidden.contains("BtnExitLabel"),
        "nil (omitted) counts as visible"
    );
    let _ = fs::remove_file(path);
}

#[test]
fn ui_panels_images_grids_and_drag_lua_api() {
    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_ui_viewport(1280, 720);
    let path = script(
        "ui.panel('bg', 0, 0, 200, 200, 0.1, 0.1, 0.2, 1)\n\
         ui.image('icon', 10, 10, 32, 32, 'assets/tomato.png', 1, 0, 0)\n\
         ui.grid('bag', 10, 60, 4, 2, 40, 4)\n\
         ui.grid_cell('bag', 1, 'assets/tomato.png', 1, 0, 0, 1)\n\
         assert(ui.grid_hit('bag', 20, 70) == 1)\n\
         assert(ui.grid_hit('bag', 400, 70) == nil)\n\
         ui.modal('shop', 300, 100, 220, 140, 'Market')\n\
         assert(ui.drag_begin('bag', 1))\n\
         assert(ui.drag_active().index == 1)\n\
         function on_drag(from_id, from_index, to_id, to_index)\n\
           dragged = from_id .. ':' .. from_index .. ':' .. to_id .. ':' .. to_index\n\
         end\n\
         assert(ui.drag_end('bag', 2))\n\
         assert(dragged == 'bag:1:bag:2')\n\
         ui.tooltip('tip', 20, 280, 'Hoe', 120, 28)\n\
         ui.modal_hide('shop')\n\
         local icon = ui.image_get('icon')\n\
         assert(icon.texture == 'assets/tomato.png')\n\
         assert(icon.x == 10 and icon.y == 10 and icon.width == 32 and icon.height == 32)\n\
         assert(icon.r == 1 and icon.g == 0 and icon.b == 0)\n\
         assert(ui.image_get('missing') == nil)\n\
         ui.image('tmp', 0, 0, 8, 8, 'assets/tomato.png', 1, 1, 1)\n\
         ui.clear('tmp')\n\
         assert(ui.image_get('tmp') == nil)\n\
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let dynamic = runtime
        .with_dynamic_ui(|panels, images, grids| (panels.clone(), images.clone(), grids.clone()));
    assert!(dynamic.0.contains_key("bg"));
    assert!(dynamic.1.contains_key("icon"));
    assert!(dynamic.2.contains_key("bag"));
    assert_eq!(dynamic.2["bag"].cells.len(), 8);
    let _ = fs::remove_file(path);
}

/// `ui.measure_text` / `ui.text_get`（M30）：中文排版的度量与读回。
#[test]
fn measure_text_and_text_get_lua_api() {
    let root = std::env::temp_dir().join(format!("rustlua-measure-{}", std::process::id()));
    let fonts = root.join("assets").join("fonts");
    fs::create_dir_all(&fonts).unwrap();
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
    fs::copy(&fixture, fonts.join("NotoSansSC-VF.ttf")).unwrap();
    fs::write(
        root.join("project.toml"),
        "font = \"assets/fonts/NotoSansSC-VF.ttf\"\n",
    )
    .unwrap();

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    let path = script(
        "
         local cjk = ui.measure_text('江湖', 15)
         assert(cjk.missing == '' and cjk.missing_count == 0)
         assert(cjk.glyphs == 2 and cjk.lines == 1)
         assert(cjk.width > 0 and cjk.height > 0)
         local ascii = ui.measure_text('AB', 15)
         -- CJK 满宽、ASCII 半宽：这正是按字数裁剪会算错的地方。
         assert(ascii.width < cjk.width)
         local long = ui.measure_text('江湖', 30)
         assert(long.width > cjk.width * 1.9 and long.width < cjk.width * 2.1)
         local two = ui.measure_text('江湖\\n有田', 15)
         assert(two.lines == 2 and two.height > cjk.height)
         local emoji = ui.measure_text('江湖🙂', 15)
         assert(emoji.missing_count == 1 and emoji.missing == '🙂')
         assert(not pcall(function() return ui.measure_text('x', 0) end))
         assert(not pcall(function() return ui.measure_text(nil, 15) end))
         ui.text_at('hud:line', 12, 30, '江湖有田', 1.0, 15)
         ui.text_at('hud:scaled', 12, 60, '江湖', 2.0)
         local line = ui.text_get('hud:line')
         assert(line.text == '江湖有田' and line.x == 12 and line.y == 30)
         assert(line.size == 15 and line.scale == 1.0 and line.visible == true)
         -- 未给 font_size 时按渲染口径回落到 scale × EM_PX(12)。
         assert(ui.text_get('hud:scaled').size == 24)
         assert(ui.text_get('missing') == nil)
         function update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn click_event_dispatches_lua_on_click() {
    use crate::core::input::MouseButton;
    use crate::core::scene::{Scene, SceneEntityId};

    let root = std::env::temp_dir().join(format!("rustlua-click-{}", std::process::id()));
    let scenes = root.join("scenes");
    fs::create_dir_all(&scenes).unwrap();
    let mut menu = Scene::new("menu");
    menu.add_entity(SceneEntityId(1), "Start", None).unwrap();
    menu.entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "UiEvents".into(),
            "event=click;function=on_click".to_owned(),
        );
    menu.save(scenes.join("menu.scene")).unwrap();

    let mut runtime = LuaRuntime::new().unwrap();
    runtime.set_project_root(root.clone());
    let load = script("scene.load('menu')\nfunction update() return 'ok' end");
    runtime.execute_project_script(&load).unwrap();
    runtime.process_scene_transition().unwrap();

    // 定义全局 on_click + 检查用 update（同一脚本，VM 不替换）。
    let click = script(
        "function on_click(name) clicked = name end\nfunction update() return tostring(clicked or 'nil') end",
    );
    runtime.execute_project_script(&click).unwrap();
    let mut input = InputSnapshot::default();
    input.cursor_moved(50.0, 20.0);
    input.mouse_event(MouseButton::Left, true);
    // 命中矩形由运行时 compute_click_targets 预计算；此处直接传入覆盖点击点的目标。
    runtime
        .process_ui_clicks(
            &input,
            &[UiClickTarget {
                name: "Start".to_owned(),
                origin: [0.0, 0.0],
                size: [1280.0, 720.0],
                functions: vec!["on_click".to_owned()],
            }],
        )
        .unwrap();

    let result = runtime.call_update().unwrap();
    assert_eq!(
        result, "Start",
        "on_click('Start') fired for the structured UiEvents entity"
    );

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(load);
    let _ = fs::remove_file(click);
}

/// M20-02：`ui.focus_*` 焦点环——方向移动、环绕、网格列、越界与未知方向。
#[test]
fn ui_focus_lua_api_navigates_lists_and_grids() {
    let mut runtime = LuaRuntime::new().unwrap();
    runtime
        .execute_console(
            // 单列 3 项：下/上移动 + 边界环绕。
            "assert(ui.focus_open('list', 3) == 3)\n\
             assert(ui.focus_count('list') == 3)\n\
             assert(ui.focus_index('list') == nil)\n\
             assert(ui.focus_move('list', 'down') == 0)\n\
             assert(ui.focus_move('list', 'down') == 1)\n\
             assert(ui.focus_move('list', 'down') == 2)\n\
             assert(ui.focus_move('list', 'down') == 0)\n\
             assert(ui.focus_move('list', 'up') == 2)\n\
             assert(ui.focus_select('list', 1) == true)\n\
             assert(ui.focus_index('list') == 1)\n\
             assert(ui.focus_select('list', 9) == false)\n\
             assert(ui.focus_cycle('list', true) == 2)\n\
             assert(ui.focus_cycle('list', false) == 1)\n\
             do local ok = pcall(function() return ui.focus_move('list', 'sideways') end)\n\
                assert(not ok, 'unknown direction must error') end\n\
             assert(ui.focus_move('missing', 'down') == nil)\n\
             assert(ui.focus_close('list') == true)\n\
             assert(ui.focus_count('list') == 0)\n\
             return 'ok'",
        )
        .unwrap();
    runtime
        .execute_console(
            // 2 列网格：下移同列、右移同行、边界环绕。
            "ui.focus_open('grid', 4, 2)\n\
             assert(ui.focus_select('grid', 0) == true)\n\
             assert(ui.focus_move('grid', 'down') == 2)\n\
             assert(ui.focus_move('grid', 'down') == 0)\n\
             assert(ui.focus_move('grid', 'right') == 1)\n\
             assert(ui.focus_move('grid', 'right') == 0)\n\
             assert(ui.focus_cycle('grid', true) == 1)\n\
             return 'ok'",
        )
        .unwrap();
}

/// M20-02：`ui.scroll_*` 滚动状态——钳制、按行、滚入视口与可见区间。
#[test]
fn ui_scroll_lua_api_clamps_and_reveals_items() {
    let mut runtime = LuaRuntime::new().unwrap();
    runtime
        .execute_console(
            "ui.scroll_open('s', 100, 250)\n\
             assert(math.abs(ui.scroll_max('s') - 150) < 1e-6)\n\
             assert(math.abs(ui.scroll_by('s', 30) - 30) < 1e-6)\n\
             assert(math.abs(ui.scroll_offset('s') - 30) < 1e-6)\n\
             assert(math.abs(ui.scroll_by('s', -100) + 30) < 1e-6)\n\
             assert(math.abs(ui.scroll_lines('s', 2, 20) - 40) < 1e-6)\n\
             assert(math.abs(ui.scroll_page('s', 1) - 100) < 1e-6)\n\
             assert(math.abs(ui.scroll_offset('s') - 140) < 1e-6)\n\
             assert(math.abs(ui.scroll_page('s', 1) - 10) < 1e-6)\n\
             assert(math.abs(ui.scroll_offset('s') - 150) < 1e-6)\n\
             assert(math.abs(ui.scroll_page('s', 1)) < 1e-6)\n\
             assert(ui.scroll_to_visible('s', 20, 30) == true)\n\
             assert(math.abs(ui.scroll_offset('s') - 20) < 1e-6)\n\
             assert(ui.scroll_to_visible('s', 20, 30) == false)\n\
             local range = ui.scroll_visible('s')\n\
             assert(math.abs(range.start - 20) < 1e-6 and math.abs(range.stop - 120) < 1e-6)\n\
             ui.scroll_extents('s', 100, 60)\n\
             assert(math.abs(ui.scroll_max('s')) < 1e-6)\n\
             assert(math.abs(ui.scroll_offset('s')) < 1e-6)\n\
             assert(ui.scroll_offset('missing') == nil)\n\
             assert(ui.scroll_close('s') == true)\n\
             return 'ok'",
        )
        .unwrap();
}
