use super::*;

fn section(
    section: Section,
    items: &[&'static str],
    truncated: bool,
) -> SectionResults<&'static str> {
    SectionResults {
        section,
        items: items.to_vec(),
        truncated,
    }
}

fn header(section: Section, label: &str) -> Entry<&'static str> {
    Entry::Header {
        section,
        label: label.to_owned(),
    }
}

#[test]
fn tabs_cycle_in_both_directions() {
    assert_eq!(SearchTab::All.next(), SearchTab::Classes);
    assert_eq!(SearchTab::Symbols.next(), SearchTab::Files);
    assert_eq!(SearchTab::Files.next(), SearchTab::Text);
    assert_eq!(SearchTab::Text.next(), SearchTab::All);
    assert_eq!(SearchTab::All.previous(), SearchTab::Text);
    assert_eq!(SearchTab::Classes.previous(), SearchTab::All);
    for tab in SearchTab::ALL {
        assert_eq!(tab.next().previous(), tab);
    }
}

#[test]
fn sections_point_at_their_tabs() {
    assert_eq!(Section::Classes.tab(), SearchTab::Classes);
    assert_eq!(Section::Symbols.tab(), SearchTab::Symbols);
    assert_eq!(Section::Files.tab(), SearchTab::Files);
    assert_eq!(Section::Files.label(), "Files");
}

#[test]
fn merge_keeps_section_order_and_puts_a_header_over_each() {
    let entries = merge_sections(
        vec![
            section(Section::Classes, &["UserViewModel"], false),
            section(Section::Symbols, &["user", "userId"], false),
            section(Section::Files, &["User.kt"], false),
        ],
        SECTION_LIMIT,
    );

    assert_eq!(
        entries,
        vec![
            header(Section::Classes, "Classes"),
            Entry::Item("UserViewModel"),
            header(Section::Symbols, "Symbols"),
            Entry::Item("user"),
            Entry::Item("userId"),
            header(Section::Files, "Files"),
            Entry::Item("User.kt"),
        ]
    );
}

#[test]
fn merge_leaves_out_empty_sections() {
    let entries = merge_sections(
        vec![
            section(Section::Classes, &[], false),
            section(Section::Symbols, &[], true),
            section(Section::Files, &["build.gradle.kts"], false),
        ],
        SECTION_LIMIT,
    );

    assert_eq!(
        entries,
        vec![
            header(Section::Files, "Files"),
            Entry::Item("build.gradle.kts")
        ]
    );
}

#[test]
fn merge_caps_each_section_and_says_how_many_are_shown() {
    let entries = merge_sections(
        vec![
            section(Section::Classes, &["A", "B", "C"], false),
            section(Section::Files, &["a.kt", "b.kt", "c.kt"], true),
        ],
        2,
    );

    assert_eq!(
        entries,
        vec![
            header(Section::Classes, "Classes · 2 of 3"),
            Entry::Item("A"),
            Entry::Item("B"),
            header(Section::Files, "Files · 2 of 3+"),
            Entry::Item("a.kt"),
            Entry::Item("b.kt"),
        ]
    );
}

#[test]
fn a_section_cut_off_by_its_search_says_so_even_when_all_rows_fit() {
    let entries = merge_sections(vec![section(Section::Symbols, &["a"], true)], 2);

    assert_eq!(entries[0], header(Section::Symbols, "Symbols · 1 of 1+"));
}

#[test]
fn only_build_files_or_an_earlier_index_start_indexing() {
    let signals = |has_build_files, has_snapshot| ProjectSignals {
        has_build_files,
        has_snapshot,
    };
    assert!(should_index(signals(true, false)));
    assert!(should_index(signals(false, true)));
    assert!(!should_index(signals(false, false)));
}

#[test]
fn indexed_projects_get_tabs_unless_the_finished_index_is_empty() {
    for signals in [
        ProjectSignals {
            has_build_files: true,
            has_snapshot: false,
        },
        ProjectSignals {
            has_build_files: false,
            has_snapshot: true,
        },
    ] {
        assert!(offers_code_tabs(signals, IndexStatus::Indexing));
        assert!(offers_code_tabs(
            signals,
            IndexStatus::Ready { files: Some(3) }
        ));
        assert!(offers_code_tabs(
            signals,
            IndexStatus::Ready { files: None }
        ));
        assert!(!offers_code_tabs(
            signals,
            IndexStatus::Ready { files: Some(0) }
        ));
    }
}

#[test]
fn other_folders_never_get_tabs() {
    let signals = ProjectSignals {
        has_build_files: false,
        has_snapshot: false,
    };

    assert!(!offers_code_tabs(
        signals,
        IndexStatus::Ready { files: Some(40) }
    ));
    assert!(!offers_code_tabs(signals, IndexStatus::Indexing));
}
