//! Headless editor performance baseline (docs/55 阶段 0 / ER-01).
//!
//! Every scenario runs without a GPU or window, so the report is reproducible in CI and on
//! any workstation. Scenarios assert their own invariants (bounded logs, history budget,
//! background scan, save/load equivalence) and the command exits non-zero when one fails,
//! so a plausible-looking number can never hide a broken code path.

use crate::EditorApp;
use crate::{ConsoleLogLevelFilter, ConsoleTab};
use editor::{
    CommandKind, FixtureSize, ResourceBrowserState, SceneDocument, build_console_fixture,
    build_scene_fixture, percentiles_ms,
};
use rustlua_ecs::transform::Transform3D;
use rustlua_infra::logging::LogSink;

// P0-3：跨场景的 span 收集器。
//
// 基线里每个场景都新建一个 `EditorApp`（因而各有独立的 `Profiler`）。要把它们合成
// 一份可读的 Chrome trace，就需要在 app 销毁前把 span 取出来、按"上一段结束时刻"
// 平移后累积。`thread_local` 足够——基线全程单线程。
thread_local! {
    static TRACE_COLLECTOR: std::cell::RefCell<Option<rustlua_gpu_profiler_trace::Collector>> =
        const { std::cell::RefCell::new(None) };
}

/// span 收集器的薄封装（放在 `perf_baseline` 内，避免为一次性用途污染 `editor` 的公开 API）。
mod rustlua_gpu_profiler_trace {
    use engine::host::profile_trace::{CaptureLevel, SpanRecord, SpanTrace};

    pub(super) struct Collector {
        trace: SpanTrace,
        /// 已收进 trace 的尾部时刻；下一段从这里之后开始。
        next_offset_ns: u64,
    }

    impl Collector {
        pub(super) fn new(capacity: usize) -> Self {
            Self {
                trace: SpanTrace::new(CaptureLevel::Fine, capacity),
                next_offset_ns: 0,
            }
        }

        /// 追加一段（该段的时间从 0 起），并把偏移推到本段之后。
        pub(super) fn absorb(&mut self, records: &[SpanRecord]) {
            if records.is_empty() {
                return;
            }
            self.trace.push_records(records, self.next_offset_ns);
            // 段间留 1 微秒空隙：Chrome 的时间轴上一个像素都不放大时也不会把两段看成同一帧。
            let segment_end = records
                .iter()
                .map(|record| record.start_ns.saturating_add(record.dur_ns))
                .max()
                .unwrap_or(0);
            self.next_offset_ns = self
                .next_offset_ns
                .saturating_add(segment_end)
                .saturating_add(1_000);
        }

        pub(super) fn to_json(&self) -> String {
            self.trace.to_chrome_trace_json()
        }

        pub(super) fn span_count(&self) -> usize {
            self.trace.len()
        }
    }
}
use rustlua_scene::scene::SceneEntityId;
use rustlua_ui::ui_virtual_list::{SlotPool, VirtualList, VirtualListConfig, bind_window};
use std::{fs, path::PathBuf, time::Instant};

/// One measured scenario.
#[derive(Clone, Debug)]
pub(crate) struct ScenarioReport {
    pub name: String,
    pub iterations: usize,
    pub last_ms: f32,
    pub p50_ms: f32,
    pub p95_ms: f32,
    pub p99_ms: f32,
    pub max_ms: f32,
    /// Counts and invariant outcomes that give the timing context.
    pub notes: Vec<String>,
    /// False when a scenario invariant failed; the report then exits non-zero.
    pub ok: bool,
}

/// Aggregate report produced by `--perf-baseline`.
#[derive(Clone, Debug)]
pub(crate) struct BaselineReport {
    pub scenarios: Vec<ScenarioReport>,
}

impl BaselineReport {
    pub fn ok(&self) -> bool {
        self.scenarios.iter().all(|scenario| scenario.ok)
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str("=== RustLua editor performance baseline (docs/55 ER-01) ===\n");
        out.push_str(&format!(
            "profile={} os={}\n",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            std::env::consts::OS
        ));
        out.push_str("note: absolute values only compare on identical hardware and profile\n\n");
        for scenario in &self.scenarios {
            out.push_str(&format!(
                "{:<34} n={:<5} last={:>9.3} p50={:>9.3} p95={:>9.3} p99={:>9.3} max={:>9.3} ms  {}\n",
                scenario.name,
                scenario.iterations,
                scenario.last_ms,
                scenario.p50_ms,
                scenario.p95_ms,
                scenario.p99_ms,
                scenario.max_ms,
                if scenario.ok { "ok" } else { "FAILED" }
            ));
            for note in &scenario.notes {
                out.push_str("    - ");
                out.push_str(note);
                out.push('\n');
            }
        }
        let failures = self
            .scenarios
            .iter()
            .filter(|scenario| !scenario.ok)
            .count();
        out.push_str(&format!(
            "\nscenarios={} failed={} result={}\n",
            self.scenarios.len(),
            failures,
            if failures == 0 { "ok" } else { "FAILED" }
        ));
        out
    }
}

fn summarize(name: &str, samples: &[f32], notes: Vec<String>, ok: bool) -> ScenarioReport {
    let stats = percentiles_ms(samples).unwrap_or_default();
    ScenarioReport {
        name: name.to_owned(),
        iterations: samples.len(),
        last_ms: samples.last().copied().unwrap_or(0.0),
        p50_ms: stats.p50,
        p95_ms: stats.p95,
        p99_ms: stats.p99,
        max_ms: stats.max,
        notes,
        ok,
    }
}

fn instant_scenario(name: &str, elapsed_ms: f32, notes: Vec<String>, ok: bool) -> ScenarioReport {
    ScenarioReport {
        name: name.to_owned(),
        iterations: 1,
        last_ms: elapsed_ms,
        p50_ms: elapsed_ms,
        p95_ms: elapsed_ms,
        p99_ms: elapsed_ms,
        max_ms: elapsed_ms,
        notes,
        ok,
    }
}

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rustlua-editor-baseline-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp root must be creatable");
    root
}

/// Minimal project description for a headless app instance.
fn baseline_app(document: SceneDocument) -> TracedApp {
    baseline_app_with_sink(document, 512)
}

/// 开了 span 捕获的 `EditorApp`：作用域结束时（Drop）把本段的 span 交给
/// [`TRACE_COLLECTOR`]，时间轴按段平移。
///
/// 用 `Deref`/`DerefMut` 包一层而不是改 13 个场景的写法：`app.selection`、
/// `app.document` 这些原有用法一个字都不用动，捕获开关也只在一个地方管。
struct TracedApp {
    app: EditorApp,
    tracing: bool,
}

impl std::ops::Deref for TracedApp {
    type Target = EditorApp;
    fn deref(&self) -> &EditorApp {
        &self.app
    }
}

impl std::ops::DerefMut for TracedApp {
    fn deref_mut(&mut self) -> &mut EditorApp {
        &mut self.app
    }
}

impl Drop for TracedApp {
    fn drop(&mut self) {
        if !self.tracing {
            return;
        }
        self.app.profiler.stop_span_capture();
        let records = self.app.profiler.span_records_for_timeline(usize::MAX);
        TRACE_COLLECTOR.with(|collector| {
            if let Some(collector) = collector.borrow_mut().as_mut() {
                collector.absorb(&records);
            }
        });
    }
}

/// 同 [`baseline_app`]，但自定日志 sink 容量：控制台体量场景需要 sink 深度与灌入量同级
/// （sink 是控制台行的唯一来源）。
fn baseline_app_with_sink(document: SceneDocument, sink_capacity: usize) -> TracedApp {
    let mut app = EditorApp::new(
        editor::ProjectInfo {
            root: std::env::temp_dir().join("rustlua-editor-baseline-project"),
            version: "baseline".to_owned(),
            directories: Vec::new(),
        },
        None,
        LogSink::new(sink_capacity),
    );
    // Private-field access is available because this module lives in the same crate.
    app.document.current_scene = Some(document);
    // P0-3：只有本次运行要求导出 trace 时才开捕获（见 `run_and_write_report` 的
    // `--perf-trace`）——不开时不采集，基线本身的耗时读数不受影响。
    let tracing = TRACE_COLLECTOR.with(|collector| collector.borrow().is_some());
    if tracing {
        app.profiler
            .start_span_capture(engine::host::profile_trace::CaptureLevel::Fine);
    }
    TracedApp { app, tracing }
}

/// Apply the baseline edit transformation: move one entity by `step` on X.
fn apply_edit(app: &mut EditorApp, id: SceneEntityId, step: usize) -> bool {
    let Some(document) = app.document.current_scene.as_mut() else {
        return false;
    };
    app.history
        .execute(
            document,
            "Baseline move",
            CommandKind::Transform(id),
            |scene| {
                let entity = scene
                    .scene
                    .entities
                    .get_mut(&id)
                    .ok_or_else(|| "entity missing".to_owned())?;
                let mut transform = entity
                    .components
                    .get("Transform")
                    .and_then(|value| Transform3D::from_component_str(value).ok())
                    .unwrap_or_default();
                transform.position.x = step as f32;
                entity
                    .components
                    .insert("Transform".to_owned(), transform.to_component_str());
                scene.mark_dirty();
                Ok::<(), String>(())
            },
        )
        .unwrap_or(false)
}

