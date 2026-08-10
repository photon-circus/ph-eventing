//! Fixed-size, stack-allocated ring buffer — no heap, no alloc, no atomics.
//!
//! [`RingBuf`] is a single-owner (`&mut self`) ring that overwrites the
//! oldest element when full. It requires only `T: Copy` and is ideal for
//! sample windows, local event logs, and anywhere a simple circular buffer
//! is needed without cross-thread sharing.
//!
//! Slots are `MaybeUninit<T>` and only live entries are ever read, which is
//! what lets the `Default` bound go. The cost is that this type is no longer
//! free of `unsafe`: see the safety note on [`RingBuf`].
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

use core::mem::MaybeUninit;

/// A ring buffer of `N` elements stored entirely on the stack.
///
/// Once full, new pushes overwrite the oldest entry. Iteration with
/// [`iter()`](RingBuf::iter) yields elements from oldest to newest.
///
/// # Safety note
/// Slots are stored as `MaybeUninit<T>` so that `T: Default` is not required.
/// Exactly one invariant makes every read sound: **the `len` entries ending at
/// `head` have all been written by [`push`](RingBuf::push)**. Every read goes
/// through the private `index` helper, which addresses only that range, and
/// every public accessor checks `len` before calling it. A change that lets
/// `len` outrun the number of writes is undefined behaviour, not a logic bug.
pub struct RingBuf<T: Copy, const N: usize> {
    buf: [MaybeUninit<T>; N],
    /// Write cursor — always points to the *next* slot to write.
    head: usize,
    /// Number of elements currently stored (≤ N).
    len: usize,
}

impl<T: Copy, const N: usize> RingBuf<T, N> {
    /// Create a new, empty ring buffer.
    ///
    /// This is a `const fn`, so the buffer can be built in a `const` or
    /// `static` initialiser. Note that `push`, `pop`, and `clear` take
    /// `&mut self`, so a bare `static RingBuf` is read-only and of little use,
    /// and `static mut` is a hard error to reference under edition 2024. The
    /// pattern this actually enables is const-initialising the buffer *inside*
    /// an interior-mutability wrapper, which is how a single-owner buffer is
    /// reached from an interrupt context:
    ///
    /// ```text
    /// // with critical-section, cortex-m, or similar:
    /// static LOG: Mutex<RefCell<RingBuf<u32, 64>>> =
    ///     Mutex::new(RefCell::new(RingBuf::new()));
    /// ```
    ///
    /// Without a const `new` that initialiser is impossible and you need a
    /// `StaticCell` or a `OnceCell` and a runtime init step.
    ///
    /// # Panics
    /// Does not panic. `N == 0` fails the build via a const assertion, so a
    /// zero-capacity ring cannot be constructed at all.
    pub const fn new() -> Self {
        const {
            assert!(N > 0, "RingBuf capacity N must be > 0");
        }
        Self {
            buf: [const { MaybeUninit::uninit() }; N],
            head: 0,
            len: 0,
        }
    }

    /// Append a value, overwriting the oldest entry once the ring is full.
    ///
    /// This never fails and never blocks; if losing the oldest entry is not
    /// acceptable, use [`crate::EventBuf`], whose `push` reports when full.
    pub fn push(&mut self, val: T) {
        self.buf[self.head] = MaybeUninit::new(val);
        self.head = (self.head + 1) % N;
        if self.len < N {
            self.len += 1;
        }
    }

    /// Remove and return the oldest element, or `None` if empty.
    ///
    /// # This does not make `RingBuf` a queue
    /// [`push`](Self::push) still **overwrites** when the ring is full. Adding
    /// `pop` gives you a window you can also drain from the front; it does not
    /// give you delivery. If a producer outruns whoever is popping, entries are
    /// discarded silently and `pop` has no way to report it — there is no drop
    /// counter here, unlike [`crate::SeqRing`], and no backpressure, unlike
    /// [`crate::EventBuf`].
    ///
    /// Reach for this to drain a log you have finished collecting. Do not
    /// reach for it to move data between two contexts that are both live.
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let idx = self.index(0);
        // SAFETY: `len > 0`, so `index(0)` addresses the oldest of the `len`
        // slots written by `push`.
        let val = unsafe { self.buf[idx].assume_init() };
        self.len -= 1;
        Some(val)
    }

    /// Number of elements currently stored, always in `0..=N`.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the ring holds no elements.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns `true` if the ring is at capacity, so the next
    /// [`push`](Self::push) will overwrite the oldest entry.
    pub fn is_full(&self) -> bool {
        self.len == N
    }

    /// Number of elements the ring can hold.
    #[inline]
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Drop every element, resetting the ring to empty.
    ///
    /// The backing array is left as-is; only the cursors are reset, so this is
    /// O(1) and does not touch the stored values.
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Index of the `i`-th live element, 0 = oldest.
    ///
    /// Callers must ensure `i < self.len`; the result is only a valid slot
    /// under that precondition.
    #[inline(always)]
    const fn index(&self, i: usize) -> usize {
        (self.head + N - self.len + i) % N
    }

    /// Read the `i`-th element (0 = oldest).
    pub fn get(&self, i: usize) -> Option<T> {
        if i >= self.len {
            return None;
        }
        let idx = self.index(i);
        // SAFETY: `i < len`, so `index` addresses one of the `len` slots
        // written by `push`.
        Some(unsafe { self.buf[idx].assume_init() })
    }

    /// Most recently pushed element.
    pub fn latest(&self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let idx = if self.head == 0 { N - 1 } else { self.head - 1 };
        // SAFETY: `len > 0`, so the slot behind `head` was written by `push`.
        Some(unsafe { self.buf[idx].assume_init() })
    }

    /// Iterate over elements oldest→newest.
    pub fn iter(&self) -> RingIter<'_, T, N> {
        RingIter { ring: self, pos: 0 }
    }
}

