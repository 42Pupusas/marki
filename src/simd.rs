//! SIMD-accelerated byte scanning for the inline and block parsers.
//!
//! On x86/x86\_64 with SSE2 (baseline for all x86\_64), processes 16 bytes per
//! iteration. Falls back to a scalar loop on other architectures.

/// Find the first byte in `haystack[offset..]` that matches any byte in `needles`.
/// Returns the absolute index into `haystack`, or `None` if not found.
///
/// `needles` must contain 1..=8 distinct bytes (unused slots should duplicate
/// an existing needle — the implementation always checks all 8 lanes).
#[inline]
pub fn find_byte_set(haystack: &[u8], offset: usize, needles: &ByteSet) -> Option<usize> {
    if offset >= haystack.len() {
        return None;
    }
    #[cfg(target_arch = "x86_64")]
    {
        // SAFETY: SSE2 is baseline on all x86_64 processors.
        unsafe { find_byte_set_sse2(haystack, offset, needles) }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        find_byte_set_scalar(haystack, offset, needles)
    }
}

/// Find the first occurrence of a single byte in `haystack[offset..]`.
#[inline]
pub fn find_byte(haystack: &[u8], offset: usize, needle: u8) -> Option<usize> {
    if offset >= haystack.len() {
        return None;
    }
    #[cfg(target_arch = "x86_64")]
    {
        unsafe { find_byte_sse2(haystack, offset, needle) }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        memchr_scalar(haystack, offset, needle)
    }
}

/// A pre-computed set of bytes to search for, stored in a format ready for SIMD.
pub struct ByteSet {
    /// The needle bytes. Unused slots duplicate `bytes[0]`.
    bytes: [u8; 8],
    /// Scalar fallback lookup table.
    table: [bool; 256],
}

impl ByteSet {
    /// Create a new `ByteSet` from a slice of distinct bytes (max 8).
    #[inline]
    pub const fn new(needles: &[u8]) -> Self {
        assert!(!needles.is_empty() && needles.len() <= 8);
        let mut bytes = [needles[0]; 8];
        let mut table = [false; 256];
        let mut i = 0;
        while i < needles.len() {
            bytes[i] = needles[i];
            table[needles[i] as usize] = true;
            i += 1;
        }
        Self { bytes, table }
    }
}

// ---------------------------------------------------------------------------
// SSE2 implementation (x86_64 baseline)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{
    __m128i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_or_si128, _mm_set1_epi8,
};

/// Reinterpret a `u8` as `i8` (bit-preserving cast for SSE2 intrinsics).
#[cfg(target_arch = "x86_64")]
#[allow(clippy::inline_always)]
#[inline(always)]
const fn as_i8(b: u8) -> i8 {
    i8::from_ne_bytes([b])
}

