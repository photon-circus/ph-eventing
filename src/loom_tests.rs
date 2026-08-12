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

use crate::{CountedSignal, EventBuf, EventFlags, EventMask, LatestBuf, SeqRing};
use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use loom::thread;

/// Concurrent takes partition increments between snapshots without losing or
/// duplicating them (CountedSignal contract T1-T3 and A1).
#[test]
fn counted_signal_take_partitions_increments() {
    loom::model(|| {
        let signal = Arc::new(CountedSignal::new());

        let producer_signal = Arc::clone(&signal);
        let producer = thread::spawn(move || {
            let producer = producer_signal.try_producer().unwrap();
            producer.increment();
            producer.increment();
        });

        let consumer_signal = Arc::clone(&signal);
        let consumer = thread::spawn(move || {
            let consumer = consumer_signal.try_consumer().unwrap();
            u64::from(consumer.take_count().count()) + u64::from(consumer.take_count().count())
        });

        producer.join().unwrap();
        let early = consumer.join().unwrap();
        let final_count = signal.try_consumer().unwrap().take_count().count();
        assert_eq!(early + u64::from(final_count), 2);
    });
}

/// At the saturation boundary, a take between the producer's observe and
/// commit moves the increment into the new epoch; it cannot make the RMW wrap
/// or lose the increment (contract I2-I3, T2-T3, and A2-A3).
#[test]
fn counted_signal_saturation_boundary_is_linearizable() {
    loom::model(|| {
        let signal = Arc::new(CountedSignal::with_count_for_model(u32::MAX - 1));

        let producer_signal = Arc::clone(&signal);
        let producer = thread::spawn(move || {
            producer_signal.try_producer().unwrap().increment();
        });

        let consumer_signal = Arc::clone(&signal);
        let consumer =
            thread::spawn(move || consumer_signal.try_consumer().unwrap().take_count().count());

        producer.join().unwrap();
        let early = consumer.join().unwrap();
        let final_count = signal.try_consumer().unwrap().take_count().count();
        assert_eq!(
            u64::from(early) + u64::from(final_count),
            u64::from(u32::MAX)
        );
    });
}

