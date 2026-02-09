//! Fixed-size, stack-allocated ring buffer — no heap, no alloc, no atomics.
//!
//! [`RingBuf`] is a single-owner (`&mut self`) ring that overwrites the
//! oldest element when full. It requires `T: Copy + Default` and is ideal
//! for sample windows, local event logs, and anywhere a simple circular
//! buffer is needed without cross-thread sharing.
//!
//! For a lock-free SPSC ring with sequence tracking, see [`crate::SeqRing`].
//! For a lock-free SPSC ring with backpressure, see [`crate::EventBuf`].
//!
//! # Example
//! ```
//! use ph_eventing::RingBuf;
//!
//! let mut r = RingBuf::<u32, 4>::new();
//! r.push(10);
//! r.push(20);
//! assert_eq!(r.latest(), Some(20));
//! assert_eq!(r.get(0), Some(10)); // oldest
//! ```

/// A ring buffer of `N` elements stored entirely on the stack.
///
/// Once full, new pushes overwrite the oldest entry. Iteration with
/// [`iter()`](RingBuf::iter) yields elements from oldest to newest.
pub struct RingBuf<T: Copy + Default, const N: usize> {
    buf: [T; N],
    /// Write cursor — always points to the *next* slot to write.
    head: usize,
    /// Number of elements currently stored (≤ N).
    len: usize,
}

impl<T: Copy + Default, const N: usize> RingBuf<T, N> {
    pub fn new() -> Self {
        assert!(N > 0, "RingBuf capacity N must be > 0");
        Self {
            buf: [T::default(); N],
            head: 0,
            len: 0,
        }
    }

    pub fn push(&mut self, val: T) {
        self.buf[self.head] = val;
        self.head = (self.head + 1) % N;
        if self.len < N {
            self.len += 1;
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_full(&self) -> bool {
        self.len == N
    }

    /// Number of elements the ring can hold.
    #[inline]
    pub const fn capacity(&self) -> usize {
        N
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Read the `i`-th element (0 = oldest).
    pub fn get(&self, i: usize) -> Option<T> {
        if i >= self.len {
            return None;
        }
        let idx = if self.len < N {
            i
        } else {
            (self.head + i) % N
        };
        Some(self.buf[idx])
    }

    /// Most recently pushed element.
    pub fn latest(&self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let idx = if self.head == 0 { N - 1 } else { self.head - 1 };
        Some(self.buf[idx])
    }

    /// Iterate over elements oldest→newest.
    pub fn iter(&self) -> RingIter<'_, T, N> {
        RingIter { ring: self, pos: 0 }
    }
}

impl<T: Copy + Default, const N: usize> Default for RingBuf<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy + Default, const N: usize> core::fmt::Debug for RingBuf<T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RingBuf")
            .field("len", &self.len)
            .field("capacity", &N)
            .finish()
    }
}

/// Iterator over [`RingBuf`] elements from oldest to newest.
pub struct RingIter<'a, T: Copy + Default, const N: usize> {
    ring: &'a RingBuf<T, N>,
    pos: usize,
}

impl<'a, T: Copy + Default, const N: usize> Iterator for RingIter<'a, T, N> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        let val = self.ring.get(self.pos)?;
        self.pos += 1;
        Some(val)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.ring.len().saturating_sub(self.pos);
        (remaining, Some(remaining))
    }
}

impl<T: Copy + Default, const N: usize> ExactSizeIterator for RingIter<'_, T, N> {}

impl<T: Copy + Default, const N: usize> core::fmt::Debug for RingIter<'_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RingIter")
            .field("remaining", &(self.ring.len().saturating_sub(self.pos)))
            .finish()
    }
}

impl<'a, T: Copy + Default, const N: usize> IntoIterator for &'a RingBuf<T, N> {
    type Item = T;
    type IntoIter = RingIter<'a, T, N>;

    fn into_iter(self) -> RingIter<'a, T, N> {
        self.iter()
    }
}

impl<T: Copy + Default, const N: usize> crate::traits::Sink<T> for RingBuf<T, N> {
    type Error = core::convert::Infallible;

    #[inline]
    fn try_push(&mut self, val: T) -> Result<(), core::convert::Infallible> {
        self.push(val);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_ring_is_empty() {
        let r = RingBuf::<u32, 4>::new();
        assert!(r.is_empty());
        assert!(!r.is_full());
        assert_eq!(r.len(), 0);
        assert_eq!(r.latest(), None);
        assert_eq!(r.get(0), None);
    }

    #[test]
    fn push_and_get() {
        let mut r = RingBuf::<u32, 4>::new();
        r.push(10);
        r.push(20);
        r.push(30);
        assert_eq!(r.len(), 3);
        assert_eq!(r.get(0), Some(10));
        assert_eq!(r.get(1), Some(20));
        assert_eq!(r.get(2), Some(30));
        assert_eq!(r.get(3), None);
        assert_eq!(r.latest(), Some(30));
    }

    #[test]
    fn overwrite_oldest_when_full() {
        let mut r = RingBuf::<u32, 3>::new();
        r.push(1);
        r.push(2);
        r.push(3);
        assert!(r.is_full());

        r.push(4); // overwrites 1
        assert_eq!(r.len(), 3);
        assert_eq!(r.get(0), Some(2));
        assert_eq!(r.get(1), Some(3));
        assert_eq!(r.get(2), Some(4));
        assert_eq!(r.latest(), Some(4));
    }

    #[test]
    fn clear_resets_state() {
        let mut r = RingBuf::<u32, 4>::new();
        r.push(1);
        r.push(2);
        r.clear();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert_eq!(r.latest(), None);
    }

    #[test]
    fn iter_oldest_to_newest() {
        let mut r = RingBuf::<u32, 4>::new();
        for i in 1..=6 {
            r.push(i);
        }
        // capacity 4, pushed 6 → oldest is 3
        let v: std::vec::Vec<u32> = r.iter().collect();
        assert_eq!(v, [3, 4, 5, 6]);
    }

    #[test]
    fn iter_exact_size() {
        let mut r = RingBuf::<u32, 4>::new();
        r.push(1);
        r.push(2);
        let it = r.iter();
        assert_eq!(it.len(), 2);
    }

    #[test]
    fn default_is_new() {
        let r: RingBuf<u8, 8> = RingBuf::default();
        assert!(r.is_empty());
    }

    #[test]
    #[should_panic(expected = "RingBuf capacity N must be > 0")]
    fn zero_capacity_panics() {
        let _ = RingBuf::<u32, 0>::new();
    }

    #[test]
    fn capacity_returns_n() {
        let r = RingBuf::<u32, 8>::new();
        assert_eq!(r.capacity(), 8);
    }

    #[test]
    fn into_iter_for_ref() {
        let mut r = RingBuf::<u32, 4>::new();
        r.push(1);
        r.push(2);
        r.push(3);
        let v: std::vec::Vec<u32> = (&r).into_iter().collect();
        assert_eq!(v, [1, 2, 3]);
    }
}