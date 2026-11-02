use super::*;
use crate::framegate::GuiSmoke;
use editor::{DEFAULT_FRAME_BUDGET_MS, EditorProfiler};
use engine::host::SceneColorFormat;
use rustlua_ecs::scheduler::{ScheduleStage, SystemTiming};
use rustlua_gpu::ReadbackMetrics;
use rustlua_render::post_process::Tonemap;
use rustlua_render::render_settings::{IblMode, IblTier};
use std::time::{Duration, Instant};

fn section<'a>(sections: &'a [PerformanceSection], title: &str) -> &'a PerformanceSection {
    sections
        .iter()
        .find(|section| section.title == title)
        .unwrap_or_else(|| panic!("缺少分组 {title}"))
}

/// 采样一批帧（其中一帧超预算）+ 一次面板/命令耗时 + 计数器/gauge。
fn sampled_report() -> ProfilerReport {
    let mut profiler = EditorProfiler::new(DEFAULT_FRAME_BUDGET_MS);
    let start = Instant::now();
    profiler.begin_frame(start);
    profiler.end_frame(start + Duration::from_millis(4));
    profiler.begin_frame(start);
    profiler.end_frame(start + Duration::from_millis(25));
    profiler.record_panel("hierarchy", Duration::from_millis(3));
    profiler.record_panel("inspector", Duration::from_millis(1));
    profiler.record_command("undo", Duration::from_millis(2));
    profiler.bump("resource_refresh_applied");
    profiler.bump("hierarchy_cache_hit");
    profiler.set_gauge("scene_entities", 7.0);
    profiler.set_gauge("resource_pending_imports", 2.0);
    profiler.report()
}

#[test]
fn sections_cover_frame_cache_and_editor_state() {
    let report = sampled_report();
    let layout = LayoutCacheStats {
        hits: 3,
        misses: 1,
        invalidations: 2,
        entries: 4,
    };
    let counts = EditorPerfCounts {
        undo_len: 5,
        undo_dropped: 1,
        hierarchy_rows: 6,
        resource_scans: 1,
    };
    let sections = performance_sections(&report, &layout, counts, None, None);

    let frame = section(&sections, "编辑器帧");
    assert!(frame.default_open);
    let p95 = frame
        .rows
        .iter()
        .find(|row| row.raw_key == "frame_p95_ms")
        .expect("帧分组必须有 p95");
    assert_eq!(p95.value, "25.00 ms");
    assert!(p95.warn, "p95 越过预算必须标警示：{p95:?}");

    let cache = section(&sections, "布局缓存");
    assert_eq!(
        cache
            .rows
            .iter()
            .find(|row| row.raw_key == "layout_cache_hit_rate")
            .expect("必须有命中率")
            .value,
        "75.0%"
    );

    // 旧 HUD 那一行的数字必须一个不少地出现在「编辑器状态」里。
    let state = section(&sections, "编辑器状态");
    for (key, value) in [
        ("history_undo_len", "5"),
        ("history_dropped_entries", "1"),
        ("hierarchy_visible_rows", "6"),
        ("scene_entities", "7"),
        ("resource_entries", "0"),
        ("resource_pending_imports", "2"),
        ("resource_refresh_applied", "1"),
    ] {
        let row = state
            .rows
            .iter()
            .find(|row| row.raw_key == key)
            .unwrap_or_else(|| panic!("编辑器状态缺少 {key}"));
        assert_eq!(row.value, value, "{key} 的值不对：{row:?}");
    }
    assert!(
        state
            .rows
            .iter()
            .find(|row| row.raw_key == "resource_pending_imports")
            .expect("待导入行")
            .warn,
        "有待导入资源时必须标警示"
    );
}

