use super::*;

fn parse(command: &str) -> Option<Target> {
    parse_command(command, |_| None)
}

fn container(command: &str) -> ContainerInvocation {
    match parse(command) {
        Some(Target::Container(invocation)) => invocation,
        other => panic!("expected a container for {command:?}, got {other:?}"),
    }
}

fn ssh(command: &str) -> SshInvocation {
    match parse(command) {
        Some(Target::Ssh(invocation)) => invocation,
        other => panic!("expected SSH for {command:?}, got {other:?}"),
    }
}

fn named(name: &str) -> ContainerRef {
    ContainerRef::Named(name.to_owned())
}

#[test]
fn docker_exec_targets_the_named_container() {
    let invocation = container("docker exec -it my-app claude --resume");
    assert_eq!(invocation.engine, Engine::Docker);
    assert_eq!(invocation.program, "docker");
    assert_eq!(invocation.container, named("my-app"));
    assert_eq!(invocation.inner_command.as_deref(), Some("claude --resume"));
    assert!(invocation.global_args.is_empty());
}

#[test]
fn docker_exec_skips_option_values() {
    let invocation = container(
        "docker exec -e FOO=bar --user 1000:1000 -w /work --env-file .env -it abc123 zsh",
    );
    assert_eq!(invocation.container, named("abc123"));
    assert_eq!(invocation.inner_command.as_deref(), Some("zsh"));

    let invocation = container("docker exec --workdir=/app -u root web bash");
    assert_eq!(invocation.container, named("web"));
}

#[test]
fn detached_sessions_are_ignored() {
    assert_eq!(parse("docker exec -d web touch x"), None);
    assert_eq!(parse("docker run -d nginx"), None);
    assert_eq!(parse("docker run -dit ubuntu"), None);
    assert_eq!(parse("docker compose run --detach web"), None);
}

#[test]
fn docker_run_without_name_targets_the_image() {
    let invocation = container("docker run -it --rm -v $PWD:/work -w /work node:20 claude");
    assert_eq!(
        invocation.container,
        ContainerRef::Image("node:20".to_owned())
    );
    assert_eq!(invocation.inner_command.as_deref(), Some("claude"));

    let invocation = container(
        "docker run --rm -it -e ANTHROPIC_API_KEY -p 8080:80 --mount type=bind,src=.,dst=/w \
         --network host --platform linux/amd64 ghcr.io/acme/agent:1.2 codex",
    );
    assert_eq!(
        invocation.container,
        ContainerRef::Image("ghcr.io/acme/agent:1.2".to_owned())
    );
    assert_eq!(invocation.inner_command.as_deref(), Some("codex"));
}

#[test]
fn docker_run_with_name_targets_the_name() {
    assert_eq!(
        container("docker run -it --name agent ubuntu bash").container,
        named("agent")
    );
    assert_eq!(
        container("docker run --name=agent -it ubuntu").container,
        named("agent")
    );
}

#[test]
fn short_option_clusters_and_attached_values() {
    let invocation = container("docker run -itp8080:80 alpine sh");
    assert_eq!(
        invocation.container,
        ContainerRef::Image("alpine".to_owned())
    );

    let invocation = container("docker run -itv /a:/b alpine sh");
    assert_eq!(
        invocation.container,
        ContainerRef::Image("alpine".to_owned())
    );

    let invocation = container("docker run -eFOO=1 -it alpine");
    assert_eq!(
        invocation.container,
        ContainerRef::Image("alpine".to_owned())
    );
    assert_eq!(invocation.inner_command, None);
}

#[test]
fn boolean_options_with_values_after_equals() {
    let invocation = container("docker run --sig-proxy=false --rm -it alpine");
    assert_eq!(
        invocation.container,
        ContainerRef::Image("alpine".to_owned())
    );
}

#[test]
fn global_options_are_kept_for_later_commands() {
    let invocation = container("docker --context colima -H unix:///x.sock --debug exec -it web sh");
    assert_eq!(
        invocation.global_args,
        ["--context", "colima", "-H", "unix:///x.sock"]
    );
    assert_eq!(invocation.container, named("web"));

    let invocation = container("docker -l debug --config=/cfg exec web sh");
    assert_eq!(invocation.global_args, ["--config=/cfg"]);
}

