#[test]
fn exposes_package_version() {
    assert_eq!(jux_core::version(), "0.1.0");
}
