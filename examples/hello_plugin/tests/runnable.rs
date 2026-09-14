//! EX-02 executable evidence for the smallest typed Cordis on-ramp.

use std::process::{Command, Stdio};

#[test]
fn hello_plugin_runs_without_tty_and_demonstrates_behavior_and_teardown() {
    let output = Command::new(env!("CARGO_BIN_EXE_hello_plugin"))
        .stdin(Stdio::null())
        .output()
        .expect("hello_plugin should launch");

    assert!(
        output.status.success(),
        "hello_plugin exited unsuccessfully: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("example output is UTF-8");
    assert!(
        stdout.contains("✓ Echo up"),
        "spawn must hand off a live quiescent FiberHandle: {stdout}"
    );
    assert!(
        stdout.contains("hello, world"),
        "Plugin behavior must be observable: {stdout}"
    );
    assert!(
        stdout.contains("✓ Echo down"),
        "consumer teardown must dispose the FiberHandle: {stdout}"
    );
    assert!(
        stdout.contains("the second Ping printed nothing"),
        "disposed Plugin listener must be gone before clean exit: {stdout}"
    );
}
