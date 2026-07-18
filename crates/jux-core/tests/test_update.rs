use jux_core::{
    DistributionChannel, DistributionMetadata, InstallerKind, UPDATE_CHECK_INTERVAL, UpdateCache,
    UpdateRecommendation,
};
use semver::Version;

#[test]
fn cache_is_due_after_twenty_four_hours() {
    let cache = cache_at(1_000);

    assert!(!cache.should_check(1_000 + UPDATE_CHECK_INTERVAL.as_secs() - 1));
    assert!(cache.should_check(1_000 + UPDATE_CHECK_INTERVAL.as_secs()));
}

#[test]
fn startup_notification_is_emitted_once_per_latest_version() {
    let mut cache = cache_at(1_000);

    assert!(cache.needs_startup_notification());
    cache.startup_notified_version = Some(cache.latest_version.clone());
    assert!(!cache.needs_startup_notification());
}

#[test]
fn recommendation_uses_fixed_homebrew_command() {
    let metadata = DistributionMetadata::new(
        DistributionChannel::Homebrew,
        InstallerKind::Homebrew,
        "0.1.0",
        "abcdef",
    )
    .expect("metadata");

    let recommendation = UpdateRecommendation::for_distribution(&metadata);

    assert_eq!(recommendation.channel, DistributionChannel::Homebrew);
    assert_eq!(
        recommendation.command.expect("command").display(),
        "brew upgrade jux-2026/tap/jux"
    );
}

#[test]
fn recommendation_uses_fixed_npm_command() {
    let metadata = DistributionMetadata::new(
        DistributionChannel::Npm,
        InstallerKind::Npm,
        "0.1.0",
        "abcdef",
    )
    .expect("metadata");

    let recommendation = UpdateRecommendation::for_distribution(&metadata);

    assert_eq!(recommendation.channel, DistributionChannel::Npm);
    assert_eq!(
        recommendation.command.expect("command").display(),
        "npm update -g @jux-2026/jux"
    );
}

fn cache_at(last_checked_at: u64) -> UpdateCache {
    UpdateCache {
        schema_version: 1,
        last_checked_at,
        current_version: Version::parse("0.1.0").expect("version"),
        latest_version: Version::parse("0.2.0").expect("version"),
        release_url: "https://github.com/jux-2026/jux/releases/tag/v0.2.0".to_owned(),
        startup_notified_version: None,
    }
}
