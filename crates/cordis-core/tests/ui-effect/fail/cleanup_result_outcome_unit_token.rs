// `CleanupResult::into_outcome` takes a crate-private token, so downstream
// code cannot call it. This fixture never names the token path: it passes
// `()` in place of the token, both directly and through a generic
// `R: CleanupResult` bound.

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
    let _ = r.into_outcome(());
}

fn main() {
    let _ = Err::<(), Forged>(Forged).into_outcome(());
    forge_generic(Err::<(), Forged>(Forged));
}
