use super::*;

fn porcelain() -> String {
    let a = "a".repeat(40);
    let b = "b".repeat(40);
    let zero = "0".repeat(40);
    format!(
        "{a} 1 1 1\nauthor Old\nauthor-time 100\nsummary old\nfilename f\n\tx\n\
         {b} 2 2 1\nauthor New\nauthor-time 200\nsummary new\nfilename f\n\ty\n\
         {zero} 3 3 1\nauthor Not Committed Yet\nauthor-time 300\nsummary s\nfilename f\n\tz\n"
    )
}

#[test]
fn newer_commits_get_stronger_backgrounds() {
    let blame = Blame::parse_porcelain(&porcelain()).unwrap();
    let alphas = age_alphas(&blame);
    let old = blame.commit_index(0).unwrap();
    let new = blame.commit_index(1).unwrap();
    let uncommitted = blame.commit_index(2).unwrap();
    assert_eq!(alphas[old], 0);
    assert!(alphas[new] > alphas[old]);
    assert_eq!(alphas[uncommitted], 0);
}

#[test]
fn column_width_grows_with_authors_up_to_a_limit() {
    let blame = Arc::new(Blame::parse_porcelain(&porcelain()).unwrap());
    let annotations = LineAnnotations::new(blame, 1_000);
    assert!(annotations.width > 0.);
    assert!(annotations.width < 400.);
}
