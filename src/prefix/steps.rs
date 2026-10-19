//! Ordered, resumable steps (design D3, wine-prefix-setup: Resumable and idempotent setup).
//!
//! Finished step ids are stored in `<prefix>/.lois-state.toml` after each step, so an
//! interrupted run continues at the first unfinished step, and a complete run is a no-op.
//! The file lives inside the prefix: deleting the prefix resets it.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::config::write_atomic;

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error("{0}")]
    Failed(String),
    #[error("cancelled")]
    Cancelled,
}

impl StepError {
    pub fn msg(s: impl std::fmt::Display) -> Self {
        StepError::Failed(s.to_string())
    }
}

impl From<io::Error> for StepError {
    fn from(e: io::Error) -> Self {
        StepError::Failed(e.to_string())
    }
}

impl From<crate::core::proc::ProcError> for StepError {
    fn from(e: crate::core::proc::ProcError) -> Self {
        match e {
            crate::core::proc::ProcError::Cancelled => StepError::Cancelled,
            other => StepError::Failed(other.to_string()),
        }
    }
}

impl From<crate::core::download::DownloadError> for StepError {
    fn from(e: crate::core::download::DownloadError) -> Self {
        match e {
            crate::core::download::DownloadError::Cancelled => StepError::Cancelled,
            other => StepError::Failed(other.to_string()),
        }
    }
}

type RunFn<C> = Box<dyn Fn(&mut C) -> Result<(), StepError> + Send + Sync>;

pub struct Step<C> {
    pub id: &'static str,
    pub title: String,
    pub run: RunFn<C>,
}

impl<C> Step<C> {
    pub fn new(
        id: &'static str,
        title: impl Into<String>,
        run: impl Fn(&mut C) -> Result<(), StepError> + Send + Sync + 'static,
    ) -> Self {
        Self { id, title: title.into(), run: Box::new(run) }
    }
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct State {
    #[serde(default)]
    pub done: Vec<String>,
}

impl State {
    pub fn load(path: &Path) -> State {
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_atomic(path, toml::to_string(self).map_err(io::Error::other)?.as_bytes())
    }

    pub fn is_done(&self, id: &str) -> bool {
        self.done.iter().any(|d| d == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepEvent {
    Skipped(usize),
    Started(usize),
    Done(usize),
    Failed(usize, String),
}

pub struct Runner {
    pub state_path: PathBuf,
}

impl Runner {
    pub fn new(state_path: impl Into<PathBuf>) -> Self {
        Self { state_path: state_path.into() }
    }

    /// Run `steps` in order, skipping finished ones. Stops at the first failure.
    pub fn run<C>(
        &self,
        steps: &[Step<C>],
        ctx: &mut C,
        on_event: &mut dyn FnMut(StepEvent),
    ) -> Result<(), StepError> {
        let mut state = State::load(&self.state_path);
        for (i, step) in steps.iter().enumerate() {
            if state.is_done(step.id) {
                on_event(StepEvent::Skipped(i));
                continue;
            }
            on_event(StepEvent::Started(i));
            match (step.run)(ctx) {
                Ok(()) => {
                    // Reload: a step may have recreated the prefix (and with it this file).
                    state = State::load(&self.state_path);
                    if !state.is_done(step.id) {
                        state.done.push(step.id.to_string());
                    }
                    state.save(&self.state_path)?;
                    on_event(StepEvent::Done(i));
                }
                Err(e) => {
                    on_event(StepEvent::Failed(i, e.to_string()));
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    pub fn is_complete<C>(&self, steps: &[Step<C>]) -> bool {
        let state = State::load(&self.state_path);
        steps.iter().all(|s| state.is_done(s.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Ctx {
        ran: Vec<&'static str>,
        fail_at: Option<&'static str>,
    }

    fn steps() -> Vec<Step<Ctx>> {
        ["win7", "fonts", "redists", "win10"]
            .into_iter()
            .map(|id| {
                Step::new(id, id, move |c: &mut Ctx| {
                    if c.fail_at == Some(id) {
                        return Err(StepError::msg(format!("{id} broke")));
                    }
                    c.ran.push(id);
                    Ok(())
                })
            })
            .collect()
    }

    #[test]
    fn interrupted_run_resumes_at_the_right_step() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Runner::new(tmp.path().join("prefix/.lois-state.toml"));
        let steps = steps();

        let mut ctx = Ctx { fail_at: Some("redists"), ..Default::default() };
        let mut events = Vec::new();
        let err = runner.run(&steps, &mut ctx, &mut |e| events.push(e)).unwrap_err();
        assert_eq!(err.to_string(), "redists broke");
        assert_eq!(ctx.ran, ["win7", "fonts"]);
        assert_eq!(events.last(), Some(&StepEvent::Failed(2, "redists broke".into())));
        assert!(!runner.is_complete(&steps));

        let mut ctx = Ctx::default();
        let mut events = Vec::new();
        runner.run(&steps, &mut ctx, &mut |e| events.push(e)).unwrap();
        assert_eq!(ctx.ran, ["redists", "win10"], "skips the finished steps");
        assert_eq!(&events[..2], &[StepEvent::Skipped(0), StepEvent::Skipped(1)]);
        assert!(runner.is_complete(&steps));

        let mut ctx = Ctx::default();
        runner.run(&steps, &mut ctx, &mut |_| {}).unwrap();
        assert!(ctx.ran.is_empty(), "a complete run changes nothing");
    }

    #[test]
    fn deleting_the_state_resets() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".lois-state.toml");
        let runner = Runner::new(&path);
        let steps = steps();
        runner.run(&steps, &mut Ctx::default(), &mut |_| {}).unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut ctx = Ctx::default();
        runner.run(&steps, &mut ctx, &mut |_| {}).unwrap();
        assert_eq!(ctx.ran.len(), 4);
    }
}
