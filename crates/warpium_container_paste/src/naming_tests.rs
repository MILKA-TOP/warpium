use std::time::{Duration, SystemTime};

use chrono::TimeZone as _;

use super::*;

fn at(seconds_ago: u64, now: SystemTime) -> SystemTime {
    now - Duration::from_secs(seconds_ago)
}

#[test]
fn prefers_formats_agents_read() {
    assert_eq!(
        choose_clipboard_image(&["image/tiff", "image/png"]),
        Some((1, "png"))
    );
    assert_eq!(choose_clipboard_image(&["image/JPEG"]), Some((0, "jpg")));
    assert_eq!(
        choose_clipboard_image(&["image/svg+xml", "image/tiff"]),
        Some((1, "tiff"))
    );
    assert_eq!(choose_clipboard_image(&["image/unknown"]), None);
    assert_eq!(choose_clipboard_image(&[]), None);
}

#[test]
fn formats_needing_conversion() {
    assert!(needs_png_conversion("tiff"));
    assert!(needs_png_conversion("TIF"));
    assert!(needs_png_conversion("heic"));
    assert!(!needs_png_conversion("png"));
    assert!(!needs_png_conversion("webp"));
}

#[test]
fn sanitizes_names() {
    assert_eq!(
        sanitize("Screen Shot 2026-11-19 at 10.00.00"),
        "Screen-Shot-2026-11-19-at-10.00.00"
    );
    assert_eq!(sanitize("ünïcødé фото"), "n-c-d");
    assert_eq!(sanitize("../../etc/passwd"), "etc-passwd");
    assert_eq!(sanitize("$(rm -rf ~)"), "rm-rf");
    assert_eq!(sanitize("...."), "");
}

#[test]
fn file_names_are_unique_and_shell_safe() {
    let now = chrono::Local
        .with_ymd_and_hms(2026, 11, 19, 10, 5, 7)
        .unwrap();
    assert_eq!(
        file_name(None, "png", now, 0),
        "warpium-paste-20261119-100507-000-1.png"
    );
    assert_eq!(
        file_name(Some("Screen Shot.PNG"), "PNG", now, 1),
        "warpium-Screen-Shot-20261119-100507-000-2.png"
    );
    assert_eq!(
        file_name(Some("фото.jpg"), "jpg", now, 0),
        "warpium-paste-20261119-100507-000-1.jpg"
    );
    assert_eq!(
        file_name(Some(".hidden"), "", now, 0),
        "warpium-hidden-20261119-100507-000-1.png"
    );
    let long = "a".repeat(100);
    let name = file_name(Some(&long), "gif", now, 0);
    assert!(name.starts_with(&format!("warpium-{}-", "a".repeat(40))));
    assert!(!name.contains(&"a".repeat(41)));
}

#[test]
fn validates_remote_dirs() {
    assert!(is_valid_remote_dir("/tmp/warpium-paste"));
    assert!(is_valid_remote_dir("/home/node/.cache/paste_1"));
    assert!(!is_valid_remote_dir("tmp/x"));
    assert!(!is_valid_remote_dir("/"));
    assert!(!is_valid_remote_dir("/tmp/a b"));
    assert!(!is_valid_remote_dir("/tmp/$(x)"));
    assert!(!is_valid_remote_dir("/tmp/../etc"));
    assert!(!is_valid_remote_dir("/tmp/'x'"));
}

#[test]
fn removes_old_and_excess_pasted_files_only() {
    let now = SystemTime::now();
    let mut entries = vec![
        ("warpium-old.png".to_owned(), at(25 * 60 * 60, now)),
        ("warpium-fresh.png".to_owned(), at(60, now)),
        ("notes.txt".to_owned(), at(48 * 60 * 60, now)),
        (".gitignore".to_owned(), at(48 * 60 * 60, now)),
    ];
    assert_eq!(files_to_remove(&entries, now), ["warpium-old.png"]);

    entries.clear();
    for index in 0..(KEEP_NEWEST + 3) {
        entries.push((format!("warpium-{index}.png"), at(index as u64, now)));
    }
    let mut removed = files_to_remove(&entries, now);
    removed.sort();
    let mut expected = (KEEP_NEWEST..KEEP_NEWEST + 3)
        .map(|index| format!("warpium-{index}.png"))
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(removed, expected);
}

#[test]
fn files_from_the_future_are_kept() {
    let now = SystemTime::now();
    let entries = vec![("warpium-a.png".to_owned(), now + Duration::from_secs(60))];
    assert!(files_to_remove(&entries, now).is_empty());
}

#[test]
fn remote_cleanup_only_touches_pasted_files() {
    let script = remote_cleanup_script();
    assert!(script.contains("-name 'warpium-*'"));
    assert!(script.contains("-mmin +1440"));
    assert!(script.contains("-maxdepth 1"));
}
