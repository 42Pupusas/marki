//! SIMD-accelerated byte scanning for the inline and block parsers.
//!
//! On `x86/x86_64` with SSE2 (baseline for all `x86_64`), processes 16 bytes
//! per iteration. Falls back to a scalar loop on other architectures.

/// Extension trait for byte-slice searches used by the parsers.
pub trait ByteSliceExt {
    /// Find the first occurrence of `needle` at or after `offset`.
    fn find_byte(&self, offset: usize, needle: u8) -> Option<usize>;

    /// Find the first byte at or after `offset` that is contained in `set`.
    fn find_byte_set(&self, offset: usize, set: &ByteSet) -> Option<usize>;
}

impl ByteSliceExt for [u8] {
    #[inline]
    fn find_byte(&self, offset: usize, needle: u8) -> Option<usize> {
        if offset >= self.len() {
            return None;
        }
        ByteSearcher(needle).find(self, offset)
    }

    #[inline]
    fn find_byte_set(&self, offset: usize, set: &ByteSet) -> Option<usize> {
        if offset >= self.len() {
            return None;
        }
        set.find(self, offset)
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

    #[inline]
    fn find(&self, haystack: &[u8], offset: usize) -> Option<usize> {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: SSE2 is baseline on all x86_64 processors.
            unsafe { self.find_sse2(haystack, offset) }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            self.find_scalar(haystack, offset)
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "sse2")]
    // _mm_loadu_si128 is an unaligned load — the pointer alignment cast is intentional.
    #[allow(clippy::cast_ptr_alignment)]
    unsafe fn find_sse2(&self, haystack: &[u8], offset: usize) -> Option<usize> {
        let bytes = &haystack[offset..];
        let len = bytes.len();
        let ptr = bytes.as_ptr();

        unsafe {
            // Load all needle lanes.
            let n0 = _mm_set1_epi8(self.bytes[0].cast_signed());
            let n1 = _mm_set1_epi8(self.bytes[1].cast_signed());
            let n2 = _mm_set1_epi8(self.bytes[2].cast_signed());
            let n3 = _mm_set1_epi8(self.bytes[3].cast_signed());
            let n4 = _mm_set1_epi8(self.bytes[4].cast_signed());
            let n5 = _mm_set1_epi8(self.bytes[5].cast_signed());
            let n6 = _mm_set1_epi8(self.bytes[6].cast_signed());
            let n7 = _mm_set1_epi8(self.bytes[7].cast_signed());

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
                let mask = _mm_movemask_epi8(eq).movemask_to_u32();
                if mask != 0 {
                    return Some(offset + i + mask.trailing_zeros() as usize);
                }
                i += 16;
            }

            // Scalar tail.
            while i < len {
                if self.table[bytes[i] as usize] {
                    return Some(offset + i);
                }
                i += 1;
            }
        }

        None
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn find_scalar(&self, haystack: &[u8], offset: usize) -> Option<usize> {
        let mut i = offset;
        while i < haystack.len() {
            if self.table[haystack[i] as usize] {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

/// Reinterpret a `_mm_movemask_epi8` result's low 16 bits as `u32`.
#[cfg(target_arch = "x86_64")]
trait MovemaskBits {
    fn movemask_to_u32(self) -> u32;
}

#[cfg(target_arch = "x86_64")]
impl MovemaskBits for i32 {
    /// Reinterpret the low 16 bits of a `_mm_movemask_epi8` result as `u32`.
    /// Movemask returns `i32` with only bits 0..15 meaningful; widening via
    /// the byte representation is lossless.
    #[allow(clippy::inline_always)]
    #[inline(always)]
    fn movemask_to_u32(self) -> u32 {
        let [lo, hi, _, _] = self.to_ne_bytes();
        u32::from(u16::from_ne_bytes([lo, hi]))
    }
}

struct ByteSearcher(u8);

impl ByteSearcher {
    #[inline]
    fn find(&self, haystack: &[u8], offset: usize) -> Option<usize> {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: SSE2 is baseline on all x86_64 processors.
            unsafe { self.find_sse2(haystack, offset) }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            self.find_scalar(haystack, offset)
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "sse2")]
    // _mm_loadu_si128 is an unaligned load — the pointer alignment cast is intentional.
    #[allow(clippy::cast_ptr_alignment)]
    unsafe fn find_sse2(&self, haystack: &[u8], offset: usize) -> Option<usize> {
        let bytes = &haystack[offset..];
        let len = bytes.len();
        let ptr = bytes.as_ptr();

        unsafe {
            let n = _mm_set1_epi8(self.0.cast_signed());

            let mut i = 0;
            while i + 16 <= len {
                let chunk = _mm_loadu_si128(ptr.add(i).cast::<__m128i>());
                let mask = _mm_movemask_epi8(_mm_cmpeq_epi8(chunk, n)).movemask_to_u32();
                if mask != 0 {
                    return Some(offset + i + mask.trailing_zeros() as usize);
                }
                i += 16;
            }

            while i < len {
                if bytes[i] == self.0 {
                    return Some(offset + i);
                }
                i += 1;
            }
        }

        None
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn find_scalar(&self, haystack: &[u8], offset: usize) -> Option<usize> {
        let mut i = offset;
        while i < haystack.len() {
            if haystack[i] == self.0 {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

// ---------------------------------------------------------------------------
// SSE2 implementation (x86_64 baseline)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{
    __m128i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_or_si128, _mm_set1_epi8,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_set_find_basic() {
        let set = ByteSet::new(b"*_[\n");
        let input = b"hello world *bold*";
        assert_eq!(input.find_byte_set(0, &set), Some(12));
    }

    #[test]
    fn byte_set_find_at_offset() {
        let set = ByteSet::new(b"*_");
        let input = b"hello *world* _foo_";
        assert_eq!(input.find_byte_set(7, &set), Some(12));
    }

    #[test]
    fn byte_set_none() {
        let set = ByteSet::new(b"*_");
        let input = b"hello world";
        assert_eq!(input.find_byte_set(0, &set), None);
    }

    #[test]
    fn byte_set_empty_input() {
        let set = ByteSet::new(b"*");
        assert_eq!(b"".find_byte_set(0, &set), None);
    }

    #[test]
    fn byte_set_offset_past_end() {
        let set = ByteSet::new(b"*");
        assert_eq!(b"hello".find_byte_set(10, &set), None);
    }

    #[test]
    fn find_single_byte() {
        let input = b"hello world\nfoo";
        assert_eq!(input.find_byte(0, b'\n'), Some(11));
        assert_eq!(input.find_byte(12, b'\n'), None);
    }

    #[test]
    fn byte_set_long_input() {
        // Ensure SIMD path works across multiple 16-byte chunks.
        let mut input = [b'a'; 100];
        input[67] = b'*';
        let set = ByteSet::new(b"*_");
        assert_eq!(input.find_byte_set(0, &set), Some(67));
        assert_eq!(input.find_byte_set(68, &set), None);
    }

    #[test]
    fn byte_set_all_8_needles() {
        let set = ByteSet::new(b"\n*_[!\\`]");
        let input = b"abcdefghijklmnop]qrs";
        assert_eq!(input.find_byte_set(0, &set), Some(16));
    }

    #[test]
    fn byte_set_first_byte() {
        let set = ByteSet::new(b"*");
        assert_eq!(b"*hello".find_byte_set(0, &set), Some(0));
    }

    #[test]
    fn byte_set_last_byte() {
        let set = ByteSet::new(b"*");
        assert_eq!(b"hello*".find_byte_set(0, &set), Some(5));
    }
}
