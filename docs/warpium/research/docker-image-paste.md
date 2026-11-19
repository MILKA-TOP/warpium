# Image paste into CLI agents in containers and over SSH

Roadmap item 5. Pasting a screenshot (`cmd-V`) into Claude Code, Codex or Kimi Code works when the
agent runs on the Mac, and does nothing when it runs inside `docker run -it …`,
`docker compose run … zsh`, `docker exec -it …` or `ssh`. This note records how Warp handles
image paste today, the options for containers, and what Warpium implements.

## How Warp handles image paste today

- **Clipboard.** `ctx.clipboard().read()` returns a `ClipboardContent` with `plain_text`, `paths`,
  `html` and `images: Option<Vec<ImageData>>` (`crates/warpui_core/src/clipboard.rs`). On macOS
  `read_image_data_from_pasteboard` (`crates/warpui/src/platform/mac/clipboard.rs`) returns every
  rendition it finds, in the order PNG, JPEG, GIF, WebP, SVG, TIFF: a screenshot gives a PNG and a
  TIFF of the same image. Finder's **Copy** of a file gives its path in `paths`, its name in
  `plain_text` and its icon as a TIFF.
- **Paste into a terminal** (`TerminalView::paste`, `app/src/terminal/view.rs`). With the input
  editor focused, the content goes into it (Warp's own agent attaches images there). With a
  long-running command, text is written to the PTY, wrapped in bracketed paste when the program
  asked for it; an image-only clipboard has no text, so nothing (or an empty bracketed paste)
  arrives.
- **CLI agents on the host.** `CLIAgentSessionsModel` knows when the active command is a CLI agent:
  `CLIAgent::detect` matches the first word of the command (after aliases and `FOO=1` prefixes)
  against `claude`, `codex`, `gemini`, … plus user-configured regexes, and the agents' Warp
  plugins report sessions over OSC. When such a session is active and the clipboard holds an
  image, Warp writes `^V` (`SYN`) to the PTY (on Windows, `Esc v` for Claude Code). The agent then
  reads the image from the system clipboard itself (`osascript`/`pbpaste` style).
- **Drops.** `drag_and_drop_files` sends dropped image files to a host CLI agent the same way
  (`paste_dropped_images_to_cli_agent` in `use_agent_footer/mod.rs`: each image is written to the
  clipboard and `^V` sent), attaches them to Warp's input at a prompt, uploads them with `scp` in a
  warpified SSH session (`SshDragAndDrop` flag), and otherwise types the shell-escaped host paths.

Inside a container none of this works: `docker run … claude` and `docker compose run app zsh`
don't match `CLIAgent::detect` (the first word is `docker`), and when it does match (a warpified
subshell running `claude`, or a user regex), the agent gets `^V`, looks at the container's
clipboard, which doesn't exist, and reports that there is no image. A typed host path names a file
the container can't see.

## What the agents accept

The common denominator is a **pasted file path**:

- **Claude Code** turns a pasted or typed path to an image file into an image attachment
  (`[Image #1]`), and reads an image path mentioned in the prompt.
