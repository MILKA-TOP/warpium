use super::*;

fn entry(pid: i32, tag: &str, message: &str) -> LogEntry {
    LogEntry {
        time: LogTime {
            sec: 1_763_200_000,
            nsec: 123_000_000,
        },
        pid,
        tid: pid + 1,
        level: LogLevel::Info,
        tag: tag.into(),
        message: message.into(),
    }
}

#[test]
fn parses_v4_records() {
    let mut bytes = encode_entry(&entry(100, "MyApp", "hello"), 4);
    bytes.extend(encode_entry(&entry(200, "Other", "world"), 4));
    let mut out = Vec::new();

    BinaryParser::new().push(&bytes, &mut out).unwrap();

    assert_eq!(
        out,
        vec![entry(100, "MyApp", "hello"), entry(200, "Other", "world")]
    );
}

#[test]
fn keeps_partial_records_for_the_next_chunk() {
    let bytes = encode_entry(&entry(100, "MyApp", "split across reads"), 4);
    let mut parser = BinaryParser::new();
    let mut out = Vec::new();

    parser.push(&bytes[..7], &mut out).unwrap();
    parser.push(&bytes[7..30], &mut out).unwrap();
    let after_partial = out.len();
    parser.push(&bytes[30..], &mut out).unwrap();

    assert_eq!(after_partial, 0);
    assert_eq!(out, vec![entry(100, "MyApp", "split across reads")]);
}

#[test]
fn parses_v1_headers() {
    let mut payload = vec![6u8];
    payload.extend_from_slice(b"Old\0crash\n\0");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&42i32.to_le_bytes());
    bytes.extend_from_slice(&43i32.to_le_bytes());
    bytes.extend_from_slice(&10u32.to_le_bytes());
    bytes.extend_from_slice(&20u32.to_le_bytes());
    bytes.extend_from_slice(&payload);
    let mut out = Vec::new();

    BinaryParser::new().push(&bytes, &mut out).unwrap();

    assert_eq!(out.len(), 1);
    assert_eq!(out[0].pid, 42);
    assert_eq!(out[0].level, LogLevel::Error);
    assert_eq!(&*out[0].message, "crash");
    assert_eq!(out[0].time, LogTime { sec: 10, nsec: 20 });
}

#[test]
fn keeps_multi_line_messages_whole() {
    let message = "FATAL EXCEPTION: main\njava.lang.IllegalStateException\n\tat a.B.c(B.kt:1)";
    let bytes = encode_entry(&entry(1, "AndroidRuntime", message), 6);
    let mut out = Vec::new();

    BinaryParser::new().push(&bytes, &mut out).unwrap();

    assert_eq!(&*out[0].message, message);
}

#[test]
fn replaces_invalid_utf8() {
    let mut record = encode_entry(&entry(1, "T", "ab"), 4);
    let len = record.len();
    record[len - 2] = 0xff;
    let mut out = Vec::new();

    BinaryParser::new().push(&record, &mut out).unwrap();

    assert_eq!(&*out[0].message, "a\u{fffd}");
}

#[test]
fn rejects_garbage_headers() {
    let mut out = Vec::new();

    let result = BinaryParser::new().push(b"--------- beginning of main\n", &mut out);

    assert!(result.is_err());
}

#[test]
fn interns_tags() {
    let mut bytes = encode_entry(&entry(1, "Same", "a"), 4);
    bytes.extend(encode_entry(&entry(1, "Same", "b"), 4));
    let mut out = Vec::new();

    BinaryParser::new().push(&bytes, &mut out).unwrap();

    assert!(Arc::ptr_eq(&out[0].tag, &out[1].tag));
}

#[test]
fn level_from_priority_clamps_unknown_values() {
    assert_eq!(LogLevel::from_priority(0), LogLevel::Verbose);
    assert_eq!(LogLevel::from_priority(3), LogLevel::Debug);
    assert_eq!(LogLevel::from_priority(7), LogLevel::Fatal);
    assert_eq!(LogLevel::from_priority(8), LogLevel::Fatal);
}

#[test]
fn formats_time_like_threadtime() {
    let time = LogTime {
        sec: 1_763_200_000,
        nsec: 123_456_789,
    };

    assert_eq!(time.format_in(&chrono::Utc), "11-15 09:46:40.123");
}
