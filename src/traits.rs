//! Common traits for event producers and consumers.
//!
//! These traits abstract over the different ring buffer types so that
//! generic code can work with any combination of producer and consumer.
//!
//! | Trait | Role | Implementors |
//! |-------|------|-------------|
//! | [`Sink`] | Accept events | [`RingBuf`], [`seq_ring::Producer`], [`event_buf::Producer`] |
//! | [`Source`] | Yield events | [`seq_ring::Consumer`], [`event_buf::Consumer`] |
//! | [`Link`] | Both — accept *and* yield | Blanket impl for any `Sink<In> + Source<Out>` |
//!
//! The free function [`forward`] transfers items from any [`Source`] to any
//! [`Sink`], stopping when the source is empty or the sink rejects a value.
//!
//! [`RingBuf`]: crate::RingBuf
//! [`seq_ring::Producer`]: crate::seq_ring::Producer
//! [`seq_ring::Consumer`]: crate::seq_ring::Consumer
//! [`event_buf::Producer`]: crate::event_buf::Producer
//! [`event_buf::Consumer`]: crate::event_buf::Consumer

/// Accept events.
///
/// `Error` is the type returned when the sink cannot accept the value.
/// Sinks that never reject (e.g. overwrite rings) use
/// [`core::convert::Infallible`].
///
/// # Implementors
/// - [`crate::RingBuf`] — always succeeds (`Error = Infallible`).
/// - [`crate::seq_ring::Producer`] — always succeeds (`Error = Infallible`).
/// - [`crate::event_buf::Producer`] — returns `Err(val)` when full (`Error = T`).
pub trait Sink<T> {
    /// The error returned when the sink cannot accept the value.
    type Error;

    /// Push a value into the sink.
    ///
    /// Returns `Ok(())` on success or `Err(Self::Error)` when the sink is
    /// unable to accept the value.
    fn try_push(&mut self, val: T) -> Result<(), Self::Error>;
}

/// Yield events.
///
/// # Implementors
/// - [`crate::seq_ring::Consumer`] — drains the next in-order item.
/// - [`crate::event_buf::Consumer`] — pops the oldest buffered item.
pub trait Source<T> {
    /// Pull the next event, or `None` if nothing is available.
    fn try_pop(&mut self) -> Option<T>;
}

/// A bidirectional pass-through: accepts `In` and yields `Out`.
///
/// This is automatically implemented for any type that is both
/// [`Sink<In>`] and [`Source<Out>`].
pub trait Link<In, Out>: Sink<In> + Source<Out> {}

impl<In, Out, L> Link<In, Out> for L where L: Sink<In> + Source<Out> {}

/// Transfer up to `max` items from a [`Source`] into a [`Sink`].
///
/// Stops early if the source is empty or the sink returns an error.
/// Returns `(transferred, last_error)` — `last_error` is `Some` only if
/// the sink rejected a value.
///
/// # Example
/// ```
/// use ph_eventing::{EventBuf, SeqRing};
/// use ph_eventing::traits::{Sink, Source, forward};
///
/// let seq = SeqRing::<u32, 8>::new();
/// let sp = seq.try_producer().expect("producer");
/// let mut sc = seq.try_consumer().expect("consumer");
///
/// sp.push(1);
/// sp.push(2);
///
/// let eb = EventBuf::<u32, 8>::new();
/// let mut ep = eb.try_producer().expect("producer");
///
/// let (n, err) = forward(&mut sc, &mut ep, 10);
/// assert_eq!(n, 2);
/// assert!(err.is_none());
/// ```
pub fn forward<T, S, K>(src: &mut S, snk: &mut K, max: usize) -> (usize, Option<K::Error>)
where
    S: Source<T>,
    K: Sink<T>,
{
    let mut count = 0;
    while count < max {
        let val = match src.try_pop() {
            Some(v) => v,
            None => break,
        };
        match snk.try_push(val) {
            Ok(()) => count += 1,
            Err(e) => return (count, Some(e)),
        }
    }
    (count, None)
}

#[cfg(test)]
mod tests {
    // The deprecated `producer()` / `consumer()` remain public API until 0.3.0,
    // so these tests are their coverage -- including the two that assert the
    // panic message. Allowing the lint here rather than at the crate root keeps
    // the warning live for library code, which is where it should bite.
    #![allow(deprecated)]

    use super::*;
    use crate::{EventBuf, RingBuf, SeqRing};

    // ── Sink tests ─────────────────────────────────────────────────

    #[test]
    fn ringbuf_as_sink() {
        let mut ring = RingBuf::<u32, 4>::new();
        assert!(ring.try_push(1).is_ok());
        assert!(ring.try_push(2).is_ok());
        assert_eq!(ring.len(), 2);
    }

