use std::{
    fmt, fs, io,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
};

use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

static LOG_GUARD: OnceLock<LogGuard> = OnceLock::new();

/// 默认 tracing 过滤器。
/// `wgpu_hal::vulkan::instance=off`：wgpu Vulkan 后端把 loader 的空内容通用消息
/// （`GENERAL [Loader Message (0x0)]`）按 error 级记录，属良性启动噪声，整体关闭该目标。
pub(crate) const DEFAULT_LOG_FILTER: &str =
    "info,wgpu_hal=warn,wgpu_core=warn,naga=warn,wgpu_hal::vulkan::instance=off";

/// Log modules exposed by the engine runtime.
#[derive(Debug, Clone, Copy)]
pub enum LogModule {
    Engine,
    Editor,
    Game,
    Script,
    Asset,
}

impl fmt::Display for LogModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Engine => "engine",
            Self::Editor => "editor",
            Self::Game => "game",
            Self::Script => "script",
            Self::Asset => "asset",
        })
    }
}

/// Context-rich error for user-facing diagnostics.
#[derive(Debug, Clone)]
pub struct EngineError {
    pub module: LogModule,
    pub operation: String,
    pub context: String,
    pub suggestion: Option<String>,
    pub source: Option<String>,
}

impl EngineError {
    pub fn new(
        module: LogModule,
        operation: impl Into<String>,
        context: impl Into<String>,
    ) -> Self {
        Self {
            module,
            operation: operation.into(),
            context: context.into(),
            suggestion: None,
            source: None,
        }
    }

    #[must_use]
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    #[must_use]
    pub fn with_source(mut self, source: impl fmt::Display) -> Self {
        self.source = Some(source.to_string());
        self
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.module, self.operation, self.context)?;
        if let Some(source) = &self.source {
            write!(f, "; cause: {source}")?;
        }
        if let Some(suggestion) = &self.suggestion {
            write!(f, "; suggestion: {suggestion}")?;
        }
        Ok(())
    }
}

impl std::error::Error for EngineError {}

struct LogGuard {
    _file_guard: tracing_appender::non_blocking::WorkerGuard,
}

/// 内存日志缓冲：作为 tracing fmt 层的 writer，供编辑器控制台实时读取。
#[derive(Clone)]
pub struct LogSink {
    buffer: Arc<Mutex<Vec<String>>>,
    capacity: usize,
}

impl LogSink {
    /// 新建缓冲，`capacity` 为保留的最大行数（超出丢弃最旧）。
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            capacity: capacity.max(1),
        }
    }

    /// 当前全部日志行（按时间先后）。
    pub fn snapshot(&self) -> Vec<String> {
        self.buffer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 清空缓冲。
    pub fn clear(&self) {
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.clear();
        }
    }

    fn push_line(&self, line: String) {
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.push(line);
            if buffer.len() > self.capacity {
                let overflow = buffer.len() - self.capacity;
                buffer.drain(..overflow);
            }
        }
    }
}

/// LogSink 的 `std::io::Write` 包装：fmt 层每次事件写一段（含换行），按行入队。
pub struct SinkWriter(LogSink);

impl io::Write for SinkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        for line in text.split_inclusive('\n') {
            let trimmed = line.trim_end_matches(['\n', '\r']);
            if !trimmed.is_empty() {
                self.0.push_line(trimmed.to_owned());
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = SinkWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SinkWriter(self.clone())
    }
}

/// Initializes console and file logging once.
///
/// `filter` follows `tracing_subscriber::EnvFilter` syntax, for example
/// `info,engine::core=debug,game=debug`.
pub fn init_logging(log_path: impl AsRef<Path>, filter: Option<&str>) -> Result<(), EngineError> {
    init_logging_with_sink(log_path, filter, None)
}