/// Extract the low 16 bits of a movemask result as a `u32` for `trailing_zeros`.
/// `_mm_movemask_epi8` returns an `i32` with only bits 0..15 set, so the
/// bitwise AND is lossless.
#[cfg(target_arch = "x86_64")]
#[allow(clippy::inline_always)]
#[inline(always)]
const fn movemask_to_u32(mask: i32) -> u32 {
    // Movemask returns 0..=0xFFFF. Reinterpret the low two bytes as u16,
    // then widen losslessly. No sign or truncation issues.
    let [lo, hi, _, _] = mask.to_ne_bytes();
    u16::from_ne_bytes([lo, hi]) as u32
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
// _mm_loadu_si128 is an unaligned load — the pointer alignment cast is intentional.
#[allow(clippy::cast_ptr_alignment)]
unsafe fn find_byte_set_sse2(haystack: &[u8], offset: usize, set: &ByteSet) -> Option<usize> {
    let bytes = &haystack[offset..];
    let len = bytes.len();
    let ptr = bytes.as_ptr();

    unsafe {
        // Load all needle lanes.
        let n0 = _mm_set1_epi8(as_i8(set.bytes[0]));
        let n1 = _mm_set1_epi8(as_i8(set.bytes[1]));
        let n2 = _mm_set1_epi8(as_i8(set.bytes[2]));
        let n3 = _mm_set1_epi8(as_i8(set.bytes[3]));
        let n4 = _mm_set1_epi8(as_i8(set.bytes[4]));
        let n5 = _mm_set1_epi8(as_i8(set.bytes[5]));
        let n6 = _mm_set1_epi8(as_i8(set.bytes[6]));
        let n7 = _mm_set1_epi8(as_i8(set.bytes[7]));

        let mut i = 0;

        // Process 16-byte chunks.
        while i + 16 <= len {
            let chunk = _mm_loadu_si128(ptr.add(i).cast::<__m128i>());
            let eq = _mm_or_si128(
                _mm_or_si128(
                    _mm_or_si128(_mm_cmpeq_epi8(chunk, n0), _mm_cmpeq_epi8(chunk, n1)),
                    _mm_or_si128(_mm_cmpeq_epi8(chunk, n2), _mm_cmpeq_epi8(chunk, n3)),
                ),
                _mm_or_si128(
                    _mm_or_si128(_mm_cmpeq_epi8(chunk, n4), _mm_cmpeq_epi8(chunk, n5)),
                    _mm_or_si128(_mm_cmpeq_epi8(chunk, n6), _mm_cmpeq_epi8(chunk, n7)),
                ),
            );
            let mask = movemask_to_u32(_mm_movemask_epi8(eq));
            if mask != 0 {
                return Some(offset + i + mask.trailing_zeros() as usize);
            }
            i += 16;
        }

        // Scalar tail.
        while i < len {
            if set.table[bytes[i] as usize] {
                return Some(offset + i);
            }
            i += 1;
        }
    }

    None
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
// _mm_loadu_si128 is an unaligned load — the pointer alignment cast is intentional.
#[allow(clippy::cast_ptr_alignment)]
unsafe fn find_byte_sse2(haystack: &[u8], offset: usize, needle: u8) -> Option<usize> {
    let bytes = &haystack[offset..];
    let len = bytes.len();
    let ptr = bytes.as_ptr();

    unsafe {
        let n = _mm_set1_epi8(as_i8(needle));

        let mut i = 0;
        while i + 16 <= len {
            let chunk = _mm_loadu_si128(ptr.add(i).cast::<__m128i>());
            let mask = movemask_to_u32(_mm_movemask_epi8(_mm_cmpeq_epi8(chunk, n)));
            if mask != 0 {
                return Some(offset + i + mask.trailing_zeros() as usize);
            }
            i += 16;
        }

        while i < len {
            if bytes[i] == needle {
                return Some(offset + i);
            }
            i += 1;
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Scalar fallback
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "x86_64"))]
fn find_byte_set_scalar(haystack: &[u8], offset: usize, set: &ByteSet) -> Option<usize> {
    let mut i = offset;
    while i < haystack.len() {
        if set.table[haystack[i] as usize] {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(not(target_arch = "x86_64"))]
fn memchr_scalar(haystack: &[u8], offset: usize, needle: u8) -> Option<usize> {
    let mut i = offset;
    while i < haystack.len() {
        if haystack[i] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_set_find_basic() {
        let set = ByteSet::new(&[b'*', b'_', b'[', b'\n']);
        let input = b"hello world *bold*";
        assert_eq!(find_byte_set(input, 0, &set), Some(12));
    }

    #[test]
    fn byte_set_find_at_offset() {
        let set = ByteSet::new(&[b'*', b'_']);
        let input = b"hello *world* _foo_";
        assert_eq!(find_byte_set(input, 7, &set), Some(12));
    }

    #[test]
    fn byte_set_none() {
        let set = ByteSet::new(&[b'*', b'_']);
        let input = b"hello world";
        assert_eq!(find_byte_set(input, 0, &set), None);
    }

    #[test]
    fn byte_set_empty_input() {
        let set = ByteSet::new(&[b'*']);
        assert_eq!(find_byte_set(b"", 0, &set), None);
    }

    #[test]
    fn byte_set_offset_past_end() {
        let set = ByteSet::new(&[b'*']);
        assert_eq!(find_byte_set(b"hello", 10, &set), None);
    }

    #[test]
    fn find_single_byte() {
        let input = b"hello world\nfoo";
        assert_eq!(find_byte(input, 0, b'\n'), Some(11));
        assert_eq!(find_byte(input, 12, b'\n'), None);
    }

    #[test]
    fn byte_set_long_input() {
        // Ensure SIMD path works across multiple 16-byte chunks.
        let mut input = vec![b'a'; 100];
        input[67] = b'*';
        let set = ByteSet::new(&[b'*', b'_']);
        assert_eq!(find_byte_set(&input, 0, &set), Some(67));
        assert_eq!(find_byte_set(&input, 68, &set), None);
    }

    #[test]
    fn byte_set_all_8_needles() {
        let set = ByteSet::new(&[b'\n', b'*', b'_', b'[', b'!', b'\\', b'`', b']']);
        let input = b"abcdefghijklmnop]qrs";
        assert_eq!(find_byte_set(input, 0, &set), Some(16));
    }

    #[test]
    fn byte_set_first_byte() {
        let set = ByteSet::new(&[b'*']);
        assert_eq!(find_byte_set(b"*hello", 0, &set), Some(0));
    }

    #[test]
    fn byte_set_last_byte() {
        let set = ByteSet::new(&[b'*']);
        assert_eq!(find_byte_set(b"hello*", 0, &set), Some(5));
    }
}
