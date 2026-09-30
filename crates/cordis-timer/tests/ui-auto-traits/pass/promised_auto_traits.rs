//! Every auto trait the normative interface promises for `cordis-timer`
//! (§Openness and auto traits). The operation futures are promised `Send`
//! when their inner future is `Send`; neither `Sync` nor `Unpin` is promised
//! for them, so neither is asserted here.

use std::future::{self, Future};

use cordis_timer::{
    Interval, Sleep, Timeout, TimeoutOutcome, TimerCancelled, TimerRegistrationError,
};

fn assert_send_sync<T: Send + Sync>() {}

fn assert_send<T: Send>() {}

/// `Timeout<F>` is `Send` for every `Send` work future `F`, and its outcome is
/// `Send + Sync` for every `Send + Sync` value.
fn generic_shapes<F: Future + Send, T: Send + Sync>() {
    assert_send::<Timeout<F>>();
    assert_send_sync::<TimeoutOutcome<T>>();
}

fn main() {
    // Errors.
    assert_send_sync::<TimerRegistrationError>();
    assert_send_sync::<TimerCancelled>();

    // Operation futures and the Timeout outcome.
    assert_send::<Sleep>();
    assert_send::<Interval>();
    let _ = generic_shapes::<future::Pending<()>, u8>;
}
