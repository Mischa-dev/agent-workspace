//! Excalidraw's stacking order: every element carries a fractional index, a
//! base-62 key that sorts as plain ASCII. This is the `fractional-indexing`
//! algorithm Excalidraw uses (generateKeyBetween), so keys the server makes
//! for agents' elements sit correctly among keys browsers made.

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const SMALLEST_INTEGER: &str = "A00000000000000000000000000";

fn digit(value: u8) -> Option<usize> {
    DIGITS.iter().position(|candidate| *candidate == value)
}

fn integer_length(head: u8) -> Option<usize> {
    match head {
        b'a'..=b'z' => Some(usize::from(head - b'a') + 2),
        b'A'..=b'Z' => Some(usize::from(b'Z' - head) + 2),
        _ => None,
    }
}

fn integer_part(key: &str) -> Option<&str> {
    let length = integer_length(*key.as_bytes().first()?)?;
    (length <= key.len()).then(|| &key[..length])
}

/// Whether a key is one Excalidraw would accept.
pub fn is_valid(key: &str) -> bool {
    if key == SMALLEST_INTEGER || !key.bytes().all(|byte| digit(byte).is_some()) {
        return false;
    }
    let Some(integer) = integer_part(key) else {
        return false;
    };
    !key[integer.len()..].ends_with('0')
}

/// A key strictly between `a` and `b` in the fractional part, where `b` may
/// be absent (no upper bound). Both have no trailing zeros.
fn midpoint(a: &[u8], b: Option<&[u8]>) -> Vec<u8> {
    if let Some(b) = b {
        let mut shared = 0;
        while shared < b.len() && a.get(shared).copied().unwrap_or(b'0') == b[shared] {
            shared += 1;
        }
        if shared > 0 {
            let mut key = b[..shared].to_vec();
            let rest_a = if shared < a.len() {
                &a[shared..]
            } else {
                &[][..]
            };
            key.extend(midpoint(rest_a, Some(&b[shared..])));
            return key;
        }
    }
    let digit_a = a.first().and_then(|byte| digit(*byte)).unwrap_or(0);
    let digit_b = b
        .and_then(|b| b.first())
        .and_then(|byte| digit(*byte))
        .unwrap_or(DIGITS.len());
    if digit_b - digit_a > 1 {
        let middle = (digit_a + digit_b).div_ceil(2);
        vec![DIGITS[middle]]
    } else if let Some(b) = b.filter(|b| b.len() > 1) {
        vec![b[0]]
    } else {
        let mut key = vec![DIGITS[digit_a]];
        let rest = if a.len() > 1 { &a[1..] } else { &[][..] };
        key.extend(midpoint(rest, None));
        key
    }
}

fn increment_integer(integer: &str) -> Option<String> {
    let bytes = integer.as_bytes();
    let head = bytes[0];
    let mut digits = bytes[1..].to_vec();
    let mut carry = true;
    for position in (0..digits.len()).rev() {
        if !carry {
            break;
        }
        let next = digit(digits[position])? + 1;
        if next == DIGITS.len() {
            digits[position] = b'0';
        } else {
            digits[position] = DIGITS[next];
            carry = false;
        }
    }
    if !carry {
        let mut key = vec![head];
        key.extend(digits);
        return String::from_utf8(key).ok();
    }
    match head {
        b'Z' => Some("a0".to_owned()),
        b'z' => None,
        _ => {
            let next_head = head + 1;
            if next_head > b'a' {
                digits.push(b'0');
            } else {
                digits.pop();
            }
            let mut key = vec![next_head];
            key.extend(digits);
            String::from_utf8(key).ok()
        }
    }
}

fn decrement_integer(integer: &str) -> Option<String> {
    let bytes = integer.as_bytes();
    let head = bytes[0];
    let mut digits = bytes[1..].to_vec();
    let last = *DIGITS.last()?;
    let mut borrow = true;
    for position in (0..digits.len()).rev() {
        if !borrow {
            break;
        }
        let current = digit(digits[position])?;
        if current == 0 {
            digits[position] = last;
        } else {
            digits[position] = DIGITS[current - 1];
            borrow = false;
        }
    }
    if !borrow {
        let mut key = vec![head];
        key.extend(digits);
        return String::from_utf8(key).ok();
    }
    match head {
        b'a' => Some(format!("Z{}", last as char)),
        b'A' => None,
        _ => {
            let previous_head = head - 1;
            if previous_head < b'Z' {
                digits.push(last);
            } else {
                digits.pop();
            }
            let mut key = vec![previous_head];
            key.extend(digits);
            String::from_utf8(key).ok()
        }
    }
}

