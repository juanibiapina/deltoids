//! Anchor axis: the BPE-friendly hash alphabet, `compute_line_hash`, the
//! `LINEhh|content` formatters, and parsing/rendering of anchors and
//! file-boundary positions.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh32::xxh32;

/// Width of a hash in characters. Anchors render as `<line><hash>`, e.g.
/// `42sr`, with no separator between the number and the hash.
pub const HASH_WIDTH: usize = 2;

/// Body separator used by `format_hash_line` between the anchor and the
/// line content: `"42sr|return foo;"`. Stable; not configurable.
pub const BODY_SEP: char = '|';

/// 647-entry table of 2-letter BPE-friendly bigrams. Ported from oh-my-pi
/// (`bigrams.json`, MIT). The order is stable forever — changing it would
/// invalidate every recorded anchor in past traces or transcripts.
static BIGRAMS: [&str; 647] = [
    "aa", "ab", "ac", "ad", "ae", "af", "ag", "ah", "ai", "aj", "ak", "al", "am", "an", "ao", "ap",
    "aq", "ar", "as", "at", "au", "av", "aw", "ax", "ay", "az", "ba", "bb", "bc", "bd", "be", "bf",
    "bg", "bh", "bi", "bj", "bk", "bl", "bm", "bn", "bo", "bp", "br", "bs", "bt", "bu", "bv", "bw",
    "bx", "by", "bz", "ca", "cb", "cc", "cd", "ce", "cf", "cg", "ch", "ci", "cj", "ck", "cl", "cm",
    "cn", "co", "cp", "cq", "cr", "cs", "ct", "cu", "cv", "cw", "cx", "cy", "cz", "da", "db", "dc",
    "dd", "de", "df", "dg", "dh", "di", "dj", "dk", "dl", "dm", "dn", "do", "dp", "dq", "dr", "ds",
    "dt", "du", "dv", "dw", "dx", "dy", "dz", "ea", "eb", "ec", "ed", "ee", "ef", "eg", "eh", "ei",
    "ej", "ek", "el", "em", "en", "eo", "ep", "eq", "er", "es", "et", "eu", "ev", "ew", "ex", "ey",
    "ez", "fa", "fb", "fc", "fd", "fe", "ff", "fg", "fh", "fi", "fj", "fk", "fl", "fm", "fn", "fo",
    "fp", "fq", "fr", "fs", "ft", "fu", "fv", "fw", "fx", "fy", "fz", "ga", "gb", "gc", "gd", "ge",
    "gf", "gg", "gh", "gi", "gj", "gl", "gm", "gn", "go", "gp", "gr", "gs", "gt", "gu", "gv", "gw",
    "gx", "gy", "gz", "ha", "hb", "hc", "hd", "he", "hf", "hg", "hh", "hi", "hj", "hk", "hl", "hm",
    "hn", "ho", "hp", "hq", "hr", "hs", "ht", "hu", "hv", "hw", "hx", "hy", "hz", "ia", "ib", "ic",
    "id", "ie", "if", "ig", "ih", "ii", "ij", "ik", "il", "im", "in", "io", "ip", "iq", "ir", "is",
    "it", "iu", "iv", "iw", "ix", "iy", "iz", "ja", "jb", "jc", "jd", "je", "jf", "jg", "jh", "ji",
    "jj", "jk", "jl", "jm", "jn", "jo", "jp", "jq", "jr", "js", "jt", "ju", "jw", "jx", "jy", "ka",
    "kb", "kc", "kd", "ke", "kf", "kg", "kh", "ki", "kj", "kk", "kl", "km", "kn", "ko", "kp", "kr",
    "ks", "kt", "ku", "kv", "kw", "kx", "ky", "la", "lb", "lc", "ld", "le", "lf", "lg", "lh", "li",
    "lj", "lk", "ll", "lm", "ln", "lo", "lp", "lr", "ls", "lt", "lu", "lv", "lw", "lx", "ly", "lz",
    "ma", "mb", "mc", "md", "me", "mf", "mg", "mh", "mi", "mj", "mk", "ml", "mm", "mn", "mo", "mp",
    "mq", "mr", "ms", "mt", "mu", "mv", "mw", "mx", "my", "mz", "na", "nb", "nc", "nd", "ne", "nf",
    "ng", "nh", "ni", "nj", "nk", "nl", "nm", "nn", "no", "np", "nr", "ns", "nt", "nu", "nv", "nw",
    "nx", "ny", "nz", "oa", "ob", "oc", "od", "oe", "of", "og", "oh", "oi", "oj", "ok", "ol", "om",
    "on", "oo", "op", "oq", "or", "os", "ot", "ou", "ov", "ow", "ox", "oy", "oz", "pa", "pb", "pc",
    "pd", "pe", "pf", "pg", "ph", "pi", "pj", "pk", "pl", "pm", "pn", "po", "pp", "pq", "pr", "ps",
    "pt", "pu", "pv", "pw", "px", "py", "pz", "qa", "qb", "qc", "qd", "qe", "qh", "qi", "ql", "qm",
    "qn", "qo", "qp", "qq", "qr", "qs", "qt", "qu", "qw", "qx", "qy", "ra", "rb", "rc", "rd", "re",
    "rf", "rg", "rh", "ri", "rk", "rl", "rm", "rn", "ro", "rp", "rq", "rr", "rs", "rt", "ru", "rv",
    "rw", "rx", "ry", "rz", "sa", "sb", "sc", "sd", "se", "sf", "sg", "sh", "si", "sj", "sk", "sl",
    "sm", "sn", "so", "sp", "sq", "sr", "ss", "st", "su", "sv", "sw", "sx", "sy", "sz", "ta", "tb",
    "tc", "td", "te", "tf", "tg", "th", "ti", "tj", "tk", "tl", "tm", "tn", "to", "tp", "tr", "ts",
    "tt", "tu", "tv", "tw", "tx", "ty", "tz", "ua", "ub", "uc", "ud", "ue", "uf", "ug", "uh", "ui",
    "uj", "uk", "ul", "um", "un", "uo", "up", "uq", "ur", "us", "ut", "uu", "uv", "uw", "ux", "uy",
    "uz", "va", "vb", "vc", "vd", "ve", "vf", "vg", "vh", "vi", "vj", "vk", "vl", "vm", "vn", "vo",
    "vp", "vq", "vr", "vs", "vt", "vu", "vv", "vw", "vx", "vy", "vz", "wa", "wb", "wc", "wd", "we",
    "wf", "wg", "wh", "wi", "wj", "wk", "wl", "wm", "wn", "wo", "wp", "wr", "ws", "wt", "wu", "wv",
    "ww", "wx", "wy", "xa", "xb", "xc", "xd", "xe", "xf", "xh", "xi", "xl", "xm", "xn", "xo", "xp",
    "xr", "xs", "xt", "xu", "xx", "xy", "xz", "ya", "yb", "yc", "yd", "ye", "yf", "yg", "yh", "yi",
    "yj", "yk", "yl", "ym", "yn", "yo", "yp", "yr", "ys", "yt", "yu", "yv", "yw", "yx", "yy", "yz",
    "za", "zb", "zc", "zd", "ze", "zf", "zg", "zh", "zi", "zk", "zl", "zm", "zn", "zo", "zp", "zr",
    "zs", "zt", "zu", "zw", "zx", "zy", "zz",
];

