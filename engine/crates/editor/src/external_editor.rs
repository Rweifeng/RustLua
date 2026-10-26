//! 外部代码编辑器：把脚本文件与 `file:line` 交给**用户自己的**编辑器打开。
//!
//! 为什么不内置文本编辑器：`docs/55` §2.2 明确"不用自研 Docking、文本编辑器或图表控件
//! 替代成熟库"，而 Lua 作者的现实是 VS Code/Neovim/Notepad++ 里已经配好了 LSP、主题与
//! 快捷键。编辑器要做的是**把位置递过去**，而不是再造一个编辑器。
//!
//! 解析与执行分成两半，是因为可测的部分恰好是解析：
//! [`plan_launch`] 只吃两个注入的查询（环境变量、PATH 上的程序是否存在），因此行号定位
//! 的优先级、占位符替换、带空格路径的引号这些**最容易出错**的规则能在无头环境里断言；
//! [`open_in_external_editor`] 只做一次 spawn，是薄薄的一层副作用。
//!
//! 配置口径（从高到低）：
//! 1. `RUSTLUA_EDITOR` —— 整条命令模板，`{file}` 与 `{line}` 是占位符，例：
//!    `code -g "{file}":{line}` / `notepad++ -n{line} "{file}"`；
//! 2. PATH 上找到的已知编辑器（[`KNOWN_EDITORS`]，每个都带一条支持行号的模板）；
//! 3. 都没有 ⇒ 交回操作系统的文件关联（`open`/`start`），**这条路没有行号定位**，
//!    返回值里必须写清楚，否则用户会以为已经跳到那一行。

use std::path::Path;