/// Build every scenario report.
pub(crate) fn run_baseline() -> BaselineReport {
    let mut scenarios = Vec::new();

    // 1. Fixture construction for each plan size.
    for size in [FixtureSize::Small, FixtureSize::Medium, FixtureSize::Large] {
        let spec = size.spec();
        let iterations = if size == FixtureSize::Large { 3 } else { 5 };
        let mut samples = Vec::with_capacity(iterations);
        let mut entities = 0;
        for _ in 0..iterations {
            let started = Instant::now();
            let fixture = build_scene_fixture(spec);
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
            entities = fixture.document.scene.entities.len();
        }
        scenarios.push(summarize(
            &format!("fixture_build_{}", size.label()),
            &samples,
            vec![format!("entities={entities} target={}", spec.entities)],
            entities == spec.entities,
        ));
    }

    // 2. Hierarchy flatten at 10k entities: full rebuild cost, warm cache cost, and the
    //    hit/miss contract.
    {
        let fixture = build_scene_fixture(FixtureSize::Large.spec());
        let expected_rows = fixture.expected_rows;

        // Full flatten: a fresh app per sample means the first query is always a real rebuild.
        let samples_count = 5;
        let mut full = Vec::with_capacity(samples_count);
        let mut full_rows = 0;
        for _ in 0..samples_count {
            let mut app = baseline_app(fixture.document.clone());
            let started = Instant::now();
            full_rows = app.hierarchy_visible_ids().len();
            full.push(started.elapsed().as_secs_f32() * 1000.0);
        }
        scenarios.push(summarize(
            "hierarchy_flatten_full_10k",
            &full,
            vec![
                format!("rows={full_rows}"),
                format!("expected_rows={expected_rows}"),
            ],
            full_rows == expected_rows,
        ));

        // Warm repeats: no state change in between, so every query must be a cache hit.
        let iterations = 20;
        let mut app = baseline_app(fixture.document.clone());
        let _ = app.hierarchy_visible_ids();
        let mut warm = Vec::with_capacity(iterations);
        let mut warm_rows = 0;
        for _ in 0..iterations {
            let started = Instant::now();
            warm_rows = app.hierarchy_visible_ids().len();
            warm.push(started.elapsed().as_secs_f32() * 1000.0);
        }
        let hits = app.profiler.counter("hierarchy_cache_hit");
        let misses = app.profiler.counter("hierarchy_cache_miss");
        scenarios.push(summarize(
            "hierarchy_flatten_warm_10k",
            &warm,
            vec![
                format!("rows={warm_rows}"),
                format!("hits={hits} misses={misses} (1 cold + {iterations} warm)"),
            ],
            warm_rows == full_rows && hits >= iterations as u64 && misses == 1,
        ));

        // Invalidation contract: each new filter string must force exactly one rebuild.
        let mut app = baseline_app(fixture.document.clone());
        let mut cold = Vec::with_capacity(iterations);
        let mut cold_rows = 0;
        for index in 0..iterations {
            app.hierarchy_filter = format!("Node{}", index + 1);
            let started = Instant::now();
            cold_rows = app.hierarchy_visible_ids().len();
            cold.push(started.elapsed().as_secs_f32() * 1000.0);
            let repeat_rows = app.hierarchy_visible_ids().len();
            assert_eq!(
                repeat_rows, cold_rows,
                "warm query must return identical rows"
            );
        }
        let misses = app.profiler.counter("hierarchy_cache_miss");
        let hits = app.profiler.counter("hierarchy_cache_hit");
        scenarios.push(summarize(
            "hierarchy_filter_invalidation_10k",
            &cold,
            vec![
                format!("last_filter_rows={cold_rows}"),
                format!("misses={misses} hits={hits} expected={iterations}"),
            ],
            misses == iterations as u64 && hits == iterations as u64,
        ));
    }

    // 3. Name search across 10k entities (plan budget: P95 < 100 ms).
    {
        let fixture = build_scene_fixture(FixtureSize::Large.spec());
        let mut app = baseline_app(fixture.document);
        let iterations = 10;
        let mut samples = Vec::with_capacity(iterations);
        let mut matched = 0;
        for index in 0..iterations {
            app.hierarchy_filter = format!("Node{}", index + 1);
            let started = Instant::now();
            matched = app.hierarchy_visible_ids().len();
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
        }
        let stats = percentiles_ms(&samples).unwrap_or_default();
        scenarios.push(summarize(
            "hierarchy_search_10k",
            &samples,
            vec![
                format!("last_filter_matches={matched}"),
                "plan budget: P95 < 100 ms".to_owned(),
            ],
            stats.p95 < 100.0,
        ));
    }

    // 4. Console and log volume: 100k lines stay bounded.
    {
        let lines = build_console_fixture(FixtureSize::Large.spec().console_lines);
        let sink = LogSink::new(500);
        let started = Instant::now();
        for line in &lines {
            sink.push(line.clone());
        }
        let elapsed = started.elapsed().as_secs_f32() * 1000.0;
        let retained = sink.snapshot().len();
        scenarios.push(instant_scenario(
            "log_sink_push_100k_bounded",
            elapsed,
            vec![format!("retained_lines={retained} cap=500")],
            retained == 500,
        ));
    }

    // 5. Selection scaling: select and prune 10k rows.
    {
        let fixture = build_scene_fixture(FixtureSize::Large.spec());
        let ids = fixture.entity_ids();
        let total = ids.len();
        let mut app = baseline_app(fixture.document);
        let iterations = 10;
        let mut samples = Vec::with_capacity(iterations);
        let mut selected = 0;
        for _ in 0..iterations {
            let started = Instant::now();
            app.selection
                .set_many(ids.clone(), None, editor::SelectionSource::Command);
            app.selection.retain_valid(|_| true);
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
            selected = app.selection.len();
        }
        scenarios.push(summarize(
            "selection_10k_set_and_prune",
            &samples,
            vec![format!("selected={selected} expected={total}")],
            selected == total,
        ));
    }

    // 6. Undo/redo below budget: full restore must be exact.
    //
    // Edits alternate between two targets because consecutive `Transform(id)` commands on the
    // same entity intentionally merge into one history entry; alternating keeps one entry per
    // edit so undo/redo fidelity is actually exercised.
    {
        let fixture = build_scene_fixture(FixtureSize::Medium.spec());
        let before = fixture.document.clone();
        let targets = [SceneEntityId(2), SceneEntityId(3)];
        let mut app = baseline_app(fixture.document);
        let edits = 100;
        let started = Instant::now();
        let mut applied = 0;
        for step in 0..edits {
            if apply_edit(&mut app, targets[step % targets.len()], step) {
                applied += 1;
            }
        }
        let edit_ms = started.elapsed().as_secs_f32() * 1000.0;
        let undo_entries = app.history.undo_len();
        let mut undos = Vec::with_capacity(applied);
        let mut undone = 0;
        while app.history.can_undo() {
            let started = Instant::now();
            app.undo();
            undos.push(started.elapsed().as_secs_f32() * 1000.0);
            undone += 1;
        }
        let restored = app
            .document
            .current_scene
            .as_ref()
            .is_some_and(|document| document.scene == before.scene);
        let mut redos = Vec::with_capacity(undone);
        let mut redone = 0;
        while app.history.can_redo() {
            let started = Instant::now();
            app.redo();
            redos.push(started.elapsed().as_secs_f32() * 1000.0);
            redone += 1;
        }
        let redone_changed = app
            .document
            .current_scene
            .as_ref()
            .is_some_and(|document| document.scene != before.scene);
        scenarios.push(summarize(
            "history_1k_edits_apply",
            &[edit_ms],
            vec![
                format!("applied={applied} edits={edits}"),
                format!("undo_entries={undo_entries} (alternating targets avoid merge)"),
            ],
            applied == edits && undo_entries == edits,
        ));
        scenarios.push(summarize(
            "history_1k_edits_undo",
            &undos,
            vec![
                format!("undone={undone}"),
                format!("document_restored_exactly={restored}"),
            ],
            restored && undone == applied,
        ));
        scenarios.push(summarize(
            "history_1k_edits_redo",
            &redos,
            vec![format!(
                "redone={redone} document_changed_again={redone_changed}"
            )],
            redone == undone && redone_changed,
        ));
    }

    // 7. History budget: 500 edits keep only the newest checkpoints and report the drops.
    {
        let fixture = build_scene_fixture(FixtureSize::Small.spec());
        let targets = [SceneEntityId(2), SceneEntityId(3)];
        let mut app = baseline_app(fixture.document);
        let edits = 500;
        for step in 0..edits {
            apply_edit(&mut app, targets[step % targets.len()], step);
        }
        let undo_len = app.history.undo_len();
        let dropped = app.history.dropped_entries();
        let budget = editor::CommandService::DEFAULT_MAX_ENTRIES;
        scenarios.push(instant_scenario(
            "history_budget_500_edits",
            0.0,
            vec![
                format!("undo_len={undo_len} budget={budget}"),
                format!("dropped_entries={dropped}"),
            ],
            undo_len == budget && dropped == (edits - budget) as u64,
        ));
    }

    // 8. Deterministic save/load round-trip at 1k entities.
    {
        let root = temp_root("roundtrip");
        let fixture = build_scene_fixture(FixtureSize::Medium.spec());
        let path = root.join("fixture.scene");
        let iterations = 5;
        let mut samples = Vec::with_capacity(iterations);
        let mut identical = false;
        for _ in 0..iterations {
            let mut document = fixture.document.clone();
            let started = Instant::now();
            document.save_as(&path).expect("fixture scene must save");
            let reloaded = SceneDocument::open(&path).expect("fixture scene must reload");
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
            identical = reloaded.scene == document.scene;
        }
        scenarios.push(summarize(
            "scene_save_load_1k",
            &samples,
            vec![format!("roundtrip_identical={identical}")],
            identical,
        ));
        let _ = fs::remove_dir_all(root);
    }

    // 9. Resource scan of 2k assets must happen off the UI thread, latest generation wins.
    {
        let root = temp_root("scan");
        fs::create_dir_all(root.join("assets")).expect("assets dir must be creatable");
        let files = 2_000;
        for index in 0..files {
            fs::write(
                root.join("assets").join(format!("texture_{index}.png")),
                b"\x89PNG\r\n\x1a\n",
            )
            .expect("fixture asset must be writable");
        }
        let mut app = baseline_app(SceneDocument::new_scene("scan"));
        app.project.root = root.clone();
        app.project_loaded = true;
        let started = Instant::now();
        app.refresh_resources();
        let request_ms = started.elapsed().as_secs_f32() * 1000.0;
        let entries_immediately = app.resources.entries().len();
        let mut waited_ms = 0.0_f32;
        let mut applied = false;
        while waited_ms < 30_000.0 {
            app.poll_resource_refresh();
            if app.resource_refresh_job.is_none() {
                applied = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            waited_ms += 5.0;
        }
        let entries = app.resources.entries().len();
        scenarios.push(instant_scenario(
            "resource_scan_2k_files_async",
            request_ms,
            vec![
                format!("ui_thread_request_ms={request_ms:.3}"),
                format!("entries_before_apply={entries_immediately} entries_after={entries}"),
                format!("applied_after_wait_ms={waited_ms:.0}"),
                "invariant: no recursive scan on the UI thread".to_owned(),
            ],
            applied && entries_immediately == 0 && entries >= files,
        ));

        let stale_before = app.profiler.counter("resource_refresh_stale_dropped");
        app.refresh_resources();
        app.refresh_resources();
        let generation_advanced = app.resource_refresh_generation >= 3;
        scenarios.push(instant_scenario(
            "resource_refresh_generation_supersede",
            0.0,
            vec![
                format!("generation={}", app.resource_refresh_generation),
                format!("stale_dropped_before={stale_before}"),
            ],
            generation_advanced,
        ));
        let _ = fs::remove_dir_all(root);
    }

    // 10. Worker-side scan cost for 1k assets (context for the async UI number).
    {
        let root = temp_root("scan-direct");
        fs::create_dir_all(root.join("assets")).expect("assets dir must be creatable");
        for index in 0..1_000 {
            fs::write(
                root.join("assets").join(format!("t{index}.lua")),
                b"return 1",
            )
            .expect("fixture asset must be writable");
        }
        let iterations = 3;
        let mut samples = Vec::with_capacity(iterations);
        let mut entries = 0;
        for _ in 0..iterations {
            let mut state = ResourceBrowserState::new();
            let started = Instant::now();
            state.refresh(&root).expect("resource scan must succeed");
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
            entries = state.entries().len();
        }
        scenarios.push(summarize(
            "resource_scan_worker_1k",
            &samples,
            vec![format!("entries={entries}")],
            entries >= 1_000,
        ));
        let _ = fs::remove_dir_all(root);
    }

    // 11. Headless UI frame cost of the Hierarchy panel at 10k entities.
    //
    // This is the plan's "10k 实体 Hierarchy 滚动" acceptance item measured without a window or
    // GPU: a real `egui` context runs the actual panel draw path. `hierarchy_rows_drawn` proves
    // virtualization is in effect (only viewport rows are built), not just that it is fast.
    {
        let fixture = build_scene_fixture(FixtureSize::Large.spec());
        let mut app = baseline_app(fixture.document);
        let context = eframe::egui::Context::default();
        let screen = eframe::egui::Rect::from_min_size(
            eframe::egui::pos2(0.0, 0.0),
            eframe::egui::vec2(320.0, 720.0),
        );
        let run_frame = |app: &mut EditorApp, context: &eframe::egui::Context| {
            let raw = eframe::egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let _ = context.run(raw, |context| {
                app.draw_hierarchy_panel(context);
            });
        };
        // Warm-up frames absorb font atlas construction and first-layout costs.
        for _ in 0..5 {
            run_frame(&mut app, &context);
        }
        let frames = 30;
        let mut samples = Vec::with_capacity(frames);
        let mut rows_before = app.profiler.counter("hierarchy_rows_drawn");
        let mut drawn_per_frame = 0_u64;
        for _ in 0..frames {
            let started = Instant::now();
            run_frame(&mut app, &context);
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
            let rows_after = app.profiler.counter("hierarchy_rows_drawn");
            drawn_per_frame = rows_after - rows_before;
            rows_before = rows_after;
        }
        let entities = app
            .document
            .current_scene
            .as_ref()
            .map_or(0, |document| document.scene.entities.len());
        let stats = percentiles_ms(&samples).unwrap_or_default();
        scenarios.push(summarize(
            "hierarchy_panel_frame_10k",
            &samples,
            vec![
                format!("entities={entities} panel=320x720"),
                format!("rows_drawn_per_frame={drawn_per_frame} (virtualized)"),
                format!("frame_time_p95={:.3}ms", stats.p95),
            ],
            drawn_per_frame > 0 && drawn_per_frame < 200,
        ));
    }

    // 12. Headless UI frame cost of the Console panel with 100k pushed log lines.
    //
    // Bounded storage (sink retains 10k) plus a cached row index plus virtual scrolling: the
    // scenario asserts only viewport rows are drawn, so "100k console entries" cannot freeze
    // the main thread by construction, and records the frame cost for the plan's budget.
    {
        let lines = build_console_fixture(FixtureSize::Large.spec().console_lines);
        let mut app = baseline_app_with_sink(SceneDocument::new_scene("console_panel"), 10_000);
        app.project_loaded = true;
        // fixture 行是非 Lua 的 tracing 行，落在编辑器分类的 sink 来源。
        for line in &lines {
            app.log_sink.push(line.clone());
        }
        app.console_log_level_filter = ConsoleLogLevelFilter::All;
        app.console_tab = ConsoleTab::Editor;
        let retained = app.log_sink.snapshot().len();
        let context = eframe::egui::Context::default();
        let screen = eframe::egui::Rect::from_min_size(
            eframe::egui::pos2(0.0, 0.0),
            eframe::egui::vec2(900.0, 260.0),
        );
        let run_frame = |app: &mut EditorApp, context: &eframe::egui::Context| {
            let raw = eframe::egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let _ = context.run(raw, |context| {
                app.draw_lua_console_panel(context);
            });
        };
        for _ in 0..5 {
            run_frame(&mut app, &context);
        }
        let frames = 30;
        let mut samples = Vec::with_capacity(frames);
        let mut indexed = 0;
        let mut hits = 0_u64;
        let mut misses = 0_u64;
        for _ in 0..frames {
            let before_hits = app.profiler.counter("console_index_cache_hit");
            let before_misses = app.profiler.counter("console_index_cache_miss");
            let started = Instant::now();
            run_frame(&mut app, &context);
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
            indexed = app.console_rows_cache.len();
            hits += app.profiler.counter("console_index_cache_hit") - before_hits;
            misses += app.profiler.counter("console_index_cache_miss") - before_misses;
        }
        let stats = percentiles_ms(&samples).unwrap_or_default();
        scenarios.push(summarize(
            "console_panel_frame_100k",
            &samples,
            vec![
                format!(
                    "pushed_lines={} retained_sink={retained} (cap 10000)",
                    lines.len()
                ),
                format!("indexed_rows={indexed}"),
                format!("index_cache hits={hits} misses={misses} over {frames} frames"),
                format!("frame_time_p95={:.3}ms panel=900x260", stats.p95),
            ],
            // 稳态帧不得重建索引，且索引行数受存储上限约束。
            indexed > 0 && indexed <= 10_000 && misses == 0,
        ));

        // 过滤词改变必须重建索引，且过滤后行数严格变小。
        let before_filter_rows = app.console_rows_cache.len();
        app.console_filter = "failing_node".to_owned();
        run_frame(&mut app, &context);
        let after_filter_rows = app.console_rows_cache.len();
        scenarios.push(instant_scenario(
            "console_filter_reindex_100k",
            samples.last().copied().unwrap_or(0.0),
            vec![
                format!("rows_before={before_filter_rows} rows_after={after_filter_rows}"),
                "filter change reuses the index cache only when the key is unchanged".to_owned(),
            ],
            after_filter_rows > 0 && after_filter_rows < before_filter_rows,
        ));
    }

    // 13. Layout gate (docs/55 §8): row content must never exceed the uniform row height,
    //     checked by measurement across DPI, zoom, panel width, and density variants instead
    //     of by eyeballing screenshots.
    //
    // `pixels_per_point` (DPI) and `zoom_factor` both scale *pixels*, so point-space layout is
    // expected to be invariant there; the variants that actually stress point space are the
    // density/text-style ones (docs/55 §6.1 Compact/Comfortable, larger fonts). The gate
    // therefore compares the *measured* content height against the *measured* row height for
    // every combination.
    {
        let fixture = build_scene_fixture(FixtureSize::Large.spec());
        let mut failures: Vec<String> = Vec::new();
        let mut combinations = 0_usize;
        let mut min_content = f32::MAX;
        let mut max_content = 0.0_f32;
        let mut min_row_height = f32::MAX;
        // (标签, 行首控件高度增量, 垂直间距增量, 正文字号增量)
        let densities: [(&str, f32, f32, f32); 3] = [
            ("compact", 0.0, 0.0, 0.0),
            ("comfortable", 3.0, 1.0, 1.0),
            ("large-text", 5.0, 2.0, 5.0),
        ];
        for (density, interact_delta, spacing_delta, text_delta) in densities {
            for pixels_per_point in [1.0_f32, 1.5, 2.0] {
                for zoom in [1.0_f32, 1.25, 1.5] {
                    for width in [160.0_f32, 320.0, 900.0] {
                        combinations += 1;
                        let mut app = baseline_app(fixture.document.clone());
                        let context = eframe::egui::Context::default();
                        context.set_pixels_per_point(pixels_per_point);
                        context.set_zoom_factor(zoom);
                        context.style_mut(|style| {
                            style.spacing.interact_size.y += interact_delta;
                            style.spacing.item_spacing.y += spacing_delta;
                            let body = style
                                .text_styles
                                .get(&eframe::egui::TextStyle::Body)
                                .cloned()
                                .unwrap_or_else(|| eframe::egui::FontId::proportional(14.0));
                            let button = style
                                .text_styles
                                .get(&eframe::egui::TextStyle::Button)
                                .cloned()
                                .unwrap_or_else(|| eframe::egui::FontId::proportional(14.0));
                            style.text_styles.insert(
                                eframe::egui::TextStyle::Body,
                                eframe::egui::FontId::new(body.size + text_delta, body.family),
                            );
                            style.text_styles.insert(
                                eframe::egui::TextStyle::Button,
                                eframe::egui::FontId::new(button.size + text_delta, button.family),
                            );
                        });
                        let screen = eframe::egui::Rect::from_min_size(
                            eframe::egui::pos2(0.0, 0.0),
                            eframe::egui::vec2(width, 640.0),
                        );
                        let run_frame = |app: &mut EditorApp, context: &eframe::egui::Context| {
                            let raw = eframe::egui::RawInput {
                                screen_rect: Some(screen),
                                ..Default::default()
                            };
                            let _ = context.run(raw, |context| {
                                app.draw_hierarchy_panel(context);
                            });
                        };
                        for _ in 0..3 {
                            run_frame(&mut app, &context);
                        }
                        // Measure the row height the panel actually uses for this style.
                        let mut measured_row_height = 0.0_f32;
                        let raw = eframe::egui::RawInput {
                            screen_rect: Some(screen),
                            ..Default::default()
                        };
                        let _ = context.run(raw, |context| {
                            eframe::egui::CentralPanel::default().show(context, |ui| {
                                measured_row_height =
                                    crate::panels::hierarchy::hierarchy_row_height(ui);
                            });
                        });
                        let content = app.hierarchy_row_probe.max_content_height();
                        let rows = app.hierarchy_row_probe.rows();
                        min_content = min_content.min(content);
                        max_content = max_content.max(content);
                        min_row_height = min_row_height.min(measured_row_height);
                        let label = format!(
                            "density={density} ppi={pixels_per_point} zoom={zoom} width={width:.0}"
                        );
                        if let Some(reason) =
                            layout_gate_violation(&label, rows, content, measured_row_height)
                        {
                            failures.push(reason);
                        }

                        // 同一组变体下也量控制台面板：日志行同样必须落在固定行高内
                        // （docs/55 §8 的"新 UI 不得重叠/裁字"覆盖两个虚拟列表）。
                        let mut console_app =
                            baseline_app(SceneDocument::new_scene("console_gate"));
                        console_app.console_log_level_filter = ConsoleLogLevelFilter::All;
                        console_app.console_tab = ConsoleTab::Editor;
                        for line in build_console_fixture(200) {
                            console_app.log_sink.push(line);
                        }
                        for _ in 0..3 {
                            let raw = eframe::egui::RawInput {
                                screen_rect: Some(screen),
                                ..Default::default()
                            };
                            let _ = context.run(raw, |context| {
                                console_app.draw_lua_console_panel(context);
                            });
                        }
                        let mut measured_console_row = 0.0_f32;
                        let raw = eframe::egui::RawInput {
                            screen_rect: Some(screen),
                            ..Default::default()
                        };
                        let _ = context.run(raw, |context| {
                            eframe::egui::TopBottomPanel::bottom("console_measure").show(
                                context,
                                |ui| {
                                    measured_console_row =
                                        crate::panels::console::console_row_height(ui);
                                },
                            );
                        });
                        let console_label = format!(
                            "console density={density} ppi={pixels_per_point} zoom={zoom} width={width:.0}"
                        );
                        if let Some(reason) = layout_gate_violation(
                            &console_label,
                            console_app.console_row_probe.rows(),
                            console_app.console_row_probe.max_content_height(),
                            measured_console_row,
                        ) {
                            failures.push(reason);
                        }
                    }
                }
            }
        }
        scenarios.push(instant_scenario(
            "layout_row_height_gate",
            0.0,
            {
                let mut notes = vec![
                    format!(
                        "density×DPI×zoom×宽度 组合数={combinations}（Hierarchy 与 Console 各检查一次）"
                    ),
                    "DPI/zoom 缩放像素；密度与正文字号才改点空间（docs/55 §6.1 两档密度）".to_owned(),
                    "判据：实测行内容高度 ≤ 实测行高，且每帧绘制行数 > 0".to_owned(),
                    format!(
                        "实测内容高度 min={min_content:.2} max={max_content:.2}；最小行高={min_row_height:.2}"
                    ),
                ];
                if failures.is_empty() {
                    notes.push(format!("{combinations} 组组合全部通过"));
                } else {
                    notes.extend(failures.iter().cloned());
                }
                notes
            },
            failures.is_empty(),
        ));
    }

    // 14. File watcher: external change → Project reflects it within the docs/55 §3.2 budget,
    //     and the per-poll cost stays off the critical path (metadata-only walk).
    {
        let root = temp_root("watch");
        fs::create_dir_all(root.join("assets")).expect("assets dir");
        fs::create_dir_all(root.join("scripts")).expect("scripts dir");
        let files = 2_000;
        for index in 0..files {
            fs::write(
                root.join("assets").join(format!("t{index}.png")),
                b"\x89PNG\r\n\x1a\n",
            )
            .expect("fixture asset");
        }
        let mut watcher = editor::FileWatcher::new(
            root.clone(),
            editor::WatchScope::default(),
            std::time::Duration::from_millis(100),
        );
        let primed = watcher.prime().expect("prime");
        assert_eq!(primed, files, "基线快照必须看到全部文件");

        // 轮询成本（UI 线程每 500 ms 付一次）。
        let polls = 10;
        let mut samples = Vec::with_capacity(polls);
        for index in 0..polls {
            let started = Instant::now();
            let diff = watcher.poll_now().expect("poll");
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
            assert!(
                diff.is_empty(),
                "无变化时不应报告差异（第 {index} 次）：{diff:?}"
            );
        }
        let stats = percentiles_ms(&samples).unwrap_or_default();
        scenarios.push(summarize(
            "file_watch_scan_2k",
            &samples,
            vec![
                format!("watched_entries={}", watcher.snapshot_len()),
                format!(
                    "p95={:.3}ms（**worker 侧**扫描成本，不进 UI 线程）",
                    stats.p95
                ),
                "无变化时 diff 为空（幂等轮询）".to_owned(),
            ],
            stats.p95 < 50.0,
        ));

        // 端到端检测延迟：外部写入 → 检测到变化。
        let (_, events_before, _, _) = watcher.stats();
        let target = root.join("assets/external_new.png");
        let writer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(120));
            fs::write(&target, b"\x89PNG\r\n\x1a\n").expect("external write");
        });
        let started = Instant::now();
        let diff = watcher
            .wait_for_change(std::time::Duration::from_secs(5))
            .expect("not cancelled")
            .expect("必须在预算内检测到外部变化");
        let latency = started.elapsed();
        writer.join().expect("writer thread");
        let (_, events_after, changed, _) = watcher.stats();
        scenarios.push(instant_scenario(
            "file_watch_external_change_latency",
            latency.as_secs_f32() * 1000.0,
            vec![
                format!("added={:?}", diff.added),
                format!(
                    "latency_ms={:.0}（docs/55 §3.2 预算 2000）",
                    latency.as_millis()
                ),
                format!(
                    "events {}→{} changed_files={changed}",
                    events_before, events_after
                ),
            ],
            latency < std::time::Duration::from_millis(2000)
                && events_after == events_before + 1
                && diff.added == vec![std::path::PathBuf::from("assets/external_new.png")],
        ));

        // 后台监听句柄：UI 线程每帧只做一次 try_recv（µs 级），扫描在 worker 上。
        let handle = editor::spawn_watcher(
            root.clone(),
            editor::WatchScope::default(),
            std::time::Duration::from_millis(100),
        );
        let idle_samples: Vec<f32> = (0..200)
            .map(|_| {
                let started = Instant::now();
                let _ = handle.try_take();
                started.elapsed().as_secs_f32() * 1000.0
            })
            .collect();
        let idle = percentiles_ms(&idle_samples).unwrap_or_default();

        let target = root.join("scripts/external.lua");
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(120));
            fs::write(&target, b"return true\n").expect("external write");
        });
        let started = Instant::now();
        let mut received = None;
        while started.elapsed() < std::time::Duration::from_secs(5) {
            if let Some(diff) = handle.try_take() {
                received = Some(diff);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let latency = started.elapsed();
        let diff = received.expect("后台监听必须在预算内报告外部变化");
        let stats = handle.stats().clone();
        handle.cancel();
        scenarios.push(summarize(
            "file_watch_worker_and_ui_cost",
            &idle_samples,
            vec![
                format!(
                    "ui_try_recv p50={:.4}ms p95={:.4}ms（每帧一次）",
                    idle.p50, idle.p95
                ),
                format!(
                    "worker last_scan={:.2}ms entries={} scans={} events={}",
                    stats.last_scan_cost().as_secs_f64() * 1000.0,
                    stats.entries(),
                    stats.polls(),
                    stats.events()
                ),
                format!(
                    "external_change_latency_ms={:.0} added={:?}",
                    latency.as_millis(),
                    diff.added
                ),
            ],
            idle.p95 < 1.0
                && latency < std::time::Duration::from_millis(2000)
                && diff.added == vec![std::path::PathBuf::from("scripts/external.lua")],
        ));

        // 删除也必须被发现（生成器/清理脚本）。
        fs::remove_file(root.join("assets/t0.png")).expect("remove fixture");
        let diff = watcher
            .wait_for_change(std::time::Duration::from_secs(5))
            .expect("not cancelled")
            .expect("删除必须被检测到");
        scenarios.push(instant_scenario(
            "file_watch_detects_removal",
            0.0,
            vec![format!("removed={:?}", diff.removed)],
            diff.removed == vec![std::path::PathBuf::from("assets/t0.png")],
        ));
        let _ = fs::remove_dir_all(root);
    }

    // 27. Play 状态机的"每帧计划"（docs/55 ER-10）。
    //
    // 不变量比计时重要：暂停必须**真的**冻结逻辑（600 帧 0 步）、单帧步进必须**恰好**
    // 走请求的帧数（不多不少）、卡顿帧的补步必须有上限（不死亡螺旋）。
    {
        let mut controller = editor::PlayController::new();
        controller.begin_start().expect("进入 Starting");
        controller.mark_running().expect("进入 Playing");
        let frame = 1.0 / 60.0;
        let mut samples = Vec::with_capacity(2048);
        let mut running_steps = 0_u32;
        for _ in 0..600 {
            let started = Instant::now();
            let plan = controller.plan_frame(frame);
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
            running_steps += plan.steps;
        }
        controller.pause().expect("暂停");
        let mut paused_steps = 0_u32;
        for _ in 0..600 {
            let started = Instant::now();
            let plan = controller.plan_frame(frame);
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
            paused_steps += plan.steps;
        }
        // 暂停态请求 5 帧：接下来 10 次规划只能跑出 5 步，之后继续冻结。
        controller.request_step(5).expect("步进请求");
        let mut step_steps = 0_u32;
        for _ in 0..10 {
            let started = Instant::now();
            let plan = controller.plan_frame(frame);
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
            step_steps += plan.steps;
        }
        let tail = controller.plan_frame(frame);
        controller.resume().expect("恢复运行");
        // 1 秒的卡顿帧：补步必须被上限截断。
        let hitch = controller.plan_frame(1.0);
        let done = controller.state() == editor::PlayState::Playing;
        scenarios.push(summarize(
            "play_pause_step_frame_plan",
            &samples,
            vec![
                format!("running_steps={running_steps} expected=600"),
                format!("paused_steps={paused_steps} expected=0"),
                format!(
                    "step_steps={step_steps} requested=5 tail_plan_steps={}",
                    tail.steps
                ),
                format!(
                    "hitch_steps={} cap={}",
                    hitch.steps,
                    editor::MAX_CATCH_UP_STEPS
                ),
            ],
            running_steps == 600
                && paused_steps == 0
                && step_steps == 5
                && tail.steps == 0
                && hitch.steps == editor::MAX_CATCH_UP_STEPS
                && !hitch.paused
                && done,
        ));
    }

    // 28. Schema 驱动的字段编辑（docs/55 阶段 2）：把"新组件不改巨型 match"的字段路径
    //     纳入门槛——每次编辑恰好一条历史、幂等提交**不**产生历史、取值确实落进文档、
    //     历史预算仍然生效。
    {
        let fixture = build_scene_fixture(FixtureSize::Medium.spec());
        let targets: Vec<SceneEntityId> = fixture.entity_ids().into_iter().take(200).collect();
        let mut document = fixture.document;
        {
            for id in &targets {
                if let Some(entity) = document.scene.entities.get_mut(id) {
                    entity
                        .components
                        .insert("Quad".to_owned(), "size=64,32;color=1,1,1,1".to_owned());
                }
            }
        }
        let mut app = baseline_app(document);
        let edits = 1000;
        let mut samples = Vec::with_capacity(edits);
        let mut applied = 0;
        let mut last_value = String::new();
        for step in 0..edits {
            let id = targets[step % targets.len()];
            // 每步都换一个**必定不同于上一次**的取值：这样 `applied == edits` 才是强不变量
            // （否则循环取值会命中"无变化"，把幂等语义混进吞吐口径里）。
            let value = format!("{},{}", 1000 + step, 900 + step);
            let started = Instant::now();
            let result = app.dispatch(editor::EditorCommand::SetComponentField {
                entity: id,
                component: "Quad".to_owned(),
                field: "size".to_owned(),
                value: value.clone(),
            });
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
            if result == editor::CommandResult::Applied {
                applied += 1;
                last_value = value;
            }
        }
        let undo_entries = app.history.undo_len();
        let dropped = app.history.dropped_entries();
        // 幂等复查：把每个目标实体的**当前原文**再提交一次，必须全部 no-op 且不新增历史。
        let mut idempotent_noops = 0;
        {
            let scene = app.document.current_scene.as_ref().expect("scene").clone();
            for id in &targets {
                let text = scene.scene.entities[id]
                    .components
                    .get("Quad")
                    .and_then(|payload| {
                        rustlua_ecs::property_path::read_field_text(
                            payload,
                            &rustlua_ecs::property_path::PropertyPath::new("Quad", "size"),
                        )
                        .ok()
                    })
                    .unwrap_or_default();
                let result = app.dispatch(editor::EditorCommand::SetComponentField {
                    entity: *id,
                    component: "Quad".to_owned(),
                    field: "size".to_owned(),
                    value: text,
                });
                if result == editor::CommandResult::NoOp {
                    idempotent_noops += 1;
                }
            }
        }
        let undo_after_idempotent = app.history.undo_len();
        // 取值确实写进了文档（抽查最后一个目标）：读回的原文 == 归一化后的提交值。
        let payload = app.document.current_scene.as_ref().unwrap().scene.entities
            [&targets[targets.len() - 1]]
            .components["Quad"]
            .clone();
        let expected_text = last_value
            .split(',')
            .map(|part| format!("{part}.000000"))
            .collect::<Vec<_>>()
            .join(",");
        let read_back = rustlua_ecs::property_path::read_field_text(
            &payload,
            &rustlua_ecs::property_path::PropertyPath::new("Quad", "size"),
        )
        .unwrap_or_default();
        scenarios.push(summarize(
            "schema_field_edit_1k",
            &samples,
            vec![
                format!("applied={applied} expected={edits}"),
                format!(
                    "idempotent_noops={idempotent_noops} expected={}",
                    targets.len()
                ),
                format!("undo_entries={undo_entries} dropped={dropped} budget={}", {
                    editor::CommandService::default().max_entries()
                }),
                format!("undo_after_idempotent={undo_after_idempotent}"),
                format!("read_back={read_back} expected_text={expected_text}"),
            ],
            applied == edits
                && idempotent_noops == targets.len()
                && undo_after_idempotent == undo_entries
                && undo_entries == 200
                && dropped == 800
                && editor::CommandService::default().max_entries()
                    == editor::CommandService::DEFAULT_MAX_ENTRIES
                && read_back == expected_text
                && payload.contains("color=1,1,1,1"),
        ));
    }

    // 30. 字段级**语义规则**必须真的走到审计路径（docs/55 阶段 2「字段校验」）。
    //
    // 场景：一个临时工程里放一个 400 实体的场景，其中 200 个脚本路径非法
    // （绝对路径 / 穿越 / 空），200 个合法。不变量：审计恰好报 200 条 schema_error、
    // 合法的 200 个一条不报（没有误报），并且审计扫描的是**磁盘上的文件**（不是内存校验）。
    {
        let root = std::env::temp_dir().join(format!(
            "rustlua-baseline-schema-rules-{}",
            std::process::id()
        ));
        let scenes = root.join("scenes");
        fs::create_dir_all(&scenes).expect("temp project scenes dir");
        let mut scene = rustlua_scene::scene::Scene::new("rules");
        let bad = [
            // Windows 上 `Path::is_absolute()` 对 `/x` 返回 false，必须由根组件判定兜住。
            "file=/etc/passwd.lua",
            "file=../outside.lua",
            "file=",
        ];
        let mut entity_index = 1_u64;
        for index in 0..200 {
            let id = SceneEntityId(entity_index);
            scene.add_entity(id, format!("bad{index}"), None).unwrap();
            scene.entities.get_mut(&id).unwrap().components.insert(
                rustlua_ecs::entity_script::SCRIPT_COMPONENT.to_owned(),
                bad[index % bad.len()].to_owned(),
            );
            entity_index += 1;
        }
        for index in 0..200 {
            let id = SceneEntityId(entity_index);
            scene.add_entity(id, format!("good{index}"), None).unwrap();
            scene.entities.get_mut(&id).unwrap().components.insert(
                rustlua_ecs::entity_script::SCRIPT_COMPONENT.to_owned(),
                format!("file=scripts/ai/patrol_{index}.lua"),
            );
            entity_index += 1;
        }
        let path = scenes.join("rules.scene");
        scene.save(&path).expect("scene saves");
        let started = Instant::now();
        let audit = editor::audit_project(&root).expect("audit runs");
        let elapsed = started.elapsed().as_secs_f32() * 1000.0;
        let bad_reasons: Vec<&str> = audit
            .findings
            .iter()
            .map(|finding| finding.reason.as_str())
            .collect();
        let absolute = bad_reasons
            .iter()
            .filter(|reason| reason.contains("绝对路径"))
            .count();
        let traversal = bad_reasons
            .iter()
            .filter(|reason| reason.contains(".."))
            .count();
        let empty = bad_reasons
            .iter()
            .filter(|reason| reason.contains("不能为空"))
            .count();
        scenarios.push(instant_scenario(
            "schema_field_rules_audit_400",
            elapsed,
            vec![
                format!(
                    "documents={} components={} findings={}",
                    audit.documents,
                    audit.components,
                    audit.findings.len()
                ),
                format!("absolute={absolute} traversal={traversal} empty={empty}"),
            ],
            audit.documents == 1
                && audit.components == 400
                && audit.findings.len() == 200
                && absolute + traversal + empty == 200
                && audit.findings.iter().all(|finding| {
                    finding.field == "file"
                        && finding.component == rustlua_ecs::entity_script::SCRIPT_COMPONENT
                }),
        ));
        let _ = fs::remove_dir_all(&root);
    }

    // 31. Prefab 往返**字节与哈希一致**（docs/55 阶段 3 验收条款）。
    //
    // 验收原文是"Prefab 往返 hash 一致"。这条不是"跑一遍没报错"就算过，而是四个具体不变量：
    // ① `to_text` → `from_text` → `to_text` 逐字节相同（序列化是定点的，不会每次多/少一个字段）；
    // ② 写盘 → 读回 → 再写盘的字节哈希与首次相同（磁盘往返也不引入漂移）；
    // ③ 实例化 → 落盘场景 → 重新打开之后，实例上的覆盖字段**逐字**保持原值；
    // ④ 把"实例 vs 源预制体"重新 diff 出来，覆盖集合与写入前**完全相同**（幂等，不会越用越多）。
    {
        use rustlua_assets::resource_db::AssetId;
        use rustlua_scene::prefab::Prefab;
        use rustlua_scene::scene::Scene;

        let root = temp_root("prefab-roundtrip");
        let prefab_path = root.join("prefabs/hero.prefab");
        let scene_path = root.join("scenes/level.scene");

        // 造一棵 3 节点的小树（根 + 两个子节点，含组件与元数据）。
        let mut source = Scene::new("prefab_source");
        let ids: Vec<SceneEntityId> = (1..=3).map(SceneEntityId).collect();
        source.add_entity(ids[0], "Hero", None).expect("root");
        source
            .add_entity(ids[1], "Body", Some(ids[0]))
            .expect("child");
        source
            .add_entity(ids[2], "Weapon", Some(ids[0]))
            .expect("child");
        for (index, id) in ids.iter().enumerate() {
            let entity = source.entities.get_mut(id).expect("entity");
            entity.components.insert(
                "Transform".to_owned(),
                format!("position={index},0,0;rotation=0,0,0,1;scale=1,1,1"),
            );
            entity
                .components
                .insert("Quad".to_owned(), "size=32,32;color=1,1,1,1".to_owned());
            entity
                .metadata
                .insert("tag".to_owned(), format!("node{index}"));
        }

        let prefab = Prefab::from_scene(&source, ids[0], AssetId::from_seed("hero"), "hero")
            .expect("建 Prefab");
        let text_first = prefab.to_text();
        let parsed = Prefab::from_text(&text_first).expect("文本回读");
        let text_second = parsed.to_text();
        let text_stable = text_first == text_second;
        prefab.save(&prefab_path).expect("保存 Prefab");
        let loaded = Prefab::load(&prefab_path).expect("读回 Prefab");
        let text_disk = loaded.to_text();
        let disk_stable = text_disk == text_first;

        // 实例化 + 一条**逐字段**覆盖，然后落盘场景并重新打开。
        let mut scene = Scene::new("level");
        let override_value = "99,88".to_owned();
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert(
            (ids[1], "property:Quad.size".to_owned()),
            override_value.clone(),
        );
        let instance = loaded
            .instantiate(&mut scene, None, &overrides)
            .expect("带覆盖实例化");
        let instance_root = instance.instance_root;
        let body = instance.source_to_instance[&ids[1]];
        scene.save(&scene_path).expect("保存场景");
        let reopened = Scene::load(&scene_path).expect("重新打开场景");

        // ③ 覆盖字段逐字保持：重开之后实例上的 Quad.size 必须还是覆盖值。
        let reopened_body = reopened.entities.get(&body).expect("Body 实例");
        let reopened_size = rustlua_ecs::property_path::read_field_text(
            &reopened_body.components["Quad"],
            &rustlua_ecs::property_path::PropertyPath::new("Quad", "size"),
        )
        .unwrap_or_default();
        let override_survived = reopened_size == "99.000000,88.000000";
        let _ = instance_root;

        // ④ 覆盖集合幂等：把"实例 vs 源"重新 diff 一遍，结果与写入前一致。
        let rediffed: std::collections::BTreeMap<(SceneEntityId, String), String> = reopened
            .entities
            .values()
            .filter_map(|entity| {
                let source_id = *loaded
                    .entities
                    .iter()
                    .find(|(_, candidate)| candidate.name == entity.name)
                    .map(|(id, _)| id)?;
                let source_quad = loaded.entities[&source_id].components.get("Quad")?;
                let current = entity.components.get("Quad")?;
                (current != source_quad).then(|| {
                    (
                        (source_id, "property:Quad.size".to_owned()),
                        current.clone(),
                    )
                })
            })
            .collect();
        let diff_idempotent = rediffed.len() == 1
            && rediffed
                .values()
                .next()
                .is_some_and(|value| value == &reopened_body.components["Quad"]);

        scenarios.push(instant_scenario(
            "prefab_roundtrip_hash",
            0.0,
            vec![
                format!("text_stable={text_stable} disk_stable={disk_stable}"),
                format!(
                    "entities={} text_bytes={}",
                    loaded.entities.len(),
                    text_first.len()
                ),
                format!("override_survived={override_survived} read_back={reopened_size}"),
                format!(
                    "rediff_overrides={} diff_idempotent={diff_idempotent}",
                    rediffed.len()
                ),
            ],
            text_stable && disk_stable && override_survived && diff_idempotent,
        ));
        let _ = fs::remove_dir_all(&root);
    }

    // 32. docs/55 ER-03「patch 原型」：字段级编辑表示成补丁之后，**内存**到底降多少，
    //     以及最要紧的那条不变量（Undo/Redo 等价）是否仍然成立。
    //
    // 计划的 §1 把"Undo 主要保存完整文档快照，大场景和长编辑会话的内存成本不可控"列为主要矛盾；
    // 阶段 1 的"历史内存预算"只是止血（上界 200 条）。这里给出同一串编辑在两条历史下的
    // **保留字节数**，并用逐字节比较证明补丁路径撤销/重做到位——"Undo 不等价"是 docs/55 §8
    // 的硬阻断项，原型不通过这一条就没有意义。
    {
        use editor::{EditPatch, PatchHistory};
        use rustlua_scene::scene::SceneEntityId;

        let fixture = build_scene_fixture(FixtureSize::Medium.spec());
        let mut base = fixture.document;
        let targets: Vec<SceneEntityId> = base.scene.entities.keys().copied().take(200).collect();
        for id in &targets {
            if let Some(entity) = base.scene.entities.get_mut(id) {
                entity
                    .components
                    .insert("Quad".to_owned(), "size=64,32;color=1,1,1,1".to_owned());
            }
        }
        let original_text = base.scene.to_text();
        let single_document_bytes = original_text.len();

        let mut patch_document = base.clone();
        let mut snapshot_document = base;
        let edits = 1_000;
        // 预算取 ≥ 编辑数：这一条量的是"**同一串编辑**在两条历史下的成本与等价性"，
        // 若把预算压到 200，两条历史都撤不回起点（各丢了 800 条），那就成了在测预算而不是测补丁。
        // 预算上限本身由 `patch_history::tests::adjacent_transforms_merge_and_budget_drops_the_oldest`
        // 与 `history_budget_500_edits` 分别钉住。
        let mut patch_history = PatchHistory::new(edits);
        let mut snapshot_history = editor::CommandHistory::new();
        let mut samples = Vec::with_capacity(edits);

        for step in 0..edits {
            let id = targets[step % targets.len()];
            let before = patch_document.scene.entities[&id].components["Quad"].clone();
            let after = format!("size={},{};color=1,1,1,1", 1000 + step, 900 + step);

            let started = Instant::now();
            patch_history.apply(
                &mut patch_document.scene,
                EditPatch::set_component(id, "Quad", Some(before), after.clone()),
            );
            samples.push(started.elapsed().as_secs_f32() * 1000.0);

            let after_for_snapshot = after;
            snapshot_history
                .execute(
                    &mut snapshot_document,
                    "Set Component",
                    CommandKind::Property,
                    move |document| {
                        if let Some(entity) = document.scene.entities.get_mut(&id) {
                            entity
                                .components
                                .insert("Quad".to_owned(), after_for_snapshot.clone());
                        }
                        Ok::<(), ()>(())
                    },
                )
                .expect("快照编辑");
        }

        // ① 两条历史应用同一串编辑之后，文档必须逐字节相同。
        let same_after_edits = patch_document.scene.to_text() == snapshot_document.scene.to_text();
        let final_text = patch_document.scene.to_text();

        // ② 全部撤销 → 回到起点；③ 全部重做 → 回到终态。
        while patch_history.can_undo() {
            patch_history.undo(&mut patch_document.scene);
        }
        while snapshot_history.can_undo() {
            snapshot_history.undo(&mut snapshot_document).ok();
        }
        let undo_equivalent = patch_document.scene.to_text() == original_text
            && snapshot_document.scene.to_text() == original_text;
        while patch_history.can_redo() {
            patch_history.redo(&mut patch_document.scene);
        }
        while snapshot_history.can_redo() {
            snapshot_history.redo(&mut snapshot_document).ok();
        }
        let redo_equivalent = patch_document.scene.to_text() == final_text
            && snapshot_document.scene.to_text() == final_text;

        // ④ 内存：补丁保留的字节 vs 快照历史保留的字节（后者按"每条 before + after"下界估算，
        //    真实快照实现还要多一次克隆分配，所以这个比较对快照侧是**保守**的）。
        let patch_bytes = patch_history.retained_bytes();
        let snapshot_bytes = single_document_bytes * patch_history.undo_len() * 2;
        scenarios.push(summarize(
            "patch_history_memory_1k_edits",
            &samples,
            vec![
                format!(
                    "single_document_bytes={single_document_bytes} entities={}",
                    patch_document.scene.entities.len()
                ),
                format!(
                    "patch_retained_bytes={patch_bytes} snapshot_retained_bytes_lower_bound={snapshot_bytes}"
                ),
                format!(
                    "ratio={:.3}% undo_len={} dropped={}",
                    100.0 * patch_bytes as f64 / snapshot_bytes.max(1) as f64,
                    patch_history.undo_len(),
                    patch_history.dropped_entries()
                ),
                format!(
                    "same_after_edits={same_after_edits} undo_equivalent={undo_equivalent} redo_equivalent={redo_equivalent}"
                ),
            ],
            same_after_edits
                && undo_equivalent
                && redo_equivalent
                && patch_bytes * 20 < snapshot_bytes,
        ));
    }

    // 32b. **生产通道的实测**（M79 P1-2b）：上一条款量的是原型（`PatchHistory` 自己那条栈），
    // 这一条量的是接进 `CommandHistory` 之后的真实历史：同一串组件写入，
    // `CommandHistory::new()`（补丁通道开）对 `CommandHistory::without_patch_compression()`
    // （纯快照）。三个读数各有各的用处：
    //   * **保留字节**——省内存的那一半，用 `retained_bytes_estimate()`（两侧同一口径，见其文档）；
    //   * **等价性**——撤销到底 / 重做到底必须逐字节对得上快照那一侧（docs/55 §8 的硬阻断项）；
    //   * **提交点耗时**——省内存不是免费的：补丁形态每次提交多做一次 `Scene` 克隆 + 全字段比较。
    //     这一栏就是"买不到的部分"的价格，跑出来若比快照侧更慢，慢多少一目了然。
    //
    // 跑**两档夹具**是刻意的：Medium（1,000 实体）是给"大场景会不会疼"的答案，
    // `patch_history_production_small`（100 实体）才是日常工程的大小——同一份判据在两个规模上
    // 各报一次，才能把"提交点成本随场景规模走"这句话变成数字，而不是留成未测项。
    {
        use rustlua_scene::scene::SceneEntityId;

        for (label, size, edits) in [
            ("patch_history_production", FixtureSize::Medium, 1_000usize),
            ("patch_history_production_small", FixtureSize::Small, 200),
        ] {
            let fixture = build_scene_fixture(size.spec());
            let mut base = fixture.document;
            let targets: Vec<SceneEntityId> =
                base.scene.entities.keys().copied().take(200).collect();
            for id in &targets {
                if let Some(entity) = base.scene.entities.get_mut(id) {
                    entity
                        .components
                        .insert("Quad".to_owned(), "size=64,32;color=1,1,1,1".to_owned());
                }
            }
            let original_text = base.scene.to_text();

            let mut patch_document = base.clone();
            let mut snapshot_document = base;
            let mut patch_history = editor::CommandHistory::new();
            let mut snapshot_history = editor::CommandHistory::without_patch_compression();
            let mut samples = Vec::with_capacity(edits);
            let mut snapshot_samples = Vec::with_capacity(edits);

            for step in 0..edits {
                let id = targets[step % targets.len()];
                let after = format!("size={},{};color=1,1,1,1", 1000 + step, 900 + step);

                let started = Instant::now();
                let value = after.clone();
                patch_history
                    .execute(
                        &mut patch_document,
                        "Set Component",
                        CommandKind::Property,
                        move |document| {
                            if let Some(entity) = document.scene.entities.get_mut(&id) {
                                entity.components.insert("Quad".to_owned(), value.clone());
                            }
                            Ok::<(), ()>(())
                        },
                    )
                    .expect("补丁通道编辑");
                samples.push(started.elapsed().as_secs_f32() * 1000.0);

                let started = Instant::now();
                let value = after;
                snapshot_history
                    .execute(
                        &mut snapshot_document,
                        "Set Component",
                        CommandKind::Property,
                        move |document| {
                            if let Some(entity) = document.scene.entities.get_mut(&id) {
                                entity.components.insert("Quad".to_owned(), value.clone());
                            }
                            Ok::<(), ()>(())
                        },
                    )
                    .expect("快照通道编辑");
                snapshot_samples.push(started.elapsed().as_secs_f32() * 1000.0);
            }

            let same_after_edits =
                patch_document.scene.to_text() == snapshot_document.scene.to_text();
            let final_text = patch_document.scene.to_text();
            while patch_history.can_undo() {
                patch_history.undo(&mut patch_document).unwrap();
                snapshot_history.undo(&mut snapshot_document).unwrap();
            }
            let undo_equivalent = patch_document.scene.to_text() == original_text
                && snapshot_document.scene.to_text() == original_text;
            while patch_history.can_redo() {
                patch_history.redo(&mut patch_document).unwrap();
                snapshot_history.redo(&mut snapshot_document).unwrap();
            }
            let redo_equivalent = patch_document.scene.to_text() == final_text
                && snapshot_document.scene.to_text() == final_text;

            let (patch_entries, snapshot_entries) = patch_history.storage_counts();
            let patch_bytes = patch_history.retained_bytes_estimate();
            let snapshot_bytes = snapshot_history.retained_bytes_estimate();
            let patch_commit_p50 = percentiles_ms(&samples).unwrap_or_default().p50;
            let snapshot_commit_p50 = percentiles_ms(&snapshot_samples).unwrap_or_default().p50;
            scenarios.push(summarize(
                label,
                &samples,
                vec![
                    format!(
                        "edits={edits} entities={} entries={} patch_form={patch_entries} snapshot_form={snapshot_entries}",
                        patch_document.scene.entities.len(),
                        patch_history.undo_len()
                    ),
                    format!(
                        "retained_bytes patch={patch_bytes} snapshot={snapshot_bytes} ratio={:.3}%",
                        100.0 * patch_bytes as f64 / snapshot_bytes.max(1) as f64
                    ),
                    format!(
                        "commit_p50_ms patch={patch_commit_p50:.4} snapshot={snapshot_commit_p50:.4}"
                    ),
                    format!(
                        "same_after_edits={same_after_edits} undo_equivalent={undo_equivalent} redo_equivalent={redo_equivalent}"
                    ),
                ],
                same_after_edits
                    && undo_equivalent
                    && redo_equivalent
                    && patch_entries == edits
                    && snapshot_entries == 0
                    && patch_bytes * 10 < snapshot_bytes,
            ));
        }
    }

    // 33. Inspector 的 schema 路径（批 10 的量尺）：**每帧到底在解析上花多少**。
    //
    // 背景：CHANGELOG 里有一条长期"有意未做"的偏离——"Inspector 每帧解析不做缓存"，理由是
    // "量级是每帧常数、约微秒"——但那是**估算**，而仓库的规矩是"只判测量是活的"。
    // 这条场景把那句话变成数字，并把同一条路径**拆账**：`camera_notes`（只读说明：解析
    // `Camera`/`Transform` 载荷 + 若干 `format!`）vs `rows_for_selection`（逐字段行构建）。
    //
    // 实测（release，本机）结论写进 notes，供"要不要缓存"这个决定复核：
    //   camera_notes ≈ 0.8~1.8 µs/帧；其中 `Camera::from_component_str` 本身 ≈ 0.6 µs；
    //   而同一帧同一条路径的 `rows_for_selection` ≈ 53 µs —— 差 30 倍，且它 **95% 不是解析**
    //   （是逐字段的 String 分配与 schema 查表）。因此"每帧解析"不是这条路径上的大头：
    //   缓存解析省下的是亚微秒，键比较的成本与它同量级；真要动手该动的是行构建，形状完全不同。
    {
        let entity = build_scene_fixture(FixtureSize::Small.spec())
            .document
            .scene
            .entities
            .values()
            .next()
            .cloned()
            .expect("fixture 至少一个实体");
        // 让这个实体同时有相机与变换，覆盖最重的 `pose=transform` 分支。
        let mut probe = entity;
        probe.components.insert(
            "Camera".to_owned(),
            "projection=perspective;position=0,1,6;target=0,0,0;fov=60;near=0.1;far=100;pose=transform"
                .to_owned(),
        );
        probe.components.insert(
            "Transform".to_owned(),
            "position=0,1,6;rotation=0,0,0,1;scale=1,1,1".to_owned(),
        );

        let iterations = 200;
        // 每轮跑 batch 次调用，摊到单次（单次只有几十微秒，直接计时会被 Instant 精度掩盖）。
        const BATCH: usize = 200;
        let mut samples = Vec::with_capacity(iterations);
        let mut per_call_ns = 0.0f64;
        for _ in 0..iterations {
            let started = Instant::now();
            for _ in 0..BATCH {
                std::hint::black_box(editor::component_notes(&probe, "Camera"));
            }
            let elapsed = started.elapsed();
            per_call_ns = elapsed.as_nanos() as f64 / BATCH as f64;
            samples.push(elapsed.as_secs_f32() * 1000.0);
        }
        let notes_ns = per_call_ns;

        let mut rows_samples = Vec::with_capacity(iterations);
        let mut rows_per_call_ns = 0.0f64;
        for _ in 0..iterations {
            let started = Instant::now();
            for _ in 0..BATCH {
                std::hint::black_box(editor::rows_for_selection(&[&probe]));
            }
            let elapsed = started.elapsed();
            rows_per_call_ns = elapsed.as_nanos() as f64 / BATCH as f64;
            rows_samples.push(elapsed.as_secs_f32() * 1000.0);
        }

        let payload = probe.components.get("Camera").cloned().unwrap_or_default();
        let parse_started = Instant::now();
        for _ in 0..BATCH {
            std::hint::black_box(rustlua_camera::camera::Camera::from_component_str(&payload).ok());
        }
        let parse_ns = parse_started.elapsed().as_nanos() as f64 / BATCH as f64;

        let components = probe.components.len();
        let notes = vec![
            format!("camera_notes_per_call_ns={notes_ns:.0}（含解析 + 若干 format 分配）"),
            format!("camera_parse_per_call_ns={parse_ns:.0}（纯解析，缓存能省掉的那部分）"),
            format!("rows_for_selection_per_call_ns={rows_per_call_ns:.0} components={components}"),
            format!(
                "parse_share_of_rows={:.2}%（行构建的大头是逐字段 String 分配与 schema 查表，不是解析）",
                100.0 * parse_ns / rows_per_call_ns.max(1.0)
            ),
            "note: 绝对数是**本机构建相关**的（debug 约是 release 的 7~8 倍；`verify.ps1` 跑的是 \
             debug，同机 A/B 用同一 profile 才有意义）——因此下面的判据只钉**量级关系**"
                .to_owned(),
            "判据：解析与只读说明都必须**远小于**行构建（parse × 10 < rows、notes × 3 < rows）；\
             camera_notes 每帧一次 ⇒ 亚微秒~十几微秒，缓存省下的量与键比较同量级 ⇒ 不做"
                .to_owned(),
        ];
        // 断言只钉"量级关系"，**不钉绝对时间**：时间是构建与机器相关的量（debug 比 release 慢
        // 7~8 倍，`verify.ps1` 走的正是 debug），把它写进 `ok` 会在换个 profile 时假红——第一版
        // 判据就是这么踩的。这两条一旦不成立（例如有人把载荷解析搬进逐字段循环，或行构建被
        // 优化到跟解析同量级），场景立刻变红。
        let ok = parse_ns * 10.0 < rows_per_call_ns && notes_ns * 3.0 < rows_per_call_ns;
        let _ = &rows_samples;
        scenarios.push(summarize("inspector_schema_path", &samples, notes, ok));
    }

    // M76-07：1000 项虚拟列表的逐帧绑定开销。量的是"算窗口 + 绑槽位 + 算矩形"，
    // 不含文本测量/绘制/上传；内存也没有堆计数器，只钉结构性上界（槽位池不随总项数增长）。
    {
        let total = 1_000;
        let viewport = 720.0;
        let frames = 1_000usize;
        let keys = (0..total)
            .map(|index| format!("item-{index}"))
            .collect::<Vec<_>>();
        let mut list = VirtualList::new(VirtualListConfig::default(), total, viewport);
        let mut pool = SlotPool::default();
        let mut samples = Vec::with_capacity(frames);
        let mut bound_total = 0usize;
        let mut bound_max = 0usize;
        for _ in 0..frames {
            let started = Instant::now();
            let bound = bind_window(&list, list.window(), &keys, &mut pool);
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
            bound_total += bound.len();
            bound_max = bound_max.max(bound.len());
            list.scroll_by(7.0);
            if list.offset() + viewport >= list.total_height() {
                list.scroll_to(0.0);
            }
        }
        let per_frame = bound_total / frames;
        let capacity = pool.capacity();
        scenarios.push(summarize(
            "ui_virtual_list_1k",
            &samples,
            vec![
                format!("total={total} frames={frames} viewport={viewport}"),
                format!("bound_per_frame={per_frame} max={bound_max}"),
                format!("slot_pool_capacity={capacity} bound_keys={}", pool.bound()),
                "note: 只量逻辑层（窗口+槽位+矩形）；无文本测量/绘制/上传，也没有堆内存计数器"
                    .to_owned(),
            ],
            per_frame <= 40 && bound_max <= 40 && capacity <= 40,
        ));
    }

    // 无头启动：项目校验 + `EditorApp` 构造 + 布局加载。窗口、GPU 设备与首帧不在本场景内
    // （那几段由 GUI smoke 与 `--gui-evidence` 负责），这里钉的是每次启动都绕不开的那部分。
    {
        let root = temp_root("startup");
        fs::write(
            root.join("project.toml"),
            "[project]\nname = \"editor-baseline\"\n",
        )
        .expect("startup fixture project.toml must be writable");
        fs::create_dir_all(root.join("scripts")).expect("startup fixture scripts dir");
        let iterations = 5usize;
        let mut samples = Vec::with_capacity(iterations);
        let mut ok = true;
        for _ in 0..iterations {
            let started = Instant::now();
            let project = editor::open_project(&root).expect("startup fixture project must open");
            let mut app = EditorApp::new(project, None, LogSink::new(512));
            app.load_layout_for_project();
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
            ok &= app.document.current_scene.is_none() && app.status.contains("尚未创建场景");
            std::hint::black_box(&app);
        }
        let _ = fs::remove_dir_all(&root);
        scenarios.push(summarize(
            "startup_headless_project_load",
            &samples,
            vec![
                "scope=headless：open_project（project.toml + scripts/ 校验）+ EditorApp 构造 + 布局加载"
                    .to_owned(),
                "excludes=window/GPU/first_frame（真窗口启动不在本场景）".to_owned(),
                format!("iterations={iterations} projects_without_scene={}", if ok { iterations } else { 0 }),
                "判据：空工程打开后不得凭空出现场景，状态栏必须是「尚未创建场景」".to_owned(),
            ],
            ok,
        ));
    }

    // Play/Scene 每帧重绘的 CPU 侧（渲染器之前）：`vertices` + `scene_3d` 提取。
    // Play 期间这两个**每帧都跑**（镜像姿态逐帧在变，视图缓存必然失效），是编辑器里唯一
    // 与实体数成正比的逐帧成本；GPU 提交不在本场景（`--perf-baseline` 全程无头）。
    {
        use engine::host::RuntimeScene;
        use rustlua_camera::camera::{Camera3D, Viewport};
        use rustlua_math::math3d::Vec3;

        const WIDTH: u32 = 1280;
        const HEIGHT: u32 = 720;
        let fixture = build_scene_fixture(FixtureSize::Medium.spec());
        let entity_count = fixture.document.scene.entities.len();
        let mut scene = fixture.document.scene.clone();
        for entity in scene.entities.values_mut() {
            entity.components.insert(
                "MeshRenderer".to_owned(),
                "mesh=assets/models/baseline.glb".to_owned(),
            );
        }
        let root = temp_root("play-redraw");
        let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene)
            .expect("play redraw fixture runtime");
        let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("baseline viewport");
        let mut camera = Camera3D {
            position: Vec3::new(0.0, 4.0, 12.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: std::f32::consts::FRAC_PI_3,
            near: 0.1,
            far: 500.0,
            pose_from_transform: false,
            enabled: true,
            clip_plane: [0.0; 4],
        };
        let iterations = 20usize;
        let mut samples = Vec::with_capacity(iterations);
        let mut drawables = 0usize;
        for index in 0..iterations {
            // 相机每帧挪一点：LOD 距离档与可见遮罩判定都要跟着重算，否则测的是缓存帧。
            camera.position.x = index as f32 * 0.01;
            let started = Instant::now();
            let _ = runtime
                .vertices(WIDTH, HEIGHT)
                .expect("baseline 2D vertices must build");
            let scene3d = runtime
                .scene_3d_with_camera(viewport, Some(camera))
                .expect("baseline 3D scene must extract");
            drawables = scene3d.drawables.len();
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
        }
        let _ = fs::remove_dir_all(&root);
        scenarios.push(summarize(
            "play_scene_redraw_frame_1k",
            &samples,
            vec![
                format!("entities={entity_count} drawables={drawables} viewport={WIDTH}x{HEIGHT}"),
                "scope=CPU 提取（vertices + scene_3d），不含 GPU 提交/回读".to_owned(),
                "Play 每帧都走这条（镜像姿态逐帧变化 ⇒ Scene 视图缓存必然失效）".to_owned(),
                format!("iterations={iterations}"),
            ],
            // 提取必须覆盖全部实体：漏了说明可见性遮罩或组件映射在夹具上不成立，
            // 那样的读数没有意义，必须判红而不是报一个漂亮的毫秒数。
            drawables == entity_count,
        ));
    }

    // 内存五窗口账（docs/66 §7.3 / 优化方案 P0-1）：归属工具，不是性能门槛。
    // 唯一的硬门是"稳态反复编辑后工作集不得持续增长"——那是"泄漏"的可操作定义。
    if std::env::var_os("RUSTLUA_SKIP_MEMORY_WINDOWS").is_none() {
        scenarios.extend(build_memory_windows());
    }

    BaselineReport { scenarios }
}

