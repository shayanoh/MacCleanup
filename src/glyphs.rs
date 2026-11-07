#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphMode {
    Nerd,
    Ascii,
}

/// Every symbol the UI draws, in either Nerd Font or plain ASCII flavor.
#[derive(Clone, Copy, Debug)]
pub struct Glyphs {
    pub mode: GlyphMode,
}

const NERD_SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const ASCII_SPINNER: &[&str] = &["|", "/", "-", "\\"];
const NERD_EIGHTHS: &[char] = &[' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
const ASCII_EIGHTHS: &[char] = &[' ', '#'];

impl Glyphs {
    pub fn new(mode: GlyphMode) -> Self {
        Self { mode }
    }

    fn pick(&self, nerd: &'static str, ascii: &'static str) -> &'static str {
        match self.mode {
            GlyphMode::Nerd => nerd,
            GlyphMode::Ascii => ascii,
        }
    }

    pub fn folder(&self) -> &'static str {
        self.pick("\u{f07b}", "[D]")
    }
    pub fn file(&self) -> &'static str {
        self.pick("\u{f15b}", "[F]")
    }
    pub fn cursor(&self) -> &'static str {
        self.pick("▸", ">")
    }
    pub fn marked(&self) -> &'static str {
        self.pick("\u{f046}", "[x]")
    }
    pub fn unmarked(&self) -> &'static str {
        self.pick("\u{f096}", "[ ]")
    }
    pub fn safe(&self) -> &'static str {
        self.pick("●", "o")
    }
    pub fn caution(&self) -> &'static str {
        self.pick("●", "!")
    }
    pub fn protected(&self) -> &'static str {
        self.pick("●", "X")
    }
    /// A Time Machine snapshot.
    pub fn snapshot(&self) -> &'static str {
        self.pick("\u{f1da}", "[T]")
    }
    /// A snapshot macOS keeps for itself.
    pub fn locked(&self) -> &'static str {
        self.pick("\u{f023}", "[S]")
    }
    pub fn pending(&self) -> &'static str {
        self.pick("\u{f017}", "(p)")
    }

    /// A tool's own cleanup command (instead of a folder).
    pub fn command(&self) -> &'static str {
        self.pick("\u{f120}", ">_")
    }
    pub fn hatch(&self) -> &'static str {
        self.pick("░", "/")
    }
    pub fn star(&self) -> &'static str {
        self.pick("★", "*")
    }
    pub fn dot(&self) -> &'static str {
        self.pick("·", "|")
    }
    pub fn arrow(&self) -> &'static str {
        self.pick("→", "->")
    }
    pub fn scroll_track(&self) -> &'static str {
        self.pick("│", "|")
    }
    pub fn scroll_thumb(&self) -> &'static str {
        self.pick("█", "#")
    }
    pub fn ellipsis(&self) -> &'static str {
        self.pick("…", "...")
    }
    pub fn check(&self) -> &'static str {
        self.pick("\u{f00c}", "ok")
    }
    pub fn warn(&self) -> &'static str {
        self.pick("\u{f071}", "!")
    }
    /// Sidebar icon for entry `i`.
    pub fn nav(&self, i: usize) -> &'static str {
        const NERD: [&str; 9] = [
            "\u{f015}", "\u{f07c}", "\u{f12d}", "\u{f187}", "\u{f1c0}", "\u{f308}", "\u{f1da}",
            "\u{f1b2}", "\u{f1f8}",
        ];
        match self.mode {
            GlyphMode::Nerd => NERD.get(i).copied().unwrap_or("\u{f111}"),
            GlyphMode::Ascii => "*",
        }
    }

    /// The ToBeDeleted icon: an empty outline, or a full one when it holds items.
    pub fn trash(&self, full: bool) -> &'static str {
        match (self.mode, full) {
            (GlyphMode::Nerd, true) => "\u{f1f8}",
            (GlyphMode::Nerd, false) => "\u{f014}",
            (GlyphMode::Ascii, true) => "#",
            (GlyphMode::Ascii, false) => "o",
        }
    }

    /// Sort-order arrows for column headers.
    pub fn sort_asc(&self) -> &'static str {
        self.pick("▲", "^")
    }
    pub fn sort_desc(&self) -> &'static str {
        self.pick("▼", "v")
    }

    pub fn spinner(&self) -> &'static [&'static str] {
        match self.mode {
            GlyphMode::Nerd => NERD_SPINNER,
            GlyphMode::Ascii => ASCII_SPINNER,
        }
    }

    /// Bar characters from empty (index 0) to full (last index).
    pub fn eighths(&self) -> &'static [char] {
        match self.mode {
            GlyphMode::Nerd => NERD_EIGHTHS,
            GlyphMode::Ascii => ASCII_EIGHTHS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_strings(g: &Glyphs) -> Vec<&'static str> {
        let mut v = vec![
            g.folder(),
            g.file(),
            g.cursor(),
            g.marked(),
            g.unmarked(),
            g.safe(),
            g.caution(),
            g.protected(),
            g.pending(),
            g.hatch(),
            g.star(),
            g.dot(),
            g.ellipsis(),
            g.check(),
            g.warn(),
            g.nav(0),
            g.nav(7),
            g.sort_asc(),
            g.sort_desc(),
            g.trash(false),
            g.trash(true),
        ];
        v.extend(g.spinner().iter().copied());
        v
    }

    #[test]
    fn ascii_mode_is_ascii_only() {
        let g = Glyphs::new(GlyphMode::Ascii);
        for s in all_strings(&g) {
            assert!(s.is_ascii(), "non-ascii glyph: {s:?}");
        }
        for c in g.eighths() {
            assert!(c.is_ascii(), "non-ascii eighth: {c:?}");
        }
    }

    #[test]
    fn nerd_mode_has_spinner_frames() {
        let g = Glyphs::new(GlyphMode::Nerd);
        assert!(g.spinner().len() >= 4);
        assert!(g.eighths().len() >= 2);
    }
}
