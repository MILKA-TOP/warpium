use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;

const NEW_TAG: &str = "v0.2026.06.03.09.49.stable_00.2";
const NEW_PLIST_VERSION: &str = "0.2026.06.03.09.49.00.2";
const OLD_PLIST_VERSION: &str = "0.2026.06.03.09.49.00.1";

/// Stands in for the macOS tools: the "disk image" is a directory whose contents `attach` copies
/// to the mountpoint.
#[derive(Default)]
struct FakeTools {
    fail_attach: bool,
    fail_signature: bool,
    detached: RefCell<Vec<PathBuf>>,
    unquarantined: RefCell<Vec<PathBuf>>,
}

impl InstallTools for FakeTools {
    fn attach(&self, dmg: &Path, mountpoint: &Path) -> Result<(), String> {
        if self.fail_attach {
            return Err("hdiutil: attach failed - image not recognized".to_owned());
        }
        copy_dir(dmg, mountpoint);
        Ok(())
    }

    fn detach(&self, mountpoint: &Path) -> Result<(), String> {
        self.detached.borrow_mut().push(mountpoint.to_owned());
        fs::remove_dir_all(mountpoint).map_err(|err| err.to_string())
    }

    fn copy_bundle(&self, from: &Path, to: &Path) -> Result<(), String> {
        copy_dir(from, to);
        Ok(())
    }

    fn verify_signature(&self, _bundle: &Path) -> Result<(), String> {
        if self.fail_signature {
            Err("code object is not signed at all".to_owned())
        } else {
            Ok(())
        }
    }

