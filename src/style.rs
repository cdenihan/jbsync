//! Minimal ANSI styling, applied only when the output is going to a terminal.
//!
//! A report is read by a person scanning for the one line that matters, so the
//! direction of a change and the name of an IDE are worth making visually
//! distinct. Piped or redirected output stays plain text, because something is
//! usually parsing it.

use std::io::IsTerminal;

#[derive(Debug, Clone, Copy)]
pub struct Style {
    enabled: bool,
}

impl Style {
    /// Styled when stdout is a terminal, plain otherwise.
    #[must_use]
    pub fn auto() -> Self {
        Self {
            // NO_COLOR is the de-facto opt-out; honouring it costs nothing.
            enabled: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    #[must_use]
    pub fn plain() -> Self {
        Self { enabled: false }
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\u{1b}[{code}m{text}\u{1b}[0m")
        } else {
            text.to_string()
        }
    }

    #[must_use]
    pub fn bold(self, text: &str) -> String {
        self.paint("1", text)
    }

    #[must_use]
    pub fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }

    #[must_use]
    pub fn cyan(self, text: &str) -> String {
        self.paint("36", text)
    }

    #[must_use]
    pub fn green(self, text: &str) -> String {
        self.paint("32", text)
    }

    #[must_use]
    pub fn yellow(self, text: &str) -> String {
        self.paint("33", text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_styling_adds_nothing() {
        let style = Style::plain();
        assert_eq!(style.bold("hi"), "hi");
        assert_eq!(style.dim("hi"), "hi");
        assert_eq!(style.cyan("hi"), "hi");
    }

    #[test]
    fn enabled_styling_wraps_and_resets() {
        let style = Style { enabled: true };
        assert_eq!(style.bold("hi"), "\u{1b}[1mhi\u{1b}[0m");
        assert_eq!(style.green("hi"), "\u{1b}[32mhi\u{1b}[0m");
    }
}
