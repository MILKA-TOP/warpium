use super::*;

#[test]
fn process_belongs_to_package_and_subprocesses() {
    assert!(process_belongs_to("com.example", "com.example"));
    assert!(process_belongs_to("com.example:remote", "com.example"));
    assert!(!process_belongs_to("com.example.other", "com.example"));
    assert!(!process_belongs_to("com.exam", "com.example"));
}

#[test]
fn parses_modern_start_proc() {
    let event = parse_activity_manager(
        "Start proc 12345:com.example.app/u0a123 for pre-top-activity \
         {com.example.app/com.example.app.MainActivity}",
    );

    assert_eq!(
        event,
        Some(ProcessEvent::Started {
            pid: 12345,
            process: "com.example.app".to_owned()
        })
    );
}

#[test]
fn parses_subprocess_start_proc() {
    let event = parse_activity_manager(
        "Start proc 2222:com.example.app:sync/u0a123 for service {com.example.app/.SyncService}",
    );

    assert_eq!(
        event,
        Some(ProcessEvent::Started {
            pid: 2222,
            process: "com.example.app:sync".to_owned()
        })
    );
}

#[test]
fn parses_legacy_start_proc() {
    let event = parse_activity_manager(
        "Start proc com.example.app for activity com.example.app/.MainActivity: pid=4321 \
         uid=10061 gids={50061, 3003}",
    );

    assert_eq!(
        event,
        Some(ProcessEvent::Started {
            pid: 4321,
            process: "com.example.app".to_owned()
        })
    );
}

#[test]
fn parses_process_death() {
    let event = parse_activity_manager("Process com.example.app (pid 12345) has died: fg  TOP ");

    assert_eq!(
        event,
        Some(ProcessEvent::Died {
            pid: 12345,
            process: "com.example.app".to_owned()
        })
    );
}

#[test]
fn ignores_other_messages() {
    assert_eq!(
        parse_activity_manager("Displayed com.example/.Main: +350ms"),
        None
    );
    assert_eq!(
        parse_activity_manager("Process com.example (pid 1) is idle"),
        None
    );
}

#[test]
fn parse_ps_reads_toybox_format() {
    let output = "  PID NAME\n    1 init\n12345 com.example.app\n12399 com.example.app:remote\n\
                  12400 com.example.application\n";

    assert_eq!(
        parse_ps(output, "com.example.app"),
        vec![
            (12345, "com.example.app".to_owned()),
            (12399, "com.example.app:remote".to_owned())
        ]
    );
}

#[test]
fn parse_ps_reads_legacy_format() {
    let output = "USER     PID   PPID  VSIZE  RSS     WCHAN    PC         NAME\n\
                  root      1     0     8904   788   ffffffff 00000000 S /init\n\
                  u0_a61    4321  1     912564 52424 ffffffff 00000000 S com.example.app\n";

    assert_eq!(
        parse_ps(output, "com.example.app"),
        vec![(4321, "com.example.app".to_owned())]
    );
}

#[test]
fn parse_ps_merges_sections_without_duplicates() {
    let output = "PID NAME\n4321 com.example.app\n\
                  USER     PID   PPID  VSIZE  RSS     WCHAN    PC         NAME\n\
                  u0_a61    4321  1     912564 52424 ffffffff 00000000 S com.example.app\n\
                  u0_a61    4400  1     912564 52424 ffffffff 00000000 S com.example.app:sync\n";

    assert_eq!(
        parse_ps(output, "com.example.app"),
        vec![
            (4321, "com.example.app".to_owned()),
            (4400, "com.example.app:sync".to_owned())
        ]
    );
}

#[test]
fn parse_ps_reads_legacy_output_after_an_empty_toybox_section() {
    let output = "USER PID PPID VSIZE RSS WCHAN PC NAME\n\
                  USER PID PPID VSIZE RSS WCHAN PC NAME\n\
                  u0_a61 4321 1 912564 52424 ffffffff 00000000 S com.example.app\n";

    assert_eq!(
        parse_ps(output, "com.example.app"),
        vec![(4321, "com.example.app".to_owned())]
    );
}

#[test]
fn parse_ps_without_header_is_empty() {
    assert!(parse_ps("", "com.example").is_empty());
    assert!(parse_ps("garbage\n1 com.example\n", "com.example").is_empty());
}
