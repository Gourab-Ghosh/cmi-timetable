//! What the search box means by a match.
//!
//! Three switches, the ones every editor has and nobody has to be taught —
//! **match case**, **whole word**, **regular expression** — and one type that
//! turns them into a yes/no about a course.
//!
//! It is a prepared matcher rather than a function on the query because a
//! filter pass asks the same question once per course, and compiling a
//! pattern (or lowercasing a needle) for each of them would put that work
//! inside the loop. Build one, use it for the whole pass.
//!
//! **A pattern that does not compile matches nothing, and says why.** Not
//! "matches everything", which would silently show the whole catalog the
//! moment a reader typed `(`, and not the previous result either, which would
//! leave a list on screen that no longer answers what the box says. The
//! caller shows [`Matcher::error`] instead — the reader is mid-thought and the
//! only useful thing to say is what is wrong with the pattern so far.

use regex_lite::Regex;

/// The search box's state: what was typed, and which of the three switches
/// are on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query<'a> {
    pub text: &'a str,
    pub match_case: bool,
    pub whole_word: bool,
    pub use_regex: bool,
}

/// A prepared answer to "does this text match?".
pub enum Matcher {
    /// Nothing was typed, so nothing is excluded.
    Everything,
    Plain {
        /// Already lowercased when the search is case-insensitive, so the
        /// haystack is the only thing lowercased inside the loop.
        needle: String,
        match_case: bool,
        whole_word: bool,
    },
    Pattern(Regex),
    /// The pattern does not compile. Carries the reason, for the reader.
    Bad(String),
}

impl Matcher {
    pub fn new(q: &Query) -> Matcher {
        let text = q.text.trim();
        if text.is_empty() {
            return Matcher::Everything;
        }
        if !q.use_regex {
            return Matcher::Plain {
                needle: if q.match_case {
                    text.to_string()
                } else {
                    text.to_lowercase()
                },
                match_case: q.match_case,
                whole_word: q.whole_word,
            };
        }
        // Both switches are expressed in the pattern itself rather than in
        // the walk: `(?i)` is regex-lite's own case-insensitivity, and `\b`
        // is what "whole word" has always meant. The alternation group
        // matters — `\b(?:a|b)\b` is what the reader means by whole-word
        // "a or b", while `\ba|b\b` would only bound the halves it touches.
        let mut pattern = String::with_capacity(text.len() + 12);
        if !q.match_case {
            pattern.push_str("(?i)");
        }
        if q.whole_word {
            pattern.push_str("\\b(?:");
            pattern.push_str(text);
            pattern.push_str(")\\b");
        } else {
            pattern.push_str(text);
        }
        match Regex::new(&pattern) {
            Ok(re) => Matcher::Pattern(re),
            Err(e) => Matcher::Bad(tidy_error(&e.to_string())),
        }
    }

    /// Does `haystack` match? For a plain search the haystack is lowercased
    /// here when needed; callers pass the text as it is meant to be read.
    pub fn matches(&self, haystack: &str) -> bool {
        match self {
            Matcher::Everything => true,
            Matcher::Bad(_) => false,
            Matcher::Pattern(re) => re.is_match(haystack),
            // This is the hot path: once per course, on every keystroke, on
            // every one of the three tabs that share a filter bar. The
            // arrangement below allocates NOTHING for the ordinary case (an
            // ASCII query against an ASCII course), which is what almost
            // every search in this app is; only a query or a name carrying
            // non-ASCII pays for a lowercased copy, and only then.
            Matcher::Plain {
                needle,
                match_case,
                whole_word,
            } => {
                if *match_case {
                    return if *whole_word {
                        contains_whole_word(haystack, needle)
                    } else {
                        haystack.contains(needle.as_str())
                    };
                }
                if haystack.is_ascii() && needle.is_ascii() {
                    return if *whole_word {
                        contains_whole_word_ci_ascii(haystack, needle)
                    } else {
                        contains_ci_ascii(haystack, needle)
                    };
                }
                // CMI publishes accented names, and `Rào` folds to `rào` only
                // with real Unicode lowercasing. One allocation, on the rare
                // path, rather than a wrong answer on it.
                let hay = haystack.to_lowercase();
                if *whole_word {
                    contains_whole_word(&hay, needle)
                } else {
                    hay.contains(needle.as_str())
                }
            }
        }
    }

    /// Why the pattern was refused, if it was. `None` for every matcher that
    /// works, including the empty one.
    pub fn error(&self) -> Option<&str> {
        match self {
            Matcher::Bad(why) => Some(why),
            _ => None,
        }
    }

    /// True when the box holds something that cannot be read as a search yet.
    pub fn is_broken(&self) -> bool {
        matches!(self, Matcher::Bad(_))
    }
}

