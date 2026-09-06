//! E27: a small, versioned editor extension registry.

use std::{collections::BTreeMap, fmt, panic::AssertUnwindSafe};

pub const PLUGIN_API_VERSION: u32 = 1;

pub trait EditorPlugin {
    fn name(&self) -> &str;
    fn version(&self) -> u32 {
        1
    }
    fn on_load(&mut self) -> Result<(), PluginError> {
        Ok(())
    }
    fn on_unload(&mut self) -> Result<(), PluginError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginError {
    Duplicate(String),
    Missing(String),
    IncompatibleVersion { plugin: String, found: u32 },
    Load(String),
    Unload(String),
    Panicked(String),
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate(name) => write!(f, "plugin {name:?} is already registered"),
            Self::Missing(name) => write!(f, "plugin {name:?} is not registered"),
            Self::IncompatibleVersion { plugin, found } => write!(
                f,
                "plugin {plugin:?} uses API v{found}; editor requires v{PLUGIN_API_VERSION}"
            ),
            Self::Load(name) => write!(f, "plugin {name:?} failed to load"),
            Self::Unload(name) => write!(f, "plugin {name:?} failed to unload"),
            Self::Panicked(name) => write!(f, "plugin {name:?} panicked"),
        }
    }
}

impl std::error::Error for PluginError {}

struct RegisteredPlugin {
    plugin: Box<dyn EditorPlugin>,
    loaded: bool,
}

/// Keeps extension failures out of the editor core. A failed load remains
/// registered but marked unavailable, so unrelated plugins and scenes work.
#[derive(Default)]
pub struct PluginRegistry {
    plugins: BTreeMap<String, RegisteredPlugin>,
}

impl PluginRegistry {
    pub fn register(&mut self, plugin: Box<dyn EditorPlugin>) -> Result<(), PluginError> {
        let name = plugin.name().to_owned();
        if self.plugins.contains_key(&name) {
            return Err(PluginError::Duplicate(name));
        }
        if plugin.version() != PLUGIN_API_VERSION {
            return Err(PluginError::IncompatibleVersion {
                plugin: name,
                found: plugin.version(),
            });
        }
        self.plugins.insert(
            name,
            RegisteredPlugin {
                plugin,
                loaded: false,
            },
        );
        Ok(())
    }

    pub fn load(&mut self, name: &str) -> Result<(), PluginError> {
        let Some(entry) = self.plugins.get_mut(name) else {
            return Err(PluginError::Missing(name.to_owned()));
        };
        if entry.loaded {
            return Ok(());
        }
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| entry.plugin.on_load()));
        match result {
            Ok(Ok(())) => {
                entry.loaded = true;
                Ok(())
            }
            Ok(Err(error)) => Err(PluginError::Load(error.to_string())),
            Err(_) => Err(PluginError::Panicked(name.to_owned())),
        }
    }

    pub fn unload(&mut self, name: &str) -> Result<(), PluginError> {
        let Some(entry) = self.plugins.get_mut(name) else {
            return Err(PluginError::Missing(name.to_owned()));
        };
        if !entry.loaded {
            return Ok(());
        }
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| entry.plugin.on_unload()));
        match result {
            Ok(Ok(())) => {
                entry.loaded = false;
                Ok(())
            }
            Ok(Err(error)) => Err(PluginError::Unload(error.to_string())),
            Err(_) => Err(PluginError::Panicked(name.to_owned())),
        }
    }

    #[must_use]
    pub fn is_loaded(&self, name: &str) -> bool {
        self.plugins.get(name).is_some_and(|entry| entry.loaded)
    }

    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.plugins.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OkPlugin;
    impl EditorPlugin for OkPlugin {
        fn name(&self) -> &str {
            "ok"
        }
    }

    struct FailPlugin;
    impl EditorPlugin for FailPlugin {
        fn name(&self) -> &str {
            "fail"
        }
        fn on_load(&mut self) -> Result<(), PluginError> {
            Err(PluginError::Load("intentional".to_owned()))
        }
    }

    struct PanicPlugin;
    impl EditorPlugin for PanicPlugin {
        fn name(&self) -> &str {
            "panic"
        }
        fn on_load(&mut self) -> Result<(), PluginError> {
            panic!("plugin bug");
        }
    }

    #[test]
    fn validates_versions_and_duplicates() {
        let mut registry = PluginRegistry::default();
        registry.register(Box::new(OkPlugin)).unwrap();
        assert!(matches!(
            registry.register(Box::new(OkPlugin)),
            Err(PluginError::Duplicate(_))
        ));
    }

    #[test]
    fn isolates_failed_and_panicking_plugins() {
        let mut registry = PluginRegistry::default();
        registry.register(Box::new(FailPlugin)).unwrap();
        registry.register(Box::new(PanicPlugin)).unwrap();
        registry.register(Box::new(OkPlugin)).unwrap();
        assert!(registry.load("fail").is_err());
        assert!(registry.load("panic").is_err());
        assert!(registry.load("ok").is_ok());
        assert_eq!(registry.names(), ["fail", "ok", "panic"]);
        assert!(!registry.is_loaded("fail"));
        assert!(!registry.is_loaded("panic"));
        assert!(registry.is_loaded("ok"));
    }
}
