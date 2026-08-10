//! Exhaustive concurrency models, checked by [Loom].
//!
//! Loom enumerates *every* legal execution of a model rather than sampling
//! schedules: every thread interleaving, and every value each relaxed load is
//! permitted to return under the C11 memory model. Where Miri explores some
//! interleavings and native tests explore whatever the host happens to pick,
//! Loom proves the absence of ordering bugs for the modelled size.
//!
//! The trade is state-space growth. Models here are deliberately tiny — a
//! two-slot buffer and two or three operations per thread — because the number
//! of executions grows exponentially. A bug that needs four items to show up is
//! almost always visible with two.
//!
//! Run with `./scripts/loom.ps1` or `./scripts/loom.sh`.
//!
//! [Loom]: https://github.com/tokio-rs/loom

// The models drive the deprecated `producer()` / `consumer()` deliberately:
// they remain public API until 0.3.0, so their orderings still need proving.
#![allow(deprecated)]

use crate::{EventBuf, SeqRing};
use loom::sync::Arc;
use loom::thread;

/// Every item the producer pushes is popped exactly once, in order, with no
/// duplicates and no losses — under every interleaving.
///
/// A capacity-2 buffer and 3 pushes forces the full-buffer path, so the
/// backpressure branch is covered rather than skipped.
#[test]
fn event_buf_spsc_is_lossless_and_ordered() {
    loom::model(|| {
        let buf = Arc::new(EventBuf::<u32, 2>::new());

        let producer_buf = Arc::clone(&buf);
        let producer = thread::spawn(move || {
            let p = producer_buf.producer();
            for i in 0..3u32 {
                let mut val = i;
                // Retry on backpressure so the stream stays complete; any gap
                // the consumer sees is then a genuine defect.
                while let Err(rejected) = p.push(val) {
                    val = rejected;
                    thread::yield_now();
                }
            }
        });

        let consumer = thread::spawn(move || {
            let c = buf.consumer();
            let mut seen = 0u32;
            while seen < 3 {
                match c.pop() {
                    Some(val) => {
                        assert_eq!(val, seen, "out-of-order or duplicated pop");
                        seen += 1;
                    }
                    None => thread::yield_now(),
                }
            }
            seen
        });

        producer.join().unwrap();
        assert_eq!(consumer.join().unwrap(), 3);
    });
}

/// `len` never reports more than the buffer can hold, whatever the producer
/// and consumer are doing to the cursors.
///
/// This is the property the bracketed `tail`/`head`/`tail` snapshot exists to
/// provide; Loom checks it against every interleaving rather than hoping a
/// native run happens to hit the bad one.
#[test]
fn event_buf_len_never_exceeds_capacity() {
    loom::model(|| {
        let buf = Arc::new(EventBuf::<u32, 2>::new());

        let producer_buf = Arc::clone(&buf);
        let producer = thread::spawn(move || {
            let p = producer_buf.producer();
            let _ = p.push(1);
            let _ = p.push(2);
        });

        let consumer_buf = Arc::clone(&buf);
        let consumer = thread::spawn(move || {
            let c = consumer_buf.consumer();
            c.pop();
            c.pop();
        });

        let observed = buf.len();
        assert!(observed <= 2, "len() reported {observed} for capacity 2");

        producer.join().unwrap();
        consumer.join().unwrap();

        assert!(buf.len() <= 2);
    });
}

/// A pop never observes a slot the producer has not published.
///
/// Pushing distinct non-zero values means a torn or premature read would
/// surface as a value outside the pushed set.
#[test]
fn event_buf_pop_never_sees_unpublished_data() {
    loom::model(|| {
        let buf = Arc::new(EventBuf::<u32, 2>::new());

        let producer_buf = Arc::clone(&buf);
        let producer = thread::spawn(move || {
            let p = producer_buf.producer();
            let _ = p.push(0xAAAA_AAAA);
            let _ = p.push(0xBBBB_BBBB);
        });

        let consumer = thread::spawn(move || {
            let c = buf.consumer();
            while let Some(val) = c.pop() {
                assert!(
                    val == 0xAAAA_AAAA || val == 0xBBBB_BBBB,
                    "pop returned unpublished data: {val:#x}"
                );
            }
        });

        producer.join().unwrap();
        consumer.join().unwrap();
    });
}

/// The `SeqRing` sequence protocol never hands the consumer a sequence the
/// producer has not published, never goes backwards, and never exceeds the
/// newest published sequence.
///
/// Scope: this models the *sequence protocol* — `next_seq`, `published_seq`,
/// and the per-slot sequences — which is where the correctness argument lives.
/// The slot payload is deliberately outside the model; see the "Known
/// deviation" section of the `seq_ring` module docs.
#[test]
fn seq_ring_consumer_never_outruns_the_producer() {
    loom::model(|| {
        let ring = Arc::new(SeqRing::<u32, 2>::new());

        let producer_ring = Arc::clone(&ring);
        let producer = thread::spawn(move || {
            let p = producer_ring.producer();
            let first = p.push(10);
            let second = p.push(20);
            assert_eq!(first, 1, "first push must take sequence 1");
            assert_eq!(second, 2, "sequences must be consecutive");
        });

        let consumer = thread::spawn(move || {
            let mut c = ring.consumer();
            let mut last = 0u32;

            for _ in 0..3 {
                let stats = c.poll_up_to(2, |seq, val| {
                    assert!(seq > last, "sequence {seq} did not advance past {last}");
                    assert!(seq <= 2, "sequence {seq} was never published");
                    // Pushes are 10 then 20 at sequences 1 then 2.
                    let expected = if seq == 1 { 10 } else { 20 };
                    assert_eq!(*val, expected, "sequence {seq} carried a stale payload");
                    last = seq;
                });

                assert!(
                    stats.newest <= 2,
                    "published sequence {} exceeds what was pushed",
                    stats.newest
                );
                // Nothing can be delivered and skipped beyond what exists.
                assert!(stats.read + stats.dropped <= 2);
            }
        });

        producer.join().unwrap();
        consumer.join().unwrap();
    });
}

/// `latest` either reports nothing or reports the newest published sequence —
/// it never invents one, and never advances the consumer cursor.
#[test]
fn seq_ring_latest_is_never_ahead_of_published() {
    loom::model(|| {
        let ring = Arc::new(SeqRing::<u32, 2>::new());

        let producer_ring = Arc::clone(&ring);
        let producer = thread::spawn(move || {
            let p = producer_ring.producer();
            p.push(10);
            p.push(20);
        });

        let consumer = thread::spawn(move || {
            let c = ring.consumer();
            let mut seen = None;
            c.latest(|seq, val| seen = Some((seq, *val)));

            if let Some((seq, val)) = seen {
                assert!((1..=2).contains(&seq), "latest invented sequence {seq}");
                let expected = if seq == 1 { 10 } else { 20 };
                assert_eq!(val, expected, "latest returned a stale payload");
            }
        });

        producer.join().unwrap();
        consumer.join().unwrap();
    });
}
