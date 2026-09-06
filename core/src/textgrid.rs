//! Generic slicing of the column-aligned ASCII grids CMI renders in `<pre>`
//! blocks. Works for both the per-branch timetable grids and the hall
//! allocation grid.
//!
//! Robustness note (verified against the live pages on 5 Aug 2026): the `|`
//! positions in a grid's header row are NOT always aligned with the `|`
//! positions in its data rows — the OCS1/OCS2/OCS3/OPDS1 branch headers are
//! 1–2 characters wider than their day rows, and the hall grid's header is
//! one character narrower than its hall rows. Slicing data rows at the
//! header's character indices would therefore misparse real data. Instead,
//! whenever a row has the same number of `|` separators as the header we
//! split the row on its *own* separators; the header's character indices are
//! kept only as a fallback for rows with a deviant pipe count.

use crate::model::Slot;
use regex_lite::Regex;
use std::sync::LazyLock;

// Tolerant of hand-edited drift: dot minutes ("09.10"), a "to" separator,
// and optional am/pm markers all count as a time range.
pub static TIME_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(\d{1,2})[:.](\d{2})\s*(am|pm)?\s*(?:[-\u{2013}\u{2014}]|to)\s*(\d{1,2})[:.](\d{2})\s*(am|pm)?",
    )
    .unwrap()
});

/// Resolve an hour to 24h form. Bare hours 1–6 are afternoon (nothing on
/// campus runs 1–6 AM); an explicit am/pm marker always wins.
fn hour_24(h: u16, meridiem: Option<&str>) -> u16 {
    match meridiem.map(str::to_ascii_lowercase).as_deref() {
        Some("pm") if h < 12 => h + 12,
        Some("am") if h == 12 => 0,
        None if (1..=6).contains(&h) => h + 12,
        _ => h,
    }
}

/// Parse the first time range in `cell` into a Slot ("09:10-10:25" → 550..625).
pub fn parse_slot(cell: &str) -> Option<Slot> {
    let caps = TIME_RANGE_RE.captures(cell)?;
    let num = |i: usize| caps.get(i).unwrap().as_str().parse::<u16>().ok();
    let mer = |i: usize| caps.get(i).map(|m| m.as_str());
    let (h1, m1, h2, m2) = (num(1)?, num(2)?, num(4)?, num(5)?);
    if h1 > 23 || h2 > 23 || m1 > 59 || m2 > 59 {
        return None;
    }
    // Whether the bare-afternoon rule actually MOVED the start. `hour_24`
    // shadows `h1` below and destroys the evidence, so ask before it runs
    // (R93 S2 / CF-2).
    let start_was_shifted = mer(3).is_none() && (1..=6).contains(&h1);
    let h1 = hour_24(h1, mer(3));
    let h2 = hour_24(h2, mer(6));
    let start = h1 * 60 + m1;
    let mut end = h2 * 60 + m2;
    // "6:30-7:45": the bare-afternoon rule shifts the start past an
    // unshifted end. A range never runs backwards — when no explicit
    // marker pins the end, it belongs to the same half-day as the start.
    //
    // Only legitimate when the start really was shifted. "11:50-11:50" was
    // never moved, so adding 12 h there does not repair a half-day guess —
    // it launders a degenerate range into 11:50–23:50, which the gate's
    // slot-sanity rule then passes and the app states as a fact.
    //
    // Do NOT replace this with `if end <= start { return None; }`. Measured:
    // that silently drops a whole column of the week (147 → 114 classes)
    // behind a "column ignored" warning that only developer mode ever shows
    // (the two `warnings.push(... column ignored ...)` sites in this file,
    // rendered at `app/src/dev.rs`'s `{format!("{} warnings", ...)}` panel).
    // Letting the non-positive slot through hands it to gate rule 6
    // ("slot sanity", `core/src/validate.rs`), which refuses the page and
    // keeps the stored snapshot — the designed answer, and drop-and-say
    // rather than silently clamp.
    if end <= start && start_was_shifted && mer(6).is_none() && h2 < 12 {
        end += 720;
    }
    Some(Slot::new(start, end))
}

