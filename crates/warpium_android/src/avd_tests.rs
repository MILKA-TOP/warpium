use std::fs;
use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::test_support::{FakeAdbServer, Next, answer_transport, length_prefixed, okay};

fn write_avd(dir: &Path, name: &str, ini: &str, config: Option<&str>) {
    fs::write(dir.join(format!("{name}.ini")), ini).unwrap();
    let avd_dir = dir.join(format!("{name}.avd"));
    fs::create_dir_all(&avd_dir).unwrap();
    if let Some(config) = config {
        fs::write(avd_dir.join("config.ini"), config).unwrap();
    }
}

#[test]
fn list_avds_reads_ini_files() {
    let dir = tempfile::tempdir().unwrap();
    write_avd(
        dir.path(),
        "Pixel_7_API_34",
        "avd.ini.encoding=UTF-8\ntarget=android-34\n",
        Some(
            "avd.ini.displayname=Pixel 7 API 34\nimage.sysdir.1=system-images/android-34/google_apis/arm64-v8a/\n",
        ),
    );
    write_avd(
        dir.path(),
        "Tablet",
        "path=/nonexistent/Tablet.avd\n",
        Some("image.sysdir.1=system-images/android-30/default/x86_64/\n"),
    );

    let avds = list_avds(&[dir.path().to_path_buf()]);

    assert_eq!(
        avds,
        vec![
            Avd {
                name: "Pixel_7_API_34".to_owned(),
                display_name: Some("Pixel 7 API 34".to_owned()),
                api_level: Some(34),
            },
            Avd {
                name: "Tablet".to_owned(),
                display_name: None,
                api_level: Some(30),
            },
        ]
    );
}

#[test]
fn list_avds_keeps_the_first_directory_on_duplicates() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_avd(first.path(), "Same", "target=android-33\n", None);
    write_avd(second.path(), "Same", "target=android-29\n", None);

    let avds = list_avds(&[first.path().to_path_buf(), second.path().to_path_buf()]);

    assert_eq!(avds.len(), 1);
    assert_eq!(avds[0].api_level, Some(33));
}

#[test]
fn list_avds_ignores_missing_directories() {
    assert!(list_avds(&[PathBuf::from("/nonexistent/avd")]).is_empty());
}

#[test]
fn avd_label_includes_api_level() {
    let avd = Avd {
        name: "Pixel_7_API_34".to_owned(),
        display_name: None,
        api_level: Some(34),
    };

    assert_eq!(avd.label(), "Pixel 7 API 34 (API 34)");
}

#[test]
fn parse_list_avds_drops_log_lines() {
    let output = "INFO    | Storing crashdata in: /tmp/android-me/emu-crash-34.1.19.db\n\
                  Pixel_7_API_34\nMedium_Phone\n";

    assert_eq!(
        parse_list_avds(output),
        vec!["Pixel_7_API_34".to_owned(), "Medium_Phone".to_owned()]
    );
}

/// A fake server with one emulator running `Pixel_7` that reports booted after `polls_until_boot`
/// boot checks.
fn booting_emulator_server(polls_until_boot: usize) -> FakeAdbServer {
    let polls = Arc::new(AtomicUsize::new(0));
    FakeAdbServer::start(move |request, stream| {
        if request == "host:devices-l" {
            okay(stream);
            length_prefixed(
                stream,
                "emulator-5554 device transport_id:1\nR58M device usb:1-1 transport_id:2\n",
            );
            return Next::Close;
        }
        if let Some(next) = answer_transport(request, stream, "emulator-5554") {
            return next;
        }
        okay(stream);
        let booted = polls.fetch_add(1, Ordering::SeqCst) + 1 >= polls_until_boot;
        let boot_completed = if booted { "1" } else { "" };
        let _ = stream
            .write_all(format!("34\n14\nGoogle\nsdk\nPixel_7\n\n{boot_completed}\n").as_bytes());
        Next::Close
    })
}

#[test]
fn wait_for_avd_boot_returns_serial_once_booted() {
    let server = booting_emulator_server(3);

    let serial = wait_for_avd_boot(
        &server.client(),
        "Pixel_7",
        None,
        Duration::from_secs(10),
        Duration::from_millis(1),
        &|| false,
    );

    assert_eq!(serial, Ok("emulator-5554".to_owned()));
}

#[test]
fn wait_for_avd_boot_times_out_for_another_avd() {
    let server = booting_emulator_server(1);

    let result = wait_for_avd_boot(
        &server.client(),
        "Other_AVD",
        None,
        Duration::from_millis(20),
        Duration::from_millis(1),
        &|| false,
    );

    assert_eq!(result, Err(BootError::TimedOut));
}

#[test]
fn wait_for_avd_boot_stops_when_cancelled() {
    let server = booting_emulator_server(usize::MAX);

    let result = wait_for_avd_boot(
        &server.client(),
        "Pixel_7",
        None,
        Duration::from_secs(10),
        Duration::from_millis(1),
        &|| true,
    );

    assert_eq!(result, Err(BootError::Cancelled));
}

#[test]
fn running_emulator_for_avd_finds_unbooted_emulator() {
    let server = booting_emulator_server(usize::MAX);

    assert_eq!(
        running_emulator_for_avd(&server.client(), "Pixel_7"),
        Some("emulator-5554".to_owned())
    );
    assert_eq!(running_emulator_for_avd(&server.client(), "Other"), None);
}