/// 一个内存账窗口的读数（docs/66 §7.3 的五个窗口）。
///
/// 每个窗口记录**真实进程工作集**（Windows `GetProcessMemoryInfo`）与可观测的缓存条目数；
/// 窗口之间的差值才是"谁吃掉了内存"的归属依据。非 Windows 平台 `rss_mib=0`，报告里
/// 明确标注该字段缺失，绝不拿 0 冒充读数。
#[derive(Clone, Debug)]
struct MemoryWindow {
    /// 窗口名（`startup_peak` / `steady_state` / `scene_switch` / `load_document` / `exit`）。
    name: &'static str,
    /// 该窗口结束时的进程工作集（MiB）；0 表示平台不支持。
    rss_mib: f64,
    /// 缩略图缓存条目数（`EditorApp.thumbnails`）。
    thumbnails: usize,
    /// 日志 sink 保留行数（容量上限 500）。
    console_lines: usize,
    /// 场景文档 revision 与实体数：确认窗口里真的换了文档，不是空转。
    document_entities: usize,
}

impl MemoryWindow {
    fn note(&self) -> String {
        let rss = if self.rss_mib > 0.0 {
            format!("{:.1} MiB", self.rss_mib)
        } else {
            "n/a(非 Windows)".to_owned()
        };
        format!(
            "{}: rss={} thumbnails={} console_lines={} entities={}",
            self.name, rss, self.thumbnails, self.console_lines, self.document_entities
        )
    }
}