/// Seeded at `MAX`, a take that has already returned must not let a later
/// `increment` disappear: wall-clock order is established only by a Relaxed
/// gate (no Acquire/Release on the count). A stale MAX short-circuit would
/// lose the occurrence from both intervals (contract T3 and A1).
#[test]
fn counted_signal_post_take_increment_observes_reset_epoch() {
    loom::model(|| {
        let signal = Arc::new(CountedSignal::with_count_for_model(u32::MAX));
        let gate = Arc::new(AtomicU32::new(0));

        let producer_signal = Arc::clone(&signal);
        let producer_gate = Arc::clone(&gate);
        let producer = thread::spawn(move || {
            let producer = producer_signal.try_producer().unwrap();
            while producer_gate.load(Ordering::Relaxed) == 0 {
                thread::yield_now();
            }
            producer.increment();
        });

        let consumer_signal = Arc::clone(&signal);
        let consumer_gate = Arc::clone(&gate);
        let consumer = thread::spawn(move || {
            let consumer = consumer_signal.try_consumer().unwrap();
            let early = consumer.take_count().count();
            consumer_gate.store(1, Ordering::Relaxed);
            early
        });

        producer.join().unwrap();
        let early = consumer.join().unwrap();
        let final_count = signal.try_consumer().unwrap().take_count().count();
        assert_eq!(early, u32::MAX);
        assert_eq!(final_count, 1);
    });
}

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
            // Every individual report is preserved — P4 is a per-call claim,
            // and a per-call bitmap is what catches a report that is right in
            // aggregate but wrong on two calls (e.g. [false, true, false]
            // where [false, false, true] is correct).
            let mut replaced = [false; 3];
            for value in 1..=3u32 {
                replaced[(value - 1) as usize] = producer.publish([value, value]).replaced_unread;
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
            let mut taken = [false; 3];
            for _ in 0..3 {
                if let Some(item) = consumer.take_latest() {
                    assert!((1..=3).contains(&item.generation));
                    assert_eq!(item.value, [item.generation; 2]);
                    assert!(item.generation > last_generation);
                    assert_eq!(item.skipped, item.generation - last_generation - 1);
                    last_generation = item.generation;
                    taken[(item.generation - 1) as usize] = true;
                }
                thread::yield_now();
            }
            taken
        });

        let replaced = producer.join().unwrap();
        let taken = consumer.join().unwrap();
        let pending = channel
            .try_consumer()
            .unwrap()
            .take_latest()
            .map(|item| item.generation);

        // Per-publication correlation (P4): publish(k) displaced an unread
        // value if and only if generation k-1 was neither taken by the
        // consumer nor left pending at the end. Once publish(k) lands,
        // generation k-1 is either already taken or displaced — it can never
        // be taken later — so this holds in every interleaving. publish(1)
        // found an empty channel and can never report a displacement.
        assert!(!replaced[0]);
        for k in 2..=3u32 {
            let predecessor_survived = taken[(k - 2) as usize] || pending == Some(k - 1);
            assert_eq!(
                replaced[(k - 1) as usize],
                !predecessor_survived,
                "publish({k}) report disagrees with its predecessor's fate"
            );
        }

        // Aggregate conservation stays as a second, independent check: every
        // publication ends in exactly one bucket — taken, displaced while
        // unread, or pending at the end.
        let replaced_count = replaced.iter().filter(|&&r| r).count();
        let taken_count = taken.iter().filter(|&&t| t).count();
        assert_eq!(
            replaced_count + taken_count + usize::from(pending.is_some()),
            3
        );
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
            let take_spinning = |expected: u32| loop {
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
        let first_dropped = Arc::new(AtomicBool::new(false));
        let first_channel = Arc::clone(&channel);
        let first_signal = Arc::clone(&first_dropped);
        let first = thread::spawn(move || {
            let producer = first_channel.try_producer().unwrap();
            assert_eq!(producer.publish(1).generation, 1);
            drop(producer);
            // Relaxed deliberately: the role handoff's own Release-drop /
            // AcqRel-swap edge must carry the continuation state; this flag
            // only sequences the model and supplies no happens-before for it.
            first_signal.store(true, Ordering::Relaxed);
        });

        let second = thread::spawn(move || {
            while !first_dropped.load(Ordering::Relaxed) {
                thread::yield_now();
            }
            // The acquisition swap is an RMW, so it reads the latest
            // `producer_taken` value in modification order: after the
            // observed drop it succeeds on the first try, and every
            // terminating execution completes the cross-context handoff
            // (evaluation L3) instead of treating a failed claim as a
            // vacuous pass.
            let producer = channel
                .try_producer()
                .expect("role released by the observed drop");
            producer.publish(2).generation
        });

        first.join().unwrap();
        // Continuation is unconditional: generation resumes, never restarts.
        assert_eq!(second.join().unwrap(), 2);
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
            drop(consumer);
            // Relaxed deliberately: successful reacquisition, not this test
            // signal, must publish the role-owned continuation state.
            first_signal.store(true, Ordering::Relaxed);
        });

        let publisher_channel = Arc::clone(&channel);
        let publisher_start = Arc::clone(&first_took);
        let publisher_done = Arc::clone(&later_published);
        let publisher = thread::spawn(move || {
            // Yield-retry until the first take happened: every terminating
            // execution publishes generations 2 and 3 into the post-take
            // channel state instead of skipping the scenario.
            while !publisher_start.load(Ordering::Relaxed) {
                thread::yield_now();
            }
            let producer = publisher_channel.try_producer().unwrap();
            let _ = producer.publish(2);
            let _ = producer.publish(3);
            publisher_done.store(true, Ordering::Release);
        });

        let second = thread::spawn(move || {
            while !later_published.load(Ordering::Acquire) {
                thread::yield_now();
            }
            // The first handle was dropped before this flag chain fired, and
            // the acquisition swap is an RMW reading the latest role flag in
            // modification order — the handoff completes in every
            // terminating execution (evaluation L3), no vacuous branch.
            let consumer = channel
                .try_consumer()
                .expect("role released by the observed drop");
            consumer
                .take_latest()
                .expect("generation 3 pending after the Release/Acquire handoff")
        });

        first.join().unwrap();
        publisher.join().unwrap();
        let item = second.join().unwrap();
        assert_eq!(item.generation, 3);
        assert_eq!(item.value, 3);
        assert_eq!(item.skipped, 1);
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