#[derive(Debug, Clone)]
pub struct RawRow {
    /// Trimmed first cell: day token (branch grid), day name or hall name
    /// (hall grid).
    pub label: String,
    /// One entry per header slot column, untrimmed.
    pub cells: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RawGrid {
    /// Trimmed first header cell — the branch code on branch grids, empty on
    /// the hall grid.
    pub label0: String,
    /// Slot columns derived from the header, in order.
    pub slots: Vec<Slot>,
    pub rows: Vec<RawRow>,
    /// Non-empty, non-separator lines above the header (semester label and
    /// branch title live here on the real pages).
    pub leading: Vec<String>,
    /// Non-empty lines below the grid that carry no cells (e.g. the hall
    /// page's `*Note:` footnotes).
    pub trailing: Vec<String>,
    pub warnings: Vec<String>,
}

/// A separator line consists only of ruling characters (`=`, `-`, `+`, `_`,
/// `~`), pipes and whitespace, with at least three ruling characters. Pipes
/// are neutral so a "|----|----|" rule doesn't masquerade as a data row —
/// while a blank CELL row ("|      |      |", zero ruling chars) stays one.
fn is_separator(line: &str) -> bool {
    let mut ruling = 0usize;
    for c in line.chars() {
        match c {
            '=' | '-' | '+' | '_' | '~' => ruling += 1,
            ' ' | '\t' | '|' => {}
            _ => return false,
        }
    }
    ruling >= 3
}

/// Split a row into segments between `|` characters. The segment before the
/// first pipe is the label cell; a trailing empty segment (after a final
/// pipe) is preserved so indices line up with the header's segments.
fn split_pipes(line: &str) -> Vec<&str> {
    line.split('|').collect()
}

/// Fallback: slice `line` at the header's `|` byte positions (kept UTF-8-safe
/// by walking back to a char boundary). The pages are hand-typed, so a cut
/// landing inside a token is nudged to a nearby space (±2 bytes) rather than
/// bisecting a course code.
fn slice_at(line: &str, positions: &[usize]) -> Vec<String> {
    let bytes = line.as_bytes();
    // Where to cut, given the HEADER's separator positions and a row whose
    // own separators have drifted.
    //
    // A `|` is the real separator, so look for one first and a little
    // further afield than for a space. Cutting a space instead — which is
    // what this did — put the cut one or two characters off the row's own
    // pipe, which both truncated the name ("Lecture Hall 8" | "03|…") and
    // left the pipe inside a cell, inventing rooms like "Lecture Hall 803|"
    // that then appeared in the Halls view and the free-hall finder (§8.3).
    let nudge = |p: usize| -> usize {
        if p >= line.len() || bytes[p] == b'|' {
            return p;
        }
        for d in 1..=3usize {
            if p >= d && bytes[p - d] == b'|' {
                return p - d;
            }
            if p + d < line.len() && bytes[p + d] == b'|' {
                return p + d;
            }
        }
        if bytes[p] == b' ' {
            return p;
        }
        for d in 1..=2usize {
            if p >= d && bytes[p - d] == b' ' {
                return p - d;
            }
            if p + d < line.len() && bytes[p + d] == b' ' {
                return p + d;
            }
        }
        p
    };
    let mut out = Vec::with_capacity(positions.len() + 1);
    let mut start = 0usize;
    for &p in positions {
        let mut p = nudge(p.min(line.len()));
        while p > 0 && !line.is_char_boundary(p) {
            p -= 1;
        }
        let s = start.min(p);
        out.push(line[s..p].to_string());
        start = (p + 1).min(line.len());
        while start < line.len() && !line.is_char_boundary(start) {
            start += 1;
        }
    }
    out.push(line[start.min(line.len())..].to_string());
    out
}

/// Pipe-less variant: slice at column start positions without consuming a
/// separator character. Hand-typed rows drift a character or two from the
/// header, so cuts landing inside a token are nudged to a nearby space.
fn slice_at_cols(line: &str, positions: &[usize]) -> Vec<String> {
    let bytes = line.as_bytes();
    let nudge = |p: usize| -> usize {
        if p >= line.len() || bytes[p] == b' ' || bytes[p] == b'\t' {
            return p;
        }
        // Nearest space wins: a token nudged whole into the column it
        // straddles from, whichever side that is.
        for d in 1..=2usize {
            if p >= d && (bytes[p - d] == b' ' || bytes[p - d] == b'\t') {
                return p - d;
            }
            if p + d < line.len() && (bytes[p + d] == b' ' || bytes[p + d] == b'\t') {
                return p + d;
            }
        }
        p
    };
    // The FIRST cut ends the label. A token straddling it belongs to the
    // label ("Lecture Hall 803" must not shed its number into the first
    // data column), so that cut extends right to the token's end instead
    // of hunting for the nearest space.
    let nudge_label = |p: usize| -> usize {
        if p >= line.len() || bytes[p] == b' ' || bytes[p] == b'\t' {
            return p;
        }
        let mut q = p;
        while q < line.len() && bytes[q] != b' ' && bytes[q] != b'\t' {
            q += 1;
        }
        q
    };
    let mut out = Vec::with_capacity(positions.len() + 1);
    let mut start = 0usize;
    for (i, &p) in positions.iter().enumerate() {
        let raw = p.min(line.len());
        let mut p = if i == 0 { nudge_label(raw) } else { nudge(raw) }.min(line.len());
        while p > 0 && !line.is_char_boundary(p) {
            p -= 1;
        }
        let s = start.min(p);
        out.push(line[s..p].to_string());
        start = p;
    }
    out.push(line[start.min(line.len())..].to_string());
    out
}

/// Try to interpret `text` as a column-aligned grid: a header row with ≥3
/// time ranges, columns separated by `|` — or, when no pipes exist at all
/// (a re-typed page could drop the vertical rules), by column alignment
/// alone. Returns `None` when no such header is found.
pub fn parse_grid(text: &str) -> Option<RawGrid> {
    let text = text.replace('\r', "");
    let lines: Vec<&str> = text.lines().collect();

    if let Some(header_idx) = lines
        .iter()
        .position(|l| l.contains('|') && TIME_RANGE_RE.find_iter(l).count() >= 3)
    {
        return parse_grid_piped(&lines, header_idx);
    }
    let header_idx = lines
        .iter()
        .position(|l| TIME_RANGE_RE.find_iter(l).count() >= 3)?;
    parse_grid_columns(&lines, header_idx)
}

fn parse_grid_piped(lines: &[&str], header_idx: usize) -> Option<RawGrid> {
    let header = lines[header_idx];
    let header_segs = split_pipes(header);
    let header_pipes = header_segs.len() - 1;
    let pipe_positions: Vec<usize> = header.match_indices('|').map(|(i, _)| i).collect();

    let mut warnings = Vec::new();

    // Map header segments (index > 0) to slot columns. Segments that don't
    // parse as a time range (e.g. the empty segment after the final pipe)
    // are skipped; a non-empty middle segment that fails to parse is worth a
    // warning because it shifts nothing but loses a column.
    let mut col_map: Vec<(usize, Slot)> = Vec::new();
    for (j, seg) in header_segs.iter().enumerate().skip(1) {
        match parse_slot(seg) {
            Some(slot) => col_map.push((j, slot)),
            None => {
                if !seg.trim().is_empty() {
                    warnings.push(format!(
                        "grid header cell {j} ({:?}) is not a time range; column ignored",
                        seg.trim()
                    ));
                }
            }
        }
    }
    if col_map.is_empty() {
        return None;
    }

    let label0 = header_segs[0].trim().to_string();
    let slots: Vec<Slot> = col_map.iter().map(|(_, s)| *s).collect();

    let leading: Vec<String> = lines[..header_idx]
        .iter()
        .filter(|l| !l.trim().is_empty() && !is_separator(l))
        .map(|l| l.trim().to_string())
        .collect();

    let mut rows = Vec::new();
    let mut trailing = Vec::new();
    for line in &lines[header_idx + 1..] {
        if line.trim().is_empty() || is_separator(line) {
            continue;
        }
        if !line.contains('|') {
            trailing.push(line.trim().to_string());
            continue;
        }
        // Deviant pipe count (a pipe dropped or added by hand): slice at
        // the header's byte positions — column alignment survives such
        // edits, and the nudge keeps cuts off the middle of tokens. (A
        // "map each cell to its nearest column by pipe position" scheme
        // was tried and REJECTED: it moves aligned cells into the merged
        // segment's starting column, losing their real time slots.)
        let own_pipes = line.matches('|').count();
        let owned_segs: Vec<String> = if own_pipes == header_pipes {
            split_pipes(line)
                .into_iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            let mut segs = slice_at(line, &pipe_positions);
            // Belt and braces for the same failure: no hall name and no
            // course code contains a separator, so one that survived inside
            // a segment means the cut still missed. Blank it rather than let
            // it become part of a name.
            let stray = segs.iter().any(|s| s.contains('|'));
            if stray {
                warnings.push(format!(
                    "grid row {:?} has a different number of '|' separators than the header; \
                     sliced by column position and stray separators dropped",
                    line.trim().chars().take(40).collect::<String>()
                ));
                for s in segs.iter_mut() {
                    *s = s.replace('|', " ");
                }
            }
            segs
        };
        let label = owned_segs
            .first()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let cells: Vec<String> = col_map
            .iter()
            .map(|(j, _)| owned_segs.get(*j).cloned().unwrap_or_default())
            .collect();
        rows.push(RawRow { label, cells });
    }

    Some(RawGrid {
        label0,
        slots,
        rows,
        leading,
        trailing,
        warnings,
    })
}

/// Space-aligned fallback (no pipes anywhere in the header): each time
/// range in the header starts a column at the beginning of the whitespace
/// run before it, and every row is sliced at those positions. The gate's
/// substance rules still judge the result — this only keeps a benign
/// formatting change from zeroing the whole page.
fn parse_grid_columns(lines: &[&str], header_idx: usize) -> Option<RawGrid> {
    let header = lines[header_idx];
    let mut warnings =
        vec!["grid has no '|' separators; columns derived from spacing alone".to_string()];

    let mut positions: Vec<usize> = Vec::new();
    let mut col_map: Vec<(usize, Slot)> = Vec::new();
    for (i, m) in TIME_RANGE_RE.find_iter(header).enumerate() {
        // Anchor a touch before the time so slightly left-shifted cell
        // content isn't truncated — but BOUNDED, or a blank header label
        // cell (the hall grid's) would swallow the whole label column.
        let mut p = m.start();
        let lo = p.saturating_sub(2);
        let bytes = header.as_bytes();
        while p > lo && (bytes[p - 1] == b' ' || bytes[p - 1] == b'\t') {
            p -= 1;
        }
        positions.push(p);
        match parse_slot(m.as_str()) {
            Some(slot) => col_map.push((i + 1, slot)),
            None => warnings.push(format!(
                "grid header time {:?} did not parse; column ignored",
                m.as_str()
            )),
        }
    }
    if col_map.is_empty() {
        return None;
    }

    let header_segs = slice_at_cols(header, &positions);
    let label0 = header_segs
        .first()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let slots: Vec<Slot> = col_map.iter().map(|(_, s)| *s).collect();

    let leading: Vec<String> = lines[..header_idx]
        .iter()
        .filter(|l| !l.trim().is_empty() && !is_separator(l))
        .map(|l| l.trim().to_string())
        .collect();

    let mut rows = Vec::new();
    let mut trailing = Vec::new();
    for line in &lines[header_idx + 1..] {
        let t = line.trim();
        if t.is_empty() || is_separator(line) {
            continue;
        }
        // Footnote-style lines aren't rows in either grid shape.
        if t.starts_with('*') {
            trailing.push(t.to_string());
            continue;
        }
        let owned_segs = slice_at_cols(line, &positions);
        let label = owned_segs
            .first()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        // Without pipes there is nothing structural separating grid rows
        // from prose beneath the grid — filter what can't be a row: a
        // sentence-length label ("Note: rooms may change …" sliced at
        // column offsets), or a colon label whose cells carry TIMES (an
        // office-hours list like "Tuesday: 9:00-10:15, 2:00-3:15"). Real
        // labels are short day/hall names; real cells carry codes.
        let cells_have_times = owned_segs.iter().skip(1).any(|c| TIME_RANGE_RE.is_match(c));
        if label.chars().count() > 24 || (label.contains(':') && cells_have_times) {
            trailing.push(t.to_string());
            continue;
        }
        let cells: Vec<String> = col_map
            .iter()
            .map(|(j, _)| owned_segs.get(*j).cloned().unwrap_or_default())
            .collect();
        rows.push(RawRow { label, cells });
    }

    Some(RawGrid {
        label0,
        slots,
        rows,
        leading,
        trailing,
        warnings,
    })
}

/// Tokens found in one grid cell.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CellTokens {
    /// (course code, had a trailing '+').
    pub codes: Vec<(String, bool)>,
    /// A `TMP*` marker was present.
    pub temp: bool,
    /// Codes carrying a `%`. Kept in `codes` — deleting a catalog code over an
    /// upstream typo would take a real course off every user's timetable — but
    /// reported, because such a code cannot travel in a `?c=` link (R93 M5) and
    /// the app leaves it out of every address bar it writes.
    pub percent: Vec<String>,
}

