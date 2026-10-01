//! `examples/game_api` 的**工程级门**：示例工程是"活文档"，它的价值全在"真的能跑"。
//!
//! 四条漂移只有跑起来才看得见，所以这里各有一条门钉住：
//!
//! | 门 | 钉住的漂移 |
//! |---|---|
//! | [`sample_project_runs_every_scene_headless`] | 场景加载不了、脚本在实例化或步进期报错 |
//! | [`sample_project_scripts_all_compile`] | 演示模块语法错（`require` 不到就整段失效） |
//! | [`sample_project_demos_cover_every_lua_api`] | 引擎加了 API 却没有演示 |
//! | [`sample_project_scenes_cover_every_runtime_component`] | 引擎加了组件却没进场景 |
//! | [`sample_project_index_menu_lists_every_scene`] | 新增演示场景却没进入口菜单 |
//! | [`sample_project_index_click_opens_the_demo`] | 菜单点击没有真的切场景 |
//!
//! 覆盖面的**事实源是运行期**（`_G` 里的表与函数），不是任何文档或手抄清单：
//! 引擎新注册一个 Lua 函数，这条门立刻要求示例工程补上演示。

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use super::RuntimeScene;
use crate::core::{
    input::{InputSnapshot, MouseButton},
    scene::Scene,
    script::{LUA_API_PROBE, LuaRuntime},
    typed_scene::ComponentId,
};

fn project_root() -> PathBuf {
    crate::core::games_dir().join("examples").join("game_api")
}

/// 入口菜单自身的场景名：它是演示的**目录**，不是演示之一，因此不进 `demos.lua`。
const MENU_SCENE: &str = "index";

/// 工程里的全部场景文件（按名字排序）。
fn scene_files(root: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(root.join("scenes"))
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", root.join("scenes").display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "scene")
        })
        .collect::<Vec<_>>();
    files.sort();
    assert!(!files.is_empty(), "示例工程没有任何场景");
    files
}

/// 递归收集 `.lua` 文件。
fn lua_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            lua_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "lua") {
            out.push(path);
        }
    }
}

fn script_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    lua_files(&root.join("scripts"), &mut files);
    files.sort();
    assert!(!files.is_empty(), "示例工程没有任何脚本");
    files
}

fn scene_name(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `scripts/demos.lua` 里登记的演示场景名（清单是唯一真源）。
fn registered_scenes(root: &Path) -> BTreeSet<String> {
    let text = fs::read_to_string(root.join("scripts").join("demos.lua"))
        .expect("读取 scripts/demos.lua 失败");
    let mut scenes = BTreeSet::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("scene =") else {
            continue;
        };
        if let Some(start) = rest.find('"')
            && let Some(end) = rest[start + 1..].find('"')
        {
            scenes.insert(rest[start + 1..start + 1 + end].to_owned());
        }
    }
    assert!(
        scenes.len() >= 10,
        "scripts/demos.lua 里只解析出 {} 个演示场景，清单可能被改坏了",
        scenes.len()
    );
    scenes
}

