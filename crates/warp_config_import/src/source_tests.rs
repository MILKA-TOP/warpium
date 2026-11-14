use std::fs::File;
use std::time::{Duration, SystemTime};

use super::*;

fn touch(path: &Path, modified: SystemTime) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    File::create(path).unwrap().set_modified(modified).unwrap();
}

#[test]
fn channel_paths_match_warp_layout() {
    let home = Path::new("/Users/someone");
    let stable = WarpInstallation::for_channel(WarpChannel::Stable, home);
    assert_eq!(stable.data_dir, home.join(".warp"));
    assert_eq!(stable.home_config_dir, home.join(".warp"));
    assert_eq!(
        stable.defaults_plist,
        home.join("Library/Preferences/dev.warp.Warp-Stable.plist")
    );
    assert_eq!(stable.settings_file(), home.join(".warp/settings.toml"));

    // Preview has its own data directory but still reads skills and MCP config from `.warp`.
    let preview = WarpInstallation::for_channel(WarpChannel::Preview, home);
    assert_eq!(preview.data_dir, home.join(".warp-preview"));
    assert_eq!(preview.home_config_dir, home.join(".warp"));

    let dev = WarpInstallation::for_channel(WarpChannel::Dev, home);
    assert_eq!(dev.data_dir, home.join(".warp-dev"));
    assert_eq!(dev.home_config_dir, home.join(".warp-dev"));
    assert_eq!(WarpChannel::Dev.bundle_id(), "dev.warp.Warp-Dev");
}

#[test]
fn discovers_only_installed_channels() {
    let home = tempfile::tempdir().unwrap();
    assert!(discover_installations(home.path()).is_empty());

    std::fs::create_dir(home.path().join(".warp")).unwrap();
    // Warpium's own directory is not a source.
    std::fs::create_dir(home.path().join(".warp-oss")).unwrap();
    // A channel that only left user defaults behind still counts.
    touch(
        &home
            .path()
            .join("Library/Preferences/dev.warp.Warp-Dev.plist"),
        SystemTime::now(),
    );

    let channels: Vec<_> = discover_installations(home.path())
        .into_iter()
        .map(|installation| installation.channel)
        .collect();
    assert_eq!(channels, vec![WarpChannel::Stable, WarpChannel::Dev]);
}

#[test]
fn picks_most_recently_used_installation() {
    let home = tempfile::tempdir().unwrap();
    let old = SystemTime::now() - Duration::from_secs(60 * 60 * 24 * 30);
    let recent = SystemTime::now() - Duration::from_secs(60);

    touch(&home.path().join(".warp/settings.toml"), old);
    std::fs::create_dir(home.path().join(".warp-preview")).unwrap();
    // Set directory times last: creating a file inside bumps them.
    for dir in [".warp", ".warp-preview"] {
        File::open(home.path().join(dir))
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    touch(
        &home
            .path()
            .join("Library/Preferences/dev.warp.Warp-Preview.plist"),
        recent,
    );

    let chosen = most_recently_used(discover_installations(home.path())).unwrap();
    assert_eq!(chosen.channel, WarpChannel::Preview);
}

#[test]
fn ties_prefer_stable() {
    let home = Path::new("/nonexistent-home-for-warp-import-tests");
    let installations = vec![
        WarpInstallation::for_channel(WarpChannel::Stable, home),
        WarpInstallation::for_channel(WarpChannel::Preview, home),
        WarpInstallation::for_channel(WarpChannel::Dev, home),
    ];
    let chosen = most_recently_used(installations).unwrap();
    assert_eq!(chosen.channel, WarpChannel::Stable);
    assert!(most_recently_used(Vec::new()).is_none());
}
