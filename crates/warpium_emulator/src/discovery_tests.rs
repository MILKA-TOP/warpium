use std::{
    fs,
    path::{Path, PathBuf},
};

use super::*;

const FULL_FILE: &str = "\
port.serial=5554
port.adb=5555
avd.name=Pixel_8_API_35
avd.id=Pixel_8_API_35
avd.dir=/Users/dev/.android/avd/Pixel_8_API_35.avd
emulator.version=35.2.10
emulator.build=12414864
cmdline=\"/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64\" -avd Pixel_8_API_35 -qt-hide-window -grpc-use-token
grpc.port=8554
grpc.token=c2VjcmV0K3Rva2VuPT0=
grpc.jwks=/tmp/jwks
grpc.jwk_active=/tmp/jwks/active.jwk
";

fn linux_env() -> DiscoveryEnv {
    DiscoveryEnv {
        os: HostOs::Linux,
        override_dir: None,
        home: Some(PathBuf::from("/home/dev")),
        xdg_runtime_dir: Some(PathBuf::from("/run/user/1000")),
        uid: Some(1000),
        local_app_data: None,
        temp_dir: Some(PathBuf::from("/tmp")),
        user: Some("dev".to_owned()),
    }
}

#[test]
fn parses_full_discovery_file() {
    let emulator = parse_discovery_file("pid_4242.ini", FULL_FILE).unwrap();

    assert_eq!(
        emulator,
        RunningEmulator {
            pid: 4242,
            console_port: 5554,
            adb_port: Some(5555),
            grpc_port: 8554,
            grpc_token: Some(GrpcToken::new("c2VjcmV0K3Rva2VuPT0=")),
            grpc_uses_tls: false,
            avd_name: Some("Pixel_8_API_35".to_owned()),
            avd_dir: Some(PathBuf::from("/Users/dev/.android/avd/Pixel_8_API_35.avd")),
            emulator_version: Some("35.2.10".to_owned()),
            command_line: Some(
                "\"/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64\" -avd Pixel_8_API_35 \
                 -qt-hide-window -grpc-use-token"
                    .to_owned()
            ),
        }
    );
}

#[test]
fn keeps_equals_signs_inside_token() {
    let emulator = parse_discovery_file(
        "pid_1.ini",
        "port.serial=5554\ngrpc.port=8554\ngrpc.token=ab==\n",
    )
    .unwrap();

    assert_eq!(emulator.grpc_token, Some(GrpcToken::new("ab==")));
}

#[test]
fn parses_crlf_line_endings() {
    let emulator = parse_discovery_file(
        "pid_7.ini",
        "port.serial=5556\r\ngrpc.port=8556\r\ngrpc.token=tok\r\navd.name=Tablet\r\n",
    )
    .unwrap();

    assert_eq!(emulator.console_port, 5556);
    assert_eq!(emulator.grpc_port, 8556);
    assert_eq!(emulator.grpc_token, Some(GrpcToken::new("tok")));
    assert_eq!(emulator.avd_name.as_deref(), Some("Tablet"));
}

#[test]
fn accepts_file_without_token_as_unauthenticated() {
    let emulator = parse_discovery_file("pid_1.ini", "port.serial=5554\ngrpc.port=8554\n").unwrap();

    assert_eq!(emulator.grpc_token, None);
}

#[test]
fn rejects_file_without_grpc_port() {
    let result = parse_discovery_file("pid_1.ini", "port.serial=5554\ngrpc.token=tok\n");

    assert_eq!(result, Err(ParseError::MissingKey("grpc.port")));
}

#[test]
fn rejects_file_without_console_port() {
    let result = parse_discovery_file("pid_1.ini", "grpc.port=8554\n");

    assert_eq!(result, Err(ParseError::MissingKey("port.serial")));
}

#[test]
fn rejects_non_numeric_grpc_port() {
    let result = parse_discovery_file("pid_1.ini", "port.serial=5554\ngrpc.port=abc\n");

    assert_eq!(
        result,
        Err(ParseError::InvalidValue {
            key: "grpc.port",
            value: "abc".to_owned()
        })
    );
}

#[test]
fn flags_tls_endpoints() {
    let emulator = parse_discovery_file(
        "pid_1.ini",
        "port.serial=5554\ngrpc.port=8554\ngrpc.server_cert=/certs/emulator.pem\n",
    )
    .unwrap();

    assert!(emulator.grpc_uses_tls);
}

#[test]
fn reads_pid_from_file_name_only_for_discovery_files() {
    assert_eq!(pid_from_file_name("pid_123.ini"), Some(123));
    assert_eq!(pid_from_file_name("pid_.ini"), None);
    assert_eq!(pid_from_file_name("pid_12a.ini"), None);
    assert_eq!(pid_from_file_name("pid_123.ini.tmp"), None);
    assert_eq!(pid_from_file_name("other.ini"), None);
}

#[test]
fn token_debug_output_is_redacted() {
    let token = GrpcToken::new("super-secret");

    assert_eq!(format!("{token:?}"), "GrpcToken(<redacted>)");
}

#[test]
fn maps_console_port_to_adb_serial_and_endpoint() {
    let emulator = parse_discovery_file("pid_1.ini", "port.serial=5556\ngrpc.port=8556\n").unwrap();

    assert_eq!(emulator.adb_serial(), "emulator-5556");
    assert_eq!(emulator.grpc_endpoint(), "http://localhost:8556");
}

