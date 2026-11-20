use super::*;

#[test]
fn round_trips_sha1_and_sha256() {
    let sha1 = "0123456789abcdef0123456789abcdef01234567";
    let oid = Oid::from_hex(sha1).unwrap();
    assert_eq!(oid.to_hex(), sha1);
    assert_eq!(oid.short(), "01234567");
    assert_eq!(oid.as_bytes().len(), 20);

    let sha256 = "ab".repeat(32);
    let oid = Oid::from_hex(&sha256).unwrap();
    assert_eq!(oid.to_hex(), sha256);
    assert_eq!(oid.as_bytes().len(), 32);
}

#[test]
fn rejects_bad_ids() {
    assert!(Oid::from_hex("abc").is_none());
    assert!(Oid::from_hex(&"g".repeat(40)).is_none());
    assert!(Oid::from_hex(&"a".repeat(41)).is_none());
}

#[test]
fn upper_case_parses_to_lower_case() {
    let oid = Oid::from_hex(&"AB".repeat(20)).unwrap();
    assert_eq!(oid.to_hex(), "ab".repeat(20));
    assert!(oid.starts_with_hex("ABAB"));
    assert!(!oid.starts_with_hex(""));
}

#[test]
fn zero_id() {
    assert!(Oid::from_hex(&"0".repeat(40)).unwrap().is_zero());
    assert!(!Oid::from_hex(&"01".repeat(20)).unwrap().is_zero());
}

#[test]
fn hex_id_heuristic() {
    assert!(looks_like_hex_id("abc1"));
    assert!(looks_like_hex_id("DEADBEEF"));
    assert!(!looks_like_hex_id("abc"));
    assert!(!looks_like_hex_id("fix bug"));
    assert!(!looks_like_hex_id(&"a".repeat(65)));
}