#[test]
fn container_subcommand_aliases() {
    assert_eq!(
        container("docker container exec -it web sh").container,
        named("web")
    );
    assert_eq!(
        container("docker container run -it alpine").container,
        ContainerRef::Image("alpine".to_owned())
    );
}

#[test]
fn attach_and_start() {
    assert_eq!(container("docker attach web").container, named("web"));
    assert_eq!(
        container("docker attach --detach-keys ctrl-x web").container,
        named("web")
    );
    assert_eq!(container("docker start -ai web").container, named("web"));
    assert_eq!(parse("docker start web"), None);
}

#[test]
fn other_subcommands_are_not_sessions() {
    assert_eq!(parse("docker ps"), None);
    assert_eq!(parse("docker logs -f web"), None);
    assert_eq!(parse("docker compose up"), None);
    assert_eq!(parse("docker compose logs -f"), None);
    assert_eq!(parse("docker"), None);
    assert_eq!(parse("ls -la"), None);
    assert_eq!(parse("claude"), None);
}

#[test]
fn podman_and_nerdctl() {
    let invocation = container("podman --connection remote exec -it web bash");
    assert_eq!(invocation.engine, Engine::Podman);
    assert_eq!(invocation.program, "podman");
    assert_eq!(invocation.global_args, ["--connection", "remote"]);
    assert_eq!(invocation.container, named("web"));

    let invocation = container("nerdctl -n k8s.io run -it alpine");
    assert_eq!(invocation.engine, Engine::Nerdctl);
    assert_eq!(invocation.global_args, ["-n", "k8s.io"]);
    assert_eq!(
        invocation.container,
        ContainerRef::Image("alpine".to_owned())
    );
}

#[test]
fn program_paths_are_kept() {
    let invocation = container("/usr/local/bin/docker exec -it web sh");
    assert_eq!(invocation.program, "/usr/local/bin/docker");
}

#[test]
fn compose_run_targets_a_one_off_container_of_the_service() {
    let invocation =
        container("docker compose -f dev/compose.yml -p MyProj run --rm -e X=1 app zsh");
    assert_eq!(
        invocation.container,
        ContainerRef::ComposeService(ComposeService {
            service: "app".to_owned(),
            project: Some("MyProj".to_owned()),
            project_directory: None,
            files: vec!["dev/compose.yml".to_owned()],
            one_off: true,
        })
    );
    assert_eq!(invocation.inner_command.as_deref(), Some("zsh"));
}

#[test]
fn compose_exec_targets_the_service_container() {
    let invocation = container("docker compose --project-directory ../x exec -u node app claude");
    assert_eq!(
        invocation.container,
        ContainerRef::ComposeService(ComposeService {
            service: "app".to_owned(),
            project: None,
            project_directory: Some("../x".to_owned()),
            files: Vec::new(),
            one_off: false,
        })
    );
}

#[test]
fn compose_run_with_name_targets_the_name() {
    assert_eq!(
        container("docker compose run --name agent app").container,
        named("agent")
    );
}

#[test]
fn compose_front_ends() {
    let invocation = container("docker-compose run app bash");
    assert_eq!(invocation.engine, Engine::Docker);
    assert_eq!(invocation.program, "docker");
    assert!(matches!(
        invocation.container,
        ContainerRef::ComposeService(_)
    ));

    let invocation = container("podman-compose exec app bash");
    assert_eq!(invocation.engine, Engine::Podman);
    assert_eq!(invocation.program, "podman");

    let invocation = container("podman compose run app");
    assert_eq!(invocation.engine, Engine::Podman);

    let invocation = container("docker --context remote compose exec app sh");
    assert_eq!(invocation.global_args, ["--context", "remote"]);
}

#[test]
fn wrappers_and_assignments_are_skipped() {
    assert_eq!(
        container("FOO=1 BAR=2 docker exec -it web sh").container,
        named("web")
    );
    assert_eq!(
        container("sudo -E docker exec -it web sh").container,
        named("web")
    );
    assert_eq!(
        container("sudo -u root docker exec -it web sh").container,
        named("web")
    );
    assert_eq!(
        container("env -i PATH=/bin docker exec -it web sh").container,
        named("web")
    );
    assert_eq!(container("exec docker attach web").container, named("web"));
    assert_eq!(
        container("time nice -n 5 docker attach web").container,
        named("web")
    );
}