/// 同 `init_logging`，额外把 tracing 日志实时回传到一个共享缓冲（如编辑器控制台）。
/// `sink` 为 `None` 时行为与 `init_logging` 完全一致。
pub fn init_logging_with_sink(
    log_path: impl AsRef<Path>,
    filter: Option<&str>,
    sink: Option<LogSink>,
) -> Result<(), EngineError> {
    if LOG_GUARD.get().is_some() {
        return Ok(());
    }
    let log_path = log_path.as_ref();
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            EngineError::new(
                LogModule::Engine,
                "initialize logging",
                format!("cannot create log directory {}", parent.display()),
            )
            .with_source(error)
            .with_suggestion("check that the project log directory is writable")
        })?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .map_err(|error| {
            EngineError::new(
                LogModule::Engine,
                "initialize logging",
                format!("cannot open log file {}", log_path.display()),
            )
            .with_source(error)
            .with_suggestion("check the log path and file permissions")
        })?;
    let (non_blocking, guard) = tracing_appender::non_blocking(file);
    let filter = EnvFilter::try_new(filter.unwrap_or(DEFAULT_LOG_FILTER))
            .map_err(|error| {
                EngineError::new(
                    LogModule::Engine,
                    "initialize logging",
                    "invalid log filter",
                )
                .with_source(error)
                .with_suggestion("use tracing filter syntax such as info or debug,game=trace")
            })?;
    let console = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_ansi(false);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_target(true)
        .with_ansi(false);
    // 可选：把日志实时回传到共享缓冲（Option<Layer> 作为 Layer 叠加）。
    let sink_layer = sink.map(|sink| {
        tracing_subscriber::fmt::layer()
            .with_writer(sink)
            .with_target(true)
            .with_ansi(false)
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .with(sink_layer)
        .with(file_layer)
        .try_init()
        .map_err(|error| {
            EngineError::new(
                LogModule::Engine,
                "initialize logging",
                "a global subscriber is already installed",
            )
            .with_source(error)
            .with_suggestion("initialize logging only once during application startup")
        })?;
    let _ = LOG_GUARD.set(LogGuard { _file_guard: guard });
    Ok(())
}

/// Registers a panic hook that writes a crash report to `crash_log_path`
/// (version, timestamp, thread, panic message, location, backtrace) before the
/// default hook runs. Safe to call once per process.
pub fn install_crash_handler(crash_log_path: impl AsRef<Path>) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let crash_log_path = crash_log_path.as_ref().to_path_buf();
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::capture();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let message = if let Some(payload) = info.payload().downcast_ref::<&str>() {
            (*payload).to_owned()
        } else if let Some(payload) = info.payload().downcast_ref::<String>() {
            payload.clone()
        } else {
            "non-string panic payload".to_owned()
        };
        let location = info
            .location()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "unknown".to_owned());
        let thread = std::thread::current().name().unwrap_or("main").to_owned();
        let report = format!(
            "=== P2 crash report ===\n\
             version={}\n\
             timestamp={}\n\
             thread={}\n\
             panic={}\n\
             location={}\n\
             backtrace:\n{backtrace}\n",
            crate::core::version(),
            timestamp,
            thread,
            message,
            location,
        );
        // 原子替换写入：避免并发 panic（如测试中其他线程的故意 panic）
        // 用 fs::write 截断+写入交叠导致报告损坏。
        if let Some(parent) = crash_log_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let temporary = crash_log_path.with_extension("tmp");
        let _ = fs::write(&temporary, &report);
        let _ = fs::remove_file(&crash_log_path);
        let _ = fs::rename(&temporary, &crash_log_path);
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_handler_writes_report_with_payload_and_version() {
        let path = std::env::temp_dir().join(format!("p2-crash-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let original = std::panic::take_hook();
        install_crash_handler(&path);
        let result = std::panic::catch_unwind(|| panic!("boom-marker"));
        assert!(result.is_err(), "panic was caught");
        // 并行测试（scheduler 的故意 panic）可能先写同一路径；轮询直到看到本测试标记。
        let mut text = String::new();
        for _ in 0..40 {
            text = std::fs::read_to_string(&path).unwrap_or_default();
            if text.contains("boom-marker") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(text.contains("P2 crash report"));
        assert!(text.contains("panic=boom-marker"));
        assert!(text.contains("version="));
        assert!(text.contains("backtrace:"));
        std::panic::set_hook(original);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn formats_context_and_suggestion() {
        let error = EngineError::new(LogModule::Script, "load script", "game/scripts/main.lua")
            .with_source("syntax error near '<eof>'")
            .with_suggestion("check the script syntax and line reported by Lua");
        let text = error.to_string();
        assert!(text.contains("[script] load script"));
        assert!(text.contains("game/scripts/main.lua"));
        assert!(text.contains("suggestion:"));
    }

    #[test]
    fn module_names_are_stable() {
        assert_eq!(LogModule::Engine.to_string(), "engine");
        assert_eq!(LogModule::Editor.to_string(), "editor");
        assert_eq!(LogModule::Game.to_string(), "game");
        assert_eq!(LogModule::Script.to_string(), "script");
        assert_eq!(LogModule::Asset.to_string(), "asset");
    }

    #[test]
    fn log_sink_captures_lines_trims_newlines_and_caps_capacity() {
        use std::io::Write;
        let sink = LogSink::new(3);
        let mut writer = SinkWriter(sink.clone());
        writer.write_all(b"first line\n").unwrap();
        writer.write_all(b"second line\n").unwrap();
        writer.write_all(b"third\nfourth\n").unwrap();
        writer.flush().unwrap();

        // 容量 3 丢弃最旧的 first line；行尾换行被去除。
        let lines = sink.snapshot();
        assert_eq!(lines, vec!["second line", "third", "fourth"]);
        sink.clear();
        assert!(sink.snapshot().is_empty());
    }

    /// 默认过滤器关闭 `wgpu_hal::vulkan::instance` 目标（空内容 loader 噪声），
    /// 其它 wgpu_hal 目标仍按 `wgpu_hal=warn` 过滤。
    #[test]
    fn default_filter_suppresses_vulkan_loader_noise() {
        use tracing_subscriber::layer::SubscriberExt;

        let sink = LogSink::new(64);
        let layer = tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_ansi(false)
            .with_writer(sink.clone());
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new(DEFAULT_LOG_FILTER))
            .with(layer);

        tracing::subscriber::with_default(subscriber, || {
            // 空 loader 噪声：该 target 已 off，不应出现。
            tracing::error!(target: "wgpu_hal::vulkan::instance", "GENERAL [Loader Message (0x0)]");
            // 其它 wgpu_hal 目标：error 保留，info（warn 以下）过滤。
            tracing::error!(target: "wgpu_hal::vulkan::device", "real device error");
            tracing::info!(target: "wgpu_hal::vulkan::device", "device info noise");
        });

        let lines = sink.snapshot();
        assert!(lines.iter().all(|line| !line.contains("Loader Message")),
            "loader 噪声应被过滤：{lines:?}");
        assert!(lines.iter().any(|line| line.contains("real device error")),
            "同 crate 其它目标的 error 应保留：{lines:?}");
        assert!(lines.iter().all(|line| !line.contains("device info noise")),
            "wgpu_hal=warn 应过滤 info：{lines:?}");
    }
}
