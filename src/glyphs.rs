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
    pub fn pending(&self) -> &'static str {
        self.pick("\u{f017}", "(p)")
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
