// The module holding `CleanupResult::into_outcome`'s token is private, so
// the token's path cannot be named downstream. This proves only path
// privacy; the sibling `cleanup_result_outcome_{missing,default,unit}_token`
// fixtures prove the method itself requires a token.

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
