//! Reading UTF-8, and matching by UTF-16 code unit over UTF-8 text (`units.go`, and what the Go source takes from
//! the `unicode/utf8` package).
//!
//! ECMA-262 matches a pattern that has no u flag a code unit at a time. The engine reads a pattern without the u
//! flag only when the u flag grammar refuses it, and such a pattern is matched that way. A character of one, two or
//! three bytes of UTF-8 is one code unit. A character of four bytes is two, a high surrogate and a low one, so the
//! matcher has to be able to stand between them. That position is written as the offset of the character's third
//! byte. Each half then spans two bytes, every position is still an offset into the text, a group is still a pair
//! of offsets, and no UTF-16 copy of the text is made.
//!
//! The third byte of a character is a continuation byte, which is no position at all when matching by code point,
//! so the two readings cannot be confused. Only a text with a character of four bytes differs between the two, and
//! the instructions that compare bytes (a literal run, a word boundary, a line terminator) need no change: none of
//! them can hold at the position between the halves, and all of them are right at every other.

use super::Rune;

/// What a byte that is not UTF-8 decodes to, one byte at a time.
pub(crate) const RUNE_ERROR: Rune = 0xFFFD;
/// The first byte value that is not a character by itself.
pub(crate) const RUNE_SELF: u8 = 0x80;

#[inline(always)]
fn continuation(b: u8) -> bool {
    b & 0xC0 == 0x80
}

/// The code point at the start of `p` and its width in bytes: `(RUNE_ERROR, 1)` for bytes that are not UTF-8, and
/// `(RUNE_ERROR, 0)` for no bytes. (`utf8.DecodeRune`.)
#[inline]
pub(crate) fn decode_rune(p: &[u8]) -> (Rune, usize) {
    let Some(&p0) = p.first() else { return (RUNE_ERROR, 0) };
    if p0 < RUNE_SELF {
        return (p0 as Rune, 1);
    }
    match p0 {
        0xC2..=0xDF if p.len() >= 2 && continuation(p[1]) => {
            return (((p0 & 0x1F) as Rune) << 6 | (p[1] & 0x3F) as Rune, 2);
        }
        0xE0..=0xEF if p.len() >= 3 && continuation(p[2]) => {
            let (lo, hi) = match p0 {
                0xE0 => (0xA0, 0xBF),
                0xED => (0x80, 0x9F),
                _ => (0x80, 0xBF),
            };
            if p[1] >= lo && p[1] <= hi {
                return (((p0 & 0x0F) as Rune) << 12 | ((p[1] & 0x3F) as Rune) << 6 | (p[2] & 0x3F) as Rune, 3);
            }
        }
        0xF0..=0xF4 if p.len() >= 4 && continuation(p[2]) && continuation(p[3]) => {
            let (lo, hi) = match p0 {
                0xF0 => (0x90, 0xBF),
                0xF4 => (0x80, 0x8F),
                _ => (0x80, 0xBF),
            };
            if p[1] >= lo && p[1] <= hi {
                return (
                    ((p0 & 0x07) as Rune) << 18
                        | ((p[1] & 0x3F) as Rune) << 12
                        | ((p[2] & 0x3F) as Rune) << 6
                        | (p[3] & 0x3F) as Rune,
                    4,
                );
            }
        }
        _ => {}
    }
    (RUNE_ERROR, 1)
}

/// The code point at the end of `p` and its width in bytes. (`utf8.DecodeLastRune`.)
#[inline]
pub(crate) fn decode_last_rune(p: &[u8]) -> (Rune, usize) {
    let end = p.len();
    if end == 0 {
        return (RUNE_ERROR, 0);
    }
    let last = p[end - 1];
    if last < RUNE_SELF {
        return (last as Rune, 1);
    }
    // A character is four bytes at most, so its first byte is one of the last four.
    let lim = end.saturating_sub(4);
    let mut start = end - 1;
    while start > lim {
        start -= 1;
        if !continuation(p[start]) {
            break;
        }
    }
    let (r, size) = decode_rune(&p[start..end]);
    if start + size != end {
        return (RUNE_ERROR, 1);
    }
    (r, size)
}

/// The two UTF-16 code units of a code point beyond the Basic Multilingual Plane.
#[inline]
pub(crate) fn high_surrogate(c: Rune) -> Rune {
    0xD800 + ((c - 0x10000) >> 10)
}

#[inline]
pub(crate) fn low_surrogate(c: Rune) -> Rune {
    0xDC00 + ((c - 0x10000) & 0x3FF)
}