/// 采集一个内存窗口的读数。
fn sample_memory_window(app: &EditorApp, name: &'static str) -> MemoryWindow {
    MemoryWindow {
        name,
        rss_mib: crate::gui_evidence::working_set_mib(),
        thumbnails: app.thumbnails.len(),
        console_lines: app.log_sink.len(),
        document_entities: app
            .document
            .current_scene
            .as_ref()
            .map_or(0, |document| document.scene.entities.len()),
    }
}

/// 五窗口内存账（`docs/66 §7.3`、本方案 P0-1）。
///
/// **这是归属工具，不是门槛**：它证明"每个窗口各吃多少内存 / 缓存有没有持续增长"，
/// 为 P1 的"做或不做"提供读数；它不判红，因为绝对 RSS 与机器强相关。
/// 唯一的硬不变式是**稳态反复编辑后工作集不得单调增长**——这正是"泄漏"的可操作定义。
fn build_memory_windows() -> Vec<ScenarioReport> {
    let spec = FixtureSize::Medium.spec();
    let fixture = build_scene_fixture(spec);
    let mut windows = Vec::new();

    // 窗口 1：启动峰值 —— 建完应用（含首帧资源扫描前的状态）立即采样。
    let mut app = baseline_app(fixture.document.clone());
    windows.push(sample_memory_window(&app, "startup_peak"));

    // 窗口 2：稳定运行 —— 反复编辑 200 次（超过 Undo 上限，迫使历史真的做取舍），
    // 采样"进入稳态"与"稳态末尾"两侧；两侧之差就是持续增长量。
    let ids = fixture.entity_ids();
    for step in 0..200usize {
        let id = ids[step % ids.len()];
        apply_edit(&mut app, id, step);
    }
    let steady_enter = sample_memory_window(&app, "steady_state_enter");
    for step in 200..400usize {
        let id = ids[step % ids.len()];
        apply_edit(&mut app, id, step);
    }
    let steady_end = sample_memory_window(&app, "steady_state_end");
    windows.push(steady_enter.clone());
    windows.push(steady_end.clone());

    // 窗口 3：场景切换 —— 换一份 4 倍大的文档，再换回来；缓存必须跟着换代。
    let big = build_scene_fixture(FixtureSize::Large.spec());
    app.set_current_scene_document(big.document.clone());
    windows.push(sample_memory_window(&app, "scene_switch_large"));
    app.set_current_scene_document(fixture.document.clone());
    windows.push(sample_memory_window(&app, "scene_switch_back"));

    // 窗口 4：读档 —— 从磁盘读回同一份文档，与内存版等价（内容一致才算读成功）。
    let root = temp_root("memory-windows");
    let scene_path = root.join("perf_fixture.scene");
    // 先落盘一份（writer 走真实 save_scene 路径），再让 loader 从磁盘读回来。
    // `project_loaded` 是 save 的前置闸门（否则 status 会说"请先导入游戏工程"），
    // 这里是无窗口基线，显式置位以走真实保存逻辑而不是绕过它。
    let mut writer = baseline_app(fixture.document.clone());
    writer.project.root = root.clone();
    writer.project_loaded = true;
    writer.document.current_scene.as_mut().expect("scene").path = Some(scene_path.clone());
    let wrote = writer.save_scene();
    let mut loader = baseline_app(SceneDocument::new_scene("empty"));
    loader.project.root = root.clone();
    loader.project_loaded = true;
    let read_ok = if wrote {
        loader.open_scene_document(&scene_path).is_ok()
            && loader
                .document
                .current_scene
                .as_ref()
                .is_some_and(|document| document.scene.entities.len() == spec.entities)
    } else {
        false
    };
    windows.push(sample_memory_window(&loader, "load_document"));

    // 窗口 5：退出 —— 归还 Scene View 原生纹理与渲染资源后采样（这是 on_exit 的同一条路径）。
    app.release_scene_view_textures();
    windows.push(sample_memory_window(&app, "exit"));

    let _ = fs::remove_dir_all(&root);

    // 硬不变式：稳态不得单调增长。阈值用 +1 MiB 容差吸收 allocator 抖动，
    // 超了就是"编辑越多内存越高"，即可持续增长——必须判红。
    //
    // 非 Windows 拿不到真实 RSS（两个读数都是 0）：此时**无法证明**没有增长，
    // 记为不达标并把理由写进 notes，绝不让这道门在 Linux CI 上静默通过。
    let growth_mib = steady_end.rss_mib - steady_enter.rss_mib;
    let measured = steady_enter.rss_mib > 0.0 && steady_end.rss_mib > 0.0;
    let steady_ok = measured && growth_mib <= 1.0;
    // 读档正确性是硬门：否则"读档窗口"的读数没有意义。
    let ok = steady_ok && read_ok;

    let mut notes: Vec<String> = windows.iter().map(MemoryWindow::note).collect();
    if measured {
        notes.push(format!(
            "稳态增长 steady_end-steady_enter={growth_mib:+.2} MiB（阈值 +1.00，超了=持续增长）"
        ));
    } else {
        notes.push(
            "稳态增长未测：本平台拿不到真实 RSS（非 Windows），无法证明无增长 ⇒ 判红".to_owned(),
        );
    }
    notes.push(format!(
        "read_document_ok={read_ok} save_ok={wrote}（读档窗口的读数以读成功为前提）"
    ));

    vec![instant_scenario("memory_five_windows", 0.0, notes, ok)]
}

