# cordis-timer

`cordis-timer` adds complete, generation-owned time operations to a Cordis v3
`Context` without making time policy part of `cordis-core`.

It provides three operations through `TimerExt`:

- `sleep(delay)` — one pinned one-shot delay;
- `interval(period)` — a fixed-phase stream of ticks;
- `timeout(delay, work)` — owns and races one caller Future against a pinned
  deadline.

## Installation

For an application using the `cordis` facade:

```toml
[dependencies]
cordis-rs = "0.8"
cordis-timer = "0.3"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time"] }
```

Framework and Plugin crates may depend on `cordis-core = "0.3"` instead of
`cordis-rs`; `TimerExt` is implemented for the same underlying `Context` type.

## Quick example

```rust
use std::time::Duration;

use cordis::{BoxError, Context};
use cordis_timer::{TimeoutOutcome, TimerExt};

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let ctx = Context::new();

    ctx.sleep(Duration::from_millis(10))?.await?;

    let outcome = ctx
        .timeout(Duration::from_secs(1), async { 42 })?
        .await?;

    match outcome {
        TimeoutOutcome::Completed(value) => assert_eq!(value, 42),
        TimeoutOutcome::Elapsed => println!("deadline elapsed"),
    }

    Ok(())
}
```

A usable Tokio time environment must be current when an operation is
constructed.

## Lifecycle semantics

Timer operations are not detached scheduler handles. Successful construction
registers exactly one cleanup obligation with the selected Context generation.
That gives timer completion three intentionally different classes of outcome:

- **registration refusal** — construction returns `TimerRegistrationError` and no
  operation is delivered;
- **normal timer outcome** — sleep completes, interval yields a tick, or timeout
  returns `TimeoutOutcome::{Completed, Elapsed}`;
- **generation cancellation** — a live operation returns `TimerCancelled` when
  the generation that registered it closes.

Timeout expiry is therefore not the same thing as lifecycle cancellation.
`TimeoutOutcome::Elapsed` is a normal timer result; `TimerCancelled` says the
owning Cordis generation ended first.

## Operation details

### Sleep

`Context::sleep` pins its monotonic deadline during successful construction. A
zero delay is valid. Natural completion disarms the generation cleanup
occurrence.

### Interval

`Context::interval` rejects a zero period. Its phase is anchored at successful
construction; missed ticks coalesce without shifting that phase. Generation
cancellation appears once as `Err(TimerCancelled)` and then the stream ends.

Use `futures::StreamExt` (or another `Stream` consumer) to await interval ticks.

### Timeout

`Context::timeout` owns the supplied Future. Construction does not poll the
Future. The deadline is pinned before the operation is delivered, and completion
returns either the Future output in `Completed(T)` or `Elapsed`.

Dropping a live timer operation abandons it. Drop does not synthesize a
cancellation result, and later generation cleanup does not perform a second
user-visible timer action.

## Boundary with `cordis-core`

`cordis-core` knows only that a generation has a cleanup obligation. It does not
know about Tokio deadlines, interval schedulers, timeout arbitration, or timer
outcome types. Those semantics belong entirely to this crate.

## Compatibility and MSRV

`cordis-timer` v3 began at `0.1.x`; the current `0.3.x` line depends on
`cordis-core 0.3.x`, requires **Rust 1.88 or newer**, and uses Rust edition 2024.

## Documentation

- [Cordis repository](https://github.com/dshbox/cordis-rs)
- [Timer architecture](https://github.com/dshbox/cordis-rs/blob/main/docs/v3-architecture.md)
- [v3 public timer interface](https://github.com/dshbox/cordis-rs/blob/main/docs/v3-public-interface.md#timer-facade-and-operations)
- [v3 migration inventory](https://github.com/dshbox/cordis-rs/blob/main/docs/v3-migration.md)

Licensed under MIT.
