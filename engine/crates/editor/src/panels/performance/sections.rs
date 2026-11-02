use super::*;

/// 把一帧的 profiler 报告 + 布局缓存 + 编辑器计数排成可读的分组表。
///
/// 纯函数：不碰 `&mut self`，因此"面板该显示什么"可以在无 GPU、无窗口的测试里逐条断言。
pub(crate) fn performance_sections(
    report: &ProfilerReport,
    layout: &LayoutCacheStats,
    counts: EditorPerfCounts,
    runtime: Option<&RuntimePerfSnapshot>,
    schedule: Option<&SystemFrameSnapshot>,
) -> Vec<PerformanceSection> {
    let exceeded = report.frame_budget_exceeded();
    let over_percent = if report.frame_samples == 0 {
        0.0
    } else {
        report.over_budget_frames as f64 / report.frame_samples as f64 * 100.0
    };

    let mut sections = Vec::new();

    // ⓪ 运行时：Play 预览的实时读数（帧率 / draw / setpass / 顶点面数 / 资源 / 内存）。
    // 未 Play 时只给一个说明分组，不把上一局的过期数字留在这里。
    sections.extend(runtime_perf_sections(runtime));
    sections.extend(schedule_sections(schedule));

    // ① 编辑器帧：预算是否守住，是编辑器自身性能要回答的第一个问题。
    sections.push(PerformanceSection {
        title: "编辑器帧",
        default_open: true,
        rows: vec![
            row(
                "最近一帧",
                format!("{:.2} ms", report.frame_last_ms),
                "frame_last_ms",
                exceeded,
            ),
            row(
                "p50",
                format!("{:.2} ms", report.frame_p50_ms),
                "frame_p50_ms",
                false,
            ),
            row(
                "p95",
                format!("{:.2} ms", report.frame_p95_ms),
                "frame_p95_ms",
                exceeded,
            ),
            row(
                "p99",
                format!("{:.2} ms", report.frame_p99_ms),
                "frame_p99_ms",
                exceeded,
            ),
            row(
                "最大",
                format!("{:.2} ms", report.frame_max_ms),
                "frame_max_ms",
                false,
            ),
            row(
                "预算",
                format!("{:.1} ms（60 FPS）", report.frame_budget_ms),
                "frame_budget_ms",
                false,
            ),
            row(
                "超预算帧",
                format!(
                    "{} / {}（{:.1}%）",
                    report.over_budget_frames, report.frame_samples, over_percent
                ),
                "over_budget_frames",
                exceeded,
            ),
            row(
                "采样窗口",
                format!("{} 帧", report.frame_samples),
                "frame_samples",
                false,
            ),
            row(
                "记录中",
                if report.enabled { "是" } else { "否" },
                "enabled",
                !report.enabled,
            ),
        ],
        note: None,
    });

    // ② 布局缓存：命中率是"UI 布局重算是否被收敛"的直接证据（E25）。
    let lookups = layout.hits + layout.misses;
    let hit_rate = if lookups == 0 {
        0.0
    } else {
        layout.hits as f64 / lookups as f64 * 100.0
    };
    sections.push(PerformanceSection {
        title: "布局缓存",
        default_open: true,
        rows: vec![
            row(
                "条目",
                layout.entries.to_string(),
                "layout_cache_entries",
                false,
            ),
            row("命中", layout.hits.to_string(), "layout_cache_hits", false),
            row(
                "未命中",
                layout.misses.to_string(),
                "layout_cache_misses",
                false,
            ),
            row(
                "命中率",
                format!("{hit_rate:.1}%"),
                "layout_cache_hit_rate",
                false,
            ),
            row(
                "失效",
                layout.invalidations.to_string(),
                "layout_cache_invalidations",
                false,
            ),
        ],
        note: None,
    });

    // ③ 编辑器状态：历史深度、层级/场景/资源规模与队列（旧 HUD 那一行的等价物）。
    let pending_imports = gauge(report, "resource_pending_imports");
    sections.push(PerformanceSection {
        title: "编辑器状态",
        default_open: true,
        rows: vec![
            row(
                "撤销栈深度",
                counts.undo_len.to_string(),
                "history_undo_len",
                false,
            ),
            row(
                "丢弃的历史条数",
                counts.undo_dropped.to_string(),
                "history_dropped_entries",
                false,
            ),
            row(
                "层级可见行",
                counts.hierarchy_rows.to_string(),
                "hierarchy_visible_rows",
                false,
            ),
            row(
                "场景实体",
                format_scalar(gauge(report, "scene_entities")),
                "scene_entities",
                false,
            ),
            row(
                "资源条目",
                format_scalar(gauge(report, "resource_entries")),
                "resource_entries",
                false,
            ),
            row(
                "待导入资源",
                format_scalar(pending_imports),
                "resource_pending_imports",
                pending_imports > 0.0,
            ),
            row(
                "资源扫描应用次数",
                counts.resource_scans.to_string(),
                "resource_refresh_applied",
                false,
            ),
        ],
        note: None,
    });

    // ④ 面板耗时：按 p95 降序——"谁最贵"必须一眼看出来，而不是按字母序找。
    let mut panels = report.panels.clone();
    panels.sort_by(|left, right| right.p95_ms.total_cmp(&left.p95_ms));
    let panel_total = panels.len();
    panels.truncate(MAX_PANEL_ROWS);
    sections.push(PerformanceSection {
        title: "面板耗时",
        default_open: true,
        rows: panels
            .iter()
            .map(|panel| {
                row(
                    panel.name.clone(),
                    timing_value(panel),
                    format!("panel.{}", panel.name),
                    panel.p95_ms > report.frame_budget_ms && panel.samples > 0,
                )
            })
            .collect(),
        note: (panel_total > MAX_PANEL_ROWS)
            .then(|| format!("其余 {} 个面板见「复制报告」", panel_total - MAX_PANEL_ROWS)),
    });

    // ⑤ 命令耗时：按累计耗时降序（一次慢命令比一次慢面板更值得追）。
    let mut commands = report.commands.clone();
    commands.sort_by(|left, right| right.total_ms.total_cmp(&left.total_ms));
    let command_total = commands.len();
    commands.truncate(MAX_COMMAND_ROWS);
    sections.push(PerformanceSection {
        title: "命令耗时",
        default_open: false,
        rows: commands
            .iter()
            .map(|command| {
                row(
                    command.name.clone(),
                    format!(
                        "{} · 累计 {:.1} ms",
                        timing_value(command),
                        command.total_ms
                    ),
                    format!("command.{}", command.name),
                    false,
                )
            })
            .collect(),
        note: (command_total > MAX_COMMAND_ROWS).then(|| {
            format!(
                "其余 {} 个命令见「复制报告」",
                command_total - MAX_COMMAND_ROWS
            )
        }),
    });

    // ⑥⑦ 计数器 / gauge：profiler 的扩展面。名字可能随功能增长，未登记的原样显示。
    sections.push(PerformanceSection {
        title: "计数器（累计）",
        default_open: false,
        rows: report
            .counters
            .iter()
            .map(|(name, value)| row(counter_label(name), value.to_string(), name.clone(), false))
            .collect(),
        note: None,
    });
    sections.push(PerformanceSection {
        title: "仪表（本帧快照）",
        default_open: false,
        rows: report
            .gauges
            .iter()
            .map(|(name, value)| {
                row(
                    gauge_label(name),
                    format_scalar(*value),
                    name.clone(),
                    false,
                )
            })
            .collect(),
        note: None,
    });

    sections
}

