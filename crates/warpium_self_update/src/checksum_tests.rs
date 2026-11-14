use super::*;

const DMG: &str = "Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg";
/// The `.sha256` asset of release `v0.2026.06.03.09.49.stable_00.1`, as published.
const PUBLISHED: &str = "bb9c5b35d8f4627b8a53e09cec14efd7b72c76d1136ffae19fc8bf14bb68f1e2  Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg\n";
/// SHA-256 of `hello`.
const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

fn digest(hex_digest: &str) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(hex_digest, &mut bytes).unwrap();
    bytes
}

#[test]
fn parses_the_published_checksum_file() {
    assert_eq!(
        parse_checksum_file(PUBLISHED, DMG),
        Ok(digest(
            "bb9c5b35d8f4627b8a53e09cec14efd7b72c76d1136ffae19fc8bf14bb68f1e2"
        ))
    );
}

#[test]
fn parses_binary_mode_and_uppercase_digests() {
    let contents = format!("{} *{DMG}", HELLO_SHA256.to_uppercase());

    assert_eq!(
        parse_checksum_file(&contents, DMG),
        Ok(digest(HELLO_SHA256))
    );
}

#[test]
fn parses_a_bare_digest() {
    assert_eq!(
        parse_checksum_file(&format!("{HELLO_SHA256}\n"), DMG),
        Ok(digest(HELLO_SHA256))
    );
}

#[test]
fn rejects_a_checksum_for_another_file() {
    let contents = format!("{HELLO_SHA256}  Warpium-v0.2026.06.03.09.49.stable_00.2-arm64.dmg");

    assert_eq!(
        parse_checksum_file(&contents, DMG),
        Err(ChecksumError::WrongFile {
            expected_name: DMG.to_owned(),
            found: "Warpium-v0.2026.06.03.09.49.stable_00.2-arm64.dmg".to_owned(),
        })
    );
}

#[test]
fn rejects_malformed_checksum_files() {
    for contents in [
        "",
        "not-a-digest  Warpium.dmg",
        "2cf24dba  Warpium-v0.2026.06.03.09.49.stable_00.1-arm64.dmg",
        "<html>Not Found</html>",
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824\n2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    ] {
        assert_eq!(
            parse_checksum_file(contents, DMG),
            Err(ChecksumError::Malformed {
                expected_name: DMG.to_owned()
            }),
            "{contents:?}"
        );
    }
}

#[test]
fn verifies_matching_data_hashed_in_chunks() {
    let mut hasher = Sha256Hasher::default();
    hasher.update(b"hel");
    hasher.update(b"lo");

    assert_eq!(hasher.verify(&digest(HELLO_SHA256)), Ok(()));
}

#[test]
fn refuses_data_that_does_not_match() {
    let mut hasher = Sha256Hasher::default();
    hasher.update(b"hello!");

    assert_eq!(
        hasher.verify(&digest(HELLO_SHA256)),
        Err(ChecksumError::Mismatch)
    );
}