#[test]
fn snapshot_from_report_flags_registered_systems_missing_from_timings() {
    let timing = |name: &str, micros: u64| SystemTiming {
        name: name.to_owned(),
        stage: ScheduleStage::Script,
        elapsed: Duration::from_micros(micros),
    };
    let report = FrameScheduleReport {
        steps: 2,
        total_elapsed: Duration::from_micros(1_000),
        timings: vec![timing("lua_update", 400), timing("physics2d_step", 300)],
    };
    let step_names = vec![
        "lua_update".to_owned(),
        "physics2d_step".to_owned(),
        "physics3d_step".to_owned(),
    ];
    let frame_names = vec!["clock_step".to_owned()];
    let snapshot = SystemFrameSnapshot::from_report(&report, &step_names, &frame_names);
    assert_eq!(snapshot.steps, 2);
    assert_eq!(snapshot.total, Duration::from_micros(1_000));
    assert_eq!(snapshot.systems.len(), 2);
    assert_eq!(
        snapshot.missing,
        vec!["physics3d_step".to_owned(), "clock_step".to_owned()],
        "注册表有、报告无的系统必须被点名"
    );

    let sections = performance_sections(
        &sampled_report(),
        &LayoutCacheStats::default(),
        Default::default(),
        None,
        Some(&snapshot),
    );
    let schedule = section(&sections, "运行时 · 调度系统");
    assert!(
        schedule
            .rows
            .iter()
            .find(|row| row.raw_key == "runtime.schedule_missing")
            .expect("缺失系统行")
            .warn,
        "缺失系统必须在面板上判红"
    );

    // 0 步的帧只有帧级系统执行，不能把每步系统误报成"缺失"。
    let idle = FrameScheduleReport {
        steps: 0,
        total_elapsed: Duration::from_micros(50),
        timings: Vec::new(),
    };
    assert!(
        SystemFrameSnapshot::from_report(&idle, &step_names, &frame_names)
            .missing
            .is_empty(),
        "steps=0 时不做缺失判定"
    );
}

#[test]
fn schedule_section_lists_systems_by_cost_with_consistent_sum() {
    let snapshot = SystemFrameSnapshot {
        steps: 2,
        total: Duration::from_micros(2_500),
        systems: vec![
            SystemFrameRow {
                name: "ui".to_owned(),
                elapsed: Duration::from_micros(300),
            },
            SystemFrameRow {
                name: "physics".to_owned(),
                elapsed: Duration::from_micros(1_200),
            },
            SystemFrameRow {
                name: "script".to_owned(),
                elapsed: Duration::from_micros(900),
            },
        ],
        missing: Vec::new(),
    };
    let report = sampled_report();
    let sections = performance_sections(
        &report,
        &LayoutCacheStats::default(),
        Default::default(),
        None,
        Some(&snapshot),
    );
    let schedule = section(&sections, "运行时 · 调度系统");
    assert!(schedule.default_open);
    let value = |key: &str| {
        schedule
            .rows
            .iter()
            .find(|row| row.raw_key == key)
            .unwrap_or_else(|| panic!("调度分组缺少 {key}"))
            .value
            .clone()
    };
    assert_eq!(value("runtime.schedule_steps"), "2");
    assert_eq!(value("runtime.schedule_total_ms"), "2.500 ms");
    assert_eq!(value("runtime.schedule_system_sum_ms"), "2.400 ms（96.0%）");
    assert!(
        schedule
            .rows
            .iter()
            .find(|row| row.raw_key == "runtime.schedule_system_sum_ms")
            .expect("系统合计行")
            .warn
            .eq(&false),
        "4% 的差在 5% 阈值内，不应标红"
    );

    // 头五行是步数/总数/合计/开销/系统数，其余按耗时降序。
    let listed: Vec<(String, String)> = schedule.rows[5..]
        .iter()
        .map(|row| (row.label.clone(), row.value.clone()))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("physics".to_owned(), "1.200 ms · 48.0%".to_owned()),
            ("script".to_owned(), "0.900 ms · 36.0%".to_owned()),
            ("ui".to_owned(), "0.300 ms · 12.0%".to_owned()),
        ]
    );
    assert!(
        schedule.note.is_none(),
        "一致的报告不该有说明：{:?}",
        schedule.note
    );
}