/// A key that sorts after `a` and before `b`. Either end may be open. Returns
/// None for invalid or out-of-order bounds.
pub fn key_between(a: Option<&str>, b: Option<&str>) -> Option<String> {
    if a.is_some_and(|key| !is_valid(key)) || b.is_some_and(|key| !is_valid(key)) {
        return None;
    }
    match (a, b) {
        (Some(a), Some(b)) if a >= b => None,
        (None, None) => Some("a0".to_owned()),
        (None, Some(b)) => {
            let integer = integer_part(b)?;
            let fraction = &b[integer.len()..];
            if integer == SMALLEST_INTEGER {
                let mut key = integer.as_bytes().to_vec();
                key.extend(midpoint(&[], Some(fraction.as_bytes())));
                return String::from_utf8(key).ok();
            }
            if integer.len() < b.len() {
                return Some(integer.to_owned());
            }
            decrement_integer(integer)
        }
        (Some(a), None) => {
            let integer = integer_part(a)?;
            let fraction = &a[integer.len()..];
            match increment_integer(integer) {
                Some(next) => Some(next),
                None => {
                    let mut key = integer.as_bytes().to_vec();
                    key.extend(midpoint(fraction.as_bytes(), None));
                    String::from_utf8(key).ok()
                }
            }
        }
        (Some(a), Some(b)) => {
            let integer_a = integer_part(a)?;
            let fraction_a = &a[integer_a.len()..];
            let integer_b = integer_part(b)?;
            let fraction_b = &b[integer_b.len()..];
            if integer_a == integer_b {
                let mut key = integer_a.as_bytes().to_vec();
                key.extend(midpoint(fraction_a.as_bytes(), Some(fraction_b.as_bytes())));
                return String::from_utf8(key).ok();
            }
            let next = increment_integer(integer_a)?;
            if next.as_str() < b {
                return Some(next);
            }
            let mut key = integer_a.as_bytes().to_vec();
            key.extend(midpoint(fraction_a.as_bytes(), None));
            String::from_utf8(key).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_valid, key_between};

    #[test]
    fn matches_the_reference_implementation() {
        // Cases from the fractional-indexing package's own tests.
        assert_eq!(key_between(None, None).as_deref(), Some("a0"));
        assert_eq!(key_between(None, Some("a0")).as_deref(), Some("Zz"));
        assert_eq!(key_between(None, Some("Zz")).as_deref(), Some("Zy"));
        assert_eq!(key_between(Some("a0"), None).as_deref(), Some("a1"));
        assert_eq!(key_between(Some("a1"), None).as_deref(), Some("a2"));
        assert_eq!(key_between(Some("a0"), Some("a1")).as_deref(), Some("a0V"));
        assert_eq!(key_between(Some("a1"), Some("a2")).as_deref(), Some("a1V"));
        assert_eq!(key_between(Some("a0V"), Some("a1")).as_deref(), Some("a0l"));
        assert_eq!(key_between(Some("Zz"), Some("a0")).as_deref(), Some("ZzV"));
        assert_eq!(key_between(Some("Zz"), Some("a1")).as_deref(), Some("a0"));
        assert_eq!(key_between(None, Some("Y00")).as_deref(), Some("Xzzz"));
        assert_eq!(key_between(Some("bzz"), None).as_deref(), Some("c000"));
        assert_eq!(key_between(Some("a0"), Some("a0V")).as_deref(), Some("a0G"));
        assert_eq!(key_between(Some("a0"), Some("a0G")).as_deref(), Some("a08"));
        assert_eq!(
            key_between(Some("b125"), Some("b129")).as_deref(),
            Some("b127")
        );
        assert_eq!(key_between(Some("a0"), Some("a1V")).as_deref(), Some("a1"));
        assert_eq!(key_between(Some("Zz"), Some("a01")).as_deref(), Some("a0"));
        assert_eq!(key_between(None, Some("a0V")).as_deref(), Some("a0"));
        assert_eq!(key_between(None, Some("b999")).as_deref(), Some("b99"));
        assert_eq!(key_between(Some("a1"), Some("a0")), None);
        assert_eq!(key_between(Some("a00"), None), None);
        assert!(!is_valid("A00000000000000000000000000"));
        assert!(is_valid("a0"));
        assert!(!is_valid("a0 "));
    }

    #[test]
    fn many_keys_after_each_other_stay_ordered() {
        let mut key = key_between(None, None).expect("first");
        for _ in 0..500 {
            let next = key_between(Some(&key), None).expect("next");
            assert!(next > key);
            key = next;
        }
        let mut low = "a0".to_owned();
        let high = "a1".to_owned();
        for _ in 0..60 {
            let middle = key_between(Some(&low), Some(&high)).expect("between");
            assert!(middle > low && middle < high, "{low} < {middle} < {high}");
            low = middle;
        }
    }
}