#[test]
fn detects_headless_window_launch_from_cmdline() {
    let headless = parse_discovery_file("pid_1.ini", FULL_FILE).unwrap();
    let windowed = parse_discovery_file(
        "pid_2.ini",
        "port.serial=5554\ngrpc.port=8554\ncmdline=emulator -avd X\n",
    )
    .unwrap();

    assert!(headless.is_headless_window());
    assert!(!windowed.is_headless_window());
}

#[test]
fn finds_emulator_by_adb_serial_and_pid() {
    let emulators = vec![
        parse_discovery_file("pid_10.ini", "port.serial=5554\ngrpc.port=8554\n").unwrap(),
        parse_discovery_file("pid_20.ini", "port.serial=5556\ngrpc.port=8556\n").unwrap(),
    ];

    assert_eq!(
        find_by_serial(&emulators, "emulator-5556").map(|e| e.pid),
        Some(20)
    );
    assert_eq!(find_by_serial(&emulators, "emulator-5558"), None);
    assert_eq!(find_by_serial(&emulators, "R5CT1234"), None);
    assert_eq!(
        find_by_pid(&emulators, 10).map(|e| e.console_port),
        Some(5554)
    );
}

#[test]
fn macos_dir_is_under_caches_temporary_items() {
    let env = DiscoveryEnv {
        os: HostOs::MacOs,
        home: Some(PathBuf::from("/Users/dev")),
        ..linux_env()
    };

    assert_eq!(
        discovery_dir(&env, |_| true),
        Some(PathBuf::from(
            "/Users/dev/Library/Caches/TemporaryItems/avd/running"
        ))
    );
}

#[test]
fn windows_dir_is_under_local_app_data_temp() {
    let env = DiscoveryEnv {
        os: HostOs::Windows,
        local_app_data: Some(PathBuf::from("C:/Users/dev/AppData/Local")),
        ..linux_env()
    };

    assert_eq!(
        discovery_dir(&env, |_| true),
        Some(PathBuf::from("C:/Users/dev/AppData/Local/Temp/avd/running"))
    );
}

#[test]
fn linux_prefers_xdg_runtime_dir() {
    let env = DiscoveryEnv {
        xdg_runtime_dir: Some(PathBuf::from("/run/xdg")),
        ..linux_env()
    };

    assert_eq!(
        discovery_dir(&env, |_| true),
        Some(PathBuf::from("/run/xdg/avd/running"))
    );
}

#[test]
fn linux_falls_back_to_run_user_then_android_home() {
    let env = DiscoveryEnv {
        xdg_runtime_dir: None,
        ..linux_env()
    };

    assert_eq!(
        discovery_dir(&env, |dir| dir == Path::new("/run/user/1000")),
        Some(PathBuf::from("/run/user/1000/avd/running"))
    );
    assert_eq!(
        discovery_dir(&env, |dir| dir == Path::new("/home/dev/.android")),
        Some(PathBuf::from("/home/dev/.android/avd/running"))
    );
}

#[test]
fn linux_falls_back_to_per_user_temp_dir() {
    let env = DiscoveryEnv {
        xdg_runtime_dir: None,
        ..linux_env()
    };

    assert_eq!(
        discovery_dir(&env, |_| false),
        Some(PathBuf::from("/tmp/android-dev/avd/running"))
    );
}

#[test]
fn override_dir_wins_on_every_os() {
    let env = DiscoveryEnv {
        os: HostOs::MacOs,
        override_dir: Some(PathBuf::from("/fixtures/running")),
        ..linux_env()
    };

    assert_eq!(
        discovery_dir(&env, |_| true),
        Some(PathBuf::from("/fixtures/running"))
    );
}

#[test]
fn scan_keeps_live_valid_entries_sorted_by_console_port() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("pid_30.ini"),
        "port.serial=5556\ngrpc.port=8556\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("pid_10.ini"),
        "port.serial=5554\ngrpc.port=8554\n",
    )
    .unwrap();
    fs::write(dir.path().join("pid_20.ini"), "port.serial=5558\n").unwrap();
    fs::write(
        dir.path().join("pid_40.ini"),
        "port.serial=5560\ngrpc.port=8560\n",
    )
    .unwrap();
    fs::write(dir.path().join("notes.txt"), "port.serial=1\ngrpc.port=2\n").unwrap();

    let emulators = scan_dir(dir.path(), |pid| pid != 40);

    let pids: Vec<u32> = emulators.iter().map(|emulator| emulator.pid).collect();
    assert_eq!(pids, vec![10, 30]);
}

#[test]
fn scan_of_missing_dir_is_empty() {
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(scan_dir(&dir.path().join("missing"), |_| true), Vec::new());
}

#[cfg(unix)]
#[test]
fn current_process_is_alive() {
    assert!(process_is_alive(std::process::id()));
}

#[cfg(unix)]
#[test]
fn pid_zero_and_out_of_range_pids_are_not_alive() {
    assert!(!process_is_alive(0));
    assert!(!process_is_alive(u32::MAX));
}

// Unix-only, so the Windows console-flash concern behind the lint does not apply.
#[allow(clippy::disallowed_types)]
#[cfg(unix)]
#[test]
fn exited_process_is_not_alive() {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();

    assert!(!process_is_alive(pid));
}
