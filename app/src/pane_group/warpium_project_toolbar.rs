//! Warpium: split panes for project toolbar commands.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

use warpui::{ViewContext, ViewHandle};

use super::{Direction, Event, PaneGroup};
use crate::terminal::TerminalView;
use crate::terminal::shared_session::IsSharedSessionCreator;

impl PaneGroup {
    /// Splits the focused pane with a new terminal session started in `cwd` with `env` added to
    /// its environment, focuses it and returns its view.
    pub(crate) fn add_terminal_pane_for_toolbar_command(
        &mut self,
        direction: Direction,
        cwd: PathBuf,
        env: HashMap<OsString, OsString>,
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<TerminalView> {
        let (pane_data, view) = self.create_terminal_pane_data(
            Some(cwd),
            env,
            IsSharedSessionCreator::No,
            None, /* chosen_shell */
            None, /* conversation_restoration */
            ctx,
        );
        let base_pane_id = self.focused_pane_id(ctx);
        let _ = self.add_pane(
            direction,
            Some(base_pane_id),
            Box::new(pane_data),
            true,
            ctx,
        );
        ctx.emit(Event::AppStateChanged);
        view
    }
}
