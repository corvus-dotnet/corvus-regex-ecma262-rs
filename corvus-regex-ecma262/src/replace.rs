//! `String.prototype.replace` and `split` over a match: `GetSubstitution` and `RegExp.prototype[Symbol.split]`.

use std::borrow::Cow;

use super::regex::{Captures, GroupInfo, Haystack, Regex};
use super::units::Text;

type Unit<H> = <<H as Haystack>::Text as Text>::Unit;

/// The value of a slot for a group that did not take part.
const UNSET: usize = usize::MAX;

/// Replaces the first match, or every match with `all`, by the template.
pub(crate) fn replace<'t, H: Haystack + ?Sized>(
    re: &Regex,
    text: &'t H,
    template: &[Unit<H>],
    all: bool,
) -> Cow<'t, H> {
    let input = text.units();
    let info = re.info();
    let mut slots = vec![UNSET; 2 * re.captures_len()];
    let sticky = re.flags().sticky;
    let (mut out, mut last, mut at) = (Vec::new(), 0, 0);
    let mut any = false;
    while let Ok(Some((from, to))) = re.exec::<H::Text>(input, at, sticky, None, Some(&mut slots)) {
        any = true;
        H::Text::push_lossy(&mut out, input, last, from);
        substitute::<H::Text>(&mut out, input, &slots, info, template);
        last = to;
        if !all {
            break;
        }
        at = re.next_start::<H::Text>(input, from, to);
    }
    if !any {
        return Cow::Borrowed(text);
    }
    H::Text::push_lossy(&mut out, input, last, input.len());
    Cow::Owned(H::from_units(out))
}

/// Replaces the first match, or every match with `all`, by what the function returns.
pub(crate) fn replace_with<'t>(
    re: &Regex,
    text: &'t str,
    all: bool,
    mut f: impl FnMut(&Captures<'t>) -> String,
) -> Cow<'t, str> {
    let input = text.as_bytes();
    let sticky = re.flags().sticky;
    let (mut out, mut last, mut at) = (Vec::new(), 0, 0);
    let mut any = false;
    while let Ok(Some(caps)) = re.captures_generic(text, at, sticky, None) {
        any = true;
        let m = caps.get_match();
        <str as Haystack>::Text::push_lossy(&mut out, input, last, m.start());
        out.extend_from_slice(f(&caps).as_bytes());
        last = m.end();
        if !all {
            break;
        }
        at = re.next_start::<<str as Haystack>::Text>(input, m.start(), m.end());
    }
    if !any {
        return Cow::Borrowed(text);
    }
    <str as Haystack>::Text::push_lossy(&mut out, input, last, input.len());
    Cow::Owned(String::from_utf8(out).expect("what the engine writes from UTF-8 is UTF-8"))
}

/// Appends the template with each `$` form replaced (`GetSubstitution`).
fn substitute<T: Text>(
    out: &mut Vec<T::Unit>,
    input: &[T::Unit],
    slots: &[usize],
    info: &GroupInfo,
    template: &[T::Unit],
) {
    let unit = |k: usize| -> u32 { template[k].into() };
    let digit = |k: usize| -> Option<usize> {
        let c = template.get(k).map(|&u| u.into())?;
        (0x30..=0x39).contains(&c).then(|| (c - 0x30) as usize)
    };
    let groups = slots.len() / 2 - 1;
    let (from, to) = (slots[0], slots[1]);
    let group = |out: &mut Vec<T::Unit>, g: usize| {
        if slots[2 * g] != UNSET {
            T::push_lossy(out, input, slots[2 * g], slots[2 * g + 1]);
        }
    };
    let mut k = 0;
    while k < template.len() {
        let c = unit(k);
        if c != u32::from(b'$') || k + 1 >= template.len() {
            out.push(template[k]);
            k += 1;
            continue;
        }
        let next = unit(k + 1);
        match u8::try_from(next).unwrap_or(0) {
            b'$' => {
                out.push(template[k]);
                k += 2;
            }
            b'&' => {
                T::push_lossy(out, input, from, to);
                k += 2;
            }
            b'`' => {
                T::push_lossy(out, input, 0, from);
                k += 2;
            }
            b'\'' => {
                T::push_lossy(out, input, to, input.len());
                k += 2;
            }
            b'0'..=b'9' => {
                let d1 = digit(k + 1).expect("a digit");
                if let Some(d2) = digit(k + 2)
                    && (1..=groups).contains(&(d1 * 10 + d2))
                {
                    group(out, d1 * 10 + d2);
                    k += 3;
                } else if (1..=groups).contains(&d1) {
                    group(out, d1);
                    k += 2;
                } else {
                    out.push(template[k]);
                    k += 1;
                }
            }
            b'<' => {
                // Without a named group `$<` is itself. With one, the name runs to the next `>`; a name that is no
                // group's is the empty string, and a `<` with no `>` after it is itself.
                let close = template[k + 2..].iter().position(|&u| u.into() == u32::from(b'>'));
                match close {
                    Some(n) if !info.names.is_empty() => {
                        let name = &template[k + 2..k + 2 + n];
                        if let Some(groups) = info.named(name)
                            && let Some(&g) = groups.iter().find(|&&g| slots[2 * g] != UNSET)
                        {
                            group(out, g);
                        }
                        k += n + 3;
                    }
                    _ => {
                        out.push(template[k]);
                        k += 1;
                    }
                }
            }
            _ => {
                out.push(template[k]);
                k += 1;
            }
        }
    }
}

/// Splits the text around the matches (`RegExp.prototype[Symbol.split]`), with at most `limit` items. The flags g
/// and y do not apply: split searches with a copy of the pattern that is sticky, from each position in turn, which
/// is what a search from the position finds first.
pub(crate) fn split<'t, H: Haystack + ?Sized>(re: &Regex, text: &'t H, limit: usize) -> Vec<Option<Cow<'t, H>>> {
    let input = text.units();
    let size = input.len();
    let mut out = Vec::new();
    if limit == 0 {
        return out;
    }
    let mut slots = vec![UNSET; 2 * re.captures_len()];
    if size == 0 {
        if !matches!(re.exec::<H::Text>(input, 0, true, None, Some(&mut slots)), Ok(Some(_))) {
            out.push(Some(text.piece(0, 0)));
        }
        return out;
    }
    let (mut p, mut q) = (0, 0);
    while q < size {
        let Ok(Some((from, to))) = re.exec::<H::Text>(input, q, false, None, Some(&mut slots)) else { break };
        if from >= size {
            break;
        }
        let e = to.min(size);
        if e == p {
            q = H::Text::advance(input, from, re.flags().unicode || re.flags().unicode_sets);
            continue;
        }
        out.push(Some(text.piece(p, from)));
        if out.len() == limit {
            return out;
        }
        p = e;
        for g in 1..slots.len() / 2 {
            out.push((slots[2 * g] != UNSET).then(|| text.piece(slots[2 * g], slots[2 * g + 1])));
            if out.len() == limit {
                return out;
            }
        }
        q = p;
    }
    out.push(Some(text.piece(p, size)));
    out
}
