use std::path::Path;

use super::*;

const DOCKER_INSPECT: &str = r#"[
  {
    "Id": "4f2a9c1b7d3e5f60718293a4b5c6d7e8f90123456789abcdef0123456789abcd",
    "Created": "2026-11-19T10:00:00.123456789Z",
    "State": { "Status": "running", "Running": true },
    "Image": "sha256:aaaabbbbccccddddeeeeffff00001111222233334444555566667777888899990",
    "Name": "/proj-app-run-1a2b3c",
    "Mounts": [
      { "Type": "bind", "Source": "/Users/me/proj", "Destination": "/workspace", "Mode": "", "RW": true, "Propagation": "rprivate" },
      { "Type": "bind", "Source": "/var/run/docker.sock", "Destination": "/var/run/docker.sock", "RW": true },
      { "Type": "volume", "Name": "cache", "Source": "/var/lib/docker/volumes/cache/_data", "Destination": "/cache", "RW": true }
    ],
    "Config": {
      "Hostname": "4f2a9c1b7d3e",
      "Image": "node:20",
      "Labels": {
        "com.docker.compose.project": "proj",
        "com.docker.compose.service": "app",
        "com.docker.compose.oneoff": "True",
        "com.docker.compose.project.working_dir": "/Users/me/proj"
      }
    }
  }
]"#;

const PODMAN_INSPECT: &str = r#"[
  {
    "Id": "9e8d7c6b5a4f",
    "Created": "2026-11-19T11:30:00.5+01:00",
    "State": { "Running": false },
    "Image": "abcdef",
    "ImageName": "docker.io/library/ubuntu:latest",
    "Name": "agent",
    "Mounts": null,
    "Config": { "Hostname": "agent-host", "Image": "", "Labels": null }
  }
]"#;

#[test]
fn parses_docker_inspect() {
    let containers = parse_inspect(DOCKER_INSPECT).unwrap();
    assert_eq!(containers.len(), 1);
    let container = &containers[0];
    assert_eq!(container.name, "proj-app-run-1a2b3c");
    assert_eq!(container.display_name(), "proj-app-run-1a2b3c");
    assert_eq!(container.image, "node:20");
    assert_eq!(container.hostname, "4f2a9c1b7d3e");
    assert!(container.running);
    assert!(container.created.is_some());
    assert_eq!(
        container.labels.get("com.docker.compose.service").unwrap(),
        "app"
    );
    assert_eq!(container.mounts.len(), 3);
    assert!(container.mounts[0].bind && container.mounts[0].writable);
    assert!(!container.mounts[2].bind);
}

#[test]
fn parses_podman_inspect_with_nulls() {
    let containers = parse_inspect(PODMAN_INSPECT).unwrap();
    let container = &containers[0];
    assert_eq!(container.image, "docker.io/library/ubuntu:latest");
    assert!(!container.running);
    assert!(container.labels.is_empty());
    assert!(container.mounts.is_empty());
    assert!(container.created.is_some());
}

#[test]
fn rejects_unexpected_output() {
    assert!(parse_inspect("Error: no such container").is_err());
    assert_eq!(parse_inspect("[]").unwrap(), Vec::new());
}

#[test]
fn normalizes_images() {
    assert_eq!(normalize_image("node"), "node:latest");
    assert_eq!(normalize_image("node:20"), "node:20");
    assert_eq!(normalize_image("library/node:20"), "node:20");
    assert_eq!(normalize_image("docker.io/library/node:20"), "node:20");
    assert_eq!(normalize_image("docker.io/acme/agent"), "acme/agent:latest");
    assert_eq!(normalize_image("localhost/agent"), "agent:latest");
    assert_eq!(
        normalize_image("ghcr.io/acme/agent@sha256:0123"),
        "ghcr.io/acme/agent:latest"
    );
    assert_eq!(
        normalize_image("registry.local:5000/team/agent:1"),
        "registry.local:5000/team/agent:1"
    );
    assert_eq!(
        normalize_image("ghcr.io/library/x"),
        "ghcr.io/library/x:latest"
    );
}

fn container(id: &str, image: &str, created: &str, labels: &[(&str, &str)]) -> ContainerInfo {
    ContainerInfo {
        id: id.to_owned(),
        name: format!("name-{id}"),
        image: image.to_owned(),
        image_id: format!("sha256:{id}{id}"),
        hostname: id[..id.len().min(12)].to_owned(),
        labels: labels
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
        created: DateTime::parse_from_rfc3339(created).ok(),
        running: true,
        mounts: Vec::new(),
    }
}

fn compose(service: &str, one_off: bool) -> ContainerRef {
    ContainerRef::ComposeService(ComposeService {
        service: service.to_owned(),
        project: None,
        project_directory: None,
        files: Vec::new(),
        one_off,
    })
}

fn compose_labels<'a>(
    project: &'a str,
    service: &'a str,
    one_off: &'a str,
    dir: &'a str,
) -> [(&'a str, &'a str); 4] {
    [
        (COMPOSE_PROJECT_LABEL, project),
        (COMPOSE_SERVICE_LABEL, service),
        (COMPOSE_ONE_OFF_LABEL, one_off),
        (COMPOSE_WORKING_DIR_LABEL, dir),
    ]
}

