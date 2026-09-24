//! The daily background update check's decisions (FR-CFG-6): once a
//! day at most, a newer release recognized from whatever tag shape the
//! repo uses, and a stamp file that survives a process boundary.

use lca_cli::update;
use std::time::{Duration, SystemTime};

// Verifies: FR-CFG-6 (at most once per day: a first run with no stamp
// is due, a stamp inside the day is not, a day-old stamp is again, and
// a future-dated stamp never grants a second check - the requirement
// says "at most", so the gate fails closed).
#[test]
fn the_check_is_due_at_most_once_a_day() {
    let now = SystemTime::now();
    assert!(update::due(None, now), "no stamp: due");

    let twenty_three_hours = now - Duration::from_secs(23 * 60 * 60);
    assert!(
        !update::due(Some(twenty_three_hours), now),
        "inside the day: not due"
    );

    let a_day_ago = now - Duration::from_secs(24 * 60 * 60);
    assert!(update::due(Some(a_day_ago), now), "a day old: due again");

    let future = now + Duration::from_secs(60);
    assert!(
        !update::due(Some(future), now),
        "future stamp: not due (clock skew must not buy a second check)"
    );
}

// Verifies: FR-CFG-6 (a strictly newer release is recognized: the
// repo's decorated tags, a v-prefix, and plain semver all parse; an
// equal or older tag does not count; anything unparseable answers no,
// because a check that cannot compare must not nag).
#[test]
fn newer_release_detection() {
    assert!(update::is_newer("phase6-0.2.0", "0.1.0"), "decorated newer");
    assert!(update::is_newer("v0.1.1", "0.1.0"), "v-prefixed newer");
    assert!(update::is_newer("1.0.0", "0.1.0"), "plain newer");
    assert!(!update::is_newer("phase5-0.1.0", "0.1.0"), "same semver");
    assert!(!update::is_newer("0.0.9", "0.1.0"), "older never nags");
    assert!(!update::is_newer("garbage", "0.1.0"), "unparseable: no");
    assert!(
        !update::is_newer("0.2.0", "not-a-version"),
        "either side: no"
    );
}

#[test]
fn the_stamp_survives_a_round_trip() {
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("lca-update-stamp-{unique}"));
    std::fs::create_dir_all(&dir).expect("temp dir");

    assert!(update::read_last(&dir).is_none(), "no stamp yet");
    update::write_last(&dir).expect("stamp written");
    let first = update::read_last(&dir).expect("stamp readable");
    assert!(
        !update::due(Some(first), SystemTime::now()),
        "just stamped: not due"
    );

    std::fs::remove_dir_all(&dir).ok();
}