/// 去掉 Lua 的注释与字符串字面量，只留**代码**。
///
/// 为什么必须去掉字符串：门的判据是"真的调用了这个 API"。有两个真实反例——
/// `map.goto(` 在 Lua 里**根本写不出来**（`goto` 是 5.2+ 保留字，真实调用只能写
/// `map["goto"](`），而屏显文案/注释里写 `map.goto(` 却毫无成本；`input.actions()` 之类的
/// 名字也常出现在提示文本里。去掉字符串后，只有真正的调用点才会命中。
fn strip_lua_noise(source: &str) -> String {
    #[derive(PartialEq)]
    enum State {
        Code,
        LineComment,
        LongComment(usize),
        Text(u8),
        LongText(usize),
    }

    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut state = State::Code;
    let mut index = 0;
    // 长括号的数量：`[[` / `[=[` / `[==[` …（注释与长字符串共用同一套定界符）。
    let long_bracket = |index: usize| -> Option<(usize, usize)> {
        if bytes.get(index) != Some(&b'[') {
            return None;
        }
        let mut level = 0;
        let mut cursor = index + 1;
        while bytes.get(cursor) == Some(&b'=') {
            level += 1;
            cursor += 1;
        }
        (bytes.get(cursor) == Some(&b'[')).then_some((level, cursor))
    };
    let closes_long = |index: usize, level: usize| -> Option<usize> {
        if bytes.get(index) != Some(&b']') {
            return None;
        }
        let mut cursor = index + 1;
        for _ in 0..level {
            if bytes.get(cursor) != Some(&b'=') {
                return None;
            }
            cursor += 1;
        }
        (bytes.get(cursor) == Some(&b']')).then_some(cursor)
    };

    while index < bytes.len() {
        let byte = bytes[index];
        match state {
            State::Code => {
                if byte == b'-' && bytes.get(index + 1) == Some(&b'-') {
                    state = match long_bracket(index + 2) {
                        Some((level, _)) => State::LongComment(level),
                        None => State::LineComment,
                    };
                    out.push(' ');
                    index += 2;
                    continue;
                }
                if byte == b'"' || byte == b'\'' {
                    state = State::Text(byte);
                    out.push(' ');
                    index += 1;
                    continue;
                }
                if let Some((level, end)) = long_bracket(index) {
                    state = State::LongText(level);
                    out.push(' ');
                    index = end + 1;
                    continue;
                }
                out.push(byte as char);
                index += 1;
            }
            State::LineComment => {
                if byte == b'\n' {
                    state = State::Code;
                    out.push('\n');
                }
                index += 1;
            }
            State::LongComment(level) => {
                if let Some(end) = closes_long(index, level) {
                    state = State::Code;
                    index = end + 1;
                } else {
                    index += 1;
                }
            }
            State::Text(quote) => {
                if byte == b'\\' {
                    index += 2;
                    continue;
                }
                if byte == quote {
                    state = State::Code;
                }
                index += 1;
            }
            State::LongText(level) => {
                if let Some(end) = closes_long(index, level) {
                    state = State::Code;
                    index = end + 1;
                } else {
                    index += 1;
                }
            }
        }
    }
    out
}

/// 全部演示脚本的比对文本：`(去注释去字符串, 只去注释)`。
///
/// 为什么要两份：`map["goto"](...)` 的正确性判据需要**看得到**下标里的那个字符串
/// （`goto` 是保留字，`map.goto(` 写不出来），但普通的 `t.f(` 又必须排除"只在提示文案里
/// 出现过"的假命中。因此普通调用看剥掉字符串的那份，下标调用看保留字符串的那份。
fn demo_sources(root: &Path) -> (String, String) {
    let mut stripped = String::new();
    let mut raw = String::new();
    for path in script_files(root) {
        let source = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("读取 {} 失败：{error}", path.display());
        });
        stripped.push_str(&strip_lua_noise(&source));
        stripped.push('\n');
        raw.push_str(&strip_lua_comments_only(&source));
        raw.push('\n');
    }
    (stripped, raw)
}

/// 只去掉注释（行注释与长注释），字符串字面量原样保留。
///
/// 与 [`strip_lua_noise`] 是同一个扫描骨架的"轻量版"：这里**不**跟踪字符串的结束位置，
/// 因为唯一目的是把注释抹掉；字符串里的 `--` 极少见，且误判只会让比对文本更宽松一点点。
fn strip_lua_comments_only(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'-' && bytes.get(index + 1) == Some(&b'-') {
            // 长注释 `--[[ … ]]` / `--[=[ … ]=]`
            let mut level = 0;
            let mut cursor = index + 2;
            if bytes.get(cursor) == Some(&b'[') {
                cursor += 1;
                while bytes.get(cursor) == Some(&b'=') {
                    level += 1;
                    cursor += 1;
                }
                if bytes.get(cursor) == Some(&b'[') {
                    cursor += 1;
                    loop {
                        if cursor >= bytes.len() {
                            index = cursor;
                            break;
                        }
                        if bytes[cursor] == b']' {
                            let mut probe = cursor + 1;
                            let mut seen = 0;
                            while seen < level && bytes.get(probe) == Some(&b'=') {
                                probe += 1;
                                seen += 1;
                            }
                            if seen == level && bytes.get(probe) == Some(&b']') {
                                index = probe + 1;
                                break;
                            }
                        }
                        cursor += 1;
                    }
                    out.push(' ');
                    continue;
                }
            }
            // 行注释：吃到行尾（换行保留，行号不错位）。
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        out.push(byte as char);
        index += 1;
    }
    out
}

