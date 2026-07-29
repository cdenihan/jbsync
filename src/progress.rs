//! A single self-erasing status line, so a sync is not silent while it works.
//!
//! Most of a sync's wall time is spent waiting: on `git`, and on walking IDE
//! config directories that sit beside hundreds of megabytes of plugins. That is
//! fast enough not to need a percentage bar and slow enough that saying nothing
//! looks like a hang.
//!
//! Output goes to stderr and only when stderr is a terminal, so redirecting or
//! piping a run still yields exactly the report and nothing else.

use std::io::{IsTerminal, Write};

pub struct Progress {
    enabled: bool,
    width: usize,
}

impl Default for Progress {
    fn default() -> Self {
        Self::new()
    }
}

impl Progress {
    #[must_use]
    pub fn new() -> Self {
        Self {
            enabled: std::io::stderr().is_terminal(),
            width: 0,
        }
    }

    /// A `Progress` that never writes anything, for tests and library callers.
    #[must_use]
    pub fn silent() -> Self {
        Self {
            enabled: false,
            width: 0,
        }
    }

    /// Replaces the status line. Padding covers whatever the previous, possibly
    /// longer, message left behind.
    pub fn step(&mut self, message: &str) {
        if !self.enabled {
            return;
        }
        let frame = self.frame(message);
        let mut stderr = std::io::stderr();
        let _ = write!(stderr, "{frame}");
        let _ = stderr.flush();
    }

    /// The bytes `step` would emit, and the width bookkeeping behind them.
    /// Separated so tests can check the padding without writing to a terminal.
    fn frame(&mut self, message: &str) -> String {
        let padding = self.width.saturating_sub(message.len());
        self.width = message.len();
        format!("\r{message}{:padding$}", "")
    }

    /// Clears the line, leaving the report to start on a clean row.
    pub fn clear(&mut self) {
        if !self.enabled || self.width == 0 {
            return;
        }
        let mut stderr = std::io::stderr();
        let _ = write!(stderr, "\r{:width$}\r", "", width = self.width);
        let _ = stderr.flush();
        self.width = 0;
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silent_progress_never_reports_itself_as_writing() {
        let mut progress = Progress::silent();
        progress.step("working");
        // Width stays zero, so `clear` has nothing to undo and writes nothing.
        assert_eq!(progress.width, 0);
        progress.clear();
    }

    /// A shorter message must blank what the previous, longer one left behind,
    /// or the tail of the old line survives on screen.
    #[test]
    fn a_shorter_message_erases_the_longer_one_before_it() {
        let mut progress = Progress::silent();
        assert_eq!(progress.frame("hello"), "\rhello");
        assert_eq!(progress.width, 5);
        assert_eq!(progress.frame("hi"), "\rhi   ");
        assert_eq!(progress.width, 2);
    }
}