/// 运行时（Play 预览）的四个分组；没有运行时快照时给一个明确的"未运行"分组。
///
/// 只读 `RuntimePerfSnapshot`，不碰 `EditorApp`——与编辑器分组一样保持可单测。
pub(crate) fn runtime_perf_sections(
    snapshot: Option<&RuntimePerfSnapshot>,
) -> Vec<PerformanceSection> {
    let Some(snapshot) = snapshot else {
        return vec![PerformanceSection {
            title: "运行时（播放预览）",
            default_open: true,
            rows: vec![row(
                "状态",
                "未运行（点击播放开始实时采样）",
                "runtime.status",
                false,
            )],
            note: Some(
                "开始播放后本组显示帧率 / 绘制调用 / 设置通道 / 顶点面数 / CPU/GPU / 内存 / \
                 资源加载与缓存。停止播放会清空，不保留上一局的过期读数。"
                    .to_owned(),
            ),
        }];
    };

    let texture_lookups = snapshot
        .texture_identity_hits
        .saturating_add(snapshot.texture_identity_misses);
    let texture_hit_rate = if texture_lookups == 0 {
        0.0
    } else {
        snapshot.texture_identity_hits as f64 / texture_lookups as f64 * 100.0
    };
    let mesh_lookups = snapshot
        .mesh_identity_hits
        .saturating_add(snapshot.mesh_identity_misses);
    let mesh_hit_rate = if mesh_lookups == 0 {
        0.0
    } else {
        snapshot.mesh_identity_hits as f64 / mesh_lookups as f64 * 100.0
    };
    let pipeline_lookups = snapshot
        .pipeline_variant_hits
        .saturating_add(snapshot.pipeline_variant_misses);
    let pipeline_hit_rate = if pipeline_lookups == 0 {
        0.0
    } else {
        snapshot.pipeline_variant_hits as f64 / pipeline_lookups as f64 * 100.0
    };
    let pass_list = if snapshot.passes.is_empty() {
        "（无）".to_owned()
    } else {
        snapshot.passes.join(" → ")
    };
    let short_pass_list = truncate_chars(&pass_list, 48);
    let known_gpu_bytes = snapshot
        .texture_bytes
        .saturating_add(snapshot.mesh_bytes)
        .saturating_add(snapshot.vertex_buffer_capacity);
    let process_memory = snapshot.process_memory_mib.map_or_else(
        || "不可用（仅 Windows）".to_owned(),
        |mib| format!("{mib:.1} MiB"),
    );

    vec![
        PerformanceSection {
            title: "运行时 · 帧",
            default_open: true,
            rows: vec![
                row("目标", snapshot.target, "runtime.target", false),
                row(
                    "分辨率",
                    format!("{}×{}", snapshot.resolution.0, snapshot.resolution.1),
                    "runtime.resolution",
                    false,
                ),
                row(
                    "帧率",
                    format!("{:.1} FPS", snapshot.fps),
                    "runtime.fps",
                    snapshot.fps_below_target(),
                ),
                row(
                    "帧间隔",
                    format!("{:.2} ms", snapshot.frame_ms),
                    "runtime.frame_ms",
                    snapshot.frame_ms_over_budget(),
                ),
                row(
                    "CPU 渲染",
                    format!("{:.2} ms", snapshot.render_cpu_ms),
                    "runtime.render_cpu_ms",
                    snapshot.render_cpu_ms > 16.7,
                ),
                row(
                    "GPU 渲染",
                    snapshot.gpu_timing_label(),
                    "runtime.render_gpu_ms",
                    false,
                ),
                row(
                    "运行时更新",
                    format!("{:.2} ms", snapshot.update_ms),
                    "runtime.update_ms",
                    snapshot.update_ms > 16.7,
                ),
                row(
                    "Lua 耗时",
                    format!("{:.3} ms", snapshot.lua_ms),
                    "runtime.lua_ms",
                    false,
                ),
                row(
                    "累计帧数",
                    snapshot.frames.to_string(),
                    "runtime.frames",
                    false,
                ),
            ],
            note: (snapshot.render_gpu_ms.is_none() && snapshot.gpu_timing_note.is_some())
                .then(|| snapshot.gpu_timing_label()),
        },
        PerformanceSection {
            title: "运行时 · 渲染",
            default_open: true,
            rows: vec![
                row(
                    "HDR",
                    if snapshot.hdr_active { "是" } else { "否" },
                    "runtime.hdr_active",
                    false,
                ),
                row(
                    "场景颜色格式",
                    snapshot.scene_format.as_str(),
                    "runtime.scene_format",
                    false,
                ),
                row(
                    "色调映射",
                    snapshot.tonemap_label(),
                    "runtime.tonemap",
                    false,
                ),
                row(
                    "IBL",
                    snapshot.ibl_label(),
                    "runtime.ibl",
                    false,
                ),
                row(
                    "环境",
                    snapshot.environment_label(),
                    "runtime.environment",
                    snapshot.environment_error.is_some(),
                ),
                row(
                    "绘制调用",
                    snapshot.draw_calls.to_string(),
                    "runtime.draw_calls",
                    snapshot.draw_calls > 2000,
                ),
                row(
                    "设置通道",
                    snapshot.set_pass_calls.to_string(),
                    "runtime.set_pass_calls",
                    snapshot.set_pass_calls > 200,
                ),
                row(
                    "实例数",
                    snapshot.instances.to_string(),
                    "runtime.instances",
                    false,
                ),
                row(
                    "三角形（面数）",
                    snapshot.triangles.to_string(),
                    "runtime.triangles",
                    false,
                ),
                row(
                    "顶点数",
                    snapshot.vertices.to_string(),
                    "runtime.vertices",
                    false,
                ),
                row(
                    "丢弃 Draw",
                    snapshot.dropped_draws.to_string(),
                    "runtime.dropped_draws",
                    snapshot.dropped_draws > 0,
                ),
                row(
                    "渲染通道数",
                    snapshot.passes.len().to_string(),
                    "runtime.pass_count",
                    false,
                ),
                row(
                    "渲染通道",
                    format!("{} 条：{short_pass_list}", snapshot.passes.len()),
                    "runtime.passes",
                    false,
                ),
                row(
                    "2D 顶点缓冲",
                    format_bytes(snapshot.vertex_buffer_capacity),
                    "runtime.vertex_buffer_capacity",
                    false,
                ),
                row(
                    "顶点缓冲峰值",
                    format_bytes(snapshot.vertex_bytes_peak),
                    "runtime.vertex_bytes_peak",
                    false,
                ),
                row(
                    "缓冲扩容次数",
                    snapshot.vertex_buffer_growths.to_string(),
                    "runtime.vertex_buffer_growths",
                    snapshot.vertex_buffer_growths > 8,
                ),
                row(
                    "回读槽 / 丢弃",
                    format!(
                        "{} / {}",
                        snapshot.readback.slot_count, snapshot.readback.dropped
                    ),
                    "runtime.readback_slots",
                    snapshot.readback.dropped > 0,
                ),
                row(
                    "回读待处理",
                    snapshot.readback.pending.to_string(),
                    "runtime.readback_pending",
                    false,
                ),
            ],
            note: Some(format!("完整通道顺序：{pass_list}")),
        },
        PerformanceSection {
            title: "运行时 · 资源与缓存",
            default_open: true,
            rows: vec![
                row(
                    "2D/3D 贴图",
                    snapshot.loaded_textures.to_string(),
                    "runtime.loaded_textures",
                    false,
                ),
                row(
                    "字体图集",
                    snapshot.font_atlas_textures.to_string(),
                    "runtime.font_atlas_textures",
                    false,
                ),
                row(
                    "贴图显存（估算）",
                    format_bytes(snapshot.texture_bytes),
                    "runtime.texture_bytes",
                    false,
                ),
                row(
                    "贴图身份命中",
                    snapshot.texture_identity_hits.to_string(),
                    "runtime.texture_identity_hits",
                    false,
                ),
                row(
                    "贴图身份未命中",
                    snapshot.texture_identity_misses.to_string(),
                    "runtime.texture_identity_misses",
                    false,
                ),
                row(
                    "贴图身份命中率",
                    format!("{texture_hit_rate:.1}%"),
                    "runtime.texture_identity_hit_rate",
                    false,
                ),
                row(
                    "网格缓存",
                    snapshot.cached_meshes.to_string(),
                    "runtime.cached_meshes",
                    false,
                ),
                row(
                    "网格显存（估算）",
                    format_bytes(snapshot.mesh_bytes),
                    "runtime.mesh_bytes",
                    false,
                ),
                row(
                    "网格身份命中 / 未命中",
                    format!(
                        "{} / {}（{:.1}%）",
                        snapshot.mesh_identity_hits,
                        snapshot.mesh_identity_misses,
                        mesh_hit_rate
                    ),
                    "runtime.mesh_identity_stats",
                    false,
                ),
                row(
                    "着色器变体",
                    snapshot.pipeline_variants.to_string(),
                    "runtime.pipeline_variants",
                    false,
                ),
                row(
                    "变体编译耗时",
                    format!("{:.2} ms", snapshot.pipeline_compile_ms),
                    "runtime.pipeline_compile_ms",
                    snapshot.pipeline_compile_ms
                        > rustlua_gpu::renderer::pipeline_cache::VARIANT_COMPILE_BUDGET_MS as f32,
                ),
                row(
                    "变体缓存命中 / 未命中",
                    format!(
                        "{} / {}（{:.1}%）",
                        snapshot.pipeline_variant_hits,
                        snapshot.pipeline_variant_misses,
                        pipeline_hit_rate
                    ),
                    "runtime.pipeline_cache_stats",
                    false,
                ),
                row(
                    "加载模式",
                    if snapshot.async_mode {
                        "异步（后台 worker）"
                    } else {
                        "同步（编辑器预览 / 确定性输出）"
                    },
                    "runtime.load_mode",
                    false,
                ),
                row(
                    "加载线程",
                    snapshot.async_workers.to_string(),
                    "runtime.async_workers",
                    false,
                ),
                row(
                    "待加载 / 已完成",
                    format!(
                        "{} / {}",
                        snapshot.async_pending, snapshot.async_completed
                    ),
                    "runtime.async_loads",
                    snapshot.async_pending > 0,
                ),
                row(
                    "资源身份条目",
                    snapshot.asset_entries.to_string(),
                    "runtime.asset_entries",
                    false,
                ),
                row(
                    "场景实体 / 运行时实体",
                    format!(
                        "{} / {}",
                        snapshot.scene_entities, snapshot.runtime_entities
                    ),
                    "runtime.entities",
                    false,
                ),
            ],
            note: Some(
                "身份命中率按 draw 的资源句柄解析计，不等于磁盘缓存命中率；显存是上传时记录的估算值。"
                    .to_owned(),
            ),
        },
        PerformanceSection {
            title: "运行时 · 内存",
            default_open: true,
            rows: vec![
                row(
                    "进程工作集",
                    process_memory,
                    "runtime.process_memory_mib",
                    false,
                ),
                row(
                    "Lua 堆",
                    format_bytes(snapshot.lua_heap_bytes),
                    "runtime.lua_heap_bytes",
                    false,
                ),
                row(
                    "GPU 贴图",
                    format_bytes(snapshot.texture_bytes),
                    "runtime.gpu_texture_bytes",
                    false,
                ),
                row(
                    "GPU 网格",
                    format_bytes(snapshot.mesh_bytes),
                    "runtime.gpu_mesh_bytes",
                    false,
                ),
                row(
                    "2D 顶点缓冲",
                    format_bytes(snapshot.vertex_buffer_capacity),
                    "runtime.gpu_vertex_buffer_bytes",
                    false,
                ),
                row(
                    "回读 CPU 缓冲",
                    format_bytes(snapshot.readback.cpu_bytes as u64),
                    "runtime.readback_cpu_bytes",
                    false,
                ),
                row(
                    "已知 GPU 合计",
                    format_bytes(known_gpu_bytes),
                    "runtime.known_gpu_bytes",
                    false,
                ),
            ],
            note: Some(
                "进程工作集是真实 RSS（Windows GetProcessMemoryInfo）；GPU 字节是上传统计的估算值，\
                 不含驱动对齐与不可见的管线/绑定组开销。"
                    .to_owned(),
            ),
        },
        PerformanceSection {
            title: "运行时 · GPU 通道耗时（E06-3）",
            default_open: true,
            rows: gpu_pass_rows(snapshot),
            note: Some(
                "整帧与逐通道来自同一次提交；通道和与整帧的偏差预算为 ±5%。\
                 2D overlay 与 ScreenUI 共用一次 render pass，整段记在 legacy_2d_overlay 上。\
                 设备不支持 timestamp 查询时显示「不可用」，不拿 0 µs 冒充。"
                    .to_owned(),
            ),
        },
    ]
}