#[test]
fn schedule_section_flags_inconsistent_and_missing_systems() {
    let inconsistent = SystemFrameSnapshot {
        steps: 1,
        total: Duration::from_millis(1),
        systems: vec![SystemFrameRow {
            name: "physics".to_owned(),
            elapsed: Duration::from_millis(2),
        }],
        missing: Vec::new(),
    };
    let sections = performance_sections(
        &sampled_report(),
        &LayoutCacheStats::default(),
        Default::default(),
        None,
        Some(&inconsistent),
    );
    let schedule = section(&sections, "运行时 · 调度系统");
    assert!(
        schedule
            .rows
            .iter()
            .find(|row| row.raw_key == "runtime.schedule_system_sum_ms")
            .expect("系统合计行")
            .warn,
        "系统合计超过总数 5% 必须判红"
    );
    assert!(
        schedule
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("超过调度总耗时 100.0%"),
        "说明要给出差值：{:?}",
        schedule.note
    );

    // 有步进但报告里一个 system 都没有：这正是"漏 system"的故障形态，必须判红。
    let empty = SystemFrameSnapshot {
        steps: 3,
        total: Duration::from_millis(1),
        systems: Vec::new(),
        missing: vec!["lua_update".to_owned(), "physics2d_step".to_owned()],
    };
    let sections = performance_sections(
        &sampled_report(),
        &LayoutCacheStats::default(),
        Default::default(),
        None,
        Some(&empty),
    );
    let schedule = section(&sections, "运行时 · 调度系统");
    let count = schedule
        .rows
        .iter()
        .find(|row| row.raw_key == "runtime.schedule_system_count")
        .expect("系统数行");
    assert_eq!(count.value, "0");
    assert!(count.warn, "没有 per-system 计时必须判红");
    let missing = schedule
        .rows
        .iter()
        .find(|row| row.raw_key == "runtime.schedule_missing")
        .expect("缺失系统行");
    assert!(missing.warn, "注册了但没计时的系统必须判红");
    assert_eq!(missing.value, "lua_update, physics2d_step");
    assert!(
        schedule
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("缺 system 判红"),
        "说明要指出缺失的系统：{:?}",
        schedule.note
    );
    assert!(
        schedule
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("per-system"),
        "说明要指出缺少 per-system 计时：{:?}",
        schedule.note
    );
    assert!(
        schedule_sections(None).is_empty(),
        "未 Play 时不该有调度分组"
    );
}

#[test]
fn schedule_section_truncates_long_system_lists() {
    let systems: Vec<SystemFrameRow> = (0..14)
        .map(|index| SystemFrameRow {
            name: format!("system{index:02}"),
            elapsed: Duration::from_micros(100 + index),
        })
        .collect();
    let snapshot = SystemFrameSnapshot {
        steps: 1,
        total: Duration::from_micros(1_491),
        systems,
        missing: Vec::new(),
    };
    let sections = performance_sections(
        &sampled_report(),
        &LayoutCacheStats::default(),
        Default::default(),
        None,
        Some(&snapshot),
    );
    let schedule = section(&sections, "运行时 · 调度系统");
    assert_eq!(
        schedule.rows.len(),
        5 + MAX_SYSTEM_ROWS,
        "头五行 + 上限内的系统行"
    );
    assert!(
        schedule
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("其余 2 个"),
        "截断必须说明余量：{:?}",
        schedule.note
    );
    // 面板截断的只是显示；「复制报告」必须给全清单。
    assert!(
        snapshot.to_text().contains("system13"),
        "复制报告应包含未列出的系统：{}",
        snapshot.to_text()
    );
}

#[test]
fn panel_and_command_tables_are_sorted_and_bounded() {
    let mut report = sampled_report();
    // 造 20 个面板与 12 个命令：验证"按耗时排序 + 截断 + 给出余量说明"。
    report.panels = (0..20)
        .map(|index| TimingSummary {
            name: format!("panel{index:02}"),
            samples: 1,
            records: 1,
            last_ms: index as f32,
            p50_ms: index as f32,
            p95_ms: index as f32,
            max_ms: index as f32,
            total_ms: f64::from(index),
        })
        .collect();
    report.commands = (0..12)
        .map(|index| TimingSummary {
            name: format!("cmd{index:02}"),
            samples: 1,
            records: 1,
            last_ms: index as f32,
            p50_ms: index as f32,
            p95_ms: index as f32,
            max_ms: index as f32,
            total_ms: f64::from(index),
        })
        .collect();
    let sections = performance_sections(
        &report,
        &LayoutCacheStats::default(),
        Default::default(),
        None,
        None,
    );

    let panels = section(&sections, "面板耗时");
    assert_eq!(panels.rows.len(), MAX_PANEL_ROWS);
    assert_eq!(panels.rows[0].label, "panel19", "必须按 p95 降序");
    assert!(
        panels
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("其余 8 个"),
        "截断必须给出余量：{:?}",
        panels.note
    );

    let commands = section(&sections, "命令耗时");
    assert_eq!(commands.rows.len(), MAX_COMMAND_ROWS);
    assert_eq!(commands.rows[0].label, "cmd11", "必须按累计耗时降序");
    assert!(!commands.default_open, "长清单默认收起");
}

