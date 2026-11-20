use tempfile::TempDir;

use super::*;
use crate::lex::position_of;
use crate::model::Language;

const MODEL: &str = "core/src/main/kotlin/com/example/model/User.kt";
const REPO: &str = "core/src/main/kotlin/com/example/data/UserRepository.kt";
const UTIL: &str = "core/src/main/kotlin/com/example/util/Strings.kt";
const SCREEN: &str = "app/src/main/kotlin/com/example/app/UserScreen.kt";
const JAVA: &str = "app/src/main/java/com/example/app/Legacy.java";

fn project() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    let files = [
        (
            MODEL,
            "package com.example.model\n\
             \n\
             open class Entity {\n    val id: Long = 0\n    fun touch() {}\n}\n\
             \n\
             data class User(val name: String, val email: String) : Entity() {\n\
                 fun displayName(): String = name\n\
                 private fun secret() {}\n\
                 companion object {\n        fun guest(): User = User(\"\", \"\")\n    }\n\
             }\n\
             \n\
             enum class Role { ADMIN, GUEST }\n",
        ),
        (
            REPO,
            "package com.example.data\n\
             \n\
             import com.example.model.User\n\
             \n\
             interface UserRepository {\n    fun loadUser(id: Long): User\n    fun saveUser(user: User)\n}\n",
        ),
        (
            UTIL,
            "package com.example.util\n\
             \n\
             import com.example.model.User\n\
             \n\
             fun User.initials(): String = name.take(1)\n\
             fun formatUserName(value: String): String = value\n",
        ),
        (
            SCREEN,
            "package com.example.app\n\
             \n\
             import com.example.data.UserRepository\n\
             import com.example.model.User\n\
             \n\
             class UserScreen(private val repository: UserRepository) {\n\
                 private val title = \"Users\"\n\
             \n\
                 fun render(userId: Long) {\n\
                     val user: User = repository.loadUser(userId)\n\
                     // CURSOR\n\
                 }\n\
             }\n",
        ),
        (
            JAVA,
            "package com.example.app;\n\
             \n\
             import com.example.model.User;\n\
             \n\
             public class Legacy {\n\
                 public static int counter = 0;\n\
                 public void run(User user) {\n\
                     // CURSOR\n\
                 }\n\
             }\n",
        ),
    ];
    for (rel_path, text) in files {
        let path = dir.path().join(rel_path);
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("create dirs");
        std::fs::write(path, text).expect("write fixture");
    }
    dir
}

/// Completes after typing `typed` in place of the `// CURSOR` line of `rel_path`.
fn complete(index: &CodeIndex, rel_path: &str, typed: &str) -> Option<CompletionResult> {
    let original = std::fs::read_to_string(index.root().join(rel_path)).expect("fixture exists");
    let text = original.replace("// CURSOR", typed);
    let cursor = original.find("// CURSOR").expect("cursor marker") + typed.len();
    let language = Language::from_path(std::path::Path::new(rel_path)).expect("a source file");
    let file = SourceFile {
        rel_path,
        text: &text,
        language,
    };
    index.complete(&file, position_of(&text, &line_starts(&text), cursor), 50)
}

fn names(result: &CompletionResult) -> Vec<&str> {
    result.items.iter().map(|item| item.name.as_str()).collect()
}

fn item<'r>(result: &'r CompletionResult, name: &str) -> &'r CompletionItem {
    result
        .items
        .iter()
        .find(|item| item.name == name)
        .unwrap_or_else(|| panic!("{name} in {:?}", names(result)))
}

#[test]
fn lists_members_of_a_typed_local_with_inherited_ones_and_extensions() {
    let dir = project();
    let index = CodeIndex::build(dir.path(), 1);

    let result = complete(&index, SCREEN, "user.").expect("completes");

    assert!(result.member_access);
    assert!(!result.approximate);
    let found = names(&result);
    for expected in ["name", "email", "displayName", "id", "touch", "initials"] {
        assert!(found.contains(&expected), "{expected} in {found:?}");
    }
    assert!(!found.contains(&"secret"), "private members are hidden");
    assert!(
        !found.contains(&"guest"),
        "companion members need the class"
    );
    assert_eq!(
        item(&result, "initials").import.as_deref(),
        Some("com.example.util.initials")
    );
    assert_eq!(item(&result, "name").source, CompletionSource::Member);
}

#[test]
fn filters_members_by_prefix_and_camel_humps() {
    let dir = project();
    let index = CodeIndex::build(dir.path(), 1);

    let prefix = complete(&index, SCREEN, "user.dis").expect("completes");
    assert_eq!(names(&prefix), ["displayName"]);
    assert_eq!(prefix.prefix, "dis");

    let camel = complete(&index, SCREEN, "repository.sU").expect("completes");
    assert_eq!(names(&camel), ["saveUser"]);
}

#[test]
fn static_access_lists_companion_members_and_enum_entries() {
    let dir = project();
    let index = CodeIndex::build(dir.path(), 1);

    let user = complete(&index, SCREEN, "User.").expect("completes");
    assert!(names(&user).contains(&"guest"), "{:?}", names(&user));
    assert!(!names(&user).contains(&"name"));
}

#[test]
fn plain_names_rank_locals_and_scope_before_imports_and_unimported() {
    let dir = project();
    let index = CodeIndex::build(dir.path(), 1);

    let result = complete(&index, SCREEN, "u").expect("completes");

    assert_eq!(item(&result, "user").source, CompletionSource::Local);
    assert_eq!(item(&result, "userId").source, CompletionSource::Local);
    assert_eq!(item(&result, "User").source, CompletionSource::Import);
    let found = names(&result);
    let position = |name: &str| found.iter().position(|n| *n == name).expect("listed");
    assert!(position("user") < position("User"));

    let format = complete(&index, SCREEN, "formatU").expect("completes");
    let formatter = item(&format, "formatUserName");
    assert_eq!(formatter.source, CompletionSource::NotImported);
    assert_eq!(
        formatter.import.as_deref(),
        Some("com.example.util.formatUserName")
    );

    let scope = complete(&index, SCREEN, "tit").expect("completes");
    assert_eq!(item(&scope, "title").source, CompletionSource::Scope);
}

#[test]
fn completes_members_in_java() {
    let dir = project();
    let index = CodeIndex::build(dir.path(), 1);

    let result = complete(&index, JAVA, "user.dis").expect("completes");
    assert_eq!(names(&result), ["displayName"]);

    let local = complete(&index, JAVA, "cou").expect("completes");
    assert_eq!(item(&local, "counter").source, CompletionSource::Scope);
}

#[test]
fn offers_nothing_in_comments_or_new_declarations() {
    let dir = project();
    let index = CodeIndex::build(dir.path(), 1);

    assert!(complete(&index, SCREEN, "val us").is_none());
    assert!(complete(&index, SCREEN, "// us").is_none());
}