/// 逐通道 GPU 时间分组（E06-3）。不可用时只给一行原因。
pub(crate) fn gpu_pass_rows(snapshot: &RuntimePerfSnapshot) -> Vec<PerformanceRow> {
    let report = &snapshot.gpu_passes;
    if !report.supported {
        return vec![row(
            "状态",
            format!("不可用（{}）", report.reason.as_deref().unwrap_or("不支持")),
            "runtime.gpu_pass_supported",
            false,
        )];
    }
    let Some(frame_us) = report.frame_us else {
        return vec![row(
            "状态",
            "等待首次 GPU 回读",
            "runtime.gpu_pass_supported",
            false,
        )];
    };
    let mut rows = vec![row(
        "整帧 / 通道和",
        format!("{frame_us} µs / {} µs", report.pass_sum_us),
        "runtime.gpu_pass_frame_us",
        false,
    )];
    if let Some(delta) = report.delta_pct {
        rows.push(row(
            "偏差（预算 ±5%）",
            format!("{delta:+.2}%"),
            "runtime.gpu_pass_delta_pct",
            report.consistent == Some(false),
        ));
    }
    for timing in &report.passes {
        rows.push(row_with_bar(
            timing.pass.as_str(),
            format!("{} µs", timing.gpu_us),
            format!("runtime.gpu_pass.{}", timing.pass.as_str()),
            timing.gpu_us as f32 / frame_us as f32,
        ));
    }
    rows
}

