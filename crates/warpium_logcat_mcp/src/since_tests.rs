use super::*;

fn now() -> DateTime<Local> {
    Local
        .from_local_datetime(
            &NaiveDate::from_ymd_opt(2026, 11, 15)
                .unwrap()
                .and_hms_milli_opt(10, 30, 0, 0)
                .unwrap(),
        )
        .unwrap()
}

fn local_secs(year: i32, month: u32, day: u32, h: u32, m: u32, s: u32) -> i64 {
    Local
        .from_local_datetime(
            &NaiveDate::from_ymd_opt(year, month, day)
                .unwrap()
                .and_hms_opt(h, m, s)
                .unwrap(),
        )
        .unwrap()
        .timestamp()
}

#[test]
fn parses_durations_with_units() {
    assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
    assert_eq!(parse_duration("2m"), Some(Duration::from_secs(120)));
    assert_eq!(parse_duration("2 min"), Some(Duration::from_secs(120)));
    assert_eq!(parse_duration("1.5h"), Some(Duration::from_secs(5400)));
    assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
}

#[test]
fn rejects_durations_without_a_known_unit() {
    assert_eq!(parse_duration("10"), None);
    assert_eq!(parse_duration("10:23:45"), None);
    assert_eq!(parse_duration("2d"), None);
}

#[test]
fn relative_since_counts_back_from_the_device_clock() {
    let since = parse_since("2m", now(), 5).unwrap();

    assert_eq!(
        since,
        Since::Time(LogTime {
            sec: local_secs(2026, 11, 15, 10, 28, 5),
            nsec: 0
        })
    );
}

#[test]
fn time_of_day_means_today() {
    let since = parse_since("10:23:45.250", now(), 0).unwrap();

    assert_eq!(
        since,
        Since::Time(LogTime {
            sec: local_secs(2026, 11, 15, 10, 23, 45),
            nsec: 250_000_000
        })
    );
}

#[test]
fn logcat_style_timestamp_takes_the_current_year() {
    let since = parse_since("11-14 23:59:59.000", now(), 0).unwrap();

    assert_eq!(
        since,
        Since::Time(LogTime {
            sec: local_secs(2026, 11, 14, 23, 59, 59),
            nsec: 0
        })
    );
}

#[test]
fn parses_iso_timestamps_with_and_without_zone() {
    let local = parse_since("2026-11-15T10:00:00", now(), 0).unwrap();
    let utc = parse_since("2026-11-15T10:00:00Z", now(), 0).unwrap();

    assert_eq!(
        local,
        Since::Time(LogTime {
            sec: local_secs(2026, 11, 15, 10, 0, 0),
            nsec: 0
        })
    );
    assert_eq!(
        utc,
        Since::Time(LogTime {
            sec: 1_794_736_800,
            nsec: 0
        })
    );
}

#[test]
fn parses_marks() {
    assert_eq!(parse_since("mark:42", now(), 0), Ok(Since::Seq(42)));
    assert!(parse_since("mark:x", now(), 0).is_err());
}

#[test]
fn explains_unreadable_input() {
    let error = parse_since("yesterday", now(), 0).unwrap_err();

    assert!(error.contains("\"30s\""), "{error}");
}

#[test]
fn logcat_style_timestamp_after_new_year_takes_last_year() {
    let new_year = Local
        .from_local_datetime(
            &NaiveDate::from_ymd_opt(2027, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 30)
                .unwrap(),
        )
        .unwrap();

    let since = parse_since("12-31 23:59:58.000", new_year, 0).unwrap();

    assert_eq!(
        since,
        Since::Time(LogTime {
            sec: local_secs(2026, 12, 31, 23, 59, 58),
            nsec: 0
        })
    );
}
