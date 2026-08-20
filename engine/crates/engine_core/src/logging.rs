use std::{fmt, fs, path::Path, sync::OnceLock};

use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

static LOG_GUARD: OnceLock<LogGuard> = OnceLock::new();

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

/// Initializes console and file logging once.
///
/// `filter` follows `tracing_subscriber::EnvFilter` syntax, for example
/// `info,engine_core=debug,game_runner=debug`.
pub fn init_logging(log_path: impl AsRef<Path>, filter: Option<&str>) -> Result<(), EngineError> {
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
    let filter =
        EnvFilter::try_new(filter.unwrap_or("info,wgpu_hal=warn,wgpu_core=warn,naga=warn"))
            .map_err(|error| {
                EngineError::new(
                    LogModule::Engine,
                    "initialize logging",
                    "invalid log filter",
                )
                .with_source(error)
                .with_suggestion(
                    "use tracing filter syntax such as info or debug,game_runner=trace",
                )
            })?;
    let console = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_ansi(false);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_target(true)
        .with_ansi(false);
    tracing_subscriber::registry()
        .with(filter)
        .with(console)
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
