//! Vectorised searches over UTF-16 code units: what `memchr` and `memmem` are for bytes, which this crate's UTF-16
//! searches could only use over the bytes of the units, looking at every zero high byte of ASCII text and passing
//! over the occurrences at odd offsets. Each kernel is plain Rust over blocks of sixteen units, written so that the
//! compiler makes it into vector compares; on x86-64 a copy compiled for AVX2 is chosen at run time where the
//! processor has it, as `memchr` does.

use std::mem::MaybeUninit;

/// A set of code units as up to eight ranges, each `(lo, hi - lo)`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UnitSet {
    ranges: [(u16, u16); 8],
    n: u8,
    /// The ranges that start in ASCII come first, and this many of them: a block of ASCII units is tested against
    /// those alone.
    na: u8,
}

/// The units looked at one by one before a vector search is called, where a hit is likely to be close.
const PROBE: usize = 8;

impl UnitSet {
    /// The set of the ranges (inclusive), at most eight.
    pub(crate) fn new(ranges: &[(u16, u16)]) -> Option<UnitSet> {
        if ranges.is_empty() || ranges.len() > 8 {
            return None;
        }
        let mut r = [(u16::MAX, 0u16); 8];
        for (slot, &(lo, hi)) in r.iter_mut().zip(ranges) {
            *slot = (lo, hi - lo);
        }
        let na = ranges.iter().take_while(|&&(lo, _)| lo < 0x80).count();
        let na = if ranges[na..].iter().all(|&(lo, _)| lo >= 0x80) { na } else { ranges.len() };
        Some(UnitSet { ranges: r, n: ranges.len() as u8, na: na as u8 })
    }

    /// Whether the set holds a unit.
    #[inline(always)]
    pub(crate) fn contains(&self, u: u16) -> bool {
        self.ranges[..self.n as usize].iter().any(|&(lo, len)| u.wrapping_sub(lo) <= len)
    }

    /// The first unit at or after `from` in the set.
    #[inline]
    pub(crate) fn find(&self, units: &[u16], from: usize) -> Option<usize> {
        let probe = units.get(from..)?.iter().take(PROBE).position(|&u| self.contains(u));
        if let Some(i) = probe {
            return Some(from + i);
        }
        let at = from + PROBE;
        if at >= units.len() {
            return None;
        }
        let na = usize::from(self.na);
        match self.n {
            1 => find_ranges::<1>(units, at, &self.ranges, na),
            2 => find_ranges::<2>(units, at, &self.ranges, na),
            3 => find_ranges::<3>(units, at, &self.ranges, na),
            4 => find_ranges::<4>(units, at, &self.ranges, na),
            5 => find_ranges::<5>(units, at, &self.ranges, na),
            6 => find_ranges::<6>(units, at, &self.ranges, na),
            7 => find_ranges::<7>(units, at, &self.ranges, na),
            _ => find_ranges::<8>(units, at, &self.ranges, na),
        }
    }
}

/// The first unit at or after `from` in the first `N` ranges.
#[inline]
fn find_ranges<const N: usize>(units: &[u16], from: usize, ranges: &[(u16, u16); 8], na: usize) -> Option<usize> {
    let r: [(u16, u16); N] = ranges[..N].try_into().expect("N ranges");
    #[cfg(target_arch = "x86_64")]
    {
        let at = if std::is_x86_feature_detected!("avx2") {
            // SAFETY: the processor has AVX2.
            unsafe { x86::ranges_avx2::<N>(units, from, &r, na) }
        } else {
            // SAFETY: SSE2 is part of x86-64.
            unsafe { x86::ranges_sse2::<N>(units, from, &r, na) }
        };
        match at {
            Ok(found) => return found,
            Err(rest) => return tail(units, rest, |u| hit(&r, u)),
        }
    }
    #[allow(unreachable_code)]
    {
        let _ = na;
        tail(units, from, |u| hit(&r, u))
    }
}

#[inline(always)]
fn hit<const N: usize>(r: &[(u16, u16); N], u: u16) -> bool {
    r.iter().any(|&(lo, len)| u.wrapping_sub(lo) <= len)
}

#[inline(always)]
fn tail(units: &[u16], from: usize, hit: impl Fn(u16) -> bool) -> Option<usize> {
    units.get(from..)?.iter().position(|&u| hit(u)).map(|i| from + i)
}