    #[test]
    fn seq_producer_as_sink() {
        let ring = SeqRing::<u32, 4>::new();
        let mut p = ring.producer();
        assert!(p.try_push(10).is_ok());
        assert!(p.try_push(20).is_ok());
    }

    #[test]
    fn event_producer_as_sink() {
        let buf = EventBuf::<u32, 2>::new();
        let mut p = buf.producer();
        assert!(p.try_push(1).is_ok());
        assert!(p.try_push(2).is_ok());
        assert_eq!(p.try_push(3), Err(3));
    }

    // ── Source tests ───────────────────────────────────────────────

    #[test]
    fn seq_consumer_as_source() {
        let ring = SeqRing::<u32, 4>::new();
        let p = ring.producer();
        let mut c = ring.consumer();

        p.push(10);
        p.push(20);

        assert_eq!(c.try_pop(), Some(10));
        assert_eq!(c.try_pop(), Some(20));
        assert_eq!(c.try_pop(), None);
    }

    #[test]
    fn event_consumer_as_source() {
        let buf = EventBuf::<u32, 4>::new();
        let p = buf.producer();
        let mut c = buf.consumer();

        p.push(10).unwrap();
        p.push(20).unwrap();

        assert_eq!(c.try_pop(), Some(10));
        assert_eq!(c.try_pop(), Some(20));
        assert_eq!(c.try_pop(), None);
    }

    // ── forward tests ──────────────────────────────────────────────

    #[test]
    fn forward_seq_to_event() {
        let seq = SeqRing::<u32, 8>::new();
        let sp = seq.producer();
        let mut sc = seq.consumer();

        sp.push(1);
        sp.push(2);
        sp.push(3);

        let eb = EventBuf::<u32, 8>::new();
        let mut ep = eb.producer();

        let (n, err) = forward(&mut sc, &mut ep, 10);
        assert_eq!(n, 3);
        assert!(err.is_none());

        let ec = eb.consumer();
        assert_eq!(ec.pop(), Some(1));
        assert_eq!(ec.pop(), Some(2));
        assert_eq!(ec.pop(), Some(3));
    }

    #[test]
    fn forward_event_to_ringbuf() {
        let eb = EventBuf::<u32, 8>::new();
        let ep = eb.producer();
        let mut ec = eb.consumer();

        ep.push(10).unwrap();
        ep.push(20).unwrap();

        let mut ring = RingBuf::<u32, 4>::new();

        let (n, err) = forward(&mut ec, &mut ring, 10);
        assert_eq!(n, 2);
        assert!(err.is_none());
        assert_eq!(ring.get(0), Some(10));
        assert_eq!(ring.get(1), Some(20));
    }

    #[test]
    fn forward_stops_when_sink_full() {
        let src_buf = EventBuf::<u32, 8>::new();
        let sp = src_buf.producer();
        let mut sc = src_buf.consumer();

        for i in 0..5 {
            sp.push(i).unwrap();
        }

        let dst_buf = EventBuf::<u32, 2>::new();
        let mut dp = dst_buf.producer();

        let (n, err) = forward(&mut sc, &mut dp, 10);
        assert_eq!(n, 2);
        assert_eq!(err, Some(2)); // third item rejected
    }

    #[test]
    fn forward_empty_source_transfers_nothing() {
        let seq = SeqRing::<u32, 4>::new();
        let _sp = seq.producer();
        let mut sc = seq.consumer();

        let eb = EventBuf::<u32, 4>::new();
        let mut ep = eb.producer();

        let (n, err) = forward(&mut sc, &mut ep, 10);
        assert_eq!(n, 0);
        assert!(err.is_none());
    }

    // ── generic helper to prove trait-generic code compiles ────────

    fn drain_all_into_vec<T: Copy, S: Source<T>>(src: &mut S) -> std::vec::Vec<T> {
        let mut out = std::vec::Vec::new();
        while let Some(v) = src.try_pop() {
            out.push(v);
        }
        out
    }

    #[test]
    fn generic_drain_seq() {
        let ring = SeqRing::<u32, 4>::new();
        let p = ring.producer();
        let mut c = ring.consumer();

        p.push(1);
        p.push(2);
        p.push(3);

        let v = drain_all_into_vec(&mut c);
        assert_eq!(v, [1, 2, 3]);
    }

    #[test]
    fn generic_drain_event() {
        let buf = EventBuf::<u32, 4>::new();
        let p = buf.producer();
        let mut c = buf.consumer();

        p.push(1).unwrap();
        p.push(2).unwrap();

        let v = drain_all_into_vec(&mut c);
        assert_eq!(v, [1, 2]);
    }
}
