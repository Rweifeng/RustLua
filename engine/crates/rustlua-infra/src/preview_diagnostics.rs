//! E24: deduplicated preview diagnostics shared by Game View and Console.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum PreviewErrorKind {
    SceneLoad,
    ScriptRuntime,
    AssetMissing,
    Render,
    Input,
    Layout,
}

impl PreviewErrorKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SceneLoad => "SceneLoad",
            Self::ScriptRuntime => "ScriptRuntime",
            Self::AssetMissing => "AssetMissing",
            Self::Render => "Render",
            Self::Input => "Input",
            Self::Layout => "Layout",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PreviewDiagnostic {
    pub kind: PreviewErrorKind,
    pub entity: Option<u64>,
    pub component: Option<String>,
    pub message: String,
}

impl PreviewDiagnostic {
    #[must_use]
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.kind.label(),
            self.entity.map_or_else(String::new, |id| id.to_string()),
            self.component.as_deref().unwrap_or_default(),
            self.message
        )
    }

    #[must_use]
    pub fn summary(&self) -> String {
        let location = match (self.entity, &self.component) {
            (Some(entity), Some(component)) => format!(" entity={entity} component={component}"),
            (Some(entity), None) => format!(" entity={entity}"),
            (None, Some(component)) => format!(" component={component}"),
            (None, None) => String::new(),
        };
        format!("[{}]{} {}", self.kind.label(), location, self.message)
    }
}

impl fmt::Display for PreviewDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.summary())
    }
}

/// Stores one entry per unique diagnostic and counts later repeats without
/// flooding the UI. Fixing the source is expressed by replacing the set.
#[derive(Debug, Default)]
pub struct DiagnosticCollector {
    entries: Vec<(PreviewDiagnostic, u32)>,
}

impl DiagnosticCollector {
    pub fn push(&mut self, diagnostic: PreviewDiagnostic) {
        if let Some((_, count)) = self
            .entries
            .iter_mut()
            .find(|(entry, _)| entry.key() == diagnostic.key())
        {
            *count = count.saturating_add(1);
            return;
        }
        self.entries.push((diagnostic, 1));
    }

    #[must_use]
    pub fn diagnostics(&self) -> Vec<&PreviewDiagnostic> {
        self.entries.iter().map(|(entry, _)| entry).collect()
    }

    #[must_use]
    pub fn occurrences(&self, diagnostic: &PreviewDiagnostic) -> u32 {
        self.entries
            .iter()
            .find(|(entry, _)| entry.key() == diagnostic.key())
            .map_or(0, |(_, count)| *count)
    }

    pub fn replace_all(&mut self, diagnostics: impl IntoIterator<Item = PreviewDiagnostic>) {
        self.entries.clear();
        for diagnostic in diagnostics {
            self.push(diagnostic);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic(message: &str) -> PreviewDiagnostic {
        PreviewDiagnostic {
            kind: PreviewErrorKind::AssetMissing,
            entity: Some(7),
            component: Some("Image".to_owned()),
            message: message.to_owned(),
        }
    }

    #[test]
    fn deduplicates_and_counts_repeats() {
        let mut collector = DiagnosticCollector::default();
        collector.push(diagnostic("missing.png"));
        collector.push(diagnostic("missing.png"));
        assert_eq!(collector.diagnostics().len(), 1);
        assert_eq!(collector.occurrences(&diagnostic("missing.png")), 2);
        assert!(collector.diagnostics()[0].summary().contains("entity=7"));
    }

    #[test]
    fn replace_all_clears_repaired_diagnostics() {
        let mut collector = DiagnosticCollector::default();
        collector.push(diagnostic("missing.png"));
        collector.replace_all([]);
        assert!(collector.is_empty());
    }
}
