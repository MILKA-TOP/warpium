use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum PaletteMode {
    Command,
    Navigation,
    LaunchConfig,
    WarpDrive,
    Files,
    Conversations,
    /// Warpium: Go to Class, over the project's code index.
    CodeClasses,
    /// Warpium: Go to Symbol, over the project's code index.
    CodeSymbols,
    /// Warpium: the results of Go to Definition or Find Usages.
    CodeNavigation,
}
