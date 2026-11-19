use super::*;

#[test]
fn relative_times() {
    let now = 1_700_000_000;
    assert_eq!(relative_time(now, now), "just now");
    assert_eq!(relative_time(now + 30, now), "just now");
    assert_eq!(relative_time(now - 60, now), "1 minute ago");
    assert_eq!(relative_time(now - 5 * 60, now), "5 minutes ago");
    assert_eq!(relative_time(now - 3 * 3600, now), "3 hours ago");
    assert_eq!(relative_time(now - 30 * 3600, now), "yesterday");
    assert_eq!(relative_time(now - 4 * 86_400, now), "4 days ago");
    assert_eq!(relative_time(now - 65 * 86_400, now), "2 months ago");
    assert_eq!(relative_time(now - 800 * 86_400, now), "2 years ago");
}

#[test]
fn local_dates_have_fixed_shapes() {
    assert_eq!(local_date(1_700_000_000).len(), "2023-11-14".len());
    assert_eq!(
        local_date_time(1_700_000_000).len(),
        "2023-11-14 22:13".len()
    );
}

#[test]
fn truncates_by_characters() {
    assert_eq!(truncate_chars("Ada", 5), "Ada");
    assert_eq!(truncate_chars("Ada Lovelace", 5), "Ada …");
    assert_eq!(truncate_chars("Владимир", 4), "Вла…");
}