/// Judge one layout observation.
///
/// Extracted so the gate's own logic is unit-tested — a gate that cannot fail is not a gate.
/// `Some(reason)` means this combination violates docs/55 §8 ("no overlap, no clipped text").
fn layout_gate_violation(
    label: &str,
    rows: usize,
    content_height: f32,
    row_height: f32,
) -> Option<String> {
    if rows == 0 {
        return Some(format!("{label}: 没有绘制任何行（虚拟滚动未生效）"));
    }
    if content_height <= 0.0 {
        // 探针必须量到真实内容：否则"内容 ≤ 行高"会退化成恒真条件。
        return Some(format!("{label}: 内容高度探针为 0（测量未生效）"));
    }
    if content_height > row_height {
        return Some(format!(
            "{label}: 行内容 {content_height:.2} > 行高 {row_height:.2}（会重叠/裁字）"
        ));
    }
    None
}

/// Baseline entry point used by `--perf-baseline`; returns whether every scenario passed.
pub(crate) fn run_and_write_report(
    report_path: Option<&std::path::Path>,
    trace_path: Option<&std::path::Path>,
) -> Result<bool, String> {
    // P0-3：只在要求导出时才装收集器。装了之后每个 `baseline_app` 都会开 Fine 捕获，
    // 各段在 Drop 时汇总到这里。
    if trace_path.is_some() {
        TRACE_COLLECTOR.with(|collector| {
            *collector.borrow_mut() = Some(rustlua_gpu_profiler_trace::Collector::new(
                engine::host::profile_trace::DEFAULT_SPAN_CAPACITY,
            ));
        });
    }
    let report = run_baseline();
    let text = report.to_text();
    print!("{text}");
    if let Some(path) = report_path {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("创建报告目录失败 {}: {error}", parent.display()))?;
        }
        fs::write(path, &text)
            .map_err(|error| format!("写入基线报告失败 {}: {error}", path.display()))?;
    }
    if let Some(path) = trace_path {
        let (json, spans) = TRACE_COLLECTOR.with(|collector| {
            let collector = collector.borrow();
            let collector = collector
                .as_ref()
                .ok_or_else(|| "trace 收集器未初始化".to_owned())?;
            Ok::<_, String>((collector.to_json(), collector.span_count()))
        })?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("创建 trace 目录失败 {}: {error}", parent.display()))?;
        }
        fs::write(path, &json)
            .map_err(|error| format!("写入 trace 失败 {}: {error}", path.display()))?;
        println!(
            "\ntrace: {} 个 span -> {}（{} 字节，Chrome / Perfetto 可直接打开）",
            spans,
            path.display(),
            json.len()
        );
        TRACE_COLLECTOR.with(|collector| {
            *collector.borrow_mut() = None;
        });
    }
    Ok(report.ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_text_formats_every_section() {
        // Formatting check only: builds a synthetic report so `cargo test` stays fast.
        let report = BaselineReport {
            scenarios: vec![
                summarize("demo_cold", &[1.0, 2.0], vec!["rows=10".to_owned()], true),
                instant_scenario("demo_budget", 0.5, vec!["undo_len=200".to_owned()], true),
            ],
        };
        let text = report.to_text();
        assert!(text.contains("demo_cold"));
        assert!(text.contains("demo_budget"));
        assert!(text.contains("rows=10"));
        assert!(text.contains("scenarios=2 failed=0 result=ok"));
        assert!(report.ok());
    }

    #[test]
    fn failed_scenario_marks_report_not_ok() {
        let report = BaselineReport {
            scenarios: vec![instant_scenario("demo_fail", 0.0, vec![], false)],
        };
        assert!(!report.ok());
        assert!(report.to_text().contains("FAILED"));
    }

    /// 门槛判定必须真的能判红：三种违规都要被抓到，合规组合不得误报。
    #[test]
    fn layout_gate_detects_every_violation_kind() {
        // 合规：内容低于行高。
        assert!(layout_gate_violation("ok", 26, 18.0, 22.0).is_none());
        // 内容与行高相等：边界上仍算合规（不重叠）。
        assert!(layout_gate_violation("equal", 26, 22.0, 22.0).is_none());
        // 内容高于行高 → 会重叠/裁字。
        let overflow = layout_gate_violation("overflow", 26, 24.5, 22.0).expect("必须判红");
        assert!(overflow.contains("重叠"), "{overflow}");
        // 一行都没画 → 虚拟滚动没生效。
        let no_rows = layout_gate_violation("no_rows", 0, 18.0, 22.0).expect("必须判红");
        assert!(no_rows.contains("没有绘制"), "{no_rows}");
        // 探针为 0 → 测量本身失效（否则门槛恒真）。
        let dead_probe = layout_gate_violation("dead_probe", 26, 0.0, 22.0).expect("必须判红");
        assert!(dead_probe.contains("探针"), "{dead_probe}");
    }

    /// 失败场景必须让报告判红（与 `failed_scenario_marks_report_not_ok` 同源语义）。
    #[test]
    fn layout_gate_failure_marks_report_not_ok() {
        let report = BaselineReport {
            scenarios: vec![instant_scenario(
                "layout_row_height_gate",
                0.0,
                vec![
                    layout_gate_violation("ppi=2 zoom=1.5 width=160", 26, 30.0, 22.0)
                        .expect("must violate"),
                ],
                false,
            )],
        };
        assert!(!report.ok());
        assert!(report.to_text().contains("FAILED"));
    }

    /// The full suite is the CI/quality-gate entry point
    /// (`cargo run -p editor -- --perf-baseline`), because it takes ~90 s in a debug build
    /// and exits non-zero when a scenario's invariant fails. Run explicitly with
    /// `cargo test -p editor --bin editor -- --ignored`.
    #[test]
    #[ignore = "slow: run via `cargo run -p editor -- --perf-baseline`"]
    fn baseline_scenarios_all_pass() {
        let report = run_baseline();
        let failed: Vec<&str> = report
            .scenarios
            .iter()
            .filter(|scenario| !scenario.ok)
            .map(|scenario| scenario.name.as_str())
            .collect();
        assert!(failed.is_empty(), "failed baseline scenarios: {failed:?}");
    }
}
