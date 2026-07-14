use jux_core::{
    DISTRIBUTION_METADATA_SLOT_SIZE, DistributionChannel, DistributionMetadata, InstallerKind,
    embedded_distribution_metadata, inject_distribution_metadata,
};

#[test]
fn unbranded_build_exposes_unknown_channel() {
    let metadata = embedded_distribution_metadata().expect("embedded metadata");

    assert_eq!(metadata.channel, DistributionChannel::Unknown);
    assert_eq!(metadata.installer, InstallerKind::Unknown);
    assert_eq!(metadata.application_version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn slot_is_exactly_one_kibibyte_and_round_trips() {
    let metadata = DistributionMetadata::new(
        DistributionChannel::Homebrew,
        InstallerKind::Homebrew,
        "1.2.3",
        "0123456789012345678901234567890123456789",
    )
    .expect("metadata");

    let slot = metadata.to_slot().expect("slot");
    assert_eq!(slot.len(), DISTRIBUTION_METADATA_SLOT_SIZE);
    assert_eq!(
        DistributionMetadata::from_slot(&slot).expect("parsed metadata"),
        metadata
    );
}

#[test]
fn injector_changes_only_the_reserved_slot() {
    let temporary = assert_fs::TempDir::new().expect("temporary directory");
    let input = temporary.path().join("input");
    let output = temporary.path().join("output");
    let prefix = b"executable-prefix";
    let suffix = b"executable-suffix";
    let blank = embedded_slot();
    let mut executable = prefix.to_vec();
    executable.extend_from_slice(&blank);
    executable.extend_from_slice(suffix);
    std::fs::write(&input, &executable).expect("write input");
    let metadata = DistributionMetadata::new(
        DistributionChannel::GithubRelease,
        InstallerKind::Bash,
        "0.2.0",
        "abcdef",
    )
    .expect("metadata");

    inject_distribution_metadata(&input, &output, &metadata).expect("inject metadata");

    let branded = std::fs::read(output).expect("read output");
    assert_eq!(&branded[..prefix.len()], prefix);
    assert_eq!(&branded[branded.len() - suffix.len()..], suffix);
    assert_eq!(branded.len(), executable.len());
    let slot = &branded[prefix.len()..prefix.len() + DISTRIBUTION_METADATA_SLOT_SIZE];
    assert_eq!(
        DistributionMetadata::from_slot(slot).expect("slot"),
        metadata
    );
}

fn embedded_slot() -> [u8; DISTRIBUTION_METADATA_SLOT_SIZE] {
    let mut slot = DistributionMetadata::unbranded()
        .to_slot()
        .expect("unbranded slot");
    slot[64] = 0;
    slot[65] = 0;
    slot
}
