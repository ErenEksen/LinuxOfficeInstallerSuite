pub mod config;
pub mod download;
pub mod ledger;
pub mod paths;
pub mod proc;
pub mod selfinstall;
pub mod time;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared cancellation flag, checked between download chunks, process output lines and steps.
#[derive(Clone, Default, Debug)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Progress reported by long operations to the UI.
#[derive(Clone, Debug, PartialEq)]
pub enum Report {
    /// Fraction of the current stage (0.0–1.0), or `None` for indeterminate.
    Progress(Option<f32>),
    /// A line for the live log panel.
    Line(String),
    /// Short live status of the current sub-step, e.g. download size and speed.
    Detail(String),
}
