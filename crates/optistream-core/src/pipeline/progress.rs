//! Progress shared between a running pass and the UI.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// Shared progress state, polled by the UI.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    pub cancel: AtomicBool,
    pub current: Mutex<String>,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            0.0
        } else {
            self.done.load(Ordering::Relaxed) as f32 / total as f32
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub(super) fn step(&self, label: &Path) {
        self.done.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut c) = self.current.lock() {
            *c = label.display().to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_and_step() {
        let p = Progress::default();
        assert_eq!(p.fraction(), 0.0);
        p.total.store(4, Ordering::Relaxed);
        p.step(Path::new("a.ydd"));
        assert_eq!(p.fraction(), 0.25);
        assert_eq!(*p.current.lock().unwrap(), "a.ydd");
        assert!(!p.is_cancelled());
        p.cancel.store(true, Ordering::Relaxed);
        assert!(p.is_cancelled());
    }
}