impl<T: Copy, const N: usize> Default for RingBuf<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> core::fmt::Debug for RingBuf<T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RingBuf")
            .field("len", &self.len)
            .field("capacity", &N)
            .finish()
    }
}

/// Iterator over [`RingBuf`] elements from oldest to newest.
pub struct RingIter<'a, T: Copy, const N: usize> {
    ring: &'a RingBuf<T, N>,
    pos: usize,
}

impl<'a, T: Copy, const N: usize> Iterator for RingIter<'a, T, N> {
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

impl<T: Copy, const N: usize> ExactSizeIterator for RingIter<'_, T, N> {}

impl<T: Copy, const N: usize> core::fmt::Debug for RingIter<'_, T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RingIter")
            .field("remaining", &(self.ring.len().saturating_sub(self.pos)))
            .finish()
    }
}

impl<'a, T: Copy, const N: usize> IntoIterator for &'a RingBuf<T, N> {
    type Item = T;
    type IntoIter = RingIter<'a, T, N>;

    fn into_iter(self) -> RingIter<'a, T, N> {
        self.iter()
    }
}

/// Yields the oldest entry via [`RingBuf::pop`].
///
/// # Caution
/// This makes `RingBuf` satisfy [`Link`](crate::traits::Link), so it can sit on
/// either side of [`forward`](crate::traits::forward). Be deliberate about the
/// `Source` side: `RingBuf` overwrites on `push`, so anything a live producer
/// dropped before `forward` ran is simply gone, and `forward` will report a
/// clean transfer of whatever survived. Neither the count it returns nor its
/// `Err` slot can express "and 40 more were lost". Use `SeqRing` when you need
/// that reported, or `EventBuf` when you need it not to happen.
impl<T: Copy, const N: usize> crate::traits::Source<T> for RingBuf<T, N> {
    #[inline]
    fn try_pop(&mut self) -> Option<T> {
        self.pop()
    }
}

impl<T: Copy, const N: usize> crate::traits::Sink<T> for RingBuf<T, N> {
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

    /// Deliberately does not derive `Default`. If the bound ever comes back,
    /// this stops compiling -- which is the point: the whole value of storing
    /// slots as `MaybeUninit` is that `T` no longer has to have a meaningless
    /// "zero" value invented for it.
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct NoDefault(u32);

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

    // `zero_capacity_panics` used to live here and could not survive the const
    // assertion: `RingBuf::<u32, 0>::new()` no longer builds, so there is no
    // runtime panic left to catch and no way to write the negative case as a
    // `#[test]`. The rejection is now enforced by the compiler instead, which
    // is stronger -- but it means nothing in this suite covers it, so the
    // const assertion itself is the only thing keeping N > 0 true.

    #[test]
    fn pop_returns_oldest_and_survives_wrap() {
        let mut r = RingBuf::<u32, 3>::new();
        r.push(1);
        r.push(2);
        r.push(3);
        r.push(4); // overwrites 1; window is 2,3,4
        assert_eq!(r.pop(), Some(2));
        assert_eq!(r.len(), 2);
        assert_eq!(r.get(0), Some(3));
        assert_eq!(r.latest(), Some(4));
        assert_eq!(r.pop(), Some(3));

        // push after a pop must resume from the right slot
        r.push(5);
        assert_eq!(r.get(0), Some(4));
        assert_eq!(r.get(1), Some(5));
        assert_eq!(r.pop(), Some(4));
        assert_eq!(r.pop(), Some(5));
        assert_eq!(r.pop(), None);
        assert!(r.is_empty());
    }

    /// The documented hazard, pinned as behaviour rather than left as prose:
    /// overwritten entries are gone and `pop` cannot report them.
    #[test]
    fn pop_cannot_report_overwritten_entries() {
        let mut r = RingBuf::<u32, 2>::new();
        for i in 0..6 {
            r.push(i);
        }
        // Four entries were discarded; nothing in the API says so.
        assert_eq!(r.pop(), Some(4));
        assert_eq!(r.pop(), Some(5));
        assert_eq!(r.pop(), None);
    }

    #[test]
    fn const_new_works_in_const_context() {
        // The const initialiser is the feature. A `static RingBuf` is itself
        // near-useless because every mutator needs `&mut self` -- the real
        // shape is this one nested inside an interior-mutability wrapper, and
        // that is exactly what a non-const `new` makes impossible.
        const EMPTY: RingBuf<u32, 4> = RingBuf::new();
        static LOG: RingBuf<u32, 8> = RingBuf::new();

        assert!(EMPTY.is_empty());
        assert_eq!(EMPTY.capacity(), 4);
        assert!(LOG.is_empty());
        assert_eq!(LOG.capacity(), 8);

        // Const-constructed and runtime-constructed rings behave identically.
        let mut r = EMPTY;
        r.push(1);
        assert_eq!(r.get(0), Some(1));
    }

    #[test]
    fn works_without_default_bound() {
        let mut r = RingBuf::<NoDefault, 2>::new();
        r.push(NoDefault(1));
        r.push(NoDefault(2));
        assert_eq!(r.get(0), Some(NoDefault(1)));
        assert_eq!(r.latest(), Some(NoDefault(2)));
        r.push(NoDefault(3)); // overwrites
        assert_eq!(r.get(0), Some(NoDefault(2)));
        assert_eq!(r.latest(), Some(NoDefault(3)));
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