fn lua_string_literal(source: &str) -> String {
    assert!(
        !source.contains("]==========]"),
        "脚本里出现长括号定界符，换一个定界符再编译"
    );
    source.to_owned()
}

#[test]
fn sample_project_runs_every_scene_headless() {
    let root = project_root();
    let mut stepped = 0;
    for path in scene_files(&root) {
        let name = scene_name(&path);
        let scene = Scene::load(&path).unwrap_or_else(|error| panic!("加载 {name} 失败：{error}"));
        let mut runtime = RuntimeScene::from_scene(Some(&root), scene)
            .unwrap_or_else(|error| panic!("实例化 {name} 失败：{error}"));
        // 三步足够让"每帧都跑的 update()"暴露参数错、缺实体、越界这类问题。
        runtime
            .run_script_steps(3, 1.0 / 60.0, &InputSnapshot::default())
            .unwrap_or_else(|error| panic!("步进 {name} 失败：{error}"));
        stepped += 1;
    }
    assert!(
        stepped >= 10,
        "示例工程的场景太少（{stepped}），门没有实际意义"
    );
}

#[test]
fn sample_project_scripts_all_compile() {
    let root = project_root();
    let mut runtime = LuaRuntime::new().expect("lua vm");
    runtime.set_project_root(root.clone());
    let files = script_files(&root);
    for path in &files {
        let source = fs::read_to_string(path).expect("读取脚本失败");
        let wrapped = format!(
            "local chunk, err = load([==========[\n{}]==========], \"@{name}\")\n\
             if not chunk then error(err, 0) end\n\
             return \"ok\"",
            lua_string_literal(&source),
            // 块名要转义反斜杠：Lua 字符串里的 `\C` 是非法转义序列（Windows 路径必踩）。
            name = path.display().to_string().replace('\\', "/")
        );
        runtime
            .execute_console(&wrapped)
            .unwrap_or_else(|error| panic!("{} 编译失败：{error}", path.display()));
    }
    assert!(files.len() >= 10, "示例工程的脚本太少（{}）", files.len());
}

/// `scripts/api_demo.lua` 的共享取景框（状态行 + 按键提示）所占的屏幕带：贴屏幕底部往上排，
/// 因此最坏情况（多行状态 + 多行提示）会一直占到 y≈600。
///
/// 它们都是 **Lua 动态 UI**，而场景里的 `Text`/`Image` 永远画在动态 UI **之上**
/// （`runtime_scene.rs` 的 `push_scene_ui` 排在 `push_ui_text` 之后）。因此"场景实体压住
/// 取景框"是这类工程最常见的可见缺陷：脚本照常跑、数值照常写，屏幕上却什么都没有。
///
/// 判据刻意只抓**大面积**覆盖（铺满屏幕的背景、整条横栏）：小控件（按钮、色块、把手）
/// 贴到状态行附近是合理的布局选择，一刀切会把它们全判红。容器子实体的 rect 在
/// `UiRectLayout::resolve` 里是**相对父矩形**的，因此先按"是否落在视口内"过滤掉。
const OVERLAY_BAND: (f32, f32, f32, f32) = (16.0, 600.0, 1264.0, 710.0);
/// 覆盖宽/高达到这个量级才算"挡住了读数"。
const OVERLAY_COVER_MIN: (f32, f32) = (400.0, 20.0);

