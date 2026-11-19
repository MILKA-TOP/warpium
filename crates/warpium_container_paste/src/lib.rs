//! Warpium: pasting images into CLI agents that run inside a container or on an SSH host.
//!
//! Agents read a pasted image from the system clipboard, which they can't reach from a container
//! or a remote host. Warpium instead copies the image there with the host's own CLI (`docker`,
//! `podman`, `nerdctl` or `ssh`) and pastes its path, which the agents accept as an image.
//! See `docs/warpium/research/docker-image-paste.md`.

mod deliver;
mod inspect;
mod invocation;
mod naming;

pub use deliver::{
    ImageSource, Method, OwnedHints, PasteError, PasteImage, PasteRequest, Pasted, Tools, paste,
    remove_old_files,
};
pub use inspect::{
    ContainerInfo, Mount, SelectionHints, container_path, mount_for_cwd, normalize_image,
    parse_inspect, select_container,
};
pub use invocation::{
    ComposeService, ContainerInvocation, ContainerRef, Engine, SshInvocation, Target, parse_command,
};
pub use naming::{
    DEFAULT_REMOTE_DIR, FILE_PREFIX, MAX_IMAGE_BYTES, choose_clipboard_image, file_name,
    files_to_remove, is_valid_remote_dir, sanitize,
};