/// 运行时调度系统分组：直接读 `FrameScheduleReport` 的最近一帧（E06-1）。
///
/// `steps` 是最近一次 `run_script_steps` 的固定步数，`total` 是整次调度的墙钟时间，
/// 每个 system 的耗时已跨步累加。注册表里有、报告里没有的系统直接判红（缺 system 是
/// 接线故障）；系统合计**超过**总耗时 5% 也判红（计时口径不一致）。反向的差是调度器
/// 自身开销，只作信息显示。没有运行时（未 Play / 还没步进）时不出分组。
pub(crate) fn schedule_sections(snapshot: Option<&SystemFrameSnapshot>) -> Vec<PerformanceSection> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    if snapshot.steps == 0 && snapshot.systems.is_empty() {
        return Vec::new();
    }

    let total_ms = snapshot.total.as_secs_f64() * 1000.0;
    let system_sum: Duration = snapshot.systems.iter().map(|system| system.elapsed).sum();
    let sum_ms = system_sum.as_secs_f64() * 1000.0;
    let accounting_broken = if total_ms > 0.0 {
        sum_ms - total_ms > total_ms * 0.05
    } else {
        sum_ms > 0.0
    };
    let overhead_ms = (total_ms - sum_ms).max(0.0);
    let share = |elapsed: Duration| {
        if total_ms > 0.0 {
            elapsed.as_secs_f64() * 1000.0 / total_ms * 100.0
        } else {
            0.0
        }
    };

    let mut systems = snapshot.systems.clone();
    systems.sort_by(|a, b| b.elapsed.cmp(&a.elapsed).then_with(|| a.name.cmp(&b.name)));
    let listed = systems.len().min(MAX_SYSTEM_ROWS);

    let mut rows = vec![
        row(
            "固定步",
            snapshot.steps.to_string(),
            "runtime.schedule_steps",
            false,
        ),
        row(
            "调度总耗时",
            format!("{total_ms:.3} ms"),
            "runtime.schedule_total_ms",
            false,
        ),
        row(
            "系统合计",
            format!("{sum_ms:.3} ms（{:.1}%）", share(system_sum)),
            "runtime.schedule_system_sum_ms",
            accounting_broken,
        ),
        row(
            "调度开销",
            format!(
                "{overhead_ms:.3} ms（{:.1}%）",
                share(Duration::from_secs_f64(overhead_ms / 1000.0))
            ),
            "runtime.schedule_overhead_ms",
            false,
        ),
        row(
            "系统数",
            systems.len().to_string(),
            "runtime.schedule_system_count",
            systems.is_empty(),
        ),
    ];
    if !snapshot.missing.is_empty() {
        rows.push(row(
            "缺失系统",
            truncate_chars(&snapshot.missing.join(", "), 64),
            "runtime.schedule_missing",
            true,
        ));
    }
    rows.extend(systems.iter().take(listed).map(|system| {
        row(
            system.name.clone(),
            format!(
                "{:.3} ms · {:.1}%",
                system.elapsed.as_secs_f64() * 1000.0,
                share(system.elapsed)
            ),
            format!("runtime.system.{}", system.name),
            false,
        )
    }));

    let mut notes = Vec::new();
    if !snapshot.missing.is_empty() {
        notes.push(format!(
            "调度器已注册但报告未计时的系统：{}（缺 system 判红）。",
            snapshot.missing.join(", ")
        ));
    }
    if accounting_broken {
        notes.push(format!(
            "系统合计超过调度总耗时 {:.1}%（阈值 5%）：计时口径不一致，报告不可信。",
            if total_ms > 0.0 {
                (sum_ms - total_ms) / total_ms * 100.0
            } else {
                100.0
            }
        ));
    }
    if systems.is_empty() {
        notes.push("报告没有 per-system 计时：检查调度器统计是否接线。".to_owned());
    }
    if systems.len() > listed {
        notes.push(format!(
            "仅列出耗时前 {listed} 个系统，其余 {} 个见「复制报告」。",
            systems.len() - listed
        ));
    }

    vec![PerformanceSection {
        title: "运行时 · 调度系统",
        default_open: true,
        rows,
        note: (!notes.is_empty()).then(|| notes.join("\n")),
    }]
}

