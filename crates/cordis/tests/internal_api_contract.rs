//! Compatibility evidence for the published sibling `internal-api` seam.

#[test]
fn sibling_feature_unification_makes_direct_core_internal_reachable() {
    let ctx = cordis_core::Context::new();
    assert!(cordis_core::__internal::generation_cleanup_admitted(&ctx));

    // Type-check the Loader sibling seam as part of the same frozen contract.
    // The work is inert; this call only proves the published signature remains
    // available when a sibling activates `internal-api`.
    cordis_core::__internal::detach_completion(async {});
}