#[test]
fn selects_the_newest_running_container_of_an_image() {
    let mut stopped = container("cccccccccccccccc", "node:20", "2026-11-19T12:00:00Z", &[]);
    stopped.running = false;
    let containers = vec![
        container("aaaaaaaaaaaaaaaa", "node:20", "2026-11-19T10:00:00Z", &[]),
        container(
            "bbbbbbbbbbbbbbbb",
            "docker.io/library/node:20",
            "2026-11-19T11:00:00Z",
            &[],
        ),
        stopped,
        container("dddddddddddddddd", "node:22", "2026-11-19T13:00:00Z", &[]),
    ];
    let selected = select_container(
        &containers,
        &ContainerRef::Image("node:20".to_owned()),
        &SelectionHints::default(),
    )
    .unwrap();
    assert_eq!(selected.id, "bbbbbbbbbbbbbbbb");

    assert!(
        select_container(
            &containers,
            &ContainerRef::Image("python".to_owned()),
            &SelectionHints::default()
        )
        .is_none()
    );
}

#[test]
fn the_hostname_hint_wins_over_age() {
    let containers = vec![
        container("aaaaaaaaaaaaaaaa", "node:20", "2026-11-19T10:00:00Z", &[]),
        container("bbbbbbbbbbbbbbbb", "node:20", "2026-11-19T11:00:00Z", &[]),
    ];
    let hints = SelectionHints {
        cwd: None,
        hostname: Some("aaaaaaaaaaaa"),
    };
    let selected = select_container(
        &containers,
        &ContainerRef::Image("node:20".to_owned()),
        &hints,
    )
    .unwrap();
    assert_eq!(selected.id, "aaaaaaaaaaaaaaaa");
}

#[test]
fn matches_images_by_id_prefix() {
    let containers = vec![container(
        "abcdef012345",
        "something-else",
        "2026-11-19T10:00:00Z",
        &[],
    )];
    assert!(
        select_container(
            &containers,
            &ContainerRef::Image("abcdef012345".to_owned()),
            &SelectionHints::default()
        )
        .is_some()
    );
}

#[test]
fn selects_named_containers_by_name_or_id_prefix() {
    let containers = vec![container("abcdef012345", "x", "2026-11-19T10:00:00Z", &[])];
    let hints = SelectionHints::default();
    assert!(
        select_container(
            &containers,
            &ContainerRef::Named("name-abcdef012345".to_owned()),
            &hints
        )
        .is_some()
    );
    assert!(
        select_container(&containers, &ContainerRef::Named("abcd".to_owned()), &hints).is_some()
    );
    assert!(
        select_container(
            &containers,
            &ContainerRef::Named("other".to_owned()),
            &hints
        )
        .is_none()
    );
}

#[test]
fn compose_prefers_the_project_in_the_shell_directory() {
    let containers = vec![
        container(
            "aaaaaaaaaaaaaaaa",
            "x",
            "2026-11-19T12:00:00Z",
            &compose_labels("other", "app", "True", "/Users/me/other"),
        ),
        container(
            "bbbbbbbbbbbbbbbb",
            "x",
            "2026-11-19T10:00:00Z",
            &compose_labels("proj", "app", "True", "/Users/me/proj"),
        ),
        container(
            "cccccccccccccccc",
            "x",
            "2026-11-19T13:00:00Z",
            &compose_labels("proj", "db", "False", "/Users/me/proj"),
        ),
    ];
    let hints = SelectionHints {
        cwd: Some(Path::new("/Users/me/proj")),
        hostname: None,
    };
    let selected = select_container(&containers, &compose("app", true), &hints).unwrap();
    assert_eq!(selected.id, "bbbbbbbbbbbbbbbb");

    // From a subfolder, Compose finds the project's file upwards.
    let hints = SelectionHints {
        cwd: Some(Path::new("/Users/me/proj/src")),
        hostname: None,
    };
    let selected = select_container(&containers, &compose("app", true), &hints).unwrap();
    assert_eq!(selected.id, "bbbbbbbbbbbbbbbb");

    // Without a directory, the newest container of the service.
    let selected = select_container(
        &containers,
        &compose("app", true),
        &SelectionHints::default(),
    )
    .unwrap();
    assert_eq!(selected.id, "aaaaaaaaaaaaaaaa");
}

#[test]
fn compose_project_flag_filters_by_normalized_name() {
    let containers = vec![
        container(
            "aaaaaaaaaaaaaaaa",
            "x",
            "2026-11-19T12:00:00Z",
            &compose_labels("other", "app", "True", "/a"),
        ),
        container(
            "bbbbbbbbbbbbbbbb",
            "x",
            "2026-11-19T10:00:00Z",
            &compose_labels("myproj", "app", "True", "/b"),
        ),
    ];
    let target = ContainerRef::ComposeService(ComposeService {
        service: "app".to_owned(),
        project: Some("My.Proj".to_owned()),
        project_directory: None,
        files: Vec::new(),
        one_off: true,
    });
    let selected = select_container(&containers, &target, &SelectionHints::default()).unwrap();
    assert_eq!(selected.id, "bbbbbbbbbbbbbbbb");
}

