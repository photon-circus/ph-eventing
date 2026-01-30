# ph-eventing

Eventing library for embedded systems with no-std support.

## Features
- Lock-free single-producer single-consumer sequence ring.
- Producer never blocks; consumer can be slow and will drop old items when it lags.
- No allocation, no dynamic dispatch.

## Usage
```rust
use ph_eventing::SeqRing;

let ring = SeqRing::<u32, 64>::new();
let producer = ring.producer();
let mut consumer = ring.consumer();

producer.push(123);
consumer.poll_one(|seq, v| {
    assert_eq!(seq, 1);
    assert_eq!(*v, 123);
});
```

## Semantics
- Sequence numbers are monotonically increasing `u32` values; `0` is reserved for "empty".
- When the producer wraps the ring, old values are overwritten.
- If the consumer lags by more than `N`, it skips ahead and reports drops via `PollStats`.

## Testing
Host tests require `std` and can be run with:
```
cargo test
```

## License
MIT. See `LICENSE`.
