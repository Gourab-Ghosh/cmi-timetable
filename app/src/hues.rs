//! Stable branch hue coding: hash of the branch code → hue; saturation and
//! lightness are fixed per theme in CSS. The same hue identifies the branch
//! in every view. The alarm color is reserved for clashes and never
//! generated here (hues near it are nudged away).

/// FNV-1a over the branch code, mapped to a hue in degrees.
pub fn branch_hue(branch_code: &str) -> u16 {
    let mut hash: u32 = 0x811c9dc5;
    for b in branch_code.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    let mut hue = (hash % 360) as u16;
    // Keep a wide berth around the alarm crimson (~350°): shift hues in
    // [335°, 360°) ∪ [0°, 10°) into safer territory.
    if hue >= 335 {
        hue -= 130;
    } else if hue < 10 {
        hue += 40;
    }
    hue
}

/// The hue for a course: its first branch, or a neutral slate for
/// branch-less courses (status c).
pub fn course_hue(branches: &[String]) -> u16 {
    branches.first().map(|b| branch_hue(b)).unwrap_or(215)
}