#[test]
fn compose_explicit_files_and_directories() {
    let containers = vec![
        container(
            "aaaaaaaaaaaaaaaa",
            "x",
            "2026-11-19T12:00:00Z",
            &compose_labels("a", "app", "True", "/Users/me/proj"),
        ),
        container(
            "bbbbbbbbbbbbbbbb",
            "x",
            "2026-11-19T10:00:00Z",
            &compose_labels("b", "app", "True", "/Users/me/proj/dev"),
        ),
    ];
    let hints = SelectionHints {
        cwd: Some(Path::new("/Users/me/proj")),
        hostname: None,
    };
    let with_file = ContainerRef::ComposeService(ComposeService {
        service: "app".to_owned(),
        project: None,
        project_directory: None,
        files: vec!["dev/compose.yml".to_owned()],
        one_off: true,
    });
    assert_eq!(
        select_container(&containers, &with_file, &hints)
            .unwrap()
            .id,
        "bbbbbbbbbbbbbbbb"
    );
    let with_dir = ContainerRef::ComposeService(ComposeService {
        service: "app".to_owned(),
        project: None,
        project_directory: Some("./dev/../dev".to_owned()),
        files: Vec::new(),
        one_off: true,
    });
    assert_eq!(
        select_container(&containers, &with_dir, &hints).unwrap().id,
        "bbbbbbbbbbbbbbbb"
    );
}

#[test]
fn compose_exec_prefers_the_service_container_over_one_offs() {
    let containers = vec![
        container(
            "aaaaaaaaaaaaaaaa",
            "x",
            "2026-11-19T12:00:00Z",
            &compose_labels("p", "app", "True", "/p"),
        ),
        container(
            "bbbbbbbbbbbbbbbb",
            "x",
            "2026-11-19T10:00:00Z",
            &compose_labels("p", "app", "False", "/p"),
        ),
    ];
    let hints = SelectionHints::default();
    assert_eq!(
        select_container(&containers, &compose("app", false), &hints)
            .unwrap()
            .id,
        "bbbbbbbbbbbbbbbb"
    );
    assert_eq!(
        select_container(&containers, &compose("app", true), &hints)
            .unwrap()
            .id,
        "aaaaaaaaaaaaaaaa"
    );
}

#[test]
fn podman_compose_project_label() {
    let containers = vec![container(
        "aaaaaaaaaaaaaaaa",
        "x",
        "2026-11-19T12:00:00Z",
        &[
            (PODMAN_COMPOSE_PROJECT_LABEL, "proj"),
            (COMPOSE_SERVICE_LABEL, "app"),
        ],
    )];
    let target = ContainerRef::ComposeService(ComposeService {
        service: "app".to_owned(),
        project: Some("proj".to_owned()),
        project_directory: None,
        files: Vec::new(),
        one_off: true,
    });
    assert!(select_container(&containers, &target, &SelectionHints::default()).is_some());
}

fn bind(source: &str, destination: &str, writable: bool) -> Mount {
    Mount {
        bind: true,
        source: source.to_owned(),
        destination: destination.to_owned(),
        writable,
    }
}

#[test]
fn picks_the_innermost_writable_bind_mount_holding_the_cwd() {
    let mounts = vec![
        bind("/Users/me", "/home/me", true),
        bind("/Users/me/proj", "/workspace", true),
        bind("/Users/me/proj/ro", "/ro", false),
        bind("/var/run/docker.sock", "/var/run/docker.sock", true),
        Mount {
            bind: false,
            source: "/Users/me/proj/src".to_owned(),
            destination: "/src".to_owned(),
            writable: true,
        },
    ];
    let mount = mount_for_cwd(&mounts, Path::new("/Users/me/proj/src")).unwrap();
    assert_eq!(mount.destination, "/workspace");
    let mount = mount_for_cwd(&mounts, Path::new("/Users/me/proj/ro/x")).unwrap();
    assert_eq!(mount.destination, "/workspace");
    assert!(mount_for_cwd(&mounts, Path::new("/tmp")).is_none());
    // `/Users/me/project` is not inside `/Users/me/proj`.
    let mount = mount_for_cwd(&mounts, Path::new("/Users/me/project")).unwrap();
    assert_eq!(mount.destination, "/home/me");
}

#[test]
fn maps_host_paths_into_the_mount() {
    let mount = bind("/Users/me/proj", "/workspace/", true);
    assert_eq!(
        container_path(&mount, Path::new("/Users/me/proj/.warpium/paste/a.png")).as_deref(),
        Some("/workspace/.warpium/paste/a.png")
    );
    assert_eq!(container_path(&mount, Path::new("/elsewhere/a.png")), None);
}
