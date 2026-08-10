//! Declarative bring-up for a `static` SPSC buffer.
//!
//! This is the crate's one concession to ergonomics, and it is made the way
//! `AGENTS.md` says to make it: **at compile time, for zero runtime cost**.
//! The macro expands to a `static`, two type aliases, and one function. It
//! introduces no allocation, no indirection, and no instruction that would not
//! be there if you wrote it out.

/// Declare a `static` SPSC buffer with its handle types and a one-shot take.
///
/// # What it solves
///
/// A `const fn new` puts the buffer in `.bss` — no flash, no startup code. The
/// handles are a different matter: they are `Send + !Sync`, which is exactly
/// what makes it sound to move a producer into an ISR and a consumer into a
/// task loop, and a `static` requires `Sync`. **Handles can therefore never
/// live in a `static`, whatever the constructor looks like.** Taking them stays
/// a runtime step, permanently.
///
/// What is left is boilerplate, and it is genuinely awkward: every function
/// that accepts a handle must spell out
/// `ph_eventing::event_buf::Producer<'static, u32, 64>`. This macro names those
/// types for you.
///
/// # Example
///
/// ```
/// ph_eventing::static_spsc! {
///     /// Telemetry from the sampling ISR to the reporting task.
///     pub mod telemetry: EventBuf<u32, 64>;
/// }
///
/// // `Tx` and `Rx` are ordinary type aliases — usable in signatures.
/// fn on_sample(tx: &telemetry::Tx, v: u32) {
///     let _ = tx.push(v);
/// }
///
/// let (tx, rx) = telemetry::take().expect("first take");
/// on_sample(&tx, 7);
/// assert_eq!(rx.pop(), Some(7));
///
/// // SPSC is still enforced: there is only ever one of each.
/// assert!(telemetry::take().is_none());
/// ```
///
/// `SeqRing` works the same way:
///
/// ```
/// ph_eventing::static_spsc! {
///     pub mod events: SeqRing<u32, 32>;
/// }
///
/// let (tx, mut rx) = events::take().expect("first take");
/// tx.push(1);
/// assert_eq!(rx.poll_one_value(), Some((1, 1)));
/// ```
///
/// # Notes
///
/// - `take()` is **all-or-nothing**. If the consumer cannot be taken it drops
///   the producer rather than leaving the buffer half-claimed, so a failed call
///   leaves nothing stranded.
/// - It uses the fallible constructors, so no panic path reaches your binary.
///   On a microcontroller a panic is a reset, and the panic machinery costs
///   flash.
/// - The generated module owns its `static`. Nothing else can reach the buffer
///   except through `take()`, which is what keeps the SPSC guarantee honest.
#[macro_export]
macro_rules! static_spsc {
    (
        $(#[$attr:meta])*
        $vis:vis mod $name:ident : EventBuf<$t:ty, $n:tt>;
    ) => {
        $(#[$attr])*
        // The expansion cannot know whether the caller's context makes these
        // reachable; the crate denies `unreachable_pub` for its own code, which
        // should not leak into generated code.
        #[allow(unreachable_pub)]
        $vis mod $name {
            // Needed when `$t` names a type from the caller's scope; unused
            // when it is a primitive, which is the common case.
            #[allow(unused_imports)]
            use super::*;

            /// Write handle. `Send + !Sync`: move it into the producing context.
            pub type Tx = $crate::event_buf::Producer<'static, $t, $n>;
            /// Read handle. `Send + !Sync`: move it into the consuming context.
            pub type Rx = $crate::event_buf::Consumer<'static, $t, $n>;

            static BUF: $crate::EventBuf<$t, $n> = $crate::EventBuf::new();

            /// Take both handles. `None` if they have already been taken.
            ///
            /// All-or-nothing: a failed call leaves the buffer untouched.
            pub fn take() -> ::core::option::Option<(Tx, Rx)> {
                let tx = BUF.try_producer()?;
                match BUF.try_consumer() {
                    ::core::option::Option::Some(rx) => {
                        ::core::option::Option::Some((tx, rx))
                    }
                    // Dropping the producer restores its flag, so a failed take
                    // cannot strand the buffer half-claimed.
                    ::core::option::Option::None => {
                        ::core::mem::drop(tx);
                        ::core::option::Option::None
                    }
                }
            }

            /// The buffer itself, for observers like `len()`.
            pub fn buffer() -> &'static $crate::EventBuf<$t, $n> {
                &BUF
            }
        }
    };

    (
        $(#[$attr:meta])*
        $vis:vis mod $name:ident : SeqRing<$t:ty, $n:tt>;
    ) => {
        $(#[$attr])*
        // The expansion cannot know whether the caller's context makes these
        // reachable; the crate denies `unreachable_pub` for its own code, which
        // should not leak into generated code.
        #[allow(unreachable_pub)]
        $vis mod $name {
            // Needed when `$t` names a type from the caller's scope; unused
            // when it is a primitive, which is the common case.
            #[allow(unused_imports)]
            use super::*;

            /// Write handle. `Send + !Sync`: move it into the producing context.
            pub type Tx = $crate::seq_ring::Producer<'static, $t, $n>;
            /// Read handle. `Send + !Sync`: move it into the consuming context.
            pub type Rx = $crate::seq_ring::Consumer<'static, $t, $n>;

            static RING: $crate::SeqRing<$t, $n> = $crate::SeqRing::new();

            /// Take both handles. `None` if they have already been taken.
            ///
            /// All-or-nothing: a failed call leaves the ring untouched.
            pub fn take() -> ::core::option::Option<(Tx, Rx)> {
                let tx = RING.try_producer()?;
                match RING.try_consumer() {
                    ::core::option::Option::Some(rx) => {
                        ::core::option::Option::Some((tx, rx))
                    }
                    ::core::option::Option::None => {
                        ::core::mem::drop(tx);
                        ::core::option::Option::None
                    }
                }
            }

            /// The ring itself, for observers like `capacity()`.
            pub fn ring() -> &'static $crate::SeqRing<$t, $n> {
                &RING
            }
        }
    };
}

#[cfg(all(test, not(loom)))]
mod tests {
    // The macro is used at module scope, so each case needs its own module.
    crate::static_spsc! {
        /// Doc attributes must pass through.
        pub mod eb: EventBuf<u32, 4>;
    }

    crate::static_spsc! {
        pub mod sr: SeqRing<u32, 4>;
    }

    #[test]
    fn event_buf_module_round_trips() {
        let (tx, rx) = eb::take().expect("first take");
        tx.push(7).unwrap();
        assert_eq!(rx.pop(), Some(7));
        assert_eq!(eb::buffer().capacity(), 4);
    }

    #[test]
    fn event_buf_take_is_once_only() {
        // Ordering between tests is not guaranteed, so assert the invariant
        // rather than a particular first/second outcome: there is never more
        // than one live pair.
        let first = eb::take();
        let second = eb::take();
        assert!(
            first.is_none() || second.is_none(),
            "take() handed out two live pairs"
        );
    }

    #[test]
    fn seq_ring_module_round_trips() {
        let (tx, mut rx) = sr::take().expect("first take");
        tx.push(9);
        assert_eq!(rx.poll_one_value(), Some((1, 9)));
        assert_eq!(sr::ring().capacity(), 4);
    }

    /// The property that separates `take()` from two separate calls: a failed
    /// take must not leave the buffer half-claimed.
    #[test]
    fn failed_take_strands_nothing() {
        crate::static_spsc! {
            mod partial: EventBuf<u32, 2>;
        }

        // Hold the consumer, so `take` gets past the producer and then fails.
        let rx = partial::buffer().try_consumer().expect("consumer");
        assert!(partial::take().is_none());

        // If `take` had leaked the producer, this would be None.
        let tx = partial::buffer()
            .try_producer()
            .expect("producer must still be free after a failed take");
        tx.push(1).unwrap();
        assert_eq!(rx.pop(), Some(1));
    }

    #[test]
    fn handles_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<eb::Tx>();
        assert_send::<eb::Rx>();
        assert_send::<sr::Tx>();
        assert_send::<sr::Rx>();
    }
}
