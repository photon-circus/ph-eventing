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

use crate::{EventBuf, LatestBuf, SeqRing};
use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use loom::thread;

/// The three-slot exchange transfers complete payload ownership in both
/// directions. Loom's tracked cells make either a missing Release (offered
/// slot) or missing Acquire (claimed slot) observable as an access violation.
#[test]
fn latest_buf_returns_only_complete_publications() {
    loom::model(|| {
        let channel = Arc::new(LatestBuf::<[u32; 2]>::new());

        let producer_channel = Arc::clone(&channel);
        let producer = thread::spawn(move || {
            let producer = producer_channel.try_producer().unwrap();
            // Each publication's report is preserved: `replaced_unread` is
            // the producer-side half of the loss ledger, and the conservation
            // assert after the joins checks it against the consumer's takes.
            let mut replaced = 0u32;
            for value in 1..=3u32 {
                if producer.publish([value, value]).replaced_unread {
                    replaced += 1;
                }
            }
            replaced
        });

        let consumer_channel = Arc::clone(&channel);
        let consumer = thread::spawn(move || {
            let consumer = consumer_channel.try_consumer().unwrap();
            // Conservation across the run, not just per-item sanity: each
            // observed generation must be strictly newer than the last
            // (at-most-once, C-family), and `skipped` must equal exactly the
            // generations passed over since the previous take (C3/O2; no wrap
            // at these magnitudes, so the formula reduces to the difference).
            let mut last_generation = 0u32;
            let mut taken = 0u32;
            for _ in 0..3 {
                if let Some(item) = consumer.take_latest() {
                    assert!((1..=3).contains(&item.generation));
                    assert_eq!(item.value, [item.generation; 2]);
                    assert!(item.generation > last_generation);
                    assert_eq!(item.skipped, item.generation - last_generation - 1);
                    last_generation = item.generation;
                    taken += 1;
                }
                thread::yield_now();
            }
            taken
        });

        let replaced = producer.join().unwrap();
        let taken = consumer.join().unwrap();

        // Every publication ends in exactly one bucket — taken by the
        // consumer, displaced while unread (the producer's report), or still
        // pending at the end. The exchange's linearization makes this hold in
        // every interleaving; a report derived from stale state breaks it.
        let pending = u32::from(channel.try_consumer().unwrap().take_latest().is_some());
        assert_eq!(replaced + taken + pending, 3);
    });
}

/// Forces a slot to travel producer -> consumer -> exchange -> producer.
/// Relaxed phase markers influence scheduling without supplying the
/// happens-before edges that the exchange itself must provide.
///
/// The handshakes spin (with yields) instead of giving up after a bounded
/// number of polls: every terminating execution therefore completes the
/// full reuse cycle — publish 1, take 1, publish 2, take 2, publish 3 —
/// and the final take is asserted after the joins, so the model cannot
/// pass without exercising the reuse path it exists to check.
#[test]
fn latest_buf_reused_slot_keeps_exclusive_ownership() {
    loom::model(|| {
        let channel = Arc::new(LatestBuf::<[u32; 2]>::new());
        let phase = Arc::new(AtomicU32::new(0));

        let producer_channel = Arc::clone(&channel);
        let producer_phase = Arc::clone(&phase);
        let producer = thread::spawn(move || {
            let producer = producer_channel.try_producer().unwrap();
            // Each handshake guarantees the prior publication was taken
            // before the next publish, so no publish here may ever report a
            // displaced-unread predecessor.
            assert!(!producer.publish([1, 1]).replaced_unread);
            producer_phase.store(1, Ordering::Relaxed);
            while producer_phase.load(Ordering::Relaxed) < 2 {
                thread::yield_now();
            }
            assert!(!producer.publish([2, 2]).replaced_unread);
            producer_phase.store(3, Ordering::Relaxed);
            while producer_phase.load(Ordering::Relaxed) < 4 {
                thread::yield_now();
            }
            assert!(!producer.publish([3, 3]).replaced_unread);
        });

        let consumer_channel = Arc::clone(&channel);
        let consumer_phase = Arc::clone(&phase);
        let consumer = thread::spawn(move || {
            let consumer = consumer_channel.try_consumer().unwrap();
            // The phase marker orders scheduling only; visibility of the
            // publication itself must arrive through the exchange, so the
            // take retries until it does.
            let mut take_spinning = |expected: u32| loop {
                if let Some(item) = consumer.take_latest() {
                    assert_eq!(item.generation, expected);
                    assert_eq!(item.value, [expected; 2]);
                    assert_eq!(item.skipped, 0);
                    break;
                }
                thread::yield_now();
            };
            while consumer_phase.load(Ordering::Relaxed) < 1 {
                thread::yield_now();
            }
            take_spinning(1);
            consumer_phase.store(2, Ordering::Relaxed);
            while consumer_phase.load(Ordering::Relaxed) < 3 {
                thread::yield_now();
            }
            take_spinning(2);
            consumer_phase.store(4, Ordering::Relaxed);
        });

        producer.join().unwrap();
        consumer.join().unwrap();

        // Generation 3 was published after the in-thread consumer handle
        // dropped; a reacquired handle must find it pending with exact
        // accounting (channel-resident continuation, A.3).
        let consumer = channel.try_consumer().unwrap();
        let item = consumer
            .take_latest()
            .expect("generation 3 pending after the producer joined");
        assert_eq!(item.generation, 3);
        assert_eq!(item.value, [3, 3]);
        assert_eq!(item.skipped, 0);
    });
}

