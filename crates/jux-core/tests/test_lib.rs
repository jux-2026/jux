#[test]
fn exposes_package_version() {
    assert_eq!(jux_core::version(), env!("CARGO_PKG_VERSION"));
}