/// 场景声明的 UI 实体不得大面积压住共享取景框。
#[test]
fn sample_project_shared_overlay_is_not_covered() {
    let root = project_root();
    let viewport = [1280.0, 720.0];
    let (band_left, band_top, band_right, band_bottom) = OVERLAY_BAND;
    let mut offenders = Vec::new();
    for path in scene_files(&root) {
        let scene = Scene::load(&path).unwrap_or_else(|error| panic!("加载场景失败：{error}"));
        for entity in scene.entities.values() {
            let Some(payload) = entity.components.get("UiLayout") else {
                continue;
            };
            let Ok(layout) = crate::core::ui_layout::UiRectLayout::from_component_str(payload)
            else {
                continue;
            };
            let rect = layout.resolve(viewport);
            // 容器子实体的 rect 是相对父矩形的，落不进视口；这类不参与判定。
            let inside_viewport = rect.x >= -1.0
                && rect.y >= -1.0
                && rect.right() <= viewport[0] + 1.0
                && rect.bottom() <= viewport[1] + 1.0;
            if !inside_viewport {
                continue;
            }
            let overlap_w = rect.right().min(band_right) - rect.x.max(band_left);
            let overlap_h = rect.bottom().min(band_bottom) - rect.y.max(band_top);
            if overlap_w >= OVERLAY_COVER_MIN.0 && overlap_h >= OVERLAY_COVER_MIN.1 {
                offenders.push(format!(
                    "{}: 实体 {} 的 UI 矩形 ({:.0},{:.0})-({:.0},{:.0}) 挡住状态/提示行 {:.0}x{:.0}px",
                    scene_name(&path),
                    entity.name,
                    rect.x,
                    rect.y,
                    rect.right(),
                    rect.bottom(),
                    overlap_w,
                    overlap_h
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "共享取景框被场景 UI 覆盖（脚本的读数会看不见）：\n  - {}\n\
         修法：把铺满屏幕的背景从场景 `Image`/`Text` 换成 `Quad`（世界通道）或脚本里的 \
         `ui.panel`（动态面板在动态文本之下、世界内容之上）。",
        offenders.join("\n  - ")
    );
}

/// 场景里声明的 `Text` / `WorldText` 必须能用工程字体完整画出来。
///
/// 判据用 `ui.measure_text`（与渲染同口径：同一种字体、同一套 advance），
/// `missing_count > 0` 就意味着屏幕上会出现豆腐块/方框——示例工程的文案是给人看的，
/// 缺字等于文案没写。
#[test]
fn sample_project_scene_text_has_every_glyph() {
    let root = project_root();
    let mut runtime = LuaRuntime::new().expect("lua vm");
    runtime.set_project_root(root.clone());
    let mut offenders = Vec::new();
    let mut checked = 0;
    for path in scene_files(&root) {
        let scene = Scene::load(&path).unwrap_or_else(|error| panic!("加载场景失败：{error}"));
        for entity in scene.entities.values() {
            for (component, payload) in &entity.components {
                let text = match component.as_str() {
                    "Text" => crate::core::ui::UiText::from_component_str(payload)
                        .ok()
                        .map(|text| text.text),
                    "WorldText" => crate::core::world_text::WorldText::from_component_str(payload)
                        .ok()
                        .map(|text| text.text.text),
                    _ => None,
                };
                let Some(text) = text else { continue };
                if text.trim().is_empty() || text.contains("]==========]") {
                    continue;
                }
                checked += 1;
                let probe = format!(
                    "(function() local m = ui.measure_text([==========[{text}]==========], 14) \
                     return {{ missing_count = m.missing_count, missing = m.missing }} end)()"
                );
                let value = runtime.eval_json(&probe).unwrap_or_else(|error| {
                    panic!("度量 {}.{} 失败：{error}", scene_name(&path), entity.name)
                });
                let missing = value["missing_count"].as_u64().unwrap_or(0);
                if missing > 0 {
                    offenders.push(format!(
                        "{}: 实体 {} 的 {} 有 {} 个缺字（工程字体没有字形）：{}",
                        scene_name(&path),
                        entity.name,
                        component,
                        missing,
                        value["missing"].as_str().unwrap_or("?")
                    ));
                }
            }
        }
    }
    assert!(checked >= 20, "只检查到 {checked} 段文本，门没有实际意义");
    assert!(
        offenders.is_empty(),
        "场景文案在工程字体下缺字（屏幕上会是方框；改写文案或给 assets/fonts 补字形）：\n  - {}",
        offenders.join("\n  - ")
    );
}

#[test]
fn sample_project_demos_cover_every_lua_api() {
    let root = project_root();
    let runtime = LuaRuntime::new().expect("lua vm");
    let surface = runtime
        .eval_json(LUA_API_PROBE)
        .expect("导出运行时 API 面失败");
    let (sources, raw_sources) = demo_sources(&root);

    let mut total = 0;
    let mut missing = Vec::new();
    for table in surface["tables"].as_array().expect("tables") {
        let name = table["name"].as_str().expect("table name");
        for func in table["funcs"].as_array().expect("funcs") {
            let func = func.as_str().expect("func name");
            total += 1;
            // 直接调用 `ui.panel(...)` 与**把函数本身交给受控调用**
            // （`api.try(camera2d.get, ...)` / `pcall(light.set, ...)`，Lua 里这是惯用的
            // "调用它但容忍宿主帧缺失"写法）都算演示；用局部变量间接持有则不算。
            // `map["goto"](...)` 也必须算：`goto` 是 Lua 5.2+ 的**保留字**，
            // `map.goto(` 语法上根本写不出来，索引写法是唯一能调用它的方式。
            let accepted = [
                format!("{name}.{func}("),
                format!("{name}.{func},"),
                format!("{name}.{func})"),
            ];
            let bracket = format!("{name}[\"{func}\"](");
            if !accepted.iter().any(|form| sources.contains(form))
                && !raw_sources.contains(&bracket)
            {
                missing.push(format!("{name}.{func}"));
            }
        }
    }
    assert!(
        total >= 250,
        "运行时 API 面只有 {total} 个函数，探针可能失效了"
    );
    assert!(
        missing.is_empty(),
        "示例工程缺少 {} 个 API 的演示（共 {total} 个）：{missing:#?}\n\
         补演示的位置：scripts/demo_*.lua（每个模块的演示都要求真的调用一次，注释不算）",
        missing.len()
    );
}

#[test]
fn sample_project_scenes_cover_every_runtime_component() {
    let root = project_root();
    let mut text = String::new();
    for path in scene_files(&root) {
        text.push_str(&fs::read_to_string(&path).expect("读取场景失败"));
        text.push('\n');
    }
    let mut missing = Vec::new();
    for component in ComponentId::ALL {
        let name = component.as_str();
        // 组件行形如 `component\t<实体 id>\t<组件名>\t<载荷>`。
        if !text.contains(&format!("\t{name}\t")) {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "示例工程没有任何场景用到这些运行时组件：{missing:?}"
    );
}

#[test]
fn sample_project_index_menu_lists_every_scene() {
    let root = project_root();
    let registered = registered_scenes(&root);
    let on_disk: BTreeSet<String> = scene_files(&root)
        .iter()
        .map(|path| scene_name(path))
        // 入口菜单自己不是演示：它不进清单（否则菜单里会出现"进入菜单"的卡片）。
        .filter(|name| name != MENU_SCENE)
        .collect();
    assert_eq!(
        registered, on_disk,
        "scripts/demos.lua 与 scenes/ 不一致：清单是入口菜单、路由与测试的共同真源"
    );

    let index = fs::read_to_string(root.join("scenes").join("index.scene")).expect("读取入口场景");
    let entities: BTreeSet<String> = index
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            match (fields.next(), fields.next(), fields.next()) {
                (Some("entity"), Some(_id), Some(name)) => Some(name.to_owned()),
                _ => None,
            }
        })
        .collect();
    for name in &registered {
        assert!(
            entities.contains(name),
            "入口菜单缺少 {name} 的卡片（实体名必须与场景名一致，点击回调直接用它 scene.load）"
        );
    }
    let cards = index.matches("UiEvents").count();
    assert!(
        cards >= registered.len(),
        "入口菜单只有 {cards} 个 UiEvents 卡片，但有 {} 个演示场景",
        registered.len()
    );
}

#[test]
fn sample_project_index_click_opens_the_demo() {
    let root = project_root();
    let registered = registered_scenes(&root);

    // 逐个走一遍"菜单点击 → scene.load → 过渡 → 新场景的演示模块 on_create"。
    // **每轮都重新实例化入口场景**：点击后 `active` 模块会变成目标场景的演示模块，
    // 而"点击菜单卡片"这件事只有入口场景的模块负责。
    for name in &registered {
        let scene = Scene::load(root.join("scenes").join("index.scene")).expect("加载入口场景");
        let mut runtime = RuntimeScene::from_scene(Some(&root), scene).expect("实例化入口场景");
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("入口场景步进");
        runtime
            .script
            .execute_console(&format!("on_click('{name}')"))
            .unwrap_or_else(|error| panic!("点击 {name} 的卡片失败：{error}"));
        runtime
            .script
            .process_scene_transition()
            .unwrap_or_else(|error| panic!("切到 {name} 失败：{error}"));
        let current = runtime
            .script
            .execute_console("return tostring(scene.reload())")
            .expect("读当前场景");
        assert_eq!(
            current.trim(),
            name.as_str(),
            "点击 {name} 的卡片后应当切到该场景"
        );
        // 新场景的工程脚本已重新 `on_create`，再步进一帧确认它的 update 能跑。
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .unwrap_or_else(|error| panic!("{name} 的演示步进失败：{error}"));
    }
}

/// 入口菜单的每张卡片都要能**被鼠标真的点到**：矩形来自 `UiLayout` 的真实解算
/// （`scene_entity_ui_layout_rects` 就是渲染用的那一份），点击走
/// `UiEvents` → 全局 `on_click` → 路由 → `scene.load` 的完整链路。
///
/// 只断言"有 UiEvents 组件"是不够的：卡片的 anchors/offsets 写错时组件仍在，
/// 但点在卡片上什么都不会发生。
#[test]
fn sample_project_index_cards_are_clickable() {
    let root = project_root();
    let registered = registered_scenes(&root);
    let viewport = [1280.0, 720.0];

    for name in &registered {
        let scene = Scene::load(root.join("scenes").join("index.scene")).expect("加载入口场景");
        let card = scene
            .entities
            .values()
            .find(|entity| entity.name == *name)
            .unwrap_or_else(|| panic!("入口场景缺少名为 {name} 的卡片实体"))
            .id;
        let mut runtime = RuntimeScene::from_scene(Some(&root), scene).expect("实例化入口场景");
        let rects = runtime.scene_entity_ui_layout_rects([card], viewport);
        let rect = *rects
            .get(&card.0)
            .unwrap_or_else(|| panic!("{name} 的卡片没有解算出矩形（UiLayout 缺失或非法）"));
        assert!(
            rect.w > 0.0 && rect.h > 0.0,
            "{name} 的卡片矩形是空的：{rect:?}"
        );

        let mut input = InputSnapshot::default();
        input.cursor_moved(
            (rect.x + rect.w / 2.0) as f64,
            (rect.y + rect.h / 2.0) as f64,
        );
        input.mouse_event(MouseButton::Left, true);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .unwrap_or_else(|error| panic!("点击 {name} 的卡片失败：{error}"));
        runtime
            .script
            .process_scene_transition()
            .unwrap_or_else(|error| panic!("点击 {name} 后切场景失败：{error}"));
        let current = runtime
            .script
            .execute_console("return tostring(scene.reload())")
            .expect("读当前场景");
        assert_eq!(
            current.trim(),
            name.as_str(),
            "在 {name} 卡片中心按左键后应当切到该场景（矩形 {rect:?}）"
        );
    }
}
