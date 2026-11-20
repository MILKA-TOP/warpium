use std::sync::mpsc;
use std::time::Duration;

use super::*;
use crate::blame::{Blame, blame_args};
use crate::diff::{changed_files_args, file_diff_args, parse_name_status, parse_unified_diff};
use crate::graph::Half;
use crate::log::Revisions;
use crate::refs::RefKind;
use crate::show::{details_args, parse_details};
use crate::test_support::{commit_file, fast_import_commits, git, init_repo};

const WAIT: Duration = Duration::from_secs(20);

/// Collects events until the stream finishes or fails.
fn collect(receiver: &mpsc::Receiver<LogEvent>) -> (Vec<LogRow>, Option<LogEvent>) {
    let mut rows = Vec::new();
    loop {
        match receiver.recv_timeout(WAIT).expect("an event") {
            LogEvent::Rows(batch) => rows.extend(batch),
            end @ (LogEvent::Finished { .. } | LogEvent::Failed(_)) => return (rows, Some(end)),
        }
    }
}

#[test]
fn streams_a_history_with_a_merge() {
    let repo = init_repo();
    let path = repo.path();
    commit_file(path, "a.txt", "1\n", "Initial");
    git(path, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(path, "b.txt", "feature\n", "Feature work");
    git(path, &["checkout", "--quiet", "main"]);
    commit_file(path, "a.txt", "1\n2\n", "Main work");
    git(
        path,
        &[
            "merge",
            "--quiet",
            "--no-ff",
            "-m",
            "Merge feature",
            "feature",
        ],
    );
    git(path, &["tag", "v1"]);

    let (sender, receiver) = mpsc::channel();
    let _stream = spawn_log(path, &LogQuery::default(), 100, move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    let (rows, end) = collect(&receiver);
    assert!(matches!(end, Some(LogEvent::Finished { total: 4 })));
    let subjects: Vec<&str> = rows.iter().map(|row| row.commit.subject.as_str()).collect();
    assert_eq!(subjects[0], "Merge feature");
    assert_eq!(subjects[3], "Initial");

    let merge = &rows[0];
    assert_eq!(merge.commit.parents.len(), 2);
    assert!(
        merge
            .commit
            .refs
            .iter()
            .any(|r| r.is_head && r.name == "main")
    );
    assert!(
        merge
            .commit
            .refs
            .iter()
            .any(|r| r.kind == RefKind::Tag && r.name == "v1")
    );
    assert!(
        merge
            .graph
            .segments
            .iter()
            .any(|segment| segment.half == Half::Bottom && segment.to == 1)
    );
    assert_eq!(rows[3].graph.width, 2, "both lanes end at the root");
}

#[test]
fn reads_only_as_far_as_requested() {
    let repo = init_repo();
    fast_import_commits(repo.path(), 6_000);

    let (sender, receiver) = mpsc::channel();
    let stream = spawn_log(repo.path(), &LogQuery::default(), 100, move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    let mut received = 0;
    match receiver.recv_timeout(WAIT).expect("first rows") {
        LogEvent::Rows(rows) => received += rows.len(),
        other => panic!("unexpected {other:?}"),
    }
    assert!(received >= 100);
    assert!(received < 6_000, "stopped early, got {received}");
    // Nothing more arrives until more is requested.
    assert!(receiver.recv_timeout(Duration::from_millis(300)).is_err());

    stream.request(usize::MAX);
    let (rows, end) = collect(&receiver);
    assert_eq!(received + rows.len(), 6_000);
    assert!(matches!(end, Some(LogEvent::Finished { total: 6_000 })));
}

#[test]
fn dropping_the_stream_stops_git() {
    let repo = init_repo();
    fast_import_commits(repo.path(), 3_000);
    let (sender, receiver) = mpsc::channel();
    let stream = spawn_log(repo.path(), &LogQuery::default(), 10, move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    assert!(matches!(receiver.recv_timeout(WAIT), Ok(LogEvent::Rows(_))));
    drop(stream);
    // The thread exits without reporting a failure, and drops its sender.
    loop {
        match receiver.recv_timeout(WAIT) {
            Ok(LogEvent::Rows(_)) => continue,
            Ok(other) => panic!("unexpected {other:?}"),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => panic!("the log thread didn't exit"),
        }
    }
}

#[test]
fn filters_and_file_history() {
    let repo = init_repo();
    let path = repo.path();
    commit_file(path, "a.txt", "1\n", "Add a");
    commit_file(path, "b.txt", "1\n", "Add b");
    commit_file(path, "a.txt", "2\n", "Change a");

    let history = LogQuery {
        paths: vec!["a.txt".to_owned()],
        ..Default::default()
    };
    let (sender, receiver) = mpsc::channel();
    let _stream = spawn_log(path, &history, 100, move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    let (rows, _) = collect(&receiver);
    let subjects: Vec<&str> = rows.iter().map(|row| row.commit.subject.as_str()).collect();
    assert_eq!(subjects, ["Change a", "Add a"]);
    // Parents are rewritten to the listed commits, so the graph stays connected.
    assert_eq!(rows[0].commit.parents, vec![rows[1].commit.oid]);

    let text = LogQuery {
        text: Some("ADD".to_owned()),
        ..Default::default()
    };
    let (sender, receiver) = mpsc::channel();
    let _stream = spawn_log(path, &text, 100, move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    let (rows, _) = collect(&receiver);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.graph == GraphRow::single()));
}

#[test]
fn reports_failures() {
    let repo = init_repo();
    let (sender, receiver) = mpsc::channel();
    let query = LogQuery {
        revisions: Revisions::Rev("no-such-branch".to_owned()),
        ..Default::default()
    };
    let _stream = spawn_log(repo.path(), &query, 100, move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    let (rows, end) = collect(&receiver);
    assert!(rows.is_empty());
    match end {
        Some(LogEvent::Failed(message)) => assert!(!message.starts_with("fatal:"), "{message}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn details_files_diff_and_blame_from_real_git() {
    let repo = init_repo();
    let path = repo.path();
    commit_file(path, "a.txt", "one\ntwo\n", "Initial\n\nLonger body.");
    commit_file(path, "a.txt", "one\nTWO\nthree\n", "Change a");
    let head =
        String::from_utf8(run_git(path, &["rev-parse".into(), "HEAD".into()]).unwrap()).unwrap();
    let head = crate::Oid::from_hex(head.trim()).unwrap();

    let details = String::from_utf8(run_git(path, &details_args(&head)).unwrap()).unwrap();
    let details = parse_details(&details).unwrap();
    assert_eq!(details.oid, head);
    assert_eq!(details.message, "Change a");
    let parent = details.parents[0];
    let parent_details =
        parse_details(&String::from_utf8(run_git(path, &details_args(&parent)).unwrap()).unwrap())
            .unwrap();
    assert_eq!(parent_details.message, "Initial\n\nLonger body.");

    let files =
        parse_name_status(&run_git(path, &changed_files_args(&head, Some(&parent))).unwrap());
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "a.txt");
    let root_files = parse_name_status(&run_git(path, &changed_files_args(&parent, None)).unwrap());
    assert_eq!(root_files[0].kind.letter(), 'A');

    let patch = run_git(path, &file_diff_args(&head, Some(&parent), &files[0])).unwrap();
    let lines = parse_unified_diff(&String::from_utf8_lossy(&patch));
    let added: Vec<&str> = lines
        .iter()
        .filter(|line| line.kind == crate::diff::DiffLineKind::Added)
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(added, ["TWO", "three"]);

    let edited = "one\nTWO\nthree\nfour\n";
    let output = run_git_with_input(path, &blame_args("a.txt", true), edited.as_bytes()).unwrap();
    let blame = Blame::parse_porcelain(&String::from_utf8_lossy(&output)).unwrap();
    assert_eq!(blame.line_count(), 4);
    assert_eq!(blame.line(0).unwrap().oid, parent);
    assert_eq!(blame.line(1).unwrap().oid, head);
    assert!(blame.line(3).unwrap().is_uncommitted());
}

#[test]
fn toplevel_and_relative_paths() {
    let repo = init_repo();
    std::fs::create_dir_all(repo.path().join("src/deep")).unwrap();
    let toplevel = repo_toplevel(&repo.path().join("src/deep")).unwrap();
    assert_eq!(
        toplevel.canonicalize().unwrap(),
        repo.path().canonicalize().unwrap()
    );
    let file = repo.path().join("src/deep/File.kt");
    assert_eq!(
        relative_to_repo(&toplevel, &file).as_deref(),
        Some("src/deep/File.kt")
    );
    assert_eq!(relative_to_repo(&toplevel, Path::new("/elsewhere/x")), None);

    let outside = tempfile::tempdir().unwrap();
    assert!(repo_toplevel(outside.path()).is_err());
}