/// The first `i` at or after `from` with `units[i + ia] == a` and `units[i + ib] == b`, `i` at most `last` (with
/// `last + max(ia, ib) < units.len()`).
#[inline]
fn find_pair(units: &[u16], from: usize, last: usize, a: u16, ia: usize, b: u16, ib: usize) -> Option<usize> {
    #[cfg(target_arch = "x86_64")]
    {
        let at = if std::is_x86_feature_detected!("avx2") {
            // SAFETY: the processor has AVX2.
            unsafe { x86::pair_avx2(units, from, last, a, ia, b, ib) }
        } else {
            // SAFETY: SSE2 is part of x86-64.
            unsafe { x86::pair_sse2(units, from, last, a, ia, b, ib) }
        };
        match at {
            Ok(found) => return found,
            Err(rest) => return (rest..=last).find(|&i| units[i + ia] == a && units[i + ib] == b),
        }
    }
    #[allow(unreachable_code)]
    (from..=last).find(|&i| units[i + ia] == a && units[i + ib] == b)
}

/// The vector kernels. Each returns `Ok` with what it found in the blocks it read, or `Err` with where the blocks
/// ended for the caller to go on one unit at a time.
#[cfg(target_arch = "x86_64")]
mod x86 {
    use std::arch::x86_64::*;

    macro_rules! ranges_kernel {
        ($name:ident, $feature:literal, $vec:ty, $lanes:expr, $set1:ident, $loadu:ident, $sub:ident, $xor:ident,
         $cmpgt:ident, $and:ident, $movemask:ident, $ones:expr) => {
            /// The ranges test: a unit `u` is in `(lo, len)` when `u - lo <= len` unsigned, made a signed compare
            /// by flipping the top bit of both sides.
            #[target_feature(enable = $feature)]
            pub(super) unsafe fn $name<const N: usize>(
                units: &[u16],
                from: usize,
                r: &[(u16, u16); N],
                na: usize,
            ) -> Result<Option<usize>, usize> {
                let bias = $set1(i16::MIN);
                let ascii = $set1((0x7F ^ 0x8000) as i16);
                let lo: [$vec; N] = std::array::from_fn(|k| $set1(r[k].0 as i16));
                let lim: [$vec; N] = std::array::from_fn(|k| $set1((r[k].1 ^ 0x8000) as i16));
                let p = units.as_ptr();
                let mut at = from;
                // The ranges a block misses: all ones where no range holds the unit.
                let miss = |x: $vec| {
                    let mut miss = $ones;
                    // Only where some ranges start beyond ASCII does a block of ASCII units skip them.
                    let wide = if na < N { $movemask($cmpgt($xor(x, bias), ascii)) } else { 1 };
                    for k in 0..N {
                        if k >= na && wide == 0 {
                            break;
                        }
                        let d = $xor($sub(x, lo[k]), bias);
                        miss = $and(miss, $cmpgt(d, lim[k]));
                    }
                    miss
                };
                let full = ((1u64 << (2 * $lanes)) - 1) as u32;
                // Four blocks at a time while nothing is found, as `memchr` reads.
                while at + 4 * $lanes <= units.len() {
                    // SAFETY: the four blocks are within the slice.
                    let (m0, m1, m2, m3) = unsafe {
                        (
                            miss($loadu(p.add(at).cast::<$vec>())),
                            miss($loadu(p.add(at + $lanes).cast::<$vec>())),
                            miss($loadu(p.add(at + 2 * $lanes).cast::<$vec>())),
                            miss($loadu(p.add(at + 3 * $lanes).cast::<$vec>())),
                        )
                    };
                    if $movemask($and($and(m0, m1), $and(m2, m3))) as u32 & full != full {
                        for (k, m) in [m0, m1, m2, m3].into_iter().enumerate() {
                            let m = !($movemask(m) as u32) & full;
                            if m != 0 {
                                return Ok(Some(at + k * $lanes + (m.trailing_zeros() / 2) as usize));
                            }
                        }
                    }
                    at += 4 * $lanes;
                }
                while at + $lanes <= units.len() {
                    // SAFETY: the block is within the slice.
                    let x = unsafe { $loadu(p.add(at).cast::<$vec>()) };
                    let m = !($movemask(miss(x)) as u32) & full;
                    if m != 0 {
                        return Ok(Some(at + (m.trailing_zeros() / 2) as usize));
                    }
                    at += $lanes;
                }
                Err(at)
            }
        };
    }

