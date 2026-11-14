use super::*;

#[test]
fn missing_manifest_means_initial_import() {
    let dir = tempfile::tempdir().unwrap();
    let load = Manifest::load(&dir.path().join(MANIFEST_FILE_NAME));
    assert!(load.is_initial_import());
    assert_eq!(load.into_manifest(), Manifest::default());
}

#[test]
fn round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(MANIFEST_FILE_NAME);
    let mut manifest = Manifest {
        source_channel: Some("stable".to_owned()),
        ..Default::default()
    };
    manifest
        .files
        .insert("themes/a.yaml".to_owned(), "hash".to_owned());
    manifest
        .settings
        .insert("appearance.text.font_size".to_owned(), "hash2".to_owned());
    manifest.save(&path).unwrap();

    match Manifest::load(&path) {
        ManifestLoad::Loaded(loaded) => {
            assert_eq!(loaded.version, MANIFEST_VERSION);
            assert_eq!(loaded.files, manifest.files);
            assert_eq!(loaded.settings, manifest.settings);
            assert_eq!(loaded.source_channel.as_deref(), Some("stable"));
        }
        other => panic!("expected a loaded manifest, got {other:?}"),
    }
}

#[test]
fn corrupt_or_unknown_manifest_is_not_an_initial_import() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(MANIFEST_FILE_NAME);

    std::fs::write(&path, "not json").unwrap();
    let load = Manifest::load(&path);
    assert!(matches!(load, ManifestLoad::Unreadable(_)));
    assert!(!load.is_initial_import());

    std::fs::write(&path, r#"{"version": 99}"#).unwrap();
    assert!(matches!(Manifest::load(&path), ManifestLoad::Unreadable(_)));
}