/// Split cell text into course codes. Codes are separated by whitespace,
/// `/`, `,` or a stray `|` (ragged rows can leave one inside a sliced cell); a
/// trailing `+` flags an optional course; a standalone `TMP*` token marks a
/// temporary hall booking.
///
/// The comma is a separator like `/`, and it has to be: `ALG1,ENV` is an
/// ordinary human way to write two codes sharing one slot, and read as ONE
/// code it minted a catalog entry the app's own `?c=` cannot round-trip — so
/// the course fell off every timetable that held it on the next reload, and
/// could put a course nobody picked in its place (R93 M5). No CMI code has
/// ever contained one (checked against both fixture pages, every `<pre>`
/// block), and this reaches the pasted-page door as well as Sync.
pub fn parse_cell(cell: &str) -> CellTokens {
    let mut out = CellTokens::default();
    for token in cell.split(|c: char| c.is_whitespace() || c == '/' || c == ',' || c == '|') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if token.eq_ignore_ascii_case("TMP*") {
            out.temp = true;
            continue;
        }
        let (code, plus) = match token.strip_suffix('+') {
            Some(stripped) => (stripped, true),
            None => (token, false),
        };
        if code.is_empty() {
            continue;
        }
        if code.contains('%') {
            out.percent.push(code.to_string());
        }
        out.codes.push((code.to_string(), plus));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_tokens() {
        assert_eq!(parse_cell("   "), CellTokens::default());
        let t = parse_cell("  QCOM+ ");
        assert_eq!(t.codes, vec![("QCOM".to_string(), true)]);
        assert!(!t.temp);
        let t = parse_cell(" ABC TMP* ");
        assert_eq!(t.codes, vec![("ABC".to_string(), false)]);
        assert!(t.temp);
        let t = parse_cell("A/B");
        assert_eq!(
            t.codes,
            vec![("A".to_string(), false), ("B".to_string(), false)]
        );
        // A comma between two codes in one cell is TWO codes. Read as one, it
        // minted a catalog code the app's own `?c=` cannot round-trip, so the
        // course fell off every timetable holding it on the next reload
        // (R93 M5). No CMI code has ever contained a comma.
        let t = parse_cell("ALG1,ENV");
        assert_eq!(
            t.codes,
            vec![("ALG1".to_string(), false), ("ENV".to_string(), false)]
        );
        // A `%` is KEPT — deleting a catalog code over an upstream typo would
        // take a real course off every user's timetable — and reported, so the
        // page's own warnings name it.
        let t = parse_cell("50%OFF");
        assert_eq!(t.codes, vec![("50%OFF".to_string(), false)]);
        assert_eq!(t.percent, vec!["50%OFF".to_string()]);
    }

    #[test]
    fn separator_lines() {
        assert!(is_separator("=====+===========+"));
        assert!(is_separator("-------------------+-----------+"));
        assert!(!is_separator(" Mon |           |"));
        assert!(!is_separator(""));
        // Pipes are neutral: a piped rule is a separator, a blank cell row
        // (no ruling chars at all) is not.
        assert!(is_separator("----|-----|----"));
        assert!(is_separator("____|_____|____"));
        assert!(!is_separator("|     |     |"));
    }

    #[test]
    fn slot_formats() {
        // Canonical, dot minutes, "to", am/pm, and bare afternoon hours.
        assert_eq!(parse_slot("9:10-10:25"), Some(Slot::new(550, 625)));
        assert_eq!(parse_slot("09.10-10.25"), Some(Slot::new(550, 625)));
        assert_eq!(parse_slot("9:10 to 10:25"), Some(Slot::new(550, 625)));
        assert_eq!(parse_slot("9:10am - 10:25 AM"), Some(Slot::new(550, 625)));
        assert_eq!(parse_slot("2:00-3:15"), Some(Slot::new(840, 915)));
        assert_eq!(parse_slot("12:50 PM - 1:45 pm"), Some(Slot::new(770, 825)));
        assert_eq!(
            parse_slot("17:00\u{2013}18:15"),
            Some(Slot::new(1020, 1095))
        );
        // Ranges never run backwards: an unmarked end past the shifted
        // start belongs to the same half-day ("6:30-7:45" is evening).
        assert_eq!(parse_slot("6:30-7:45"), Some(Slot::new(1110, 1185)));
        assert_eq!(parse_slot("11:50-1:05"), Some(Slot::new(710, 785)));
        assert_eq!(parse_slot("no time here"), None);
    }

    /// R93 S2 — a time range that runs backwards is handed on as it is, so
    /// the gate can refuse the page.
    ///
    /// One mistyped digit in a column heading on CMI's timetable page
    /// ("11:50-13:05" losing its 3) used to come out of this function as
    /// 11:50-23:50 and pass every check after it: the master grid drew a
    /// twelve-hour column, the clash panel grew by thirty pairs, and the app
    /// stated as a fact that a course met until ten to midnight. The +12 h
    /// only ever existed to finish the job the bare-afternoon rule starts —
    /// "6:30-7:45" is an evening class — so it may only fire when the START
    /// really was moved. Nothing else is a half-day guess, and a range that
    /// was never guessed at is not the parser's to repair.
    ///
    /// Handing the bad slot on rather than dropping it is deliberate:
    /// dropping the column loses a whole day-column of the week behind a
    /// warning only developer mode shows, while passing it on reaches gate
    /// rule 6 ("slot sanity"), which refuses the page and keeps the stored
    /// snapshot — drop-and-say, never silently clamp.
    #[test]
    fn a_backwards_range_is_handed_on_rather_than_laundered() {
        // The repair this guard must NOT break: a bare "6" start is evening,
        // so an unmarked end belongs to the same half-day.
        assert_eq!(parse_slot("6:30-7:45"), Some(Slot::new(1110, 1185)));
        // Crosses noon; the END is the one the rule moves, and it already
        // came out forwards, so nothing here changes either.
        assert_eq!(parse_slot("11:50-1:05"), Some(Slot::new(710, 785)));

        // Degenerate: the mistyped heading. Zero length, kept zero length.
        assert_eq!(parse_slot("11:50-11:50"), Some(Slot::new(710, 710)));
        // Plainly backwards, both hours unambiguous.
        assert_eq!(parse_slot("13:05-11:50"), Some(Slot::new(785, 710)));
        // A morning hour is not a half-day guess either.
        assert_eq!(parse_slot("09:30-09:30"), Some(Slot::new(570, 570)));
        // Already refused before the fix, because 12 is not "bare
        // afternoon" — kept as the control that says which half of the
        // condition is doing the work.
        assert_eq!(parse_slot("12:00-12:00"), Some(Slot::new(720, 720)));
    }

    #[test]
    fn pipeless_grid_falls_back_to_columns() {
        let text = "\
BM1    9:10-10:25   10:30-11:45   11:50-13:05
Mon    TOC          ALG
Wed                 TOC           ANA1
";
        let grid = parse_grid(text).expect("pipe-less grid parses");
        assert_eq!(grid.label0, "BM1");
        assert_eq!(grid.slots.len(), 3);
        assert_eq!(grid.slots[0], Slot::new(550, 625));
        assert_eq!(grid.rows.len(), 2);
        assert_eq!(grid.rows[0].label, "Mon");
        assert_eq!(
            parse_cell(&grid.rows[0].cells[0]).codes,
            vec![("TOC".to_string(), false)]
        );
        assert_eq!(
            parse_cell(&grid.rows[1].cells[1]).codes,
            vec![("TOC".to_string(), false)]
        );
        assert!(parse_cell(&grid.rows[1].cells[0]).codes.is_empty());
    }

    #[test]
    fn sparse_pipe_rows_keep_their_columns() {
        // Row typed with a missing pipe but intact alignment (the common
        // hand edit): every code must stay in its REAL time column, whole.
        let text = "\
X   |9:10-10:25|10:30-11:45|11:50-13:05|
Mon | AAA      | BBB       | CCC       |
Wed | DDD        BBB       | CCC       |
";
        let grid = parse_grid(text).unwrap();
        assert_eq!(grid.rows.len(), 2);
        let wed = &grid.rows[1];
        assert_eq!(wed.label, "Wed");
        let col = |i: usize| -> Vec<String> {
            parse_cell(&wed.cells[i])
                .codes
                .into_iter()
                .map(|(t, _)| t)
                .collect()
        };
        assert_eq!(col(0), vec!["DDD".to_string()], "{:?}", wed.cells);
        assert_eq!(col(1), vec!["BBB".to_string()], "{:?}", wed.cells);
        assert_eq!(col(2), vec!["CCC".to_string()], "{:?}", wed.cells);
    }
}