/// Compute the 2-character hash of a single line.
///
/// Trailing `\r` is stripped and trailing whitespace is ignored so anchors
/// survive line-ending and trailing-space-only changes. For lines with no
/// letter or digit (e.g. a lone `}`), the line number is mixed into the
/// seed so adjacent identical punctuation-only lines get distinct hashes.
pub fn compute_line_hash(line_number: usize, line: &str) -> &'static str {
    let trimmed = line.trim_end_matches(['\r', ' ', '\t']);
    let has_significant = trimmed.chars().any(|c| c.is_alphanumeric());
    let seed = if has_significant {
        0
    } else {
        line_number as u32
    };
    let idx = (xxh32(trimmed.as_bytes(), seed) as usize) % BIGRAMS.len();
    BIGRAMS[idx]
}

/// Format a single line with a hashline anchor: `"42sr|return foo;"`.
pub fn format_hash_line(line_number: usize, content: &str) -> String {
    format!(
        "{}{}{}{}",
        line_number,
        compute_line_hash(line_number, content),
        BODY_SEP,
        content
    )
}

/// Format every line of `text` with a hashline anchor, joined with `\n`.
/// `start_line` is the 1-indexed line number to assign to the first line.
pub fn format_hash_lines(text: &str, start_line: usize) -> String {
    let mut out = String::new();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let n = start_line + i;
        let _ = write!(
            out,
            "{}{}{}{}",
            n,
            compute_line_hash(n, line),
            BODY_SEP,
            line
        );
    }
    out
}

