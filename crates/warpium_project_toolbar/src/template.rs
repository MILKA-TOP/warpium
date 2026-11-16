//! Commented templates written when the user creates a toolbar config from the command palette.

/// The template for a new `.warpium/toolbar.toml`. Every example is commented out, so the file
/// starts with no buttons.
pub const TEAM_TEMPLATE: &str = r#"# Warpium project toolbar. Commit this file to share the buttons with your team.
# Buttons appear in the top-right corner of the window while a session is in this project.
# Commands from this file only run after you trust this version of it; Warpium asks again
# whenever it changes. Personal changes belong in "Edit personal toolbar overrides".
#
# Every button has an `id` and exactly one action: `command`, `workflow`, `launch_config`,
# `tab_config`, `url` or `action`. Optional: `label`, `icon`, `tooltip`, `run_config = true` to
# list it in the run-configurations dropdown (next to the run button) instead, and
# `overflow = true` to keep it in the overflow menu.
# Commands can use ${projectRoot}, ${file}, ${fileDir}, ${selection}, ${cwd} and
# ${prompt:Label} (asks for a value); Warpium quotes each one, so don't put quotes around them.
# You can also edit the toolbar with "Customize toolbar…" in its menu.
# Icons: build, play, run, stop, bug, debug, test, check, deploy, rocket, terminal, logs, device,
# phone, install, download, upload, refresh, sync, clean, trash, gear, settings, globe, web, link,
# folder, file, code, git, github, package, docs, book, search, eye, bolt, cloud, history, key,
# laptop, layout, bell, tool, workflow, or an .svg path relative to this directory.

# [[button]]
# id = "assemble"
# label = "Assemble debug"
# icon = "build"
# command = "./gradlew assembleDebug"
# run_config = true

# [[button]]
# id = "test"
# icon = "test"
# tooltip = "Run unit tests"
# command = "./gradlew test"
# target = "new_tab"            # current (default) | new_tab | split_right | split_down
# cwd = "."                     # relative to the project root (the default)
# env = { GRADLE_OPTS = "-Xmx4g" }

# [[button]]
# id = "deploy"
# icon = "deploy"
# workflow = "Deploy to staging"   # a saved workflow, by name

# [[button]]
# id = "dev-layout"
# icon = "layout"
# launch_config = "Android dev"     # or tab_config = "android-dev"

# [[button]]
# id = "ci"
# icon = "globe"
# url = "https://github.com/your-org/your-repo/actions"

# [[button]]
# id = "explorer"
# icon = "folder"
# action = "surface.project_explorer.open"
"#;

/// The template for a new personal overrides file.
pub const PERSONAL_TEMPLATE: &str = r#"# Personal Warpium toolbar overrides for this project. Not shared and always trusted.
# Entries are merged into the project's .warpium/toolbar.toml by button `id`:
# - same id: override the fields you set (an action key replaces the whole action);
# - same id with `hidden = true`: remove that button;
# - new id: add a button after the project's.

# [[button]]
# id = "assemble"
# target = "split_right"

# [[button]]
# id = "ci"
# hidden = true

# [[button]]
# id = "my-emulator"
# label = "Emulator"
# icon = "device"
# command = "emulator -avd Pixel_8_API_35"
# target = "new_tab"
"#;
