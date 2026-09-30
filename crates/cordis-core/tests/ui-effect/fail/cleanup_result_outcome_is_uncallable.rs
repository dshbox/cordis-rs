// `CleanupResult::into_outcome` is a doc-hidden required method of a
// nameable trait. It takes a crate-private token, so downstream code can
// call it neither directly nor through a generic `R: CleanupResult`
// bound: the token is unnameable, so it cannot be supplied and an
// `EffectFailure` cannot be forged.

use cordis_core::effect::CleanupResult;

#[derive(Debug)]
struct Forged;

impl std::fmt::Display for Forged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("forged")
    }
}

impl std::error::Error for Forged {}

fn forge_generic<R: CleanupResult>(r: R) {
    let _ = r.into_outcome(cordis_core::effect::sealed::OutcomeToken);
}

fn main() {
    let _ = Err::<(), Forged>(Forged).into_outcome(cordis_core::effect::sealed::OutcomeToken);
    forge_generic(Err::<(), Forged>(Forged));
}