/// The frozen entry-sample poll window (the bounded-poll fix) must neither
/// lose nor double-count items across the call boundary: in every
/// interleaving, each published sequence ends up delivered or counted
/// dropped exactly once, and deliveries arrive in strictly increasing
/// sequence order.
///
/// Payload VALUES are deliberately not asserted, matching the scoping of the
/// other SeqRing models: doing so directly reproduces the documented formal
/// seqlock race (verified on both the old and the bounded poll loop — Loom
/// serialises the non-atomic slot memory to latest-value semantics while
/// C11 coherence lets both Relaxed sequence checks keep returning the stale
/// pre-invalidation sequence, because a non-atomic read establishes no
/// happens-before). The producer's Release fence and the consumer's Acquire
/// fence are what close that window on real hardware; see the module's
/// "Known deviation" section, which cites this witness.
#[test]
fn seq_ring_frozen_poll_window_conserves_under_concurrent_publish() {
    loom::model(|| {
        let ring = Arc::new(SeqRing::<u32, 2>::new());
        let done = Arc::new(AtomicBool::new(false));

        let producer_ring = Arc::clone(&ring);
        let producer_done = Arc::clone(&done);
        let producer = thread::spawn(move || {
            let producer = producer_ring.try_producer().unwrap();
            for value in 1..=3u32 {
                producer.push(value * 10);
            }
            // Release pairs with the consumer's Acquire: once `done` is
            // observed, every publication above is visible, so the final
            // empty poll below is conclusive rather than racy.
            producer_done.store(true, Ordering::Release);
        });

        let consumer_ring = Arc::clone(&ring);
        let consumer = thread::spawn(move || {
            let mut consumer = consumer_ring.try_consumer().unwrap();
            let mut read = 0usize;
            let mut dropped = 0usize;
            let mut last_seq = 0u32;
            loop {
                // Observe `done` BEFORE polling: only an empty poll that
                // happens-after the producer's Release store is conclusive
                // evidence that nothing remains.
                let producer_was_done = done.load(Ordering::Acquire);
                let stats = consumer.poll_up_to(2, |seq, _value| {
                    assert!(seq > last_seq);
                    last_seq = seq;
                });
                read += stats.read;
                dropped += stats.dropped;
                if producer_was_done && stats.read == 0 && stats.dropped == 0 {
                    break;
                }
                thread::yield_now();
            }
            (read, dropped)
        });

        producer.join().unwrap();
        let (read, dropped) = consumer.join().unwrap();
        // Exact conservation within the span: three publications, each
        // delivered or dropped exactly once, never both, never neither.
        assert_eq!(read + dropped, 3);
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

/// One raise racing one take is returned in exactly one take window: never
/// lost, duplicated, or fabricated (EventFlags contract C1-C3).
#[test]
fn event_flags_raise_racing_take_is_partitioned_exactly() {
    loom::model(|| {
        let flags = Arc::new(EventFlags::new());
        let condition = EventMask::from_bits(1 << 3);

        let producer_flags = Arc::clone(&flags);
        let producer = thread::spawn(move || {
            producer_flags
                .try_producer()
                .expect("sole producer")
                .raise(condition);
        });

        let consumer_flags = Arc::clone(&flags);
        let first_take = thread::spawn(move || {
            consumer_flags
                .try_consumer()
                .expect("sole consumer")
                .take_all()
        });

        producer.join().unwrap();
        let first = first_take.join().unwrap();
        let second = flags
            .try_consumer()
            .expect("consumer role released")
            .take_all();

        assert_eq!(
            first | second,
            condition,
            "the raise was lost or fabricated"
        );
        assert!(
            (first & second).is_empty(),
            "one raise appeared in two take windows"
        );
    });
}

/// Distinct raises are distributed exactly across a racing take and the final
/// pending set (EventFlags contract R1, T1, and C1-C3).
#[test]
fn event_flags_distinct_raises_partition_across_takes() {
    loom::model(|| {
        let flags = Arc::new(EventFlags::new());
        let first_condition = EventMask::from_bits(1 << 0);
        let second_condition = EventMask::from_bits(1 << 31);

        let producer_flags = Arc::clone(&flags);
        let producer = thread::spawn(move || {
            let producer = producer_flags.try_producer().expect("sole producer");
            producer.raise(first_condition);
            producer.raise(second_condition);
        });

        let consumer_flags = Arc::clone(&flags);
        let first_take = thread::spawn(move || {
            consumer_flags
                .try_consumer()
                .expect("sole consumer")
                .take_all()
        });

        producer.join().unwrap();
        let first = first_take.join().unwrap();
        let second = flags
            .try_consumer()
            .expect("consumer role released")
            .take_all();
        let expected = first_condition | second_condition;

        assert_eq!(first | second, expected, "a distinct raise was lost");
        assert!(
            (first & second).is_empty(),
            "a condition was returned twice without being re-raised"
        );
        assert_eq!((first | second).bits() & !expected.bits(), 0);
    });
}

/// Observing a condition also observes memory written before its raise
/// (EventFlags contract S1). Changing either the Release `fetch_or` or Acquire
/// `swap` to Relaxed makes Loom find the stale-payload execution.
#[test]
fn event_flags_observed_raise_publishes_payload() {
    loom::model(|| {
        let flags = Arc::new(EventFlags::new());
        let payload = Arc::new(AtomicU32::new(0));
        let ready = EventMask::from_bits(1);

        let producer_flags = Arc::clone(&flags);
        let producer_payload = Arc::clone(&payload);
        let producer = thread::spawn(move || {
            let producer = producer_flags.try_producer().expect("sole producer");
            producer_payload.store(0xA5A5_5A5A, Ordering::Relaxed);
            producer.raise(ready);
        });

        let consumer = thread::spawn(move || {
            let consumer = flags.try_consumer().expect("sole consumer");
            while !consumer.take_all().contains(ready) {
                thread::yield_now();
            }
            assert_eq!(
                payload.load(Ordering::Relaxed),
                0xA5A5_5A5A,
                "the observed raise did not publish its payload"
            );
        });

        producer.join().unwrap();
        consumer.join().unwrap();
    });
}