#[test]
fn the_last_session_in_a_command_list_counts() {
    assert_eq!(
        container("cd app && docker compose run web zsh").container,
        ContainerRef::ComposeService(ComposeService {
            service: "web".to_owned(),
            project: None,
            project_directory: None,
            files: Vec::new(),
            one_off: true,
        })
    );
    assert_eq!(
        container("docker build -t img .; docker run -it img").container,
        ContainerRef::Image("img".to_owned())
    );
    assert_eq!(
        container("docker exec -it a sh || docker exec -it b sh").container,
        named("b")
    );
}

#[test]
fn aliases_are_resolved() {
    let resolve = |word: &str| match word {
        "dc" => Some("docker compose".to_owned()),
        "dex" => Some("docker exec -it".to_owned()),
        _ => None,
    };
    let Some(Target::Container(invocation)) = parse_command("dc run app", resolve) else {
        panic!("dc alias not resolved");
    };
    assert!(matches!(
        invocation.container,
        ContainerRef::ComposeService(_)
    ));
    let Some(Target::Container(invocation)) = parse_command("dex web zsh", resolve) else {
        panic!("dex alias not resolved");
    };
    assert_eq!(invocation.container, named("web"));
}

#[test]
fn unbalanced_quotes_are_not_parsed() {
    assert_eq!(parse("docker exec -it 'web sh"), None);
}

#[test]
fn ssh_sessions() {
    let invocation = ssh("ssh -p 2222 -i ~/.ssh/id -L 8080:localhost:80 -t dev@box claude");
    assert_eq!(invocation.program, "ssh");
    assert_eq!(invocation.destination, "dev@box");
    assert_eq!(invocation.options, ["-p", "2222", "-i", "~/.ssh/id"]);
    assert_eq!(invocation.inner_command.as_deref(), Some("claude"));

    let invocation = ssh("ssh -o ProxyJump=bastion -J jump -A box");
    assert_eq!(
        invocation.options,
        ["-o", "ProxyJump=bastion", "-J", "jump"]
    );
    assert_eq!(invocation.inner_command, None);

    let invocation = ssh("ssh -p2200 box");
    assert_eq!(invocation.options, ["-p", "2200"]);
}

#[test]
fn non_session_ssh_invocations_are_ignored() {
    assert_eq!(parse("ssh -G box"), None);
    assert_eq!(parse("ssh -N -L 80:x:80 box"), None);
    assert_eq!(parse("ssh -O exit box"), None);
    assert_eq!(parse("ssh"), None);
}

#[test]
fn override_values() {
    let Some(Target::Container(invocation)) = Target::from_override(" agent ") else {
        panic!("override not parsed");
    };
    assert_eq!(invocation.engine, Engine::Docker);
    assert_eq!(invocation.container, named("agent"));

    let Some(Target::Container(invocation)) = Target::from_override("podman:agent") else {
        panic!("podman override not parsed");
    };
    assert_eq!(invocation.engine, Engine::Podman);
    assert_eq!(invocation.program, "podman");

    assert_eq!(Target::from_override(""), None);
    assert_eq!(Target::from_override("two words"), None);
}

#[test]
fn paste_text_depends_on_the_agent() {
    let gemini = parse("docker run -it img /usr/bin/gemini --yolo").unwrap();
    assert_eq!(gemini.inner_program(), Some("gemini"));
    assert_eq!(gemini.paste_text_for_path("/tmp/a.png"), "@/tmp/a.png");

    let claude = parse("docker exec -it web claude").unwrap();
    assert_eq!(claude.paste_text_for_path("/tmp/a.png"), "/tmp/a.png");

    let shell = parse("docker exec -it web").unwrap();
    assert_eq!(shell.inner_program(), None);
    assert_eq!(shell.paste_text_for_path("/tmp/a.png"), "/tmp/a.png");
}