/// A parsed anchor: 1-indexed line number plus a 2-char hash. `BOF` and
/// `EOF` are not anchors; they are represented separately at the op level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub line: usize,
    pub hash: [u8; HASH_WIDTH],
}

impl Anchor {
    /// Parse an anchor token of the form `"42sr"`: one or more digits
    /// followed by exactly two ASCII lowercase letters.
    pub fn parse(token: &str) -> Result<Self, String> {
        let bytes = token.as_bytes();
        if bytes.len() < 3 {
            return Err(format!(
                "Invalid anchor {token:?}: expected '<line><2-char hash>' (e.g. \"42sr\")."
            ));
        }
        let split = bytes
            .iter()
            .position(|b| !b.is_ascii_digit())
            .ok_or_else(|| format!("Invalid anchor {token:?}: missing 2-char hash suffix."))?;
        if split == 0 {
            return Err(format!(
                "Invalid anchor {token:?}: must start with a line number."
            ));
        }
        if bytes.len() - split != HASH_WIDTH {
            return Err(format!(
                "Invalid anchor {token:?}: hash must be exactly {HASH_WIDTH} ASCII lowercase letters."
            ));
        }
        let hash_bytes = &bytes[split..];
        if !hash_bytes.iter().all(|b| b.is_ascii_lowercase()) {
            return Err(format!(
                "Invalid anchor {token:?}: hash must be ASCII lowercase letters."
            ));
        }
        let line: usize = std::str::from_utf8(&bytes[..split])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("Invalid anchor {token:?}: bad line number."))?;
        if line < 1 {
            return Err(format!(
                "Invalid anchor {token:?}: line number must be >= 1."
            ));
        }
        Ok(Anchor {
            line,
            hash: [hash_bytes[0], hash_bytes[1]],
        })
    }

    /// Render this anchor back to its `"42sr"` form.
    pub fn display(self) -> String {
        format!("{}{}", self.line, std::str::from_utf8(&self.hash).unwrap())
    }
}

/// Where an insert should land relative to its anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertSide {
    /// Insert payload lines before the anchored line.
    Before,
    /// Insert payload lines after the anchored line.
    After,
}

/// Either a real anchor, or one of the special file-boundary positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorOrBoundary {
    Anchor(Anchor),
    /// Beginning of file. Only valid with `Insert { side: Before }`.
    BeginningOfFile,
    /// End of file. Only valid with `Insert { side: After }`.
    EndOfFile,
}

