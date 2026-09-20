//! Generation-aware background resource refresh.
//!
//! The worker owns its `ResourceBrowserState`; the UI only receives a complete result and
//! applies it at an explicit commit point. A newer request supersedes older generations.

use crate::{CancellationToken, ResourceBrowserState};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshRequest {
    pub root: PathBuf,
    pub generation: u64,
}

#[derive(Debug)]
pub struct RefreshResult {
    pub generation: u64,
    pub state: ResourceBrowserState,
    pub error: Option<String>,
}

pub struct RefreshHandle {
    generation: u64,
    cancel: CancellationToken,
    receiver: Receiver<RefreshResult>,
}

/// Outcome of polling a refresh handle at the UI commit point.
#[derive(Debug)]
pub enum RefreshPoll {
    /// The worker has not produced a result yet.
    Pending,
    /// A result arrived for a superseded generation and was discarded.
    Stale,
    /// A result for the current generation, ready to be applied.
    Ready(RefreshResult),
}

impl RefreshHandle {
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
    pub fn try_take(&self) -> Option<RefreshResult> {
        self.receiver.try_recv().ok()
    }
    pub fn take_if_current(&self, current_generation: u64) -> Option<RefreshResult> {
        let result = self.try_take()?;
        (result.generation == current_generation).then_some(result)
    }

    /// Poll once, distinguishing "nothing yet" from "a stale result was dropped" so the shell
    /// can report preemption in its telemetry.
    pub fn poll(&self, current_generation: u64) -> RefreshPoll {
        match self.receiver.try_recv() {
            Err(_) => RefreshPoll::Pending,
            Ok(result) if result.generation != current_generation => RefreshPoll::Stale,
            Ok(result) => RefreshPoll::Ready(result),
        }
    }
}

pub fn spawn_refresh(request: RefreshRequest) -> RefreshHandle {
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let (sender, receiver) = mpsc::channel();
    let generation = request.generation;
    thread::spawn(move || {
        let mut state = ResourceBrowserState::new();
        let error = if worker_cancel.is_cancelled() {
            Some("资源刷新已取消".to_owned())
        } else {
            match state.refresh(&request.root) {
                Ok(()) => None,
                Err(error) => Some(error.to_string()),
            }
        };
        if !worker_cancel.is_cancelled() {
            let _ = sender.send(RefreshResult {
                generation,
                state,
                error,
            });
        }
    });
    RefreshHandle {
        generation,
        cancel,
        receiver,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };
    fn temp_project() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-refresh-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::write(root.join("assets/a.lua"), "return true").unwrap();
        root
    }
    #[test]
    fn refresh_result_keeps_generation_and_entries() {
        let root = temp_project();
        let handle = spawn_refresh(RefreshRequest {
            root: root.clone(),
            generation: 7,
        });
        let result = loop {
            if let Some(result) = handle.try_take() {
                break result;
            }
            std::thread::yield_now();
        };
        assert_eq!(result.generation, 7);
        assert_eq!(result.error, None);
        assert_eq!(result.state.entries().len(), 1);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn stale_generation_is_not_accepted() {
        let root = temp_project();
        let handle = spawn_refresh(RefreshRequest {
            root: root.clone(),
            generation: 2,
        });
        loop {
            match handle.poll(3) {
                RefreshPoll::Pending => std::thread::yield_now(),
                RefreshPoll::Stale => break,
                RefreshPoll::Ready(_) => panic!("stale result must not be accepted"),
            }
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn poll_reports_ready_for_current_generation() {
        let root = temp_project();
        let handle = spawn_refresh(RefreshRequest {
            root: root.clone(),
            generation: 5,
        });
        loop {
            match handle.poll(5) {
                RefreshPoll::Pending => std::thread::yield_now(),
                RefreshPoll::Stale => panic!("unexpected stale result"),
                RefreshPoll::Ready(result) => {
                    assert_eq!(result.generation, 5);
                    assert_eq!(result.state.entries().len(), 1);
                    break;
                }
            }
        }
        let _ = fs::remove_dir_all(root);
    }
}
