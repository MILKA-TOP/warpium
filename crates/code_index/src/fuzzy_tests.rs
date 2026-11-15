use super::*;

fn tier(query: &str, name: &str) -> Option<MatchTier> {
    Pattern::new(query)
        .expect("non-empty query")
        .match_name(name)
        .map(|found| found.tier)
}

fn score(query: &str, name: &str) -> i32 {
    Pattern::new(query)
        .expect("non-empty query")
        .match_name(name)
        .expect("matches")
        .score
}

#[test]
fn empty_query_has_no_pattern() {
    assert!(Pattern::new("").is_none());
    assert!(Pattern::new("  ").is_none());
}

#[test]
fn exact_match_ignores_case() {
    assert_eq!(
        tier("UserViewModel", "UserViewModel"),
        Some(MatchTier::Exact)
    );
    assert_eq!(
        tier("userviewmodel", "UserViewModel"),
        Some(MatchTier::Exact)
    );
}

#[test]
fn prefix_match() {
    assert_eq!(tier("UserV", "UserViewModel"), Some(MatchTier::Prefix));
    assert_eq!(tier("user", "UserViewModel"), Some(MatchTier::Prefix));
}

#[test]
fn camel_humps_from_the_first_hump() {
    assert_eq!(tier("UVM", "UserViewModel"), Some(MatchTier::Camel));
    assert_eq!(tier("uvm", "UserViewModel"), Some(MatchTier::Camel));
    assert_eq!(tier("UsViMo", "UserViewModel"), Some(MatchTier::Camel));
    assert_eq!(tier("UM", "UserViewModel"), Some(MatchTier::Camel));
    assert_eq!(tier("HS", "HTTPServer"), Some(MatchTier::Camel));
    assert_eq!(tier("gUN", "getUserName"), Some(MatchTier::Camel));
    assert_eq!(tier("MU", "MAX_USERS"), Some(MatchTier::Camel));
}

#[test]
fn camel_humps_from_a_later_hump() {
    assert_eq!(tier("VM", "UserViewModel"), Some(MatchTier::WordStart));
    assert_eq!(tier("Model", "UserViewModel"), Some(MatchTier::WordStart));
}

#[test]
fn subsequence_is_the_last_resort() {
    assert_eq!(tier("usrvw", "UserViewModel"), Some(MatchTier::Fuzzy));
}

#[test]
fn no_match_when_characters_are_missing_or_out_of_order() {
    assert_eq!(tier("UVX", "UserViewModel"), None);
    assert_eq!(tier("MVU", "UserViewModel"), None);
    assert_eq!(tier("UserViewModels", "UserViewModel"), None);
}

#[test]
fn exact_case_scores_higher_within_a_tier() {
    assert!(score("User", "User") > score("User", "user"));
}

#[test]
fn shorter_names_score_higher_within_a_tier() {
    assert!(score("UVM", "UserViewModel") > score("UVM", "UserViewModelFactory"));
}

#[test]
fn fewer_skipped_humps_score_higher() {
    assert!(score("UVM", "UserViewModel") > score("UVM", "UserProfileViewModel"));
}

#[test]
fn mask_rejects_names_missing_a_query_character() {
    let pattern = Pattern::new("UVM").expect("non-empty query");

    assert!(pattern.may_match(crate::interner::char_mask("UserViewModel")));
    assert!(!pattern.may_match(crate::interner::char_mask("UserModel")));
}

#[test]
fn matched_indices_follow_the_humps() {
    let pattern = Pattern::new("UVM").expect("non-empty query");

    assert_eq!(pattern.matched_indices("UserViewModel"), vec![0, 4, 8]);
}

#[test]
fn matched_indices_for_a_prefix() {
    let pattern = Pattern::new("use").expect("non-empty query");

    assert_eq!(pattern.matched_indices("UserViewModel"), vec![0, 1, 2]);
}
