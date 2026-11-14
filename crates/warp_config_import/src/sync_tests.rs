use super::*;

const A: Option<&str> = Some("a");
const B: Option<&str> = Some("b");
const C: Option<&str> = Some("c");

#[test]
fn three_way_rule() {
    // (base, source, target, initial import, expected)
    let cases = [
        // Only in Warp: copy it, on any run.
        (None, A, None, false, SyncDecision::Create),
        (None, A, None, true, SyncDecision::Create),
        // Identical on both sides.
        (None, A, A, false, SyncDecision::InSync),
        (B, A, A, false, SyncDecision::InSync),
        // Warp changed, Warpium didn't.
        (A, B, A, false, SyncDecision::Update),
        // Warpium changed, Warp didn't.
        (A, A, B, false, SyncDecision::KeepLocal),
        // Warpium deleted an imported item that Warp didn't change.
        (A, A, None, false, SyncDecision::KeepLocal),
        // Both changed, differently.
        (A, B, C, false, SyncDecision::Conflict),
        // Warp changed an item that Warpium deleted.
        (A, B, None, false, SyncDecision::Conflict),
        // Never imported, but Warpium has its own version.
        (None, A, B, false, SyncDecision::Conflict),
        (None, A, B, true, SyncDecision::Overwrite),
        // Warp removed an imported item.
        (A, None, A, false, SyncDecision::Remove),
        (A, None, B, false, SyncDecision::KeepLocal),
        (A, None, None, false, SyncDecision::KeepLocal),
        // Warpium-only items are never touched.
        (None, None, A, false, SyncDecision::KeepLocal),
        (None, None, A, true, SyncDecision::KeepLocal),
    ];
    for (base, source, target, initial, expected) in cases {
        assert_eq!(
            decide(base, source, target, initial),
            expected,
            "base={base:?} source={source:?} target={target:?} initial={initial}"
        );
    }
}

#[test]
fn content_hash_is_stable_sha256() {
    assert_eq!(
        content_hash(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