#[test]
fn empty_series_report_placeholders_not_panics() {
    let report = ProfilerReport {
        frame_budget_ms: DEFAULT_FRAME_BUDGET_MS,
        ..Default::default()
    };
    let sections = performance_sections(
        &report,
        &LayoutCacheStats::default(),
        Default::default(),
        None,
        None,
    );
    assert_eq!(sections.len(), 8, "七个编辑器分组 + 一个运行时占位分组");
    for title in ["面板耗时", "命令耗时", "计数器（累计）", "仪表（本帧快照）"]
    {
        assert!(
            section(&sections, title).rows.is_empty(),
            "{title} 在空报告下应为空（由绘制层显示占位）"
        );
    }
    let runtime = section(&sections, "运行时（播放预览）");
    assert_eq!(runtime.rows[0].raw_key, "runtime.status");
    assert_eq!(
        runtime.rows[0].value, "未运行（点击播放开始实时采样）",
        "未 Play 时必须明确说没在采样"
    );
    // `enabled=false` 的报告必须自曝"没在记录"，而不是显示一排 0 让人误解。
    let disabled = ProfilerReport::default();
    let sections = performance_sections(
        &disabled,
        &LayoutCacheStats::default(),
        Default::default(),
        None,
        None,
    );
    let row = section(&sections, "编辑器帧")
        .rows
        .iter()
        .find(|row| row.raw_key == "enabled")
        .expect("帧分组必须有记录开关");
    assert_eq!(row.value, "否");
    assert!(row.warn);
}

