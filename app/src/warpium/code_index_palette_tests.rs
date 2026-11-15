use super::*;

#[test]
fn char_indices_are_byte_offsets_for_ascii_names() {
    assert_eq!(char_indices("UserViewModel", &[0, 4, 8]), vec![0, 4, 8]);
}

#[test]
fn char_indices_skip_multibyte_characters() {
    assert_eq!(char_indices("ÄbC", &[2, 3]), vec![1, 2]);
}

fn usage(exact: bool) -> Usage {
    Usage {
        rel_path: "feature/news/src/main/kotlin/NewsViewModel.kt".to_owned(),
        position: code_index::Position {
            line: 10,
            column: 35,
        },
        line_text: "val items = repository.getNewsResources()".to_owned(),
        origin: code_index::Origin::Main,
        exact,
    }
}

#[test]
fn usage_rows_show_the_file_and_one_based_line() {
    let item = UsageSearchItem {
        root: PathBuf::from("/project"),
        usage: usage(true),
        score: 1.,
    };

    assert_eq!(item.label(), "NewsViewModel.kt:11");
    assert_eq!(
        item.details(),
        "  val items = repository.getNewsResources()  · feature/news/src/main/kotlin"
    );
}

#[test]
fn approximate_usages_are_marked() {
    let item = UsageSearchItem {
        root: PathBuf::from("/project"),
        usage: usage(false),
        score: 1.,
    };

    assert_eq!(
        item.details(),
        "  ≈ val items = repository.getNewsResources()  · feature/news/src/main/kotlin"
    );
}

#[test]
fn navigation_filter_matches_the_path_or_the_line() {
    let item = NavigationItem::Usage(usage(true));

    assert!(matches_filter(&item, "newsviewmodel"));
    assert!(matches_filter(&item, "getnews"));
    assert!(!matches_filter(&item, "settings"));
}