/// Whether the text has a character beyond the Basic Multilingual Plane, which is the only kind of text that code
/// units and code points read differently. Such a character starts with a byte of 0xF0 or more.
#[inline]
#[cfg(test)]
pub(crate) fn has_supplementary(input: &[u8]) -> bool {
    input.iter().any(|&c| c >= 0xF0)
}

/// The code unit at the position and the number of bytes it spans.
#[inline]
pub(crate) fn unit_at(input: &[u8], pos: usize) -> (Rune, usize) {
    let c = input[pos];
    if c >= 0xF0 {
        let (r, w) = decode_rune(&input[pos..]);
        if w == 4 {
            return (high_surrogate(r), 2);
        }
        return (RUNE_ERROR, 1);
    }
    if (0x80..0xC0).contains(&c) && pos >= 2 && input[pos - 2] >= 0xF0 {
        // Between the halves of the character that starts two bytes back.
        let (r, w) = decode_rune(&input[pos - 2..]);
        if w == 4 {
            return (low_surrogate(r), 2);
        }
    }
    decode_rune(&input[pos..])
}

/// The code unit that ends at the position and the number of bytes it spans.
#[inline]
pub(crate) fn unit_before(input: &[u8], pos: usize) -> (Rune, usize) {
    let (r, w) = decode_last_rune(&input[..pos]);
    if w == 4 {
        return (low_surrogate(r), 2);
    }
    if w == 1 && r == RUNE_ERROR && pos >= 2 && input.len() - pos >= 2 && input[pos - 2] >= 0xF0 {
        // Between the halves of the character that starts two bytes back.
        let (r, w) = decode_rune(&input[pos - 2..]);
        if w == 4 {
            return (high_surrogate(r), 2);
        }
    }
    (r, w)
}

/// A text the matcher reads: UTF-8 bytes ([`Utf8`]), or UTF-16 code units ([`Utf16`]). Every method of the matcher
/// is generic over it, so the compiler makes one matcher for each and there is no branch on the encoding in the
/// loop. A position is an offset in the units of the text. Over UTF-8 the position between the halves of a
/// character beyond the Basic Multilingual Plane is the offset of its third byte (see the top of this file); over
/// UTF-16 every unit is a position, and with the u flag a position inside a surrogate pair is not one the matcher
/// stands at.
///
/// It is public only so that the public `Haystack` trait can name it; the module that holds it is private.
pub trait Text: Copy + 'static {
    type Unit: Copy + Eq + Ord + Into<u32> + std::fmt::Debug + 'static;
    /// Whether a unit is a byte of UTF-8.
    const UTF8: bool;

    /// The code point at the position and its width in units: with the u flag a surrogate pair is one code point
    /// and a lone surrogate is a code point of its own.
    fn decode(input: &[Self::Unit], pos: usize) -> (Rune, usize);
    /// The code point that ends at the position, and its width.
    fn decode_last(input: &[Self::Unit], pos: usize) -> (Rune, usize);
    /// The UTF-16 code unit at the position and its width in units.
    fn unit_at(input: &[Self::Unit], pos: usize) -> (Rune, usize);
    /// The UTF-16 code unit that ends at the position and its width.
    fn unit_before(input: &[Self::Unit], pos: usize) -> (Rune, usize);
    /// The width of the last character of `input[from..pos]`, read as it was read going forward from `from`.
    fn give_back(input: &[Self::Unit], from: usize, pos: usize, units: bool) -> usize;
    fn line_terminator_at(input: &[Self::Unit], pos: usize) -> bool;
    fn line_terminator_before(input: &[Self::Unit], pos: usize) -> bool;
    /// Of a value kept in both encodings (a literal in UTF-8 and in UTF-16), the one in this encoding.
    fn pick<'a>(utf8: &'a [u8], utf16: &'a [u16]) -> &'a [Self::Unit];
    /// Whether the units `input[from..to]`, compared equal to a group's, are the same code points: false only when
    /// the comparison ends inside a surrogate pair of the text, which the u flag reads as one code point.
    fn whole(input: &[Self::Unit], from: usize, to: usize) -> bool;
    /// Appends `input[from..to]` to `out`. Over UTF-8 a bound between the halves of a character (which only a
    /// pattern without the u flag can make) cannot be written as UTF-8, and the half character is written as
    /// U+FFFD, as a lossy conversion of the UTF-16 text would write it; so the result is UTF-8 when the input is.
    fn push_lossy(out: &mut Vec<Self::Unit>, input: &[Self::Unit], from: usize, to: usize);
    /// The position a search from `start` begins at. With the u flag a position inside a character means the
    /// character (RegExpBuiltinExec reads the text as code points); without it every code unit is a position,
    /// which over UTF-8 is a character's first byte or the third byte of a character of four.
    fn normalize(input: &[Self::Unit], start: usize, unicode: bool) -> usize;
    /// The position after the character at `pos` (AdvanceStringIndex): a code point with the u flag and a code unit
    /// without it.
    fn advance(input: &[Self::Unit], pos: usize, unicode: bool) -> usize;
    /// The units of a UTF-8 string in this encoding.
    fn encode(s: &str) -> Vec<Self::Unit>;
    /// The bytes of the text, as the searches for literals read them (the native bytes of each UTF-16 code unit).
    fn bytes(input: &[Self::Unit]) -> &[u8];
    /// The code units of UTF-16 text (empty for UTF-8).
    fn units16(input: &[Self::Unit]) -> &[u16];

    /// The character at the position: a code unit with `units` set, and a code point otherwise.
    #[inline(always)]
    fn read(input: &[Self::Unit], pos: usize, units: bool) -> (Rune, usize) {
        if units { Self::unit_at(input, pos) } else { Self::decode(input, pos) }
    }

    /// [`Text::read`] for the character that ends at the position.
    #[inline(always)]
    fn read_last(input: &[Self::Unit], pos: usize, units: bool) -> (Rune, usize) {
        if units { Self::unit_before(input, pos) } else { Self::decode_last(input, pos) }
    }
}