/// The empty-poll Acquire load either observes a pending publication and
/// proceeds through the normal ownership swap, or returns before a concurrent
/// publication. In the latter case that publication must remain pending for
/// the next poll rather than being lost.
#[test]
fn latest_buf_empty_fast_path_preserves_concurrent_publication() {
    loom::model(|| {
        let channel = Arc::new(LatestBuf::<u32>::new());

        let producer_channel = Arc::clone(&channel);
        let producer = thread::spawn(move || {
            let producer = producer_channel.try_producer().unwrap();
            producer.publish(7)
        });

        let consumer_channel = Arc::clone(&channel);
        let consumer = thread::spawn(move || {
            let consumer = consumer_channel.try_consumer().unwrap();
            consumer.take_latest()
        });

        assert_eq!(producer.join().unwrap().generation, 1);
        let first = consumer.join().unwrap();

        let consumer = channel.try_consumer().unwrap();
        let second = consumer.take_latest();
        match first {
            Some(item) => {
                assert_eq!((item.generation, item.value), (1, 7));
                assert_eq!(second, None);
            }
            None => {
                let item = second.expect("publication remains pending after an empty poll");
                assert_eq!((item.generation, item.value), (1, 7));
            }
        }
    });
}

/// Channel-resident producer state is published by handle drop and acquired
/// by the next handle, so reacquisition in another context resumes generation.
#[test]
fn latest_buf_producer_reacquisition_continues_across_threads() {
    loom::model(|| {
        let channel = Arc::new(LatestBuf::<u32>::new());
        let first_acquired = Arc::new(AtomicBool::new(false));
        let first_channel = Arc::clone(&channel);
        let first_signal = Arc::clone(&first_acquired);
        let first = thread::spawn(move || {
            let producer = first_channel.try_producer().unwrap();
            // Signal before mutating role state. This orders initial role
            // selection but deliberately does not publish the continuation
            // write; that must travel through handle drop/acquisition.
            first_signal.store(true, Ordering::Release);
            assert_eq!(producer.publish(1).generation, 1);
        });

        let second = thread::spawn(move || {
            if first_acquired.load(Ordering::Acquire) {
                channel
                    .try_producer()
                    .map(|producer| producer.publish(2).generation)
            } else {
                None
            }
        });

        first.join().unwrap();
        // Loom explores both outcomes: acquisition while the first handle is
        // live fails, while acquisition after its Release drop succeeds and
        // must observe the channel-resident continuation state.
        if let Some(generation) = second.join().unwrap() {
            assert_eq!(generation, 2);
        }
    });
}

/// Consumer continuation state crosses the taken flag's Release/Acquire
/// handoff. The relaxed `first_took` signal controls the model without adding
/// an independent happens-before edge for `last_generation`.
#[test]
fn latest_buf_consumer_reacquisition_continues_across_threads() {
    loom::model(|| {
        let channel = Arc::new(LatestBuf::<u32>::new());
        {
            let producer = channel.try_producer().unwrap();
            assert_eq!(producer.publish(1).generation, 1);
        }

        let first_took = Arc::new(AtomicBool::new(false));
        let later_published = Arc::new(AtomicBool::new(false));

        let first_channel = Arc::clone(&channel);
        let first_signal = Arc::clone(&first_took);
        let first = thread::spawn(move || {
            let consumer = first_channel.try_consumer().unwrap();
            assert_eq!(consumer.take_latest().unwrap().generation, 1);
            // Relaxed deliberately: successful reacquisition, not this test
            // signal, must publish the role-owned continuation state.
            first_signal.store(true, Ordering::Relaxed);
        });

        let publisher_channel = Arc::clone(&channel);
        let publisher_start = Arc::clone(&first_took);
        let publisher_done = Arc::clone(&later_published);
        let publisher = thread::spawn(move || {
            if publisher_start.load(Ordering::Relaxed) {
                let producer = publisher_channel.try_producer().unwrap();
                let _ = producer.publish(2);
                let _ = producer.publish(3);
                publisher_done.store(true, Ordering::Release);
            }
        });

        let second = thread::spawn(move || {
            if later_published.load(Ordering::Acquire) {
                channel
                    .try_consumer()
                    .and_then(|consumer| consumer.take_latest())
            } else {
                None
            }
        });

        first.join().unwrap();
        publisher.join().unwrap();
        if let Some(item) = second.join().unwrap() {
            assert_eq!(item.generation, 3);
            assert_eq!(item.value, 3);
            assert_eq!(item.skipped, 1);
        }
    });
}

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
            let p = producer_buf.try_producer().unwrap();
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
            let c = buf.try_consumer().unwrap();
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
            let p = producer_buf.try_producer().unwrap();
            let _ = p.push(1);
            let _ = p.push(2);
        });

        let consumer_buf = Arc::clone(&buf);
        let consumer = thread::spawn(move || {
            let c = consumer_buf.try_consumer().unwrap();
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
            let p = producer_buf.try_producer().unwrap();
            let _ = p.push(0xAAAA_AAAA);
            let _ = p.push(0xBBBB_BBBB);
        });

        let consumer = thread::spawn(move || {
            let c = buf.try_consumer().unwrap();
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
            let p = producer_ring.try_producer().unwrap();
            let first = p.push(10);
            let second = p.push(20);
            assert_eq!(first, 1, "first push must take sequence 1");
            assert_eq!(second, 2, "sequences must be consecutive");
        });

        let consumer = thread::spawn(move || {
            let mut c = ring.try_consumer().unwrap();
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
            let p = producer_ring.try_producer().unwrap();
            p.push(10);
            p.push(20);
        });

        let consumer = thread::spawn(move || {
            let c = ring.try_consumer().unwrap();
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
