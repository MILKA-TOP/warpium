use super::*;

fn map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn decision_for(plans: &[KeyValuePlan], key: &str) -> SyncDecision {
    plans
        .iter()
        .find(|plan| plan.key == key)
        .unwrap_or_else(|| panic!("no plan for {key}"))
        .decision
}

#[test]
fn denylisted_keys_are_never_planned() {
    let source = map(&[("DidNonAnonymousUserLogIn", "true"), ("Aliases", "[]")]);
    let plans = plan_key_values(
        &keys(&["DidNonAnonymousUserLogIn", "Aliases"]),
        &source,
        &BTreeMap::new(),
        &BTreeMap::new(),
        true,
    );
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].key, "Aliases");
    assert_eq!(plans[0].decision, SyncDecision::Create);
    assert_eq!(plans[0].source_value.as_deref(), Some("[]"));
}

#[test]
fn json_key_order_and_whitespace_do_not_count_as_changes() {
    let source = map(&[("State", r#"{"a": 1, "b": [true, null]}"#)]);
    let target = map(&[("State", r#"{"b":[true,null],"a":1}"#)]);
    let plans = plan_key_values(&keys(&["State"]), &source, &target, &BTreeMap::new(), false);
    assert_eq!(decision_for(&plans, "State"), SyncDecision::InSync);
}

#[test]
fn sync_runs_update_the_base() {
    let all = keys(&["Aliases", "Banner", "Gone"]);
    let mut base = BTreeMap::new();
    let mut report = ImportReport::default();

    // First run imports everything Warp has.
    let source = map(&[("Aliases", r#"["a"]"#), ("Banner", "\"Dismissed\"")]);
    for plan in plan_key_values(&all, &source, &BTreeMap::new(), &base, true) {
        plan.finish(Ok(None), &mut base, &mut report);
    }
    assert_eq!(report.imported, keys(&["Aliases", "Banner"]));
    assert_eq!(base.len(), 2);

    // Warp changes Aliases; Warpium changed Banner on its own; Warp drops nothing yet.
    let source = map(&[("Aliases", r#"["a","b"]"#), ("Banner", "\"Dismissed\"")]);
    let target = map(&[("Aliases", r#"["a"]"#), ("Banner", "\"NotDismissed\"")]);
    let plans = plan_key_values(&all, &source, &target, &base, false);
    assert_eq!(decision_for(&plans, "Aliases"), SyncDecision::Update);
    assert_eq!(decision_for(&plans, "Banner"), SyncDecision::KeepLocal);
    assert_eq!(decision_for(&plans, "Gone"), SyncDecision::KeepLocal);

    // A failed write leaves the base alone so the next run retries.
    let aliases = plans.iter().find(|plan| plan.key == "Aliases").unwrap();
    let before = base.clone();
    let mut report = ImportReport::default();
    aliases.finish(Err("boom".to_owned()), &mut base, &mut report);
    assert_eq!(base, before);
    assert_eq!(
        report.errors,
        vec![("Aliases".to_owned(), "boom".to_owned())]
    );
}

#[test]
fn removal_in_warp_resets_an_untouched_value() {
    let mut base = BTreeMap::new();
    let mut report = ImportReport::default();
    let source = map(&[("Aliases", "[]")]);
    for plan in plan_key_values(&keys(&["Aliases"]), &source, &source, &base, false) {
        plan.finish(Ok(None), &mut base, &mut report);
    }
    assert!(base.contains_key("Aliases"));

    let plans = plan_key_values(&keys(&["Aliases"]), &BTreeMap::new(), &source, &base, false);
    assert_eq!(decision_for(&plans, "Aliases"), SyncDecision::Remove);
    plans[0].finish(Ok(None), &mut base, &mut report);
    assert!(base.is_empty());
    assert_eq!(report.removed, keys(&["Aliases"]));
}

#[test]
fn warpium_reserializing_a_value_is_not_a_local_change() {
    let all = keys(&["State"]);
    let mut base = BTreeMap::new();
    let mut report = ImportReport::default();

    // Warpium stores the imported value in its own serialization (an extra default field).
    let warp_v1 = r#"{"mode":"a"}"#;
    let stored_v1 = r#"{"mode":"a","extra":null}"#;
    let plans = plan_key_values(
        &all,
        &map(&[("State", warp_v1)]),
        &BTreeMap::new(),
        &base,
        true,
    );
    plans[0].finish(Ok(Some(stored_v1.to_owned())), &mut base, &mut report);

    // Nothing changed on either side.
    let target = map(&[("State", stored_v1)]);
    let plans = plan_key_values(&all, &map(&[("State", warp_v1)]), &target, &base, false);
    assert_eq!(decision_for(&plans, "State"), SyncDecision::InSync);

    // Warp changes the value: an update, not a conflict.
    let warp_v2 = map(&[("State", r#"{"mode":"b"}"#)]);
    let plans = plan_key_values(&all, &warp_v2, &target, &base, false);
    assert_eq!(decision_for(&plans, "State"), SyncDecision::Update);

    // Warpium changes it too: now it is a conflict.
    let target = map(&[("State", r#"{"mode":"c","extra":null}"#)]);
    let plans = plan_key_values(&all, &warp_v2, &target, &base, false);
    assert_eq!(decision_for(&plans, "State"), SyncDecision::Conflict);
}