/// 运行时分组必须覆盖用户点名的全部关键读数，并给出可断言的原始键。
#[test]
fn runtime_sections_cover_frame_render_resource_and_memory() {
    let snapshot = RuntimePerfSnapshot {
        target: "Play 预览",
        resolution: (1280, 720),
        frames: 120,
        fps: 58.5,
        frame_ms: 17.1,
        update_ms: 4.2,
        lua_ms: 1.1,
        render_cpu_ms: 2.3,
        render_gpu_ms: Some(3.4),
        gpu_timing_note: None,
        draw_calls: 42,
        set_pass_calls: 6,
        instances: 18,
        triangles: 1200,
        vertices: 3600,
        dropped_draws: 0,
        passes: vec!["clear", "legacy_2d_overlay"],
        gpu_passes: Default::default(),
        hdr_active: true,
        scene_format: SceneColorFormat::Rgba16Float,
        tonemap: Some(Tonemap::Aces),
        ibl_mode: IblMode::Full,
        ibl_tier: Some(IblTier::A),
        environment_map: Some("assets/env/studio.rlenv".to_owned()),
        environment_error: None,
        loaded_textures: 12,
        font_atlas_textures: 1,
        texture_bytes: 4 * 1024 * 1024,
        texture_identity_hits: 100,
        texture_identity_misses: 2,
        cached_meshes: 3,
        mesh_bytes: 2 * 1024 * 1024,
        mesh_identity_hits: 50,
        mesh_identity_misses: 1,
        async_mode: false,
        async_workers: 0,
        async_pending: 0,
        async_completed: 15,
        vertex_buffer_capacity: 1024 * 1024,
        vertex_buffer_growths: 1,
        vertex_bytes_peak: 900 * 1024,
        pipeline_variants: 6,
        pipeline_variant_hits: 0,
        pipeline_variant_misses: 6,
        pipeline_compile_ms: 12.5,
        readback: ReadbackMetrics {
            slot_count: 3,
            dropped: 1,
            ..Default::default()
        },
        process_memory_mib: Some(512.0),
        lua_heap_bytes: 86_113,
        asset_entries: 20,
        scene_entities: 40,
        runtime_entities: 43,
    };
    let report = sampled_report();
    let sections = performance_sections(
        &report,
        &LayoutCacheStats::default(),
        Default::default(),
        Some(&snapshot),
        None,
    );
    assert_eq!(sections.len(), 12, "七个编辑器分组 + 五个运行时分组");

    let frame = section(&sections, "运行时 · 帧");
    for (key, value) in [
        ("runtime.fps", "58.5 FPS"),
        ("runtime.frame_ms", "17.10 ms"),
        ("runtime.render_cpu_ms", "2.30 ms"),
        ("runtime.render_gpu_ms", "3.40 ms"),
        ("runtime.update_ms", "4.20 ms"),
        ("runtime.lua_ms", "1.100 ms"),
        ("runtime.frames", "120"),
    ] {
        let row = frame
            .rows
            .iter()
            .find(|row| row.raw_key == key)
            .unwrap_or_else(|| panic!("运行时帧分组缺少 {key}"));
        assert_eq!(row.value, value);
    }

    let render = section(&sections, "运行时 · 渲染");
    for (key, value) in [
        ("runtime.hdr_active", "是"),
        ("runtime.scene_format", "rgba16float"),
        ("runtime.tonemap", "aces"),
        ("runtime.ibl", "完整（档位 A）"),
        ("runtime.environment", "assets/env/studio.rlenv"),
        ("runtime.draw_calls", "42"),
        ("runtime.set_pass_calls", "6"),
        ("runtime.instances", "18"),
        ("runtime.triangles", "1200"),
        ("runtime.vertices", "3600"),
        ("runtime.dropped_draws", "0"),
        ("runtime.pass_count", "2"),
        ("runtime.passes", "2 条：clear → legacy_2d_overlay"),
        ("runtime.readback_slots", "3 / 1"),
    ] {
        let row = render
            .rows
            .iter()
            .find(|row| row.raw_key == key)
            .unwrap_or_else(|| panic!("运行时渲染分组缺少 {key}"));
        assert_eq!(row.value, value, "{key} 的值不对");
    }
    assert!(
        render
            .rows
            .iter()
            .find(|row| row.raw_key == "runtime.readback_slots")
            .expect("回读槽")
            .warn,
        "回读丢弃必须标警示"
    );

    let gpu_passes = section(&sections, "运行时 · GPU 通道耗时（E06-3）");
    let unsupported = gpu_passes
        .rows
        .iter()
        .find(|row| row.raw_key == "runtime.gpu_pass_supported")
        .expect("GPU 通道分组必须给出可用性");
    assert_eq!(unsupported.value, "不可用（不支持）");

    let resources = section(&sections, "运行时 · 资源与缓存");
    for (key, value) in [
        ("runtime.loaded_textures", "12"),
        ("runtime.texture_bytes", "4.00 MiB"),
        ("runtime.texture_identity_hit_rate", "98.0%"),
        ("runtime.cached_meshes", "3"),
        ("runtime.mesh_bytes", "2.00 MiB"),
        ("runtime.pipeline_variants", "6"),
        ("runtime.pipeline_compile_ms", "12.50 ms"),
        ("runtime.pipeline_cache_stats", "0 / 6（0.0%）"),
        ("runtime.load_mode", "同步（编辑器预览 / 确定性输出）"),
        ("runtime.async_loads", "0 / 15"),
        ("runtime.asset_entries", "20"),
        ("runtime.entities", "40 / 43"),
    ] {
        let row = resources
            .rows
            .iter()
            .find(|row| row.raw_key == key)
            .unwrap_or_else(|| panic!("运行时资源分组缺少 {key}"));
        assert_eq!(row.value, value, "{key} 的值不对");
    }

    let memory = section(&sections, "运行时 · 内存");
    for (key, value) in [
        ("runtime.process_memory_mib", "512.0 MiB"),
        ("runtime.lua_heap_bytes", "84.1 KiB"),
        ("runtime.gpu_texture_bytes", "4.00 MiB"),
        ("runtime.gpu_mesh_bytes", "2.00 MiB"),
        ("runtime.gpu_vertex_buffer_bytes", "1.00 MiB"),
        ("runtime.known_gpu_bytes", "7.00 MiB"),
    ] {
        let row = memory
            .rows
            .iter()
            .find(|row| row.raw_key == key)
            .unwrap_or_else(|| panic!("运行时内存分组缺少 {key}"));
        assert_eq!(row.value, value, "{key} 的值不对");
    }
}

