use super::*;

#[test]
fn exposes_package_version() {
    assert_eq!(version(), "0.1.0");
}