/// UTF-8 text: the bytes of a `str`, or bytes read a byte at a time as U+FFFD where they are not UTF-8.
#[derive(Clone, Copy)]
pub struct Utf8;

/// UTF-16 text, which may hold lone surrogates.
#[derive(Clone, Copy)]
pub struct Utf16;

impl Text for Utf8 {
    type Unit = u8;
    const UTF8: bool = true;

    #[inline(always)]
    fn decode(input: &[u8], pos: usize) -> (Rune, usize) {
        decode_rune(&input[pos..])
    }

    #[inline(always)]
    fn decode_last(input: &[u8], pos: usize) -> (Rune, usize) {
        decode_last_rune(&input[..pos])
    }

    #[inline(always)]
    fn unit_at(input: &[u8], pos: usize) -> (Rune, usize) {
        unit_at(input, pos)
    }

    #[inline(always)]
    fn unit_before(input: &[u8], pos: usize) -> (Rune, usize) {
        unit_before(input, pos)
    }

    #[inline(always)]
    fn give_back(input: &[u8], from: usize, pos: usize, units: bool) -> usize {
        if input[pos - 1] < RUNE_SELF {
            1
        } else if units {
            unit_before(input, pos).1
        } else {
            decode_last_rune(&input[from..pos]).1
        }
    }

    #[inline]
    fn line_terminator_at(input: &[u8], pos: usize) -> bool {
        match input[pos] {
            b'\n' | b'\r' => true,
            0xE2 => {
                pos + 2 < input.len() && input[pos + 1] == 0x80 && (input[pos + 2] == 0xA8 || input[pos + 2] == 0xA9)
            }
            _ => false,
        }
    }

    /// U+2028 and U+2029 are E2 80 A8 and E2 80 A9.
    #[inline]
    fn line_terminator_before(input: &[u8], pos: usize) -> bool {
        match input[pos - 1] {
            b'\n' | b'\r' => true,
            0xA8 | 0xA9 => pos >= 3 && input[pos - 3] == 0xE2 && input[pos - 2] == 0x80,
            _ => false,
        }
    }

    #[inline(always)]
    fn pick<'a>(utf8: &'a [u8], _: &'a [u16]) -> &'a [u8] {
        utf8
    }

    #[inline(always)]
    fn whole(_: &[u8], _: usize, _: usize) -> bool {
        true
    }

    fn push_lossy(out: &mut Vec<u8>, input: &[u8], mut from: usize, mut to: usize) {
        if from >= to {
            return;
        }
        if inside_four(input, from) {
            out.extend_from_slice("\u{FFFD}".as_bytes());
            from += 2;
        }
        let tail = to < input.len() && inside_four(input, to);
        if tail {
            to -= 2;
        }
        if from < to {
            out.extend_from_slice(&input[from..to]);
        }
        if tail {
            out.extend_from_slice("\u{FFFD}".as_bytes());
        }
    }

    fn normalize(input: &[u8], start: usize, unicode: bool) -> usize {
        if start == 0 || start >= input.len() || !continuation(input[start]) {
            return start;
        }
        let mut lead = start;
        while lead > 0 && continuation(input[lead]) {
            lead -= 1;
        }
        if !unicode && input[lead] >= 0xF0 && start == lead + 2 {
            return start;
        }
        lead
    }

