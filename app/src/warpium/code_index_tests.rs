use code_index::{Origin, Position, Usage};

use super::*;

fn usage(line: u32, via: bool) -> Usage {
    Usage {
        rel_path: "feature/news/NewsViewModel.kt".to_owned(),
        position: Position { line, column: 4 },
        line_text: "repository.load()".to_owned(),
        origin: Origin::Main,
        exact: true,
        via,
    }
}

/// `Some(line)` for a usage row, `None` for a heading.
fn rows(items: &[NavigationItem]) -> Vec<Option<u32>> {
    items
        .iter()
        .map(|item| match item {
            NavigationItem::Usage(usage) => Some(usage.position.line),
            _ => None,
        })
        .collect()
}

#[test]
fn usages_via_overrides_follow_one_heading() {
    let items = usage_items(vec![usage(1, false), usage(2, true), usage(3, true)]);

    assert_eq!(rows(&items), vec![Some(1), None, Some(2), Some(3)]);
    assert!(matches!(
        &items[1],
        NavigationItem::Group(heading) if heading == "Via implementations/overrides (2)"
    ));
}

#[test]
fn usages_without_overrides_have_no_heading() {
    let items = usage_items(vec![usage(1, false), usage(2, false)]);

    assert_eq!(rows(&items), vec![Some(1), Some(2)]);
}
