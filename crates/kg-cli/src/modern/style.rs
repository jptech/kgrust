//! Terminal colour, decided once and passed down.
//!
//! Hand-rolled ANSI rather than a colour crate: the whole requirement is five
//! SGR codes and one `is_terminal` check, and a dependency here would be visible
//! in every downstream build of the toolchain.

use std::io::IsTerminal;

/// `--color`.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum ColorChoice {
    /// Colour when stdout is a terminal and `NO_COLOR` is unset.
    #[default]
    Auto,
    Always,
    Never,
}

impl ColorChoice {
    pub fn parse(text: &str) -> Option<ColorChoice> {
        match text.to_ascii_lowercase().as_str() {
            "auto" => Some(ColorChoice::Auto),
            "always" | "yes" | "true" => Some(ColorChoice::Always),
            "never" | "no" | "false" => Some(ColorChoice::Never),
            _ => None,
        }
    }
}

/// The resolved decision, plus the palette.
#[derive(Copy, Clone, Debug)]
pub struct Styles {
    enabled: bool,
}

impl Styles {
    /// Honours `NO_COLOR` (any non-empty value) under `auto`, as the informal
    /// standard requires; `--color=always` still wins, since it is an explicit
    /// request from the caller.
    pub fn new(choice: ColorChoice) -> Styles {
        let enabled = match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                std::io::stdout().is_terminal()
                    && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
            }
        };
        Styles { enabled }
    }

    /// No escape codes at all — the shape tests compare against this.
    pub fn plain() -> Styles {
        Styles { enabled: false }
    }

    pub fn is_enabled(self) -> bool {
        self.enabled
    }

    fn wrap(self, code: &str, text: &str) -> String {
        if self.enabled { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_owned() }
    }

    pub fn error(self, text: &str) -> String {
        self.wrap("1;31", text)
    }

    pub fn warning(self, text: &str) -> String {
        self.wrap("1;33", text)
    }

    pub fn bold(self, text: &str) -> String {
        self.wrap("1", text)
    }

    pub fn dim(self, text: &str) -> String {
        self.wrap("2", text)
    }

    pub fn blue(self, text: &str) -> String {
        self.wrap("1;34", text)
    }

    pub fn green(self, text: &str) -> String {
        self.wrap("1;32", text)
    }

    /// Node kinds in a tree dump.
    pub fn kind(self, text: &str) -> String {
        self.wrap("36", text)
    }

    /// Token text in a tree dump.
    pub fn literal(self, text: &str) -> String {
        self.wrap("32", text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_styles_add_nothing() {
        let styles = Styles::plain();
        assert_eq!(styles.error("boom"), "boom");
        assert_eq!(styles.kind("SOURCE_FILE"), "SOURCE_FILE");
        assert!(!styles.is_enabled());
    }

    #[test]
    fn always_wraps_in_sgr_codes() {
        let styles = Styles::new(ColorChoice::Always);
        assert_eq!(styles.error("boom"), "\x1b[1;31mboom\x1b[0m");
        assert!(styles.is_enabled());
    }

    #[test]
    fn never_is_never() {
        assert!(!Styles::new(ColorChoice::Never).is_enabled());
    }

    #[test]
    fn color_choice_parses_the_documented_spellings() {
        assert_eq!(ColorChoice::parse("AUTO"), Some(ColorChoice::Auto));
        assert_eq!(ColorChoice::parse("never"), Some(ColorChoice::Never));
        assert_eq!(ColorChoice::parse("sometimes"), None);
    }
}