    macro_rules! pair_kernel {
        ($name:ident, $feature:literal, $vec:ty, $lanes:expr, $set1:ident, $loadu:ident, $cmpeq:ident, $and:ident,
         $movemask:ident) => {
            #[target_feature(enable = $feature)]
            #[allow(clippy::too_many_arguments)]
            pub(super) unsafe fn $name(
                units: &[u16],
                from: usize,
                last: usize,
                a: u16,
                ia: usize,
                b: u16,
                ib: usize,
            ) -> Result<Option<usize>, usize> {
                let (va, vb) = ($set1(a as i16), $set1(b as i16));
                let p = units.as_ptr();
                let mut at = from;
                while at + $lanes <= last + 1 {
                    // SAFETY: at + lanes - 1 <= last, and last + max(ia, ib) is within the slice.
                    let (x, y) =
                        unsafe { ($loadu(p.add(at + ia).cast::<$vec>()), $loadu(p.add(at + ib).cast::<$vec>())) };
                    let m = $movemask($and($cmpeq(x, va), $cmpeq(y, vb))) as u32;
                    if m != 0 {
                        return Ok(Some(at + (m.trailing_zeros() / 2) as usize));
                    }
                    at += $lanes;
                }
                Err(at)
            }
        };
    }

    /// Packs 32 units at a time (see `pack_kernel` and `pack_fold_kernel`): the units packed, and whether any was
    /// beyond ASCII.
    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn pack_avx2(dst: &mut [std::mem::MaybeUninit<u8>], src: &[u16], fold: bool) -> (usize, bool) {
        let n = src.len().min(dst.len());
        let (cap, a, letters, case, high) = (
            _mm256_set1_epi16(0x80),
            _mm256_set1_epi16(b'A' as i16),
            _mm256_set1_epi16(25),
            _mm256_set1_epi16(0x20),
            _mm256_set1_epi16(0xFF80u16 as i16),
        );
        let mut wide = _mm256_setzero_si256();
        let (sp, dp) = (src.as_ptr(), dst.as_mut_ptr());
        let mut k = 0;
        while k + 32 <= n {
            // SAFETY: the 32 units and the 32 bytes are within the slices.
            unsafe {
                let mut x = _mm256_loadu_si256(sp.add(k).cast::<__m256i>());
                let mut y = _mm256_loadu_si256(sp.add(k + 16).cast::<__m256i>());
                wide = _mm256_or_si256(wide, _mm256_and_si256(_mm256_or_si256(x, y), high));
                if fold {
                    // A capital: u - 'A' <= 25 unsigned, so min(u - 'A', 25) == u - 'A'.
                    let (dx, dy) = (_mm256_sub_epi16(x, a), _mm256_sub_epi16(y, a));
                    let ux = _mm256_cmpeq_epi16(_mm256_min_epu16(dx, letters), dx);
                    let uy = _mm256_cmpeq_epi16(_mm256_min_epu16(dy, letters), dy);
                    x = _mm256_or_si256(x, _mm256_and_si256(ux, case));
                    y = _mm256_or_si256(y, _mm256_and_si256(uy, case));
                }
                let packed = _mm256_packus_epi16(_mm256_min_epu16(x, cap), _mm256_min_epu16(y, cap));
                // packus works within each 128-bit half: put the quarters back in order.
                let ordered = _mm256_permute4x64_epi64::<0b11_01_10_00>(packed);
                _mm256_storeu_si256(dp.add(k).cast::<__m256i>(), ordered);
            }
            k += 32;
        }
        (k, _mm256_testz_si256(wide, wide) == 0)
    }

    ranges_kernel!(
        ranges_avx2,
        "avx2",
        __m256i,
        16,
        _mm256_set1_epi16,
        _mm256_loadu_si256,
        _mm256_sub_epi16,
        _mm256_xor_si256,
        _mm256_cmpgt_epi16,
        _mm256_and_si256,
        _mm256_movemask_epi8,
        _mm256_set1_epi8(-1)
    );
    ranges_kernel!(
        ranges_sse2,
        "sse2",
        __m128i,
        8,
        _mm_set1_epi16,
        _mm_loadu_si128,
        _mm_sub_epi16,
        _mm_xor_si128,
        _mm_cmpgt_epi16,
        _mm_and_si128,
        _mm_movemask_epi8,
        _mm_set1_epi8(-1)
    );
    pair_kernel!(
        pair_avx2,
        "avx2",
        __m256i,
        16,
        _mm256_set1_epi16,
        _mm256_loadu_si256,
        _mm256_cmpeq_epi16,
        _mm256_and_si256,
        _mm256_movemask_epi8
    );
    pair_kernel!(
        pair_sse2,
        "sse2",
        __m128i,
        8,
        _mm_set1_epi16,
        _mm_loadu_si128,
        _mm_cmpeq_epi16,
        _mm_and_si128,
        _mm_movemask_epi8
    );
}