/// E06-3：有样本时逐通道列行；偏差越界必须标警示，而不是只显示数字。
#[test]
fn gpu_pass_section_lists_each_pass_and_flags_inconsistency() {
    use rustlua_gpu::{GpuPassReport, PassGpuTiming};
    use rustlua_render::render_plan::RenderPass;

    let mut snapshot = RuntimePerfSnapshot {
        frames: 10,
        ..Default::default()
    };
    snapshot.gpu_passes = GpuPassReport {
        supported: true,
        reason: None,
        frame_us: Some(1000),
        passes: vec![
            PassGpuTiming {
                pass: RenderPass::Clear,
                gpu_us: 100,
            },
            PassGpuTiming {
                pass: RenderPass::DepthOpaque,
                gpu_us: 800,
            },
        ],
        pass_sum_us: 1200,
        delta_pct: Some(20.0),
        consistent: Some(false),
    };
    let sections = performance_sections(
        &sampled_report(),
        &LayoutCacheStats::default(),
        Default::default(),
        Some(&snapshot),
        None,
    );
    let gpu = section(&sections, "运行时 · GPU 通道耗时（E06-3）");
    let value = |key: &str| {
        gpu.rows
            .iter()
            .find(|row| row.raw_key == key)
            .unwrap_or_else(|| panic!("GPU 通道分组缺少 {key}"))
    };
    assert_eq!(
        value("runtime.gpu_pass_frame_us").value,
        "1000 µs / 1200 µs"
    );
    assert_eq!(value("runtime.gpu_pass.clear").value, "100 µs");
    assert_eq!(value("runtime.gpu_pass.depth_opaque").value, "800 µs");
    let bar = value("runtime.gpu_pass.depth_opaque")
        .bar
        .expect("逐通道行必须带占比条");
    assert!((bar - 0.8).abs() < 1e-6, "bar={bar}");
    let delta = value("runtime.gpu_pass_delta_pct");
    assert_eq!(delta.value, "+20.00%");
    assert!(delta.warn, "偏差超出 ±5% 必须标警示");
}

/// 低帧率/丢 draw/超预算必须标黄；GPU 不可用只给原因，不冒充数据。
#[test]
fn runtime_warnings_are_explicit() {
    let mut snapshot = RuntimePerfSnapshot {
        frames: 60,
        fps: 30.0,
        frame_ms: 33.0,
        draw_calls: 5000,
        set_pass_calls: 400,
        dropped_draws: 2,
        ..Default::default()
    };
    snapshot.gpu_timing_note = Some("不可用（设备未启用时间戳）".to_owned());
    let report = sampled_report();
    let sections = performance_sections(
        &report,
        &LayoutCacheStats::default(),
        Default::default(),
        Some(&snapshot),
        None,
    );
    let frame = section(&sections, "运行时 · 帧");
    assert!(
        frame
            .rows
            .iter()
            .find(|row| row.raw_key == "runtime.fps")
            .expect("FPS 行")
            .warn
    );
    assert!(
        frame
            .rows
            .iter()
            .find(|row| row.raw_key == "runtime.frame_ms")
            .expect("帧间隔行")
            .warn
    );
    assert_eq!(
        frame
            .rows
            .iter()
            .find(|row| row.raw_key == "runtime.render_gpu_ms")
            .expect("GPU 行")
            .value,
        "不可用（设备未启用时间戳）"
    );
    let render = section(&sections, "运行时 · 渲染");
    for key in [
        "runtime.draw_calls",
        "runtime.set_pass_calls",
        "runtime.dropped_draws",
    ] {
        assert!(
            render
                .rows
                .iter()
                .find(|row| row.raw_key == key)
                .unwrap_or_else(|| panic!("缺少 {key}"))
                .warn,
            "{key} 越过阈值必须标警示"
        );
    }
}

