//! Fixture compiled to a `cdylib` by the dynamic tests: a plain library
//! with none of the protocol exports.

pub fn hello() -> &'static str {
    "not a plugin"
}
