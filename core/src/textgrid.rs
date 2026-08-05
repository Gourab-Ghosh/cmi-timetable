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

pub static TIME_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d{1,2}):(\d{2})\s*[-\u{2013}\u{2014}]\s*(\d{1,2}):(\d{2})").unwrap()
});

/// Parse the first time range in `cell` into a Slot ("09:10-10:25" → 550..625).
pub fn parse_slot(cell: &str) -> Option<Slot> {
    let caps = TIME_RANGE_RE.captures(cell)?;
    let num = |i: usize| caps.get(i).unwrap().as_str().parse::<u16>().ok();
    let (h1, m1, h2, m2) = (num(1)?, num(2)?, num(3)?, num(4)?);
    if h1 > 23 || h2 > 23 || m1 > 59 || m2 > 59 {
        return None;
    }
    Some(Slot::new(h1 * 60 + m1, h2 * 60 + m2))
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

/// A separator line consists only of `=`, `-`, `+` and whitespace, with at
/// least three ruling characters.
fn is_separator(line: &str) -> bool {
    let mut ruling = 0usize;
    for c in line.chars() {
        match c {
            '=' | '-' | '+' => ruling += 1,
            ' ' | '\t' => {}
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
/// by walking back to a char boundary).
fn slice_at(line: &str, positions: &[usize]) -> Vec<String> {
    let mut out = Vec::with_capacity(positions.len() + 1);
    let mut start = 0usize;
    for &p in positions {
        let mut p = p.min(line.len());
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

/// Try to interpret `text` as a column-aligned grid. Returns `None` when no
/// header row (≥3 time ranges + a `|`) is found.
pub fn parse_grid(text: &str) -> Option<RawGrid> {
    let text = text.replace('\r', "");
    let lines: Vec<&str> = text.lines().collect();

    let header_idx = lines.iter().position(|l| {
        l.contains('|') && TIME_RANGE_RE.find_iter(l).count() >= 3
    })?;
    let header = lines[header_idx];
    let header_segs = split_pipes(header);
    let header_pipes = header_segs.len() - 1;
    let pipe_positions: Vec<usize> = header
        .match_indices('|')
        .map(|(i, _)| i)
        .collect();

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
        let owned_segs: Vec<String> = if line.matches('|').count() == header_pipes {
            split_pipes(line).into_iter().map(|s| s.to_string()).collect()
        } else {
            slice_at(line, &pipe_positions)
        };
        let label = owned_segs.first().map(|s| s.trim().to_string()).unwrap_or_default();
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
}

/// Split cell text into course codes. Codes are separated by whitespace or
/// `/`; a trailing `+` flags an optional course; a standalone `TMP*` token
/// marks a temporary hall booking.
pub fn parse_cell(cell: &str) -> CellTokens {
    let mut out = CellTokens::default();
    for token in cell.split(|c: char| c.is_whitespace() || c == '/') {
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
    }

    #[test]
    fn separator_lines() {
        assert!(is_separator("=====+===========+"));
        assert!(is_separator("-------------------+-----------+"));
        assert!(!is_separator(" Mon |           |"));
        assert!(!is_separator(""));
    }
}