    fn advance(input: &[u8], pos: usize, unicode: bool) -> usize {
        if pos >= input.len() {
            return pos + 1;
        }
        if unicode { pos + decode_rune(&input[pos..]).1 } else { pos + unit_at(input, pos).1 }
    }

    fn encode(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    #[inline(always)]
    fn bytes(input: &[u8]) -> &[u8] {
        input
    }

    #[inline(always)]
    fn units16(_: &[u8]) -> &[u16] {
        &[]
    }
}

/// Whether the position is the one between the halves of a character of four bytes: its third byte.
#[inline]
fn inside_four(input: &[u8], pos: usize) -> bool {
    pos >= 2
        && pos < input.len()
        && continuation(input[pos])
        && input[pos - 2] >= 0xF0
        && decode_rune(&input[pos - 2..]).1 == 4
}

#[inline(always)]
fn high(u: u16) -> bool {
    (0xD800..=0xDBFF).contains(&u)
}

#[inline(always)]
fn low(u: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&u)
}

impl Text for Utf16 {
    type Unit = u16;
    const UTF8: bool = false;

    #[inline(always)]
    fn decode(input: &[u16], pos: usize) -> (Rune, usize) {
        let u = input[pos];
        if high(u) && pos + 1 < input.len() && low(input[pos + 1]) {
            return (0x10000 + (((u as Rune) - 0xD800) << 10) + (input[pos + 1] as Rune - 0xDC00), 2);
        }
        (u as Rune, 1)
    }

    #[inline(always)]
    fn decode_last(input: &[u16], pos: usize) -> (Rune, usize) {
        let u = input[pos - 1];
        if low(u) && pos >= 2 && high(input[pos - 2]) {
            return (0x10000 + (((input[pos - 2] as Rune) - 0xD800) << 10) + (u as Rune - 0xDC00), 2);
        }
        (u as Rune, 1)
    }

    #[inline(always)]
    fn unit_at(input: &[u16], pos: usize) -> (Rune, usize) {
        (input[pos] as Rune, 1)
    }

    #[inline(always)]
    fn unit_before(input: &[u16], pos: usize) -> (Rune, usize) {
        (input[pos - 1] as Rune, 1)
    }

    #[inline(always)]
    fn give_back(input: &[u16], from: usize, pos: usize, units: bool) -> usize {
        if !units && low(input[pos - 1]) && pos >= from + 2 && high(input[pos - 2]) { 2 } else { 1 }
    }

    #[inline]
    fn line_terminator_at(input: &[u16], pos: usize) -> bool {
        matches!(input[pos], 0x0A | 0x0D | 0x2028 | 0x2029)
    }

    #[inline]
    fn line_terminator_before(input: &[u16], pos: usize) -> bool {
        matches!(input[pos - 1], 0x0A | 0x0D | 0x2028 | 0x2029)
    }

    #[inline(always)]
    fn pick<'a>(_: &'a [u8], utf16: &'a [u16]) -> &'a [u16] {
        utf16
    }

    #[inline(always)]
    fn whole(input: &[u16], from: usize, to: usize) -> bool {
        let starts_inside = from > 0 && from < input.len() && low(input[from]) && high(input[from - 1]);
        let ends_inside = to > from && to < input.len() && high(input[to - 1]) && low(input[to]);
        !(starts_inside || ends_inside)
    }

    fn push_lossy(out: &mut Vec<u16>, input: &[u16], from: usize, to: usize) {
        if from < to {
            out.extend_from_slice(&input[from..to]);
        }
    }

    fn normalize(input: &[u16], start: usize, unicode: bool) -> usize {
        if unicode && start > 0 && start < input.len() && low(input[start]) && high(input[start - 1]) {
            return start - 1;
        }
        start
    }

    fn advance(input: &[u16], pos: usize, unicode: bool) -> usize {
        if pos >= input.len() {
            return pos + 1;
        }
        if unicode { pos + Self::decode(input, pos).1 } else { pos + 1 }
    }

    fn encode(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[inline(always)]
    fn bytes(input: &[u16]) -> &[u8] {
        // SAFETY: a u16 is two initialised bytes with no padding, and a u8 has no alignment to keep.
        unsafe { std::slice::from_raw_parts(input.as_ptr().cast::<u8>(), 2 * input.len()) }
    }

    #[inline(always)]
    fn units16(input: &[u16]) -> &[u16] {
        input
    }
}

/// Whether a code unit is one of the 63 ASCII word characters.
#[inline(always)]
pub(crate) fn is_word_unit(c: u32) -> bool {
    c < 0x80 && super::charset::is_word_byte(c as u8)
}