    fn clear_quarantine(&self, bundle: &Path) -> Result<(), String> {
        self.unquarantined.borrow_mut().push(bundle.to_owned());
        Ok(())
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn write_app(app: &Path, bundle_id: &str, version: &str, marker: &str) {
    fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    fs::write(
        app.join("Contents/Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>warp-oss</string>
  <key>CFBundleIdentifier</key><string>{bundle_id}</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
</dict>
</plist>"#
        ),
    )
    .unwrap();
    fs::write(app.join("Contents/MacOS/warp-oss"), marker).unwrap();
}

fn marker(app: &Path) -> String {
    fs::read_to_string(app.join("Contents/MacOS/warp-oss")).unwrap()
}

/// A temp dir with the installed app at `Apps/Warpium.app` (marker `old`) and a disk image at
/// `image/` holding `Warpium.app` and an `Applications` link like the real one.
struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new(image_bundle_id: &str, image_version: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        write_app(
            &dir.path().join("Apps/Warpium.app"),
            BUNDLE_ID,
            OLD_PLIST_VERSION,
            "old",
        );
        let image = dir.path().join("image");
        write_app(
            &image.join("Warpium.app"),
            image_bundle_id,
            image_version,
            "new",
        );
        fs::create_dir_all(image.join("Applications")).unwrap();
        Self { dir }
    }

    fn apps(&self) -> PathBuf {
        self.dir.path().join("Apps")
    }

    fn bundle(&self) -> PathBuf {
        self.apps().join("Warpium.app")
    }

    fn mountpoint(&self) -> PathBuf {
        self.dir.path().join("mnt")
    }

    fn install(&self, tools: &FakeTools) -> Result<Installed, InstallError> {
        let version = WarpiumVersion::parse(NEW_TAG).unwrap();
        let bundle = self.bundle();
        let image = self.dir.path().join("image");
        let mountpoint = self.mountpoint();
        install(
            &InstallRequest {
                dmg: &image,
                version: &version,
                bundle: &bundle,
                mountpoint: &mountpoint,
            },
            tools,
        )
    }

    fn app_dir_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.apps())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

#[test]
fn swaps_in_the_new_app_and_keeps_the_old_one_as_a_backup() {
    let fixture = Fixture::new(BUNDLE_ID, NEW_PLIST_VERSION);
    let tools = FakeTools::default();

    let installed = fixture.install(&tools).unwrap();

    assert_eq!(marker(&fixture.bundle()), "new");
    assert_eq!(
        installed.backup,
        fixture.apps().join(".Warpium-previous.app")
    );
    assert_eq!(marker(&installed.backup), "old");
    assert_eq!(
        fixture.app_dir_entries(),
        vec![".Warpium-previous.app", "Warpium.app"]
    );
}

#[test]
fn clears_quarantine_on_the_new_app_and_detaches_the_image() {
    let fixture = Fixture::new(BUNDLE_ID, NEW_PLIST_VERSION);
    let tools = FakeTools::default();

    fixture.install(&tools).unwrap();

    assert_eq!(
        *tools.unquarantined.borrow(),
        vec![fixture.apps().join(".Warpium-update.app")]
    );
    assert_eq!(*tools.detached.borrow(), vec![fixture.mountpoint()]);
}

#[test]
fn replaces_an_earlier_backup() {
    let fixture = Fixture::new(BUNDLE_ID, NEW_PLIST_VERSION);
    write_app(
        &fixture.apps().join(".Warpium-previous.app"),
        BUNDLE_ID,
        "0.2026.01.01.00.00.00.1",
        "ancient",
    );

    let installed = fixture.install(&FakeTools::default()).unwrap();

    assert_eq!(marker(&installed.backup), "old");
}

#[test]
fn installs_outside_applications() {
    // The fixture's app folder is a temp dir, not /Applications; this pins that nothing
    // assumes otherwise.
    let fixture = Fixture::new(BUNDLE_ID, NEW_PLIST_VERSION);

    fixture.install(&FakeTools::default()).unwrap();

    assert_eq!(marker(&fixture.bundle()), "new");
}

#[test]
fn refuses_an_image_with_another_bundle_id_and_keeps_the_old_app() {
    let fixture = Fixture::new("dev.warp.Warp-Stable", NEW_PLIST_VERSION);
    let tools = FakeTools::default();

    let err = fixture.install(&tools).unwrap_err();

    assert_eq!(
        err,
        InstallError::WrongBundleId {
            found: "dev.warp.Warp-Stable".to_owned()
        }
    );
    assert_eq!(marker(&fixture.bundle()), "old");
    assert_eq!(fixture.app_dir_entries(), vec!["Warpium.app"]);
    assert_eq!(*tools.detached.borrow(), vec![fixture.mountpoint()]);
}

#[test]
fn refuses_an_image_with_another_version_and_keeps_the_old_app() {
    let fixture = Fixture::new(BUNDLE_ID, "0.2026.06.03.09.49.00.3");

    let err = fixture.install(&FakeTools::default()).unwrap_err();

    assert_eq!(
        err,
        InstallError::WrongVersion {
            expected: NEW_PLIST_VERSION.to_owned(),
            found: "0.2026.06.03.09.49.00.3".to_owned(),
        }
    );
    assert_eq!(marker(&fixture.bundle()), "old");
}

#[test]
fn refuses_an_image_without_an_app() {
    let fixture = Fixture::new(BUNDLE_ID, NEW_PLIST_VERSION);
    fs::remove_dir_all(fixture.dir.path().join("image/Warpium.app")).unwrap();

    let err = fixture.install(&FakeTools::default()).unwrap_err();

    assert_eq!(err, InstallError::AppNotFound);
}

#[test]
fn refuses_an_invalid_signature_and_removes_the_staged_copy() {
    let fixture = Fixture::new(BUNDLE_ID, NEW_PLIST_VERSION);
    let tools = FakeTools {
        fail_signature: true,
        ..Default::default()
    };

    let err = fixture.install(&tools).unwrap_err();

    assert_eq!(
        err,
        InstallError::Signature("code object is not signed at all".to_owned())
    );
    assert_eq!(marker(&fixture.bundle()), "old");
    assert_eq!(fixture.app_dir_entries(), vec!["Warpium.app"]);
}

#[test]
fn reports_a_mount_failure_and_keeps_the_old_app() {
    let fixture = Fixture::new(BUNDLE_ID, NEW_PLIST_VERSION);
    let tools = FakeTools {
        fail_attach: true,
        ..Default::default()
    };

    let err = fixture.install(&tools).unwrap_err();

    assert_eq!(
        err,
        InstallError::Mount("hdiutil: attach failed - image not recognized".to_owned())
    );
    assert_eq!(marker(&fixture.bundle()), "old");
    assert!(tools.detached.borrow().is_empty());
}

#[test]
fn refuses_to_run_outside_an_app_bundle() {
    let version = WarpiumVersion::parse(NEW_TAG).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("target/debug");

    let err = install(
        &InstallRequest {
            dmg: &dir.path().join("image"),
            version: &version,
            bundle: &bundle,
            mountpoint: &dir.path().join("mnt"),
        },
        &FakeTools::default(),
    )
    .unwrap_err();

    assert_eq!(err, InstallError::NotAnAppBundle(bundle));
    assert!(err.is_location_problem());
}

#[test]
fn refuses_a_translocated_app() {
    let version = WarpiumVersion::parse(NEW_TAG).unwrap();
    let bundle = PathBuf::from("/private/var/folders/xy/AppTranslocation/1234/d/Warpium.app");

    let err = install(
        &InstallRequest {
            dmg: Path::new("/tmp/Warpium.dmg"),
            version: &version,
            bundle: &bundle,
            mountpoint: Path::new("/tmp/mnt"),
        },
        &FakeTools::default(),
    )
    .unwrap_err();

    assert_eq!(err, InstallError::Translocated);
    assert!(err.is_location_problem());
}

#[test]
fn refuses_a_read_only_location_and_keeps_the_old_app() {
    use std::os::unix::fs::PermissionsExt as _;

    // Root ignores directory permissions, so the location would be writable after all.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let fixture = Fixture::new(BUNDLE_ID, NEW_PLIST_VERSION);
    fs::set_permissions(fixture.apps(), fs::Permissions::from_mode(0o555)).unwrap();
    let tools = FakeTools::default();

    let err = fixture.install(&tools).unwrap_err();
    fs::set_permissions(fixture.apps(), fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(err, InstallError::NotWritable(fixture.apps()));
    assert!(err.is_location_problem());
    assert_eq!(marker(&fixture.bundle()), "old");
    assert!(
        tools.detached.borrow().is_empty(),
        "nothing should be mounted"
    );
}

#[test]
fn removes_a_backup() {
    let dir = tempfile::tempdir().unwrap();
    let backup = dir.path().join(".Warpium-previous.app");
    write_app(&backup, BUNDLE_ID, OLD_PLIST_VERSION, "old");

    remove_backup(&backup).unwrap();

    assert!(!backup.exists());
}

#[test]
fn removing_a_missing_backup_succeeds() {
    let dir = tempfile::tempdir().unwrap();

    assert!(remove_backup(&dir.path().join(".Warpium-previous.app")).is_ok());
}

#[test]
fn refuses_to_remove_a_path_that_is_not_a_backup() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("Warpium.app");
    write_app(&app, BUNDLE_ID, OLD_PLIST_VERSION, "old");

    let err = remove_backup(&app).unwrap_err();

    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(app.exists());
}
