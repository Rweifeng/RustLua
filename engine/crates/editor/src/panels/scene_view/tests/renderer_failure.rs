//! Scene View 离屏渲染器创建失败时的行为：退避、只播一次、成功后清浮层。
//!
//! 真实失败没法在无头环境里造（`Renderer::new_offscreen` 只有在显存不足/管线被驱动拒绝时
//! 才返回 Err），所以这里测的是**失败状态机本身**——生产路径 `ensure_scene_renderer_instance`
//! 只做"把 Err 交给 note、把 Ok 交给 recover、在两次尝试之间看退避是否到期"这一层胶水。

use super::*;
use crate::panels::scene_view::render::{RENDERER_FAILURE_PREFIX, SCENE_RENDERER_RETRY};
use std::time::{Duration, Instant};

fn bare_app() -> EditorApp {
    let root = std::env::temp_dir().join(format!("rustlua-renderer-failure-{}", unique_token()));
    let project = editor::ProjectInfo {
        root,
        version: "test".to_owned(),
        directories: Vec::new(),
    };
    EditorApp::new(project, None, LogSink::new(64))
}

/// 失败后 2 秒内不再重试；到期后才放行——持久失败时每帧重编管线就是"空白还卡"。
#[test]
fn renderer_init_failure_backs_off_before_retrying() {
    let mut app = bare_app();
    let now = Instant::now();
    assert!(
        app.scene_renderer_retry_due(now),
        "没失败过就该立刻允许尝试"
    );

    app.scene_renderer_retry_at = Some(now + SCENE_RENDERER_RETRY);
    assert!(!app.scene_renderer_retry_due(now), "刚失败不许马上再试");
    assert!(
        !app.scene_renderer_retry_due(
            now + Duration::from_millis(SCENE_RENDERER_RETRY.as_millis() as u64 - 1)
        ),
        "退避没到点仍然不许"
    );
    assert!(
        app.scene_renderer_retry_due(now + SCENE_RENDERER_RETRY),
        "到点必须放行（驱动恢复/显存腾出来后要能自愈）"
    );
}

/// 同一条原因只播一次；原因变了要重播（换了项目/换了坏 shader 都得让人看见）。
#[test]
fn renderer_init_failure_reports_only_when_the_reason_changes() {
    let mut app = bare_app();
    assert!(
        app.note_scene_renderer_failure("out of memory"),
        "第一次失败要播报"
    );
    assert_eq!(app.scene_renderer_error.as_deref(), Some("out of memory"));
    assert!(
        !app.note_scene_renderer_failure("out of memory"),
        "同一原因重复失败不能再刷一行 Console"
    );
    assert!(
        app.note_scene_renderer_failure("pipeline rejected"),
        "原因变了必须重播"
    );
    assert_eq!(
        app.scene_renderer_error.as_deref(),
        Some("pipeline rejected")
    );
}

/// 恢复：清状态、撤掉自己那句浮层，但**不碰别人的诊断文本**。
#[test]
fn renderer_recovery_clears_only_its_own_overlay() {
    let mut app = bare_app();
    let now = Instant::now();
    app.note_scene_renderer_failure("out of memory");
    app.preview_diagnostic = Some(format!("{RENDERER_FAILURE_PREFIX}out of memory"));
    app.scene_renderer_retry_at = Some(now + SCENE_RENDERER_RETRY);

    assert!(app.recover_scene_renderer(), "有失败在身，恢复要报出去");
    assert!(app.scene_renderer_error.is_none(), "失败原因要清掉");
    assert!(
        app.scene_renderer_retry_at.is_none(),
        "退避要一起清掉，否则下一次重建视图还得等 2 秒"
    );
    assert!(
        app.preview_diagnostic.is_none(),
        "自己那句浮层必须撤掉，不然恢复了还挂着'渲染器初始化失败'"
    );
    assert!(
        !app.recover_scene_renderer(),
        "没有失败就没有'恢复'，不能凭空播一句"
    );

    // 浮层是别人的诊断（拖拽/框选/着色器重载）时，恢复不许顺手清空。
    app.note_scene_renderer_failure("out of memory");
    app.preview_diagnostic = Some("拖拽预览中".to_owned());
    assert!(app.recover_scene_renderer());
    assert_eq!(
        app.preview_diagnostic.as_deref(),
        Some("拖拽预览中"),
        "只认自己前缀的那句"
    );
}

/// 项目切换把失败状态一起清掉：留着退避时刻，新项目要白等 2 秒才重试。
#[test]
fn project_switch_resets_renderer_failure_state() {
    let mut app = bare_app();
    app.note_scene_renderer_failure("out of memory");
    app.scene_renderer_retry_at = Some(Instant::now() + SCENE_RENDERER_RETRY);
    app.preview_diagnostic = Some(format!("{RENDERER_FAILURE_PREFIX}out of memory"));

    app.reset_scene_render_resources();

    assert!(app.scene_renderer_error.is_none());
    assert!(app.scene_renderer_retry_at.is_none());
    assert!(
        app.scene_renderer_retry_due(Instant::now()),
        "切完项目要能立刻重试"
    );
}
