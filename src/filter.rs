//! fzf-style matching for the explorer's filter box.
//!
//! A query is split on spaces into terms that must ALL match. Each term is a
//! fuzzy subsequence by default, with the usual fzf operators: a leading `'`
//! means an exact substring, `^` a prefix, a trailing `$` a suffix, both
//! together a whole-name match, and `!` excludes names that contain the term.
//! Terms are case-insensitive unless they contain a capital ("smart case").

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Fuzzy,
    Exact,
    Prefix,
    Suffix,
    Whole,
    Exclude,
}

#[derive(Clone, Debug)]
struct Term {
    kind: Kind,
    /// The term's characters (lower-cased when `fold` is set).
    chars: Vec<char>,
    fold: bool,
}

/// A parsed filter query.
#[derive(Clone, Debug, Default)]
pub struct Query {
    terms: Vec<Term>,
}

pub fn parse(query: &str) -> Query {
    let terms = query.split_whitespace().filter_map(parse_term).collect();
    Query { terms }
}

fn parse_term(token: &str) -> Option<Term> {
    let (kind, body) = if let Some(rest) = token.strip_prefix('!') {
        (Kind::Exclude, rest)
    } else if let Some(rest) = token.strip_prefix('\'') {
        (Kind::Exact, rest)
    } else {
        let prefix = token.strip_prefix('^');
        let anchored = prefix.unwrap_or(token);
        let suffix = anchored.strip_suffix('$');
        match (prefix.is_some(), suffix.is_some()) {
            (true, true) => (Kind::Whole, suffix.unwrap_or_default()),
            (true, false) => (Kind::Prefix, anchored),
            (false, true) => (Kind::Suffix, suffix.unwrap_or_default()),
            (false, false) => (Kind::Fuzzy, token),
        }
    };
    if body.is_empty() {
        return None; // a lone operator means nothing
    }
    let fold = !body.chars().any(char::is_uppercase);
    let chars = body.chars().map(|c| lower(c, fold)).collect();
    Some(Term { kind, chars, fold })
}

fn lower(c: char, fold: bool) -> char {
    if fold {
        c.to_lowercase().next().unwrap_or(c)
    } else {
        c
    }
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// `Some(highlight positions)` (character indexes, sorted, unique) if
    /// `text` satisfies every term; `None` otherwise. A query that only
    /// excludes matches without highlights.
    pub fn matches(&self, text: &str) -> Option<Vec<usize>> {
        let text: Vec<char> = text.chars().collect();
        let mut positions: Vec<usize> = Vec::new();
        for term in &self.terms {
            positions.extend(term.positions(&text)?);
        }
        positions.sort_unstable();
        positions.dedup();
        Some(positions)
    }
}

impl Term {
    fn same(&self, text_char: char, term_char: char) -> bool {
        lower(text_char, self.fold) == term_char
    }

    fn starts_at(&self, text: &[char], at: usize) -> bool {
        at + self.chars.len() <= text.len()
            && self
                .chars
                .iter()
                .enumerate()
                .all(|(i, &c)| self.same(text[at + i], c))
    }

    /// The first exact occurrence of the term, as a start index.
    fn find(&self, text: &[char]) -> Option<usize> {
        (0..=text.len().saturating_sub(self.chars.len())).find(|&at| self.starts_at(text, at))
    }

    /// Highlight positions if the term matches, `None` if it does not.
    /// Excluding terms match (with no highlights) when the term is absent.
    fn positions(&self, text: &[char]) -> Option<Vec<usize>> {
        let n = self.chars.len();
        match self.kind {
            Kind::Exclude => self.find(text).is_none().then(Vec::new),
            Kind::Exact => self.find(text).map(|at| (at..at + n).collect()),
            Kind::Prefix => self.starts_at(text, 0).then(|| (0..n).collect()),
            Kind::Suffix => {
                let at = text.len().checked_sub(n)?;
                self.starts_at(text, at).then(|| (at..at + n).collect())
            }
            Kind::Whole => (text.len() == n && self.starts_at(text, 0)).then(|| (0..n).collect()),
            Kind::Fuzzy => self.fuzzy(text),
        }
    }

