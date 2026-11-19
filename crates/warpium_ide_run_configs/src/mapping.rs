//! Mapping one `<configuration>` element to what Warpium runs.

use std::collections::BTreeMap;
use std::path::Path;

use roxmltree::Node;
use warpium_project_toolbar::{CommandAction, CommandTarget};

use crate::macros::{Macros, relative_to_root};
use crate::model::{AndroidLaunch, AndroidRun, StepAction};
use crate::module::{GradleModule, gradle_launcher, resolve_module};
use crate::shell::{is_valid_env_name, join_command, quote_arg, split_args};
use crate::xml::{child, children, envs, option, option_flag, option_list, option_map};

/// Where a configuration is read: the project and the user's home directory.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Context<'a> {
    pub(crate) root: &'a Path,
    pub(crate) home: Option<&'a Path>,
}

impl<'a> Context<'a> {
    fn macros(&self, module_dir: Option<&'a Path>) -> Macros<'a> {
        Macros {
            project_dir: self.root,
            module_dir,
            user_home: self.home,
        }
    }
}

/// What a configuration maps to, before compound configurations are resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mapped {
    Step(StepAction),
    /// The members of a compound configuration, as `(type, name)`.
    Compound(Vec<(String, String)>),
    Unsupported(String),
}

/// Maps the `<configuration>` element `node` of type `type_id`.
pub(crate) fn map_configuration(node: Node<'_, '_>, type_id: &str, ctx: Context<'_>) -> Mapped {
    let result = match type_id {
        "GradleRunConfiguration" => gradle(node, ctx),
        "ShConfigurationType" => shell_script(node, ctx),
        "AndroidRunConfigurationType" => android_app(node, ctx),
        "AndroidTestRunConfigurationType" => android_instrumented_tests(node, ctx),
        "JUnit" => junit(node, ctx, false),
        "AndroidJUnit" => junit(node, ctx, true),
        "CompoundRunConfigurationType" => {
            return Mapped::Compound(
                children(node, "toRun")
                    .filter_map(|member| {
                        Some((
                            member.attribute("type")?.to_owned(),
                            member.attribute("name")?.to_owned(),
                        ))
                    })
                    .collect(),
            );
        }
        "JetRunConfigurationType" | "Application" => Err(
            "Warpium doesn't run Kotlin or Java main classes yet; use a Gradle run configuration \
             (such as the application plugin's run task)"
                .to_owned(),
        ),
        "KotlinStandaloneScriptRunConfigurationType" => {
            Err("Warpium doesn't run Kotlin scripts yet".to_owned())
        }
        other => Err(format!(
            "Warpium can't run {} run configurations",
            crate::model::type_label(other)
        )),
    };
    match result {
        Ok(step) => Mapped::Step(step),
        Err(reason) => Mapped::Unsupported(reason),
    }
}

