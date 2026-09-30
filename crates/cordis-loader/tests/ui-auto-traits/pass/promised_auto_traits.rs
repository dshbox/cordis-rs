//! Every auto trait the normative interface promises for `cordis-loader`
//! (§Openness and auto traits). `Unpin` is never promised, so it is never
//! asserted here.

use cordis_loader::{
    EntryId, LoadOutcome,
    outcome::{EntryOutcome, LoaderFailure},
    plan::PlanError,
    resolver::{JsonPrepareError, ResolverFailure, ResolverFailureKind},
};

fn assert_send_sync<T: Send + Sync>() {}

/// `JsonPrepareError<E>` is `Send + Sync` for every `Send + Sync` `E`.
fn json_prepare_error<E: std::error::Error + Send + Sync>() {
    assert_send_sync::<JsonPrepareError<E>>();
}

fn main() {
    // Errors and failures, with their kinds.
    assert_send_sync::<PlanError>();
    assert_send_sync::<LoaderFailure>();
    assert_send_sync::<ResolverFailure>();
    assert_send_sync::<ResolverFailureKind>();
    let _ = json_prepare_error::<std::io::Error>;

    // Correlation identity.
    assert_send_sync::<EntryId>();

    // Outcomes.
    assert_send_sync::<EntryOutcome>();
    assert_send_sync::<LoadOutcome>();
}