macro_rules! dispatch {
    ($(#[$doc:meta])* fn $name:ident<>($($arg:ident: $ty:ty),*) -> $ret:ty = $generic:ident;) => {
        $(#[$doc])*
        #[inline]
        fn $name($($arg: $ty),*) -> $ret {
            #[cfg(target_arch = "x86_64")]
            {
                #[target_feature(enable = "avx2")]
                unsafe fn avx2($($arg: $ty),*) -> $ret {
                    $generic($($arg),*)
                }
                if std::is_x86_feature_detected!("avx2") {
                    // SAFETY: the processor has AVX2.
                    return unsafe { avx2($($arg),*) };
                }
            }
            $generic($($arg),*)
        }
    };
}

dispatch! {
    /// Packs units into bytes: an ASCII unit as itself, any other as 0x80. Whether any unit was beyond ASCII.
    fn pack_into<>(dst: &mut [MaybeUninit<u8>], src: &[u16]) -> bool = pack_kernel;
}

dispatch! {
    /// [`pack_into`] with the ASCII capitals folded to lower case.
    fn pack_fold_into<>(dst: &mut [MaybeUninit<u8>], src: &[u16]) -> bool = pack_fold_kernel;
}

#[inline(always)]
fn pack_kernel(dst: &mut [MaybeUninit<u8>], src: &[u16]) -> bool {
    let mut wide = 0u16;
    for (d, &u) in dst.iter_mut().zip(src) {
        d.write(u.min(0x80) as u8);
        wide |= u & 0xFF80;
    }
    wide != 0
}

#[inline(always)]
fn pack_fold_kernel(dst: &mut [MaybeUninit<u8>], src: &[u16]) -> bool {
    let mut wide = 0u16;
    for (d, &u) in dst.iter_mut().zip(src) {
        let b = u.min(0x80) as u8;
        let upper = u8::from(b.wrapping_sub(b'A') < 26);
        d.write(b | upper << 5);
        wide |= u & 0xFF80;
    }
    wide != 0
}

/// Packs `src` into the start of `dst` (see [`pack_into`]): the packed bytes, and whether any unit was beyond
/// ASCII.
#[inline]
pub(crate) fn pack<'a>(dst: &'a mut [MaybeUninit<u8>], src: &[u16], fold: bool) -> (&'a [u8], bool) {
    let n = src.len().min(dst.len());
    #[cfg(target_arch = "x86_64")]
    let wide = if std::is_x86_feature_detected!("avx2") {
        // SAFETY: the processor has AVX2.
        let (done, wide) = unsafe { x86::pack_avx2(&mut dst[..n], &src[..n], fold) };
        let rest = if fold {
            pack_fold_kernel(&mut dst[done..n], &src[done..n])
        } else {
            pack_kernel(&mut dst[done..n], &src[done..n])
        };
        wide | rest
    } else if fold {
        pack_fold_into(&mut dst[..n], &src[..n])
    } else {
        pack_into(&mut dst[..n], &src[..n])
    };
    #[cfg(not(target_arch = "x86_64"))]
    let wide = if fold { pack_fold_into(&mut dst[..n], &src[..n]) } else { pack_into(&mut dst[..n], &src[..n]) };
    // SAFETY: the first n bytes were written just above.
    (unsafe { std::slice::from_raw_parts(dst.as_ptr().cast::<u8>(), n) }, wide)
}

/// The first `i` at or after `from` where `needle` occurs, found by the two units of it `pick` chooses as rarest.
#[derive(Clone, Debug)]
pub(crate) struct Needle16 {
    needle: Vec<u16>,
    ia: usize,
    ib: usize,
}

impl Needle16 {
    /// A needle, its two rarest units chosen by `rank` (higher is commoner).
    pub(crate) fn new(needle: Vec<u16>, rank: impl Fn(u16) -> u8) -> Needle16 {
        let mut ia = 0;
        for (k, &u) in needle.iter().enumerate() {
            if rank(u) < rank(needle[ia]) {
                ia = k;
            }
        }
        // The second is the rarest of the others, a unit other than the first where there is one.
        let ib = (0..needle.len())
            .filter(|&k| k != ia)
            .min_by_key(|&k| (needle[k] == needle[ia], rank(needle[k]), k))
            .unwrap_or(ia);
        Needle16 { needle, ia, ib }
    }

    pub(crate) fn len(&self) -> usize {
        self.needle.len()
    }

    /// The first occurrence at or after `from`.
    #[inline]
    pub(crate) fn find(&self, units: &[u16], from: usize) -> Option<usize> {
        let n = self.needle.len();
        if n == 1 {
            let u = self.needle[0];
            return UnitSet::new(&[(u, u)]).and_then(|s| s.find(units, from));
        }
        if from + n > units.len() {
            return None;
        }
        let last = units.len() - n;
        let (a, b) = (self.needle[self.ia], self.needle[self.ib]);
        let mut at = from;
        loop {
            let i = find_pair(units, at, last, a, self.ia, b, self.ib)?;
            if units[i..i + n] == self.needle[..] {
                return Some(i);
            }
            at = i + 1;
            if at > last {
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text with ASCII, Latin, Cyrillic, surrogates and the units the searches look for, from a fixed seed.
    fn text(seed: u64, len: usize) -> Vec<u16> {
        let alphabet = [
            b'a' as u16,
            b'b' as u16,
            b'@' as u16,
            b'S' as u16,
            0x80,
            0xE9,
            0x017F,
            0x0416,
            0xD83D,
            0xDE00,
            0x212A,
            0xFFFF,
        ];
        let mut x = seed;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                alphabet[(x % alphabet.len() as u64) as usize]
            })
            .collect()
    }

    #[test]
    fn the_vector_searches_find_what_a_loop_finds() {
        let sets: [&[(u16, u16)]; 4] = [
            &[(b'@' as u16, b'@' as u16)],
            &[(b'a' as u16, b'b' as u16), (0x017F, 0x017F)],
            &[(0x0400, 0x04FF), (0xD800, 0xDBFF), (b'S' as u16, b'S' as u16)],
            &[(1, 1), (2, 2), (3, 3), (0x80, 0x80), (0xE9, 0xE9), (0x212A, 0x212A), (0xFFFE, 0xFFFE)],
        ];
        for seed in 1..40u64 {
            let t = text(seed, (seed as usize * 37) % 300);
            for set in sets {
                let us = UnitSet::new(set).unwrap();
                for from in [0, 1, 7, 9, 33] {
                    let want = (from..t.len()).find(|&i| set.iter().any(|&(lo, hi)| (lo..=hi).contains(&t[i])));
                    assert_eq!(us.find(&t, from), want, "seed {seed} from {from} set {set:?}");
                    #[cfg(target_arch = "x86_64")]
                    {
                        let r: [(u16, u16); 8] =
                            std::array::from_fn(|k| set.get(k).map_or((u16::MAX, 0), |&(lo, hi)| (lo, hi - lo)));
                        if let Ok(found) = unsafe { x86::ranges_sse2::<8>(&t, from.min(t.len()), &r, 8) } {
                            let want8 =
                                (from..t.len()).find(|&i| r.iter().any(|&(lo, len)| t[i].wrapping_sub(lo) <= len));
                            assert_eq!(found, want8, "sse2 seed {seed} from {from}");
                        }
                    }
                }
            }
            for needle in [
                vec![b'a' as u16, b'b' as u16],
                vec![b'S' as u16, 0x017F, b'a' as u16],
                vec![0xD83D, 0xDE00, b'@' as u16],
            ] {
                let n = Needle16::new(needle.clone(), |u| (u & 0x7F) as u8);
                for from in [0, 3, 20] {
                    let want = (from..t.len().saturating_sub(needle.len() - 1)).find(|&i| t[i..].starts_with(&needle));
                    assert_eq!(n.find(&t, from), want, "seed {seed} needle {needle:?}");
                }
                #[cfg(target_arch = "x86_64")]
                if t.len() >= needle.len() {
                    let last = t.len() - needle.len();
                    let (a, b) = (needle[0], needle[needle.len() - 1]);
                    let ib = needle.len() - 1;
                    let want = (0..=last).find(|&i| t[i] == a && t[i + ib] == b);
                    match unsafe { x86::pair_sse2(&t, 0, last, a, 0, b, ib) } {
                        Ok(found) => assert_eq!(found, want, "pair sse2 seed {seed}"),
                        Err(rest) => assert_eq!((rest..=last).find(|&i| t[i] == a && t[i + ib] == b), want),
                    }
                }
            }
            for fold in [false, true] {
                let mut buf = [MaybeUninit::<u8>::uninit(); 512];
                let (packed, wide) = pack(&mut buf, &t, fold);
                let want: Vec<u8> = t
                    .iter()
                    .map(|&u| {
                        let b = u.min(0x80) as u8;
                        if fold { b.to_ascii_lowercase() } else { b }
                    })
                    .collect();
                assert_eq!(packed, &want[..], "pack seed {seed} fold {fold}");
                assert_eq!(wide, t.iter().any(|&u| u >= 0x80));
            }
        }
    }
}