/// 用户报的那个重叠：性能数字与 Scene 右上角工具条**不能**再叠在一起。
///
/// 走的是整帧生产路径（合成 `egui` 帧），断言的是两个面板本帧真实的矩形。
#[test]
fn opening_the_panel_does_not_cover_the_scene_tool_strip() {
    let mut smoke = GuiSmoke::new("perf-panel", egui::vec2(1280.0, 720.0), 1.0);
    smoke.idle(4);
    let tools = smoke.app.scene_tools_rect;
    assert!(
        tools.width() > 1.0 && tools.height() > 1.0,
        "Scene 工具小面板必须已画出：{tools:?}"
    );
    // 关着的时候不允许留下"上一帧的矩形"，否则证据会撒谎。
    assert_eq!(smoke.app.perf_panel_rect, egui::Rect::NOTHING);

    smoke.app.show_performance_panel = true;
    smoke.idle(3);
    let panel = smoke.app.perf_panel_rect;
    assert!(
        panel.width() > 1.0 && panel.height() > 1.0,
        "打开后必须真的画出窗口：{panel:?}"
    );
    assert!(
        !panel.intersects(tools),
        "性能面板默认位置压住了 Scene 工具条：panel={panel:?} tools={tools:?}"
    );

    // 反向对照：关掉后又回到 NOTHING（矩形不是"最后一次拖动的残留"）。
    smoke.app.show_performance_panel = false;
    smoke.idle(2);
    assert_eq!(smoke.app.perf_panel_rect, egui::Rect::NOTHING);
    smoke.cleanup();

    // 矮窗口：右下角放不下默认高度时，按"工具条下沿以下的可用空间"收矮，
    // 而不是被 egui 顶回屏幕上方、又压回用户报的那个角落。
    let tall_height = panel.height();
    let mut short = GuiSmoke::new("perf-panel-short", egui::vec2(1024.0, 600.0), 1.0);
    short.idle(4);
    short.app.show_performance_panel = true;
    short.idle(3);
    let panel = short.app.perf_panel_rect;
    let tools = short.app.scene_tools_rect;
    assert!(
        panel.width() > 1.0 && tools.width() > 1.0,
        "矮窗口下两者都必须画出来：panel={panel:?} tools={tools:?}"
    );
    assert!(
        panel.height() < tall_height,
        "矮窗口下默认高度必须收到可用空间内：矮={:.1} 常规={tall_height:.1}",
        panel.height()
    );
    assert!(
        !panel.intersects(tools),
        "矮窗口下性能面板压住了 Scene 工具条：panel={panel:?} tools={tools:?}"
    );
    short.cleanup();
}

/// E06-2：trace 落在项目 `target/reports/` 下，不写进资源目录。
#[test]
fn trace_export_path_is_under_project_target_reports() {
    let path = super::trace_export_path(std::path::Path::new("C:/game"));
    assert!(path.ends_with("target/reports/editor-profile-trace.json"));
    assert!(path.starts_with("C:/game"));
}

// ---- 面板里的尾部时间线（M79 P2）------------------------------------
// 绝对像素（纳秒→像素、泳道、最小宽度）由 `crate::trace_timeline` 自己的门钉；这里
// 只钉**面板这一侧**才成立的三件事：宽度取的是当帧可用宽度、鼠标点是相对分配矩形换算
// 的（差一个刻度行高）、以及"没有可画的区间"时不许画半条时间线。

fn trace_span(
    name: &str,
    category: &'static str,
    start_ns: u64,
    dur_ns: u64,
    depth: u16,
) -> SpanRecord {
    SpanRecord {
        name: name.to_owned(),
        category,
        start_ns,
        dur_ns,
        depth,
        args: Vec::new(),
    }
}

fn run_trace_probe(
    records: &[SpanRecord],
    screen_width: f32,
    pointer: Option<egui::Pos2>,
) -> egui::FullOutput {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(screen_width, 700.0),
        )),
        ..Default::default()
    };
    if let Some(pos) = pointer {
        input.events.push(egui::Event::PointerMoved(pos));
    }
    egui::Context::default().run(input, |context| {
        egui::CentralPanel::default().show(context, |ui| {
            draw_trace_timeline(ui, records);
        });
    })
}