fn gradle(node: Node<'_, '_>, ctx: Context<'_>) -> Result<StepAction, String> {
    let settings = child(node, "ExternalSystemSettings")
        .ok_or_else(|| "the configuration has no Gradle settings".to_owned())?;
    let macros = ctx.macros(None);
    let project_dir = option(settings, "externalProjectPath")
        .map(|path| macros.path(path))
        .unwrap_or_else(|| ctx.root.to_path_buf());
    let launcher = gradle_launcher(ctx.root, &project_dir);
    let tasks = option_list(settings, "taskNames")
        .iter()
        .map(|task| macros.expand(task))
        .collect::<Vec<_>>();
    let parameters = option(settings, "scriptParameters")
        .map(|parameters| macros.expand(parameters))
        .unwrap_or_default();
    // The IDE passes VM options to the Gradle JVM; system properties are the part that reaches
    // the build the same way from the command line.
    let properties = option(settings, "vmOptions")
        .map(|options| {
            split_args(&macros.expand(options))
                .into_iter()
                .filter(|option| option.starts_with("-D"))
                .map(|option| quote_arg(&option))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let project_arg = launcher
        .project_arg
        .as_deref()
        .map(|dir| format!("-p {}", quote_arg(dir)));
    let command = join_command(
        std::iter::once(launcher.program)
            .chain(project_arg.as_deref())
            .chain(tasks.iter().map(String::as_str))
            .chain(std::iter::once(parameters.as_str()))
            .chain(properties.iter().map(String::as_str)),
    );
    let env = expand_env(option_map(settings, "env"), &macros)?;
    Ok(command_step(ctx.root, command, &launcher.cwd, env))
}

fn shell_script(node: Node<'_, '_>, ctx: Context<'_>) -> Result<StepAction, String> {
    let macros = ctx.macros(None);
    let script_path = option(node, "SCRIPT_PATH").map(|path| macros.path(path));
    let (command, default_cwd) = if option_flag(node, "EXECUTE_SCRIPT_FILE", true) {
        let script_path =
            script_path.ok_or_else(|| "the configuration has no script file".to_owned())?;
        let interpreter = option(node, "INTERPRETER_PATH")
            .map(|path| quote_arg(&macros.expand(path)))
            .unwrap_or_default();
        let interpreter_options = option(node, "INTERPRETER_OPTIONS")
            .map(|options| macros.expand(options))
            .unwrap_or_default();
        let script_options = option(node, "SCRIPT_OPTIONS")
            .map(|options| macros.expand(options))
            .unwrap_or_default();
        let script = quote_arg(&script_path.to_string_lossy());
        let command = join_command([
            interpreter.as_str(),
            interpreter_options.as_str(),
            script.as_str(),
            script_options.as_str(),
        ]);
        let script_dir = script_path
            .parent()
            .map_or_else(|| ctx.root.to_path_buf(), Path::to_path_buf);
        (command, script_dir)
    } else {
        let text = option(node, "SCRIPT_TEXT")
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| "the configuration has no script text".to_owned())?;
        (text.to_owned(), ctx.root.to_path_buf())
    };
    let cwd = option(node, "SCRIPT_WORKING_DIRECTORY")
        .map(|dir| macros.path(dir))
        .unwrap_or(default_cwd);
    let env = expand_env(envs(node), &macros)?;
    Ok(command_step(ctx.root, command, &cwd, env))
}

fn android_app(node: Node<'_, '_>, ctx: Context<'_>) -> Result<StepAction, String> {
    if !option_flag(node, "DEPLOY", true) {
        return Err("Warpium only runs Android configurations that deploy the app".to_owned());
    }
    if option_flag(node, "DEPLOY_APK_FROM_BUNDLE", false) {
        return Err("Warpium doesn't deploy APKs from app bundles yet".to_owned());
    }
    let module = module(node, ctx)?;
    let launch = match option(node, "MODE").unwrap_or("default_activity") {
        "default_activity" => AndroidLaunch::DefaultActivity,
        "specific_activity" => AndroidLaunch::Activity(
            option(node, "ACTIVITY_CLASS")
                .ok_or_else(|| "the configuration names no activity to launch".to_owned())?
                .to_owned(),
        ),
        "deep_link" => AndroidLaunch::DeepLink(
            option(node, "DEEP_LINK")
                .ok_or_else(|| "the configuration has no deep link to open".to_owned())?
                .to_owned(),
        ),
        "do_nothing" => AndroidLaunch::Nothing,
        other => return Err(format!("unknown launch option `{other}`")),
    };
    Ok(StepAction::AndroidApp(AndroidRun {
        gradle_path: module.gradle_path,
        launch,
    }))
}

fn android_instrumented_tests(node: Node<'_, '_>, ctx: Context<'_>) -> Result<StepAction, String> {
    let module = module(node, ctx)?;
    let class = option(node, "CLASS_NAME");
    let filter = match option(node, "TESTING_TYPE").unwrap_or("0") {
        "0" => None,
        "1" => Some(format!(
            "-Pandroid.testInstrumentationRunnerArguments.package={}",
            option(node, "PACKAGE_NAME").ok_or("the configuration has no package")?
        )),
        "2" => Some(format!(
            "-Pandroid.testInstrumentationRunnerArguments.class={}",
            class.ok_or("the configuration has no test class")?
        )),
        "3" => Some(format!(
            "-Pandroid.testInstrumentationRunnerArguments.class={}#{}",
            class.ok_or("the configuration has no test class")?,
            option(node, "METHOD_NAME").ok_or("the configuration has no test method")?
        )),
        other => return Err(format!("unknown test scope `{other}`")),
    };
    let task = module.task("connectedDebugAndroidTest");
    gradle_module_command(ctx, &task, filter.map(|filter| quote_arg(&filter)))
}

fn junit(node: Node<'_, '_>, ctx: Context<'_>, android: bool) -> Result<StepAction, String> {
    let module = module(node, ctx)?;
    let task = test_task(&module, android)?;
    let class = option(node, "MAIN_CLASS_NAME");
    let filter = match option(node, "TEST_OBJECT").unwrap_or("class") {
        "class" => class
            .ok_or_else(|| "the configuration has no test class".to_owned())?
            .to_owned(),
        "method" => format!(
            "{}.{}",
            class.ok_or("the configuration has no test class")?,
            option(node, "METHOD_NAME").ok_or("the configuration has no test method")?
        ),
        "package" => match option(node, "PACKAGE_NAME") {
            Some(package) => format!("{package}.*"),
            None => return gradle_module_command(ctx, &task, None),
        },
        other => {
            return Err(format!(
                "Warpium only runs JUnit configurations for a class, method or package, not \
                 `{other}`"
            ));
        }
    };
    gradle_module_command(ctx, &task, Some(format!("--tests {}", quote_arg(&filter))))
}

/// The Gradle test task of the source set the IDE module stands for.
fn test_task(module: &GradleModule, android: bool) -> Result<String, String> {
    let name = match module.source_set.as_deref() {
        None | Some("test") if android => "testDebugUnitTest".to_owned(),
        None | Some("test") => "test".to_owned(),
        Some("unitTest" | "androidUnitTest") => "testDebugUnitTest".to_owned(),
        Some(source_set)
            if source_set.ends_with("Test")
                && !matches!(source_set, "commonTest" | "androidTest" | "iosTest") =>
        {
            source_set.to_owned()
        }
        Some(source_set) => {
            return Err(format!(
                "Warpium doesn't know the Gradle test task of the `{source_set}` source set"
            ));
        }
    };
    Ok(module.task(&name))
}

/// `<gradle> <task> [<extra>]`, run from the wrapper's directory.
fn gradle_module_command(
    ctx: Context<'_>,
    task: &str,
    extra: Option<String>,
) -> Result<StepAction, String> {
    let launcher = gradle_launcher(ctx.root, ctx.root);
    let command = join_command([launcher.program, task, extra.as_deref().unwrap_or_default()]);
    Ok(command_step(
        ctx.root,
        command,
        &launcher.cwd,
        BTreeMap::new(),
    ))
}

fn module(node: Node<'_, '_>, ctx: Context<'_>) -> Result<GradleModule, String> {
    let name = child(node, "module")
        .and_then(|module| module.attribute("name"))
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "the configuration has no module".to_owned())?;
    resolve_module(ctx.root, name)
        .ok_or_else(|| format!("no Gradle project found for the IDE module `{name}`"))
}

fn expand_env(
    env: BTreeMap<String, String>,
    macros: &Macros<'_>,
) -> Result<BTreeMap<String, String>, String> {
    env.into_iter()
        .map(|(name, value)| {
            if is_valid_env_name(&name) {
                Ok((name, macros.expand(&value)))
            } else {
                Err(format!("invalid environment variable name `{name}`"))
            }
        })
        .collect()
}

fn command_step(
    root: &Path,
    command: String,
    cwd: &Path,
    env: BTreeMap<String, String>,
) -> StepAction {
    StepAction::Command(CommandAction {
        command,
        target: CommandTarget::Current,
        cwd: relative_to_root(root, cwd),
        env,
    })
}

#[cfg(test)]
#[path = "mapping_tests.rs"]
mod tests;