/// 字节 → 人读单位（面板表格用）。
pub(crate) fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes_to_mib(bytes))
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}

/// 按字符截断长文本（通道列表一行放不下；完整顺序放在分组说明里换行显示）。
pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// 计数器中文名（`profiler.bump/add` 的调用点里已登记的键）。
///
/// 未登记的名字原样显示——新增计数器不改这里也能读，悬停仍能看到原始键名。
pub(crate) fn counter_label(name: &str) -> String {
    match name {
        "hierarchy_cache_hit" => "层级缓存命中",
        "hierarchy_cache_miss" => "层级缓存未命中",
        "hierarchy_rows_drawn" => "层级实际绘制行",
        "console_index_cache_hit" => "控制台索引缓存命中",
        "console_index_cache_miss" => "控制台索引缓存未命中",
        "console_diagnostic_jump" => "控制台诊断跳转",
        "resource_refresh_started" => "资源扫描启动",
        "resource_refresh_applied" => "资源扫描应用",
        "resource_refresh_cancelled" => "资源扫描取消",
        "resource_refresh_failed" => "资源扫描失败",
        "resource_refresh_stale_dropped" => "过期扫描结果丢弃",
        "file_watch_events" => "文件监视事件",
        "file_watch_changed_files" => "变化文件数",
        "layout_saved" => "布局保存",
        "layout_auto_saved" => "布局自动保存",
        "layout_save_failed" => "布局保存失败",
        "layout_reset" => "布局重置",
        "shader_watch_hits" => "着色器文件事件命中",
        "shader_reloads" => "着色器重载成功",
        "shader_reload_failures" => "着色器重载失败",
        "scene_renderer_init_failures" => "Scene 渲染器创建失败",
        "scene_renderer_recovered" => "Scene 渲染器恢复",
        "shortcuts_saved" => "快捷键改键落盘",
        "shortcuts_save_failed" => "快捷键改键落盘失败",
        other => other,
    }
    .to_owned()
}

/// Gauge 中文名（`EditorApp::update_profiler_gauges` 写入的那批）。
pub(crate) fn gauge_label(name: &str) -> String {
    match name {
        "history_undo_len" => "撤销栈深度",
        "history_redo_len" => "重做栈深度",
        "history_dropped_entries" => "丢弃的历史条数",
        "log_sink_lines" => "日志行数",
        "console_notice_lines" => "控制台提示行数",
        "console_rows_indexed" => "控制台索引行数",
        "resource_entries" => "资源条目",
        "resource_pending_imports" => "待导入资源",
        "resource_refresh_in_flight" => "资源刷新进行中",
        "scene_entities" => "场景实体",
        "hierarchy_visible_rows" => "层级可见行",
        "hierarchy_row_content_height" => "层级行内容高度",
        "console_row_content_height" => "控制台行内容高度",
        "thumbnail_cache_entries" => "缩略图缓存条目",
        "shader_reload_validate_ms" => "着色器校验耗时(ms)",
        "shader_reload_compile_ms" => "着色器编译耗时(ms)",
        other => other,
    }
    .to_owned()
}
