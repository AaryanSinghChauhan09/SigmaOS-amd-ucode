//! Integration tests for SigmaOS-amd-ucode.

use sigmaos_amd_ucode_lib::Component;

#[test]
fn component_initialises_and_runs() {
    let mut c = Component::new();
    c.init().expect("init should succeed");
    c.run().expect("run should succeed after init");
}