/// A case-insensitive `contains`, for ASCII on both sides, without allocating.
///
/// `needle` is already lowercase. Byte windows are safe here precisely
/// because both sides are ASCII — one byte, one character — which is the
/// condition the caller checks before choosing this path.
fn contains_ci_ascii(hay: &str, needle: &str) -> bool {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.is_empty() {
        return true;
    }
    if n.len() > h.len() {
        return false;
    }
    h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

/// Is `needle` in `hay` with a non-word character (or nothing) on both sides?
///
/// A word character is a letter, a digit or `_` — the same rule `\b` uses for
/// ASCII. It is NOT identical once the pattern switch is on: regex-lite's `\b`
/// and `(?i)` are ASCII-only, while this path is Unicode-aware, so a name with
/// an accented letter can answer differently in the two modes. FEATURES.md says
/// so too; do not "simplify" either of them into the other.
///
/// Allocation-free: `find` returns byte offsets that are always character
/// boundaries, and the two neighbours are read as characters, so this stays
/// correct for the accented names CMI publishes.
fn contains_whole_word(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut from = 0;
    while let Some(pos) = hay[from..].find(needle) {
        let at = from + pos;
        let end = at + needle.len();
        let before_ok = hay[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after_ok = hay[end..].chars().next().is_none_or(|c| !is_word_char(c));
        if before_ok && after_ok {
            return true;
        }
        // Step one CHARACTER past the failed start, never one byte.
        from = at + hay[at..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// The whole-word test, case-insensitively, for ASCII on both sides — again
/// without allocating, for the same reason.
fn contains_whole_word_ci_ascii(hay: &str, needle: &str) -> bool {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.is_empty() {
        return true;
    }
    if n.len() > h.len() {
        return false;
    }
    (0..=h.len() - n.len()).any(|at| {
        h[at..at + n.len()].eq_ignore_ascii_case(n)
            && (at == 0 || !is_word_byte(h[at - 1]))
            && (at + n.len() == h.len() || !is_word_byte(h[at + n.len()]))
    })
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The regex crate's message, cut down to the part a student can act on.
///
/// Its errors arrive as several lines with a caret diagram, which is useful
/// in a terminal and noise in a filter bar. The first line names the problem.
fn tidy_error(raw: &str) -> String {
    let first = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("regex parse error"))
        .unwrap_or("that pattern can't be read");
    let first = first.trim_start_matches("error: ").trim();
    // The engine's own words, turned into the thing to DO about them. These
    // are `regex-lite`'s actual phrasings — the earlier mapping was written
    // against the full `regex` crate's wording, which this dependency never
    // emits, so the reader was shown "found open group without closing ')'"
    // verbatim under their half-typed pattern.
    if first.contains("open group without closing") {
        return "add a ')' to close the group".to_string();
    }
    if first.contains("character class") && first.contains("closing bracket") {
        return "add a ']' to close the [ … ] set".to_string();
    }
    if first.contains("repetition operator missing expression")
        || first.contains("repetition quantifier expects")
    {
        return "* + ? need something in front of them".to_string();
    }
    let mut out = first.to_string();
    if let Some(rest) = out.strip_prefix("unclosed group") {
        out = format!("unclosed ( group{rest}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reason under a half-typed pattern has to say what to DO. These are
    /// `regex-lite`'s own phrasings, which is the engine this crate actually
    /// depends on — an earlier mapping was written for the full `regex`
    /// crate's wording and never fired, so readers saw "found open group
    /// without closing ')'" verbatim.
    #[test]
    fn a_broken_pattern_says_what_to_add() {
        let cases = [("(unclosed", "')'"), ("[A-", "']'")];
        for (pattern, wanted) in cases {
            let m = Matcher::new(&Query {
                text: pattern,
                use_regex: true,
                ..Query::default()
            });
            let why = m.error().unwrap_or("");
            assert!(
                why.contains(wanted),
                "{pattern:?} should tell the reader to add {wanted}, said {why:?}"
            );
            assert!(
                why.len() < 60,
                "{why:?} is too long to read in a filter bar"
            );
            assert!(
                !why.contains("regex"),
                "{why:?} names the engine, not the fix"
            );
        }
    }

    fn q<'a>(text: &'a str) -> Query<'a> {
        Query {
            text,
            ..Query::default()
        }
    }

    fn m(q: &Query) -> Matcher {
        Matcher::new(q)
    }

    #[test]
    fn an_empty_box_excludes_nothing() {
        for text in ["", "   ", "\t"] {
            let matcher = m(&q(text));
            assert!(matcher.matches("anything"));
            assert!(matcher.matches(""));
            assert!(matcher.error().is_none());
        }
    }

    #[test]
    fn plain_search_ignores_case_by_default() {
        let matcher = m(&q("toc"));
        assert!(matcher.matches("TOC Theory of Computation"));
        assert!(matcher.matches("toc"));
        assert!(!matcher.matches("Algebra"));
    }

    #[test]
    fn match_case_makes_it_exact() {
        let matcher = m(&Query {
            text: "TOC",
            match_case: true,
            ..Query::default()
        });
        assert!(matcher.matches("TOC Theory of Computation"));
        assert!(!matcher.matches("toc theory"));
    }

    #[test]
    fn whole_word_stops_matching_inside_a_word() {
        let matcher = m(&Query {
            text: "alg",
            whole_word: true,
            ..Query::default()
        });
        assert!(!matcher.matches("Algebra I"), "alg is inside Algebra");
        assert!(matcher.matches("ALG 101"), "a whole word, any case");
        assert!(matcher.matches("intro to alg"), "at the end");
        assert!(matcher.matches("alg"), "the whole haystack");
        assert!(matcher.matches("(alg)"), "brackets are not word characters");
    }

    #[test]
    fn whole_word_counts_digits_and_underscores_as_part_of_the_word() {
        let matcher = m(&Query {
            text: "toc",
            whole_word: true,
            ..Query::default()
        });
        assert!(!matcher.matches("toc2"));
        assert!(!matcher.matches("toc_b"));
        assert!(!matcher.matches("2toc"));
        assert!(matcher.matches("toc-b"), "a hyphen ends a word");
    }

    #[test]
    fn whole_word_survives_the_names_cmi_actually_publishes() {
        // An accented name must not be chopped by byte offsets.
        let matcher = m(&Query {
            text: "raghavendra",
            whole_word: true,
            ..Query::default()
        });
        assert!(matcher.matches("K. V. Raghavendra Rào"));
        let accented = m(&Query {
            text: "rào",
            whole_word: true,
            ..Query::default()
        });
        assert!(accented.matches("K. V. Raghavendra Rào"));
    }

    #[test]
    fn a_pattern_is_a_pattern_only_when_the_switch_is_on() {
        let literal = m(&q("^alg"));
        assert!(!literal.matches("Algebra"), "off, it is just text");
        assert!(literal.matches("the ^alg thing"), "and it can be found");

        let pattern = m(&Query {
            text: "^alg",
            use_regex: true,
            ..Query::default()
        });
        assert!(pattern.matches("Algebra I"), "on, it anchors");
        assert!(!pattern.matches("Linear Algebra"));
    }

    #[test]
    fn a_pattern_can_use_the_other_two_switches_too() {
        let cased = m(&Query {
            text: "^ALG",
            use_regex: true,
            match_case: true,
            ..Query::default()
        });
        assert!(cased.matches("ALG 101"));
        assert!(!cased.matches("alg 101"));

        // `\b(?:a|b)\b` — the group is what makes whole-word alternation mean
        // what the reader means.
        let worded = m(&Query {
            text: "alg|toc",
            use_regex: true,
            whole_word: true,
            ..Query::default()
        });
        assert!(worded.matches("intro to toc"));
        assert!(!worded.matches("Algebra"), "not inside a longer word");
        assert!(!worded.matches("tocsin"));
    }

    #[test]
    fn a_pattern_that_does_not_compile_matches_nothing_and_says_why() {
        let broken = m(&Query {
            text: "(unclosed",
            use_regex: true,
            ..Query::default()
        });
        assert!(broken.is_broken());
        assert!(!broken.matches("(unclosed"), "not even itself");
        assert!(!broken.matches("anything else"));
        let why = broken.error().unwrap();
        assert!(!why.is_empty());
        assert!(!why.contains('\n'), "one line for a filter bar: {why:?}");
        assert!(why.len() < 120, "{why:?}");
    }

    #[test]
    fn a_half_typed_pattern_never_shows_the_whole_catalog() {
        // The failure that matters: a reader typing `[A-` must not be handed
        // every course as though they had searched for nothing.
        for text in ["[A-", "(", "a{2,1}", "*", "\\"] {
            let matcher = m(&Query {
                text,
                use_regex: true,
                ..Query::default()
            });
            assert!(
                !matcher.matches("Algebra I") || !matcher.is_broken(),
                "{text:?} matched everything while broken"
            );
        }
    }

    #[test]
    fn the_switches_do_not_change_what_an_empty_box_means() {
        let all_on = m(&Query {
            text: "  ",
            match_case: true,
            whole_word: true,
            use_regex: true,
        });
        assert!(all_on.matches("anything"));
        assert!(!all_on.is_broken());
    }
}
