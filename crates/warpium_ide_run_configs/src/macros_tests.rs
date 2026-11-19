use std::path::{Path, PathBuf};

use super::*;

fn macros(module_dir: Option<&Path>) -> Macros<'_> {
    Macros {
        project_dir: Path::new("/work/app"),
        module_dir,
        user_home: Some(Path::new("/home/dev")),
    }
}

#[test]
fn known_macros_are_expanded_and_unknown_ones_kept() {
    let macros = macros(Some(Path::new("/work/app/core")));
    assert_eq!(
        macros.expand("$PROJECT_DIR$/a $MODULE_DIR$ $USER_HOME$/.m2 $ProjectFileDir$ $Other$"),
        "/work/app/a /work/app/core /home/dev/.m2 /work/app $Other$"
    );
}

#[test]
fn module_dir_without_a_module_is_the_project_dir() {
    assert_eq!(macros(None).expand("$MODULE_DIR$/x"), "/work/app/x");
}

#[test]
fn paths_are_resolved_from_the_project_dir() {
    let macros = macros(None);
    assert_eq!(
        macros.path("$PROJECT_DIR$/app/../lib/./x"),
        PathBuf::from("/work/app/lib/x")
    );
    assert_eq!(macros.path("scripts"), PathBuf::from("/work/app/scripts"));
    assert_eq!(macros.path("/opt/tool"), PathBuf::from("/opt/tool"));
}

#[test]
fn relative_to_root_uses_parent_components_outside_the_root() {
    let root = Path::new("/work/app");
    assert_eq!(relative_to_root(root, Path::new("/work/app")), None);
    assert_eq!(
        relative_to_root(root, Path::new("/work/app/core/data")),
        Some("core/data".to_owned())
    );
    assert_eq!(
        relative_to_root(root, Path::new("/work/other")),
        Some("../other".to_owned())
    );
    assert_eq!(
        relative_to_root(root, Path::new("/home/dev")),
        Some("../../home/dev".to_owned())
    );
}