impl AnchorOrBoundary {
    /// Parse a token. Accepts `"BOF"`, `"EOF"`, or an anchor like `"42sr"`.
    pub fn parse(token: &str) -> Result<Self, String> {
        match token {
            "BOF" => Ok(AnchorOrBoundary::BeginningOfFile),
            "EOF" => Ok(AnchorOrBoundary::EndOfFile),
            other => Anchor::parse(other).map(AnchorOrBoundary::Anchor),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computed_hash_is_two_lowercase_letters() {
        let hash = compute_line_hash(1, "fn main() {");
        assert_eq!(hash.len(), 2);
        assert!(hash.chars().all(|c| c.is_ascii_lowercase()));
    }

    #[test]
    fn hash_is_stable_across_calls() {
        let a = compute_line_hash(7, "return name.trim();");
        let b = compute_line_hash(7, "return name.trim();");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_ignores_trailing_whitespace_and_cr() {
        let plain = compute_line_hash(3, "let x = 1;");
        let trailing = compute_line_hash(3, "let x = 1;   \r");
        assert_eq!(plain, trailing);
    }

    #[test]
    fn punctuation_only_lines_get_distinct_hashes_at_different_line_numbers() {
        let a = compute_line_hash(5, "}");
        let b = compute_line_hash(6, "}");
        assert_ne!(
            a, b,
            "two `}}` lines on different rows must have different hashes"
        );
    }

    #[test]
    fn format_hash_lines_anchors_every_line() {
        let formatted = format_hash_lines("alpha\nbeta\ngamma", 1);
        let lines: Vec<&str> = formatted.split('\n').collect();
        assert_eq!(lines.len(), 3);
        for (i, line) in lines.iter().enumerate() {
            let n = i + 1;
            let prefix = format!("{n}");
            assert!(line.starts_with(&prefix), "line {n}: {line:?}");
            assert!(line.contains(BODY_SEP));
        }
    }

    #[test]
    fn format_hash_line_uses_pipe_separator() {
        let line = format_hash_line(42, "return foo;");
        assert!(line.starts_with("42"));
        let body_at = line.find(BODY_SEP).unwrap();
        // 2-digit line number + 2-char hash = 4 bytes before the pipe.
        assert_eq!(body_at, 4);
        assert_eq!(&line[body_at + 1..], "return foo;");
    }

    #[test]
    fn anchor_parses_well_formed_token() {
        let anchor = Anchor::parse("42sr").unwrap();
        assert_eq!(anchor.line, 42);
        assert_eq!(&anchor.hash, b"sr");
    }

    #[test]
    fn anchor_round_trips_through_display() {
        let a = Anchor::parse("119th").unwrap();
        assert_eq!(a.display(), "119th");
    }

    #[test]
    fn anchor_rejects_missing_hash() {
        let err = Anchor::parse("42").unwrap_err();
        assert!(err.contains("hash"), "{err}");
    }

    #[test]
    fn anchor_rejects_wrong_hash_length() {
        assert!(Anchor::parse("42s").is_err());
        assert!(Anchor::parse("42srx").is_err());
    }

    #[test]
    fn anchor_rejects_uppercase_hash() {
        assert!(Anchor::parse("42SR").is_err());
    }

    #[test]
    fn anchor_rejects_zero_line() {
        assert!(Anchor::parse("0sr").is_err());
    }

    #[test]
    fn anchor_rejects_non_numeric_prefix() {
        assert!(Anchor::parse("sr42").is_err());
    }

    #[test]
    fn anchor_or_boundary_parses_keywords_and_anchors() {
        assert!(matches!(
            AnchorOrBoundary::parse("BOF"),
            Ok(AnchorOrBoundary::BeginningOfFile)
        ));
        assert!(matches!(
            AnchorOrBoundary::parse("EOF"),
            Ok(AnchorOrBoundary::EndOfFile)
        ));
        let parsed = AnchorOrBoundary::parse("12ab").unwrap();
        match parsed {
            AnchorOrBoundary::Anchor(a) => assert_eq!(a.line, 12),
            other => panic!("expected Anchor, got {other:?}"),
        }
    }
}