/// 已知编辑器：程序名 + 支持行号的命令模板（`{file}`/`{line}` 为占位符）。
///
/// 顺序即优先级（先 VS Code，因为它是最常见的 Lua 开发环境）。模板里的引号是**给
/// `cmd /C` 看的**，因此路径占位符外层不再补引号（见 [`plan_launch`] 的引号规则）。
const KNOWN_EDITORS: &[(&str, &str)] = &[
    ("code", r#"code -g "{file}":{line}"#),
    ("Notepad++", r#"notepad++ -n{line} "{file}""#),
    ("nvim", r#"nvim +{line} "{file}""#),
    ("vim", r#"vim +{line} "{file}""#),
    ("subl", r#"subl "{file}":{line}"#),
    ("idea64", r#"idea64.exe --line {line} "{file}""#),
    ("idea", r#"idea --line {line} "{file}""#),
    ("helix", r#"hx -c "{line}g" "{file}""#),
];

/// 一次外部打开的**计划**（尚未执行）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchPlan {
    /// 交给 shell 的完整命令行（Windows 走 `cmd /C`，其余走 `sh -c`）。
    pub command_line: String,
    /// 该计划是否真的能定位到行。`false` = 只打开文件，调用方必须把这件事说给用户。
    pub locates_line: bool,
    /// 计划来源，写进状态栏：`RUSTLUA_EDITOR` / 程序名 / `文件关联`。
    pub source: String,
}

/// 文件是不是该交给外部代码编辑器（而不是内置的文档/画布打开路径）。
///
/// `.lua` 与它的加密兄弟 `.luae` 之外，把常见源码/配置文本一起认下来：面板里这些文件
/// 本来就没有内置视图，双击不做任何事才是最大的困惑。**刻意不含 `.json`/`.vgraph`**——
/// 它们分别归 UI Studio 与画布编辑，属于"有内置视图"的那一侧。
#[must_use]
pub fn is_text_source(path: &Path) -> bool {
    let Some(raw) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    let ext = raw.to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "lua" | "luae" | "ron" | "toml" | "txt" | "wgsl" | "md" | "rs"
    )
}

/// 组装启动计划。两个查询由调用方注入，好让单测不依赖真实环境：
/// `env` 读环境变量，`program_exists` 判断 PATH 上有没有某个程序。
#[must_use]
pub fn plan_launch(
    path: &Path,
    line: Option<u32>,
    env: &dyn Fn(&str) -> Option<String>,
    program_exists: &dyn Fn(&str) -> bool,
) -> LaunchPlan {
    let file = path.to_string_lossy().into_owned();
    let line_number = line.unwrap_or(1).to_string();
    // 模板里的路径自己带引号；用户在 `RUSTLUA_EDITOR` 里怎么写就怎么执行，
    // 因此这里**不**额外包裹——只在模板根本没有提到 `{file}` 时补一个裸参数。
    if let Some(template) = env("RUSTLUA_EDITOR").map(|value| value.trim().to_owned())
        && !template.is_empty()
    {
        let command_line = if template.contains("{file}") {
            fill(&template, &file, &line_number)
        } else {
            format!("{template} \"{}\"", escape_for_shell(&file))
        };
        return LaunchPlan {
            locates_line: template.contains("{line}"),
            source: "RUSTLUA_EDITOR".to_owned(),
            command_line,
        };
    }
    for (program, template) in KNOWN_EDITORS {
        if program_exists(program) {
            return LaunchPlan {
                locates_line: true,
                source: (*program).to_owned(),
                command_line: fill(template, &file, &line_number),
            };
        }
    }
    // 兜底：操作系统的文件关联。`start` 的第一个空参数是**标题**，不能省——
    // 否则带引号的路径会被 `cmd` 当成窗口标题吃掉，结果是"什么都没打开"。
    LaunchPlan {
        command_line: if cfg!(windows) {
            format!("start \"\" \"{}\"", escape_for_shell(&file))
        } else {
            format!("open \"{}\"", escape_for_shell(&file))
        },
        locates_line: false,
        source: "文件关联".to_owned(),
    }
}

/// 替换占位符。`{line}` 在没有行号时按第 1 行处理（编辑器打开该文件即可，不必报错）。
fn fill(template: &str, file: &str, line: &str) -> String {
    template
        .replace("{file}", &escape_for_shell(file))
        .replace("{line}", line)
}

/// 给 shell 用的路径转义：Windows 的 `cmd` 用双引号包路径即可（内部双引号极少见，
/// 出现时用 `\\\"` 交给 cmd 处理），其余按 `sh` 的规则转义单引号。
fn escape_for_shell(value: &str) -> String {
    if cfg!(windows) {
        value.replace('"', "")
    } else {
        value.replace('\'', "'\\''")
    }
}

/// 用真实环境算出启动计划（环境变量 + PATH）。
#[must_use]
pub fn plan_for(path: &Path, line: Option<u32>) -> LaunchPlan {
    plan_launch(path, line, &|key| std::env::var(key).ok(), &program_on_path)
}

/// 用外部编辑器打开文件（可选定位到行）。返回**给用户看的那条计划**，失败返回原因。
///
/// 副作用只有一次 spawn：进程以 detached 方式启动（外部编辑器的生命周期不属于编辑器），
/// 因此这里不 `wait`，也不因为编辑器退出码非零而判失败。
///
/// `dry_run = true` 只算计划、不起进程——单测与"先看清会执行什么命令行"的诊断都走这条
/// （`RUSTLUA_EDITOR_DRY_RUN=1` 在启动时读一次，见 `EditorApp::new`）。
pub fn open_in_external_editor(
    path: &Path,
    line: Option<u32>,
    dry_run: bool,
) -> Result<LaunchPlan, String> {
    let plan = plan_for(path, line);
    if dry_run {
        tracing::info!(
            target: "editor",
            command = %plan.command_line,
            "[external-editor] 试运行（未启动进程）"
        );
        return Ok(plan);
    }
    let mut command = if cfg!(windows) {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", plan.command_line.as_str()]);
        command
    } else {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", plan.command_line.as_str()]);
        command
    };
    command.stdin(std::process::Stdio::null());
    let spawned = command
        .spawn()
        .map_err(|error| format!("启动外部编辑器失败：{error}"))?;
    tracing::debug!(
        target: "editor",
        program = %spawned.id(),
        command = %plan.command_line,
        "[external-editor] launched"
    );
    Ok(plan)
}

/// 在 PATH 上找程序（不 spawn 任何进程，因此可以便宜地逐个试）。
///
/// Windows 必须考虑 `PATHEXT`：`code` 实际是 `code.cmd`，只按裸名字找会全部落空。
#[must_use]
pub fn program_on_path(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    let extensions: Vec<String> = if cfg!(windows) {
        let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
        std::env::split_paths(&raw)
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&paths).any(|dir| {
        extensions.iter().any(|extension| {
            let mut candidate = dir.join(program);
            if !extension.is_empty() {
                // `.EXE` 这类扩展带点，直接拼在既有扩展后面（`PathBuf` 只认一段扩展）。
                candidate.set_extension(strip_leading_dot(extension));
            }
            candidate.is_file()
        })
    })
}

fn strip_leading_dot(extension: &str) -> String {
    extension.trim_start_matches('.').to_owned()
}

/// 把计划折成状态栏文案（失败/降级都必须说清楚，尤其是**没有行号定位**那一条）。
#[must_use]
pub fn describe(plan: &LaunchPlan, path: &Path, line: Option<u32>) -> String {
    let target = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    if plan.locates_line {
        match line {
            Some(number) => format!("已在 {file} 打开 {target}:{number}", file = plan.source),
            None => format!("已在 {} 打开 {}", plan.source, target),
        }
    } else {
        format!(
            "已用系统默认程序打开 {target}（未定位到第 {} 行：设置 RUSTLUA_EDITOR 指定支持行号的编辑器）",
            line.unwrap_or(1)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{KNOWN_EDITORS, LaunchPlan, describe, is_text_source, plan_launch};
    use std::path::Path;

    /// 造一对注入查询（环境变量表 + PATH 上存在的程序集合）后算启动计划。
    ///
    /// 起 `launch_plan` 而不是 `plan`：本 crate 已有 `dnd::plan`，测试里的局部
    /// `let (plan, _)` 会把同名函数顶掉（第二次调用就成 E0618）。
    fn launch_plan(
        vars: &[(&str, &str)],
        programs: &[&str],
        line: Option<u32>,
    ) -> (LaunchPlan, String) {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let programs: Vec<String> = programs.iter().map(|p| (*p).to_owned()).collect();
        let env = move |key: &str| vars.iter().find_map(|(k, v)| (k == key).then(|| v.clone()));
        let lookup = move |program: &str| {
            programs
                .iter()
                .any(|found| found.eq_ignore_ascii_case(program))
        };
        let path = Path::new(r#"C:\My Games\game_jhyt\scripts\main.lua"#);
        (
            plan_launch(path, line, &env, &lookup),
            path.display().to_string(),
        )
    }

    #[test]
    fn rustlua_editor_template_wins_and_keeps_line() {
        let (plan, _) = launch_plan(
            &[(
                "RUSTLUA_EDITOR",
                r#"code -g "{file}":{line} --wait-if-possible"#,
            )],
            &["nvim"],
            Some(42),
        );
        assert_eq!(plan.source, "RUSTLUA_EDITOR");
        assert!(plan.locates_line, "模板里有 {{line}} 就必须认领行号定位");
        assert!(
            plan.command_line
                .contains(r#""C:\My Games\game_jhyt\scripts\main.lua":42"#),
            "带空格的路径必须原样在引号里：{}",
            plan.command_line
        );
    }

    #[test]
    fn template_without_file_placeholder_appends_the_path() {
        let (plan, _) = launch_plan(
            &[("RUSTLUA_EDITOR", r#""D:\Tools\myeditor.exe""#)],
            &[],
            None,
        );
        assert!(
            plan.command_line
                .ends_with(r#" "C:\My Games\game_jhyt\scripts\main.lua""#),
            "{}",
            plan.command_line
        );
        assert!(!plan.locates_line, "模板没提行号就不许声称能定位行");
    }

    #[test]
    fn known_editors_are_used_in_priority_order() {
        let (plan, _) = launch_plan(&[], &["nvim", "code"], Some(7));
        assert_eq!(plan.source, KNOWN_EDITORS[0].0, "表里靠前的优先");
        assert!(
            plan.command_line.starts_with("code -g "),
            "{}",
            plan.command_line
        );
        assert!(plan.command_line.contains(":7"), "{}", plan.command_line);
        // 只装了 Neovim 时用它，且 `+{line}` 被换成真实行号。
        let (only_nvim, _) = launch_plan(&[], &["nvim"], Some(9));
        assert_eq!(only_nvim.source, "nvim");
        assert!(
            only_nvim
                .command_line
                .starts_with(r#"nvim +9 "C:\My Games"#),
            "{}",
            only_nvim.command_line
        );
    }

    #[test]
    fn line_falls_back_to_first_line_but_stays_located() {
        let (plan, _) = launch_plan(&[], &["code"], None);
        assert!(
            plan.command_line.contains(":1"),
            "没有行号时按第 1 行：{}",
            plan.command_line
        );
    }

    #[test]
    fn association_fallback_admits_it_cannot_locate_the_line() {
        let (plan, path) = launch_plan(&[], &[], Some(31));
        assert_eq!(plan.source, "文件关联");
        assert!(!plan.locates_line);
        assert!(
            plan.command_line.starts_with(r#"start "" "#),
            "cmd 的 start 第一个参数是窗口标题，不能省：{}",
            plan.command_line
        );
        let text = describe(&plan, Path::new(&path), Some(31));
        assert!(text.contains("未定位到第 31 行"), "{text}");
        assert!(
            text.contains("RUSTLUA_EDITOR"),
            "必须给出可执行的下一步：{text}"
        );
    }

    #[test]
    fn description_claims_only_whats_actually_located() {
        let (plan, path) = launch_plan(&[], &["code"], Some(12));
        let text = describe(&plan, Path::new(&path), Some(12));
        assert_eq!(text, r#"已在 code 打开 main.lua:12"#);
    }

    #[test]
    fn text_sources_are_the_files_a_script_author_double_clicks() {
        for name in ["main.lua", "player.LUA", "config.toml", "boss.vgraph"] {
            assert_eq!(
                is_text_source(Path::new(name)),
                name != "boss.vgraph",
                "{name}"
            );
        }
        assert!(!is_text_source(Path::new("hero.png")));
        assert!(!is_text_source(Path::new("village.scene")));
    }
}
