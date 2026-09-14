//! Negative application-facade surface checks.

#[test]
fn internal_core_seams_do_not_escape_the_application_facade() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui-facade/fail/internal_seam.rs");
}