    /// Subsequence match highlighting the tightest window: find where the
    /// term first completes, then walk backwards to pick the latest letters.
    fn fuzzy(&self, text: &[char]) -> Option<Vec<usize>> {
        let mut next = 0;
        let mut end = None;
        for (i, &c) in text.iter().enumerate() {
            if self.same(c, self.chars[next]) {
                next += 1;
                if next == self.chars.len() {
                    end = Some(i);
                    break;
                }
            }
        }
        let end = end?;
        let mut found = Vec::with_capacity(self.chars.len());
        let mut remaining = self.chars.len();
        let mut i = end + 1;
        while remaining > 0 {
            i -= 1;
            if self.same(text[i], self.chars[remaining - 1]) {
                found.push(i);
                remaining -= 1;
            }
        }
        found.reverse();
        Some(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(query: &str, text: &str) -> Option<Vec<usize>> {
        parse(query).matches(text)
    }

    #[test]
    fn an_empty_query_matches_everything_without_highlights() {
        assert_eq!(m("", "anything"), Some(vec![]));
        assert_eq!(m("   ", "anything"), Some(vec![]));
        assert!(parse("").is_empty());
        assert!(parse("  ").is_empty());
        assert!(!parse("a").is_empty());
    }

    #[test]
    fn fuzzy_terms_match_subsequences() {
        assert_eq!(m("nm", "node_modules"), Some(vec![0, 5]));
        assert_eq!(m("nodmod", "node_modules"), Some(vec![0, 1, 2, 5, 6, 7]));
        assert_eq!(m("mn", "node_modules"), None, "order matters");
        assert_eq!(m("xyz", "node_modules"), None);
        assert_eq!(m("n", ""), None);
    }

    #[test]
    fn fuzzy_highlights_the_tightest_window() {
        // The contiguous "ab" at the end, not the scattered a...b.
        assert_eq!(m("ab", "a_x_ab"), Some(vec![4, 5]));
        assert_eq!(m("mod", "node_modules"), Some(vec![5, 6, 7]));
    }

    #[test]
    fn smart_case_is_insensitive_until_you_type_a_capital() {
        assert!(m("node", "NODE_X").is_some());
        assert!(m("make", "Makefile").is_some());
        assert!(m("Make", "Makefile").is_some());
        assert!(m("Make", "makefile").is_none(), "a capital makes it exact");
        assert!(m("NODE", "node_x").is_none());
    }

    #[test]
    fn several_terms_must_all_match() {
        assert_eq!(m("nod mod", "node_modules"), Some(vec![0, 1, 2, 5, 6, 7]));
        assert_eq!(m("nod mod", "node_x"), None);
        assert!(m("mod nod", "node_modules").is_some(), "term order is free");
        assert_eq!(m("  nod   mod  ", "node_modules").map(|p| p.len()), Some(6));
    }

    #[test]
    fn a_quote_makes_a_term_an_exact_substring() {
        assert_eq!(m("'mod", "node_modules"), Some(vec![5, 6, 7]));
        assert_eq!(m("'nm", "node_modules"), None);
        assert_eq!(m("'e_m", "node_modules"), Some(vec![3, 4, 5]));
    }

    #[test]
    fn caret_and_dollar_anchor_a_term() {
        assert_eq!(m("^no", "node_modules"), Some(vec![0, 1]));
        assert_eq!(m("^mod", "node_modules"), None);
        assert_eq!(m("es$", "node_modules"), Some(vec![10, 11]));
        assert_eq!(m("de$", "node_modules"), None);
        assert!(m("^node_modules$", "node_modules").is_some());
        assert!(m("^node$", "node_modules").is_none());
    }

    #[test]
    fn a_bang_excludes_names_containing_the_term() {
        assert_eq!(m("!mod", "node_modules"), None);
        assert_eq!(m("!mod", "node_x"), Some(vec![]));
        assert_eq!(m("no !mod", "node_x"), Some(vec![0, 1]));
        assert_eq!(m("no !mod", "node_modules"), None);
        assert_eq!(
            m("!MOD", "node_modules"),
            Some(vec![]),
            "smart case applies"
        );
    }

    #[test]
    fn lone_operators_are_ignored() {
        for q in ["'", "^", "$", "!", "' ^ $ !"] {
            assert_eq!(m(q, "anything"), Some(vec![]), "{q:?}");
        }
        assert!(m("' no", "node").is_some());
    }

    #[test]
    fn unicode_works_by_character() {
        assert_eq!(m("日本", "日本語のフォルダ"), Some(vec![0, 1]));
        assert_eq!(m("フォ", "日本語のフォルダ"), Some(vec![4, 5]));
        assert!(m("é", "café").is_some());
        assert!(m("É", "café").is_none(), "capital means exact");
    }

    #[test]
    fn highlights_are_sorted_and_unique_across_terms() {
        let pos = m("od no", "node").unwrap();
        let mut sorted = pos.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(pos, sorted);
        assert_eq!(pos, vec![0, 1, 2]);
    }
}
