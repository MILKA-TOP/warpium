#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::time::Duration;

use tempfile::TempDir;

use super::*;
use crate::invocation::parse_command;

struct FakeCli {
    dir: TempDir,
}

#[derive(Default)]
struct Behavior {
    exec_fails: bool,
    cp_fails: bool,
    ps_output: &'static str,
    inspect_json: String,
    sleep: bool,
}

impl FakeCli {
    /// A fake `docker` (and `ssh`) that logs its arguments, stores what it is sent under
    /// `received/`, and answers `ps` and `inspect` with canned output.
    fn new(behavior: Behavior) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().display().to_string();
        fs::create_dir(dir.path().join("received")).unwrap();
        fs::write(dir.path().join("inspect.json"), &behavior.inspect_json).unwrap();
        let script = format!(
            r#"#!/bin/sh
ROOT='{root}'
printf '%s\n' "$*" >> "$ROOT/log"
{sleep}
for last; do :; done
case " $* " in
  *" ps -q --no-trunc "*) printf '%s\n' '{ps_output}'; exit 0 ;;
  *" inspect "*) cat "$ROOT/inspect.json"; exit 0 ;;
  *" exec -i "*)
    if [ {exec_fails} = 1 ]; then echo 'OCI runtime exec failed: exec: "sh": executable file not found' >&2; exit 126; fi
    cat > "$ROOT/received/$last"; exit 0 ;;
  *" cp "*)
    if [ {cp_fails} = 1 ]; then echo 'Error response from daemon: read-only file system' >&2; exit 1; fi
    for a; do case "$a" in */.) src="${{a%/.}}" ;; esac; done
    cp -p "$src"/* "$ROOT/received/"; exit 0 ;;
  *" exec "*) exit 0 ;;
  *"BatchMode=yes"*)
    if [ {exec_fails} = 1 ]; then echo 'Permission denied (publickey).' >&2; exit 255; fi
    n=$(ls "$ROOT/received" | wc -l | tr -d ' ')
    cat > "$ROOT/received/ssh-$n"; exit 0 ;;
esac
echo "unexpected: $*" >&2
exit 2
"#,
            sleep = if behavior.sleep { "sleep 5" } else { "" },
            ps_output = behavior.ps_output,
            exec_fails = u8::from(behavior.exec_fails),
            cp_fails = u8::from(behavior.cp_fails),
        );
        for name in ["docker", "ssh"] {
            let path = dir.path().join(name);
            fs::write(&path, &script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { dir }
    }

    fn tools(&self) -> Tools {
        Tools {
            path_var: Some(self.dir.path().as_os_str().to_owned()),
            home: None,
            timeout: Duration::from_secs(10),
        }
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir.path().join("log")).unwrap_or_default()
    }

    fn received(&self, name: &str) -> Vec<u8> {
        fs::read(self.dir.path().join("received").join(name)).unwrap()
    }
}

fn request(cli: &FakeCli, command: &str, images: Vec<PasteImage>) -> PasteRequest {
    PasteRequest {
        target: parse_command(command, |_| None).unwrap(),
        hints: OwnedHints::default(),
        images,
        remote_dir: naming::DEFAULT_REMOTE_DIR.to_owned(),
        tools: cli.tools(),
    }
}

fn png(bytes: &[u8]) -> PasteImage {
    PasteImage {
        source: ImageSource::Bytes(bytes.to_vec()),
        extension: "png".to_owned(),
        original_name: None,
    }
}

fn file_name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap()
}

#[test]
fn streams_images_through_exec() {
    let cli = FakeCli::new(Behavior::default());
    let pasted = paste(&request(
        &cli,
        "docker --context colima exec -it web claude",
        vec![png(b"first"), png(b"second")],
    ))
    .unwrap();
    assert_eq!(pasted.method, Method::Exec);
    assert_eq!(pasted.destination, "web");
    assert_eq!(pasted.paths.len(), 2);
    for path in &pasted.paths {
        assert!(
            path.starts_with("/tmp/warpium-paste/warpium-paste-"),
            "{path}"
        );
        assert!(path.ends_with(".png"));
    }
    assert_eq!(cli.received(file_name_of(&pasted.paths[0])), b"first");
    assert_eq!(cli.received(file_name_of(&pasted.paths[1])), b"second");
    let log = cli.log();
    assert!(
        log.contains("--context colima exec -i web sh -c umask 022"),
        "{log}"
    );
}

#[test]
fn falls_back_to_cp_with_readable_permissions() {
    let cli = FakeCli::new(Behavior {
        exec_fails: true,
        ..Behavior::default()
    });
    let pasted = paste(&request(&cli, "docker exec -it web sh", vec![png(b"img")])).unwrap();
    assert_eq!(pasted.method, Method::Copy);
    let name = file_name_of(&pasted.paths[0]);
    assert_eq!(cli.received(name), b"img");
    let mode = fs::metadata(cli.dir.path().join("received").join(name))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o644);
    assert!(cli.log().contains(" web:/tmp/warpium-paste"));
}

#[test]
fn falls_back_to_the_project_bind_mount() {
    let project = tempfile::tempdir().unwrap();
    let source = project.path().display().to_string();
    let inspect_json = format!(
        r#"[{{"Id":"abc","Name":"/web","State":{{"Running":true}},
             "Mounts":[{{"Type":"bind","Source":"{source}","Destination":"/workspace","RW":true}}],
             "Config":{{"Image":"node"}}}}]"#
    );
    let cli = FakeCli::new(Behavior {
        exec_fails: true,
        cp_fails: true,
        inspect_json,
        ..Behavior::default()
    });
    let mut request = request(&cli, "docker exec -it web sh", vec![png(b"img")]);
    request.hints.cwd = Some(project.path().join("src"));
    let pasted = paste(&request).unwrap();
    assert_eq!(pasted.method, Method::Mount);
    let path = &pasted.paths[0];
    assert!(
        path.starts_with("/workspace/.warpium/paste/warpium-paste-"),
        "{path}"
    );
    let paste_dir = project.path().join(".warpium/paste");
    assert_eq!(
        fs::read(paste_dir.join(file_name_of(path))).unwrap(),
        b"img"
    );
    assert_eq!(
        fs::read_to_string(paste_dir.join(".gitignore")).unwrap(),
        "*\n"
    );
}

#[test]
fn reports_every_failed_method() {
    let cli = FakeCli::new(Behavior {
        exec_fails: true,
        cp_fails: true,
        inspect_json: "[]".to_owned(),
        ..Behavior::default()
    });
    let err = paste(&request(&cli, "docker exec -it web sh", vec![png(b"img")])).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("container `web`"), "{message}");
    assert!(message.contains("executable file not found"), "{message}");
    assert!(message.contains("read-only file system"), "{message}");
}

#[test]
fn resolves_run_sessions_through_ps_and_inspect() {
    let inspect_json = r#"[
      {"Id":"aaa111","Name":"/old","Created":"2026-11-19T10:00:00Z","State":{"Running":true},"Config":{"Image":"node:20"}},
      {"Id":"bbb222","Name":"/new","Created":"2026-11-19T11:00:00Z","State":{"Running":true},"Config":{"Image":"node:20"}},
      {"Id":"ccc333","Name":"/other","Created":"2026-11-19T12:00:00Z","State":{"Running":true},"Config":{"Image":"python"}}
    ]"#
    .to_owned();
    let cli = FakeCli::new(Behavior {
        ps_output: "aaa111 bbb222 ccc333",
        inspect_json,
        ..Behavior::default()
    });
    let pasted = paste(&request(
        &cli,
        "docker run -it --rm node:20 claude",
        vec![png(b"x")],
    ))
    .unwrap();
    assert_eq!(pasted.destination, "new");
    let log = cli.log();
    assert!(
        log.contains("inspect --type container aaa111 bbb222 ccc333"),
        "{log}"
    );
    assert!(log.contains("exec -i bbb222 sh"), "{log}");
}

#[test]
fn explains_when_no_container_matches() {
    let cli = FakeCli::new(Behavior {
        ps_output: "",
        ..Behavior::default()
    });
    let err = paste(&request(&cli, "docker run -it node:20", vec![png(b"x")])).unwrap_err();
    assert!(matches!(err, PasteError::NoContainer(_)));
    assert!(err.to_string().contains("image `node:20`"), "{err}");

    let cli = FakeCli::new(Behavior {
        ps_output: "aaa",
        inspect_json: r#"[{"Id":"aaa","State":{"Running":true},"Config":{"Image":"python"}}]"#
            .to_owned(),
        ..Behavior::default()
    });
    let err = paste(&request(&cli, "docker compose run app", vec![png(b"x")])).unwrap_err();
    assert!(err.to_string().contains("Compose service `app`"), "{err}");
}

#[test]
fn reports_a_missing_cli() {
    let cli = FakeCli::new(Behavior::default());
    let mut request = request(&cli, "podman exec -it web sh", vec![png(b"x")]);
    request.tools.path_var = Some(cli.dir.path().join("nothing").into_os_string());
    // Skip machines that have a real podman in the fallback folders.
    if request.tools.find_program("podman").is_some() {
        return;
    }
    let err = paste(&request).unwrap_err();
    assert_eq!(err, PasteError::ProgramNotFound("podman".to_owned()));
    assert!(err.to_string().contains("`podman` wasn't found"));
}

#[test]
fn reads_dropped_files_and_keeps_their_names() {
    let cli = FakeCli::new(Behavior::default());
    let source = cli.dir.path().join("Screen Shot.png");
    fs::write(&source, b"dropped").unwrap();
    let image = PasteImage {
        source: ImageSource::File(source),
        extension: "png".to_owned(),
        original_name: Some("Screen Shot.png".to_owned()),
    };
    let pasted = paste(&request(&cli, "docker exec -it web sh", vec![image])).unwrap();
    let name = file_name_of(&pasted.paths[0]);
    assert!(name.starts_with("warpium-Screen-Shot-"), "{name}");
    assert_eq!(cli.received(name), b"dropped");
}

#[test]
fn missing_files_fail_before_running_anything() {
    let cli = FakeCli::new(Behavior::default());
    let image = PasteImage {
        source: ImageSource::File(cli.dir.path().join("gone.png")),
        extension: "png".to_owned(),
        original_name: None,
    };
    let err = paste(&request(&cli, "docker exec -it web sh", vec![image])).unwrap_err();
    assert!(matches!(err, PasteError::Image(_)), "{err:?}");
    assert!(cli.log().is_empty());
}

#[test]
fn refuses_unsafe_remote_dirs() {
    let cli = FakeCli::new(Behavior::default());
    let mut request = request(&cli, "docker exec -it web sh", vec![png(b"x")]);
    request.remote_dir = "/tmp/$(reboot)".to_owned();
    assert!(matches!(paste(&request), Err(PasteError::Failed(_))));
    assert!(cli.log().is_empty());
}

#[test]
fn copies_over_ssh_without_prompting() {
    let cli = FakeCli::new(Behavior::default());
    let pasted = paste(&request(
        &cli,
        "ssh -p 2222 -t dev@box claude",
        vec![png(b"remote")],
    ))
    .unwrap();
    assert_eq!(pasted.method, Method::Ssh);
    assert_eq!(pasted.destination, "dev@box");
    assert_eq!(cli.received("ssh-0"), b"remote");
    let log = cli.log();
    assert!(log.contains("-p 2222 -o BatchMode=yes"), "{log}");
    assert!(log.contains("-T -- dev@box sh -c"), "{log}");
    assert!(log.contains(file_name_of(&pasted.paths[0])), "{log}");
    assert!(!log.contains(" -t "), "{log}");
}

#[test]
fn ssh_failures_explain_the_login_requirement() {
    let cli = FakeCli::new(Behavior {
        exec_fails: true,
        ..Behavior::default()
    });
    let err = paste(&request(&cli, "ssh box", vec![png(b"x")])).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("without a password"), "{message}");
    assert!(message.contains("Permission denied"), "{message}");
}

#[test]
fn gives_up_on_hanging_commands() {
    let cli = FakeCli::new(Behavior {
        sleep: true,
        ..Behavior::default()
    });
    let mut request = request(&cli, "docker exec -it web sh", vec![png(b"x")]);
    request.tools.timeout = Duration::from_millis(200);
    let err = paste(&request).unwrap_err();
    assert!(err.to_string().contains("didn't finish"), "{err}");
}

#[test]
fn finds_programs_in_fallback_folders() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join(".orbstack/bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("docker-test-fake"), "").unwrap();
    let tools = Tools {
        path_var: None,
        home: Some(home.path().to_path_buf()),
        timeout: Duration::from_secs(1),
    };
    assert_eq!(
        tools.find_program("docker-test-fake"),
        Some(bin.join("docker-test-fake"))
    );
    assert_eq!(tools.find_program("/nonexistent/docker"), None);
    assert!(
        tools
            .child_path(Path::new("/x/docker"))
            .to_string_lossy()
            .starts_with("/x:")
    );
}

#[test]
fn removes_old_files_from_host_folders() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("warpium-old.png");
    fs::write(&old, b"x").unwrap();
    let old_time = std::time::SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
    fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(old_time)
        .unwrap();
    fs::write(dir.path().join("warpium-new.png"), b"x").unwrap();
    fs::write(dir.path().join("keep.txt"), b"x").unwrap();
    remove_old_files(dir.path());
    assert!(!old.exists());
    assert!(dir.path().join("warpium-new.png").exists());
    assert!(dir.path().join("keep.txt").exists());
}