fn trace_filled(output: &egui::FullOutput, fill: egui::Color32, tag: &str) -> egui::Rect {
    let hits = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(shape) if shape.fill == fill && shape.stroke.is_empty() => {
                Some(shape.rect)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(hits.len(), 1, "{tag} 的底色应当只被画一次：{hits:?}");
    hits[0]
}

fn trace_texts(output: &egui::FullOutput) -> Vec<String> {
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
}

#[test]
fn the_trace_timeline_follows_the_panel_width() {
    use crate::trace_timeline as tl;

    // 窗口 = 最晚结束 = 8 ms：frame 占满、lua 从 1/4 到 5/8、tick 只有 1 µs。
    let records = vec![
        trace_span("frame", "frame", 0, 8_000_000, 0),
        trace_span("lua", "script", 2_000_000, 3_000_000, 1),
        trace_span("tick", "system", 6_000_000, 1_000, 2),
    ];
    let wide = run_trace_probe(&records, 1000.0, None);
    let narrow = run_trace_probe(&records, 700.0, None);
    let bg_wide = trace_filled(&wide, tl::BACKGROUND, "背景（宽）");
    let bg_narrow = trace_filled(&narrow, tl::BACKGROUND, "背景（窄）");
    assert!(
        bg_wide.width() - bg_narrow.width() > 50.0,
        "时间线宽度没跟着面板可用宽度走：{bg_wide:?} vs {bg_narrow:?}"
    );
    // 按比例的两条：相对位置 = 起点/时长占窗口的比例，与屏宽无关。写死任何像素都会在这里红。
    for (name, category, lane, x_fraction, w_fraction) in [
        ("frame", "frame", 0.0_f32, 0.0_f32, 1.0_f32),
        ("lua", "script", 1.0, 0.25, 0.375),
    ] {
        for (output, bg) in [(&wide, bg_wide), (&narrow, bg_narrow)] {
            let rect = trace_filled(output, tl::category_color(category), name);
            let near = |actual: f32, expected: f32, tag: &str| {
                assert!(
                    (actual - expected).abs() < 0.05,
                    "{name} 在宽 {bg:?} 下 {tag} 不对：期望 {expected}，实际 {actual}"
                );
            };
            near(rect.min.x - bg.min.x, bg.width() * x_fraction, "x");
            near(rect.width(), bg.width() * w_fraction, "宽");
            near(
                rect.min.y - bg.min.y - tl::TICK_LABEL_HEIGHT,
                lane * (12.0 + 3.0),
                "泳道 y",
            );
            near(rect.height(), 12.0, "高");
        }
    }
    // 1 µs 那条不按比例：抬到最小宽度并画红描边，面板不许把它当成真实耗时。
    let tick = trace_filled(&wide, tl::category_color("system"), "tick");
    assert!(
        (tick.width() - 2.0).abs() < 0.05,
        "零碎区间必须抬到最小宽度而不是真实宽度：{tick:?}"
    );
    let stroked = wide
        .shapes
        .iter()
        .filter(|clipped| {
            matches!(&clipped.shape, egui::Shape::Rect(shape)
                if shape.stroke.color == tl::WIDENED_STROKE)
        })
        .count();
    assert_eq!(stroked, 1, "只该有一条被标成未按比例：{stroked}");
    let texts = trace_texts(&wide);
    assert!(
        texts.iter().any(|text| text.starts_with("时间线：3 条")
            && text.contains("窗口 8.00 ms")
            && text.contains("3 条泳道")),
        "概况行没写清画了几条/多大窗口：{texts:?}"
    );
}

#[test]
fn hovering_a_timeline_bar_replaces_the_caption_with_its_hint() {
    use crate::trace_timeline as tl;

    let records = vec![
        trace_span("frame", "frame", 0, 8_000_000, 0),
        trace_span("lua", "script", 2_000_000, 3_000_000, 1),
    ];
    let plain = run_trace_probe(&records, 1000.0, None);
    let bg = trace_filled(&plain, tl::BACKGROUND, "背景");
    // lua 那条：x 在窗口的 1/4~5/8 之间，泳道 1 的 y = 刻度行 + 15..27。
    let pointer = egui::pos2(
        bg.min.x + bg.width() * 0.3,
        bg.min.y + tl::TICK_LABEL_HEIGHT + 20.0,
    );
    let hovered = run_trace_probe(&records, 1000.0, Some(pointer));
    let texts = trace_texts(&hovered);
    assert!(
        texts
            .iter()
            .any(|text| text == "lua · script · 3.000 ms · 深度 1"),
        "悬停没把那条区间的说明打出来：{texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text.starts_with("时间线：")),
        "悬停时概况行应当被单条说明顶掉（两种文案占同一行，面板高度不许跳）：{texts:?}"
    );
    // 指针还在刻度行上时不许命中：行高偏移必须减掉。
    let on_ticks = run_trace_probe(
        &records,
        1000.0,
        Some(egui::pos2(bg.min.x + 20.0, bg.min.y + 4.0)),
    );
    let texts = trace_texts(&on_ticks);
    assert!(
        texts.iter().any(|text| text.starts_with("时间线：2 条")),
        "刻度行上应当仍是概况行：{texts:?}"
    );
}

#[test]
fn an_empty_capture_draws_no_timeline_but_says_so() {
    use crate::trace_timeline as tl;

    let output = run_trace_probe(&[], 1000.0, None);
    assert!(
        !output.shapes.iter().any(|clipped| matches!(
            &clipped.shape,
            egui::Shape::Rect(shape) if shape.fill == tl::BACKGROUND
        )),
        "没捕获时不许画一条空时间线背景"
    );
    let texts = trace_texts(&output);
    assert!(
        texts
            .iter()
            .any(|text| text.starts_with("（没有可画的区间")),
        "空捕获要说清楚为什么没有图：{texts:?}"
    );
}