- **Codex CLI** attaches an image when a bracketed paste is exactly a path to an image file (its
  composer's paste handler checks whether the pasted text is an image path); `codex -i <file>`
  does the same at start-up. So each path needs its own bracketed paste.
- **Kimi Code** reads clipboard images with `ctrl-V`; a path in the prompt is read by its file
  tools.
- **Gemini CLI** attaches files written as `@path`.

Warpium therefore pastes each path as its own bracketed paste (separated by spaces), and prefixes
`@` when the command started in the container is `gemini`.

## Telling that the session is in a container, and which one

| Signal | Use |
| --- | --- |
| The active long-running command (`docker run`/`exec`/`attach`/`start -a`, `docker compose run`/`exec`, `podman …`, `nerdctl …`, `docker-compose`, `podman-compose`; after aliases, `sudo`, `env`, `FOO=1`; the last command of `a && b`) | Primary. Parsed with the engines' option tables so values (`-e X`, `-v a:b`, `--name n`) aren't mistaken for the image or container. Global options that select the daemon (`--context`, `-H`, `--connection`, …) are kept for later commands. Detached runs (`-d`) are ignored. |
| Warp's subshell info (`SubshellInitializationInfo::spawning_command`) | When Warp tracks a shell inside the container (warpified `docker exec`), the running command is the agent itself; the spawning command is the `docker` command. |
| `docker ps` + `docker inspect` | Resolves `run` without `--name` (newest running container of the image, image references normalized: `node` = `docker.io/library/node:latest`, or an image id prefix) and Compose services (labels `com.docker.compose.service`, `…project`, `…oneoff`, `…project.working_dir`: the project from `-p`, else the one whose working dir is the `--project-directory`/`-f` folder or the shell's directory or a parent of it; one-off containers preferred for `run`, the service's own for `exec`). |
| The session's hostname | Docker's default hostname is the short container id; with a tracked shell inside, it breaks ties between containers of one image. |
| `/.dockerenv`, `/proc/1/cgroup` | Only visible from inside the container, so useless to the host app (Warp's shell integration doesn't report them). |
| User override | `warpium.container_image_paste_container` names the container (`podman:`/`nerdctl:` prefixes) for wrappers that aren't recognized (`make shell`, `./dev.sh`). It applies only while a command other than a host CLI agent runs in a local shell. |

## Getting the file in

| Option | Pros | Cons |
| --- | --- | --- |
| **(a) `docker exec -i <id> sh -c 'mkdir -p "$1" && cat > "$1/$2"'`** with the image on stdin | No temp file; the file belongs to the container's default user; works with `--tmpfs /tmp` and remote daemons; creates the folder. | Needs `sh` and `cat` in the image (every image an agent runs in has them). |
| **(b) `docker cp <tmp>/. <id>:/tmp/warpium-paste`** | Works without a shell in the image. | Files belong to root (made world-readable here); can't write into tmpfs mounts; the staged file's mode carries over. |
| **(c) A bind-mounted host folder** (`docker inspect` → `Mounts`; the mount holding the shell's directory → `<source>/.warpium/paste/`, git-ignored, path mapped to `<destination>/…`) | No container commands at all. | Only for local daemons and when the project is mounted; writes into the project folder. |
| (d) Typing a base64 heredoc into the session | Needs nothing. | Would type into the agent's prompt, not a shell; out of the question. |

**Implemented:** (a), falling back to (b), then (c). The default folder is `/tmp/warpium-paste` in
the container. Every name is `warpium-<original stem or "paste">-<YYYYMMDD-HHMMSS-ms>-<n>.<ext>`,
sanitized to `[A-Za-z0-9._-]` so it needs no quoting.

**Cleanup.** After a paste, `find /tmp/warpium-paste -name 'warpium-*' -mmin +1440 -exec rm -f {} +`
runs in the background in the container (best effort; files must outlive the paste because Kimi
and Codex read them when the prompt is sent). In a bind mount, files older than a day and all but
the newest 50 are removed. The container's `/tmp` usually goes away with the container anyway.

**Formats.** The best rendition is pasted (PNG over TIFF). TIFF, BMP and HEIC, which the agents
don't read, are converted to PNG with macOS's `sips`. Images over 100 MB are refused.

## SSH

Cheap with the same approach: `ssh <user's -p/-i/-J/-F/-o/-l options> -o BatchMode=yes
-o ConnectTimeout=10 -o RemoteCommand=none -o RequestTTY=no -T <dest> "sh -c '…cat > …'"` with the
image on stdin, into `/tmp/warpium-paste` on the host. Forwarding options (`-L`, `-R`, `-D`) are
dropped so the second connection doesn't fight the first over ports. `BatchMode` makes it fail
instead of prompting, so it needs key authentication or a shared `ControlMaster` connection; the
failure toast says so. Detected from the running `ssh` command or the spawning command of a
warpified SSH session.

## Where it hooks in

- `TerminalView::paste`: before the host-agent `^V` path, when the paste goes to the PTY. Taken
  when the clipboard holds an image and no text, or only image file paths (Finder **Copy**).
- `TerminalView::drag_and_drop_files`: before the host-agent drop path, for drops made only of
  image files during a long-running command.
- The work runs on its own thread; when it finishes, the paths are pasted only if the same command
  is still running. Failures show a toast with the reason (CLI not found, no matching container,
  the stderr of each method tried).
- Docker Desktop, OrbStack, Rancher Desktop, Podman and Homebrew CLIs are found in `PATH` and their
  usual install folders, since apps started from the Dock get a minimal `PATH`.

## Future work

- `kubectl exec -it <pod> -- …` (same `exec -i … cat` approach with `kubectl exec -i`), Dev
  Containers (`devcontainer exec`, container found by its `devcontainer.local_folder` label) and
  `lima nerdctl`.
- An agent inside a container on an SSH host (`ssh box` then `docker exec` there).
- The shell's own `DOCKER_HOST`/`DOCKER_CONTEXT`: Warpium's environment is used, so a context
  chosen only by an exported variable in the shell isn't followed (pass `--context` or set
  `docker context use`).
- A per-tab container choice in the UI instead of the global override.
