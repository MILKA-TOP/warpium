use remote_server::HostId;
use remote_server::proto::{RipgrepSearchMatch, RipgrepSearchSubmatch, RipgrepSearchSuccess};
use warp_ripgrep::filter::{PathFilter, SearchFilters};
use warp_util::local_or_remote_path::LocalOrRemotePath;
use warp_util::standardized_path::StandardizedPath;

use super::GlobalSearch;

fn host() -> HostId {
    HostId::new("test-host".to_string())
}

fn proto_match(
    path: &str,
    line_number: u32,
    line_text: &str,
    submatches: Vec<(u64, u64)>,
) -> RipgrepSearchMatch {
    RipgrepSearchMatch {
        file_path: path.to_string(),
        line_number,
        line_text: line_text.to_string(),
        submatches: submatches
            .into_iter()
            .map(|(byte_start, byte_end)| RipgrepSearchSubmatch {
                byte_start,
                byte_end,
            })
            .collect(),
    }
}

#[test]
fn remote_matches_become_remote_locations_on_the_host() {
    let success = RipgrepSearchSuccess {
        matches: vec![proto_match(
            "/repo/src/main.rs",
            7,
            "fn main() {}",
            vec![(3, 7)],
        )],
        capped: false,
    };

    let results = GlobalSearch::remote_matches_to_global(&host(), success, None);

    assert_eq!(results.len(), 1);
    match &results[0].location {
        LocalOrRemotePath::Remote(remote) => {
            assert_eq!(remote.host_id, host());
            assert_eq!(remote.path.as_str(), "/repo/src/main.rs");
        }
        LocalOrRemotePath::Local(_) => panic!("expected a remote location"),
    }
    assert_eq!(results[0].line_number, 7);
    assert_eq!(results[0].column_num, Some(4));
    assert_eq!(results[0].line_text, "fn main() {}");
}

#[test]
fn remote_matches_expand_one_row_per_submatch() {
    let success = RipgrepSearchSuccess {
        matches: vec![proto_match(
            "/repo/a.rs",
            1,
            "foo foo",
            vec![(0, 3), (4, 7)],
        )],
        capped: false,
    };

    let results = GlobalSearch::remote_matches_to_global(&host(), success, None);

    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|m| m.submatches.len() == 1));
}

#[test]
fn remote_matches_with_invalid_paths_are_dropped() {
    let success = RipgrepSearchSuccess {
        matches: vec![
            proto_match("relative/path.rs", 1, "x", vec![(0, 1)]),
            proto_match("/repo/ok.rs", 2, "x", vec![(0, 1)]),
        ],
        capped: false,
    };

    let results = GlobalSearch::remote_matches_to_global(&host(), success, None);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].location.display_path(), "/repo/ok.rs");
}

#[test]
fn remote_match_leading_whitespace_is_trimmed_per_submatch() {
    // Leading whitespace before the submatch is trimmed and offsets adjusted,
    // matching local search behavior.
    let success = RipgrepSearchSuccess {
        matches: vec![proto_match("/repo/a.rs", 1, "    foo", vec![(4, 7)])],
        capped: false,
    };

    let results = GlobalSearch::remote_matches_to_global(&host(), success, None);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].line_text, "foo");
    assert_eq!(results[0].column_num, Some(5));
    assert_eq!(results[0].submatches[0].byte_start.as_usize(), 0);
    assert_eq!(results[0].submatches[0].byte_end.as_usize(), 3);
}

#[test]
fn remote_match_column_counts_characters_not_bytes() {
    let success = RipgrepSearchSuccess {
        matches: vec![proto_match("/repo/a.rs", 1, "€foo", vec![(3, 6)])],
        capped: false,
    };

    let results = GlobalSearch::remote_matches_to_global(&host(), success, None);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].column_num, Some(2));
}

#[test]
fn remote_matches_are_filtered_by_globs_relative_to_their_root() {
    let success = RipgrepSearchSuccess {
        matches: vec![
            proto_match("/repo/src/App.kt", 1, "x", vec![(0, 1)]),
            proto_match("/repo/build/gen/App.kt", 1, "x", vec![(0, 1)]),
            proto_match("/repo/src/App.java", 1, "x", vec![(0, 1)]),
            proto_match("/other/src/App.kt", 1, "x", vec![(0, 1)]),
        ],
        capped: false,
    };
    let path_filter = PathFilter::new(&SearchFilters {
        use_ignore_files: true,
        include_globs: vec!["src/**/*.kt".to_string()],
        exclude_globs: vec!["*build*".to_string()],
    })
    .unwrap();
    let roots = vec![StandardizedPath::try_new("/repo").unwrap()];

    let results =
        GlobalSearch::remote_matches_to_global(&host(), success, Some(&(path_filter, roots)));

    let paths: Vec<_> = results.iter().map(|m| m.location.display_path()).collect();
    assert_eq!(paths, vec!["/repo/src/App.kt", "/other/src/App.kt"]);
}
