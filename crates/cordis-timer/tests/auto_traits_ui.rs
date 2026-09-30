//! Compile evidence for the auto traits the normative interface promises.

use trybuild::TestCases;

#[test]
fn promised_auto_traits() {
    let t = TestCases::new();
    t.pass("tests/ui-auto-traits/pass/*.rs");
}
