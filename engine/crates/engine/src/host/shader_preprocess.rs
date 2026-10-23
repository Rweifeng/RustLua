//! P3-R04-2：只服务本仓库的 WGSL 轻量预处理器。
//!
//! 支持四条指令（必须独占一行，允许前导空白）：
//!
//! ```wgsl
//! //#if FEATURE
//! //#elif FEATURE
//! //#else
//! //#endif
//! //#include "other.wgsl"
//! ```
//!
//! 设计边界（与 `tasks/plan/P3-渲染与编辑器能力.md` §6.2 一致）：
//!
//! - **不是 C 预处理器**：不做宏、不做表达式、不做 `#ifdef`/`#ifndef`；feature 只有"定义/未定义"。
//! - **未知 feature 是错误**：`//#if` 引用的名字必须在调用方给的 `known_features` 里；拼错一个
//!   `//#if SKN` 不会被当成"未定义"而静默产出错误变体。未激活的分支也校验。
//! - **有上限**：嵌套深度与 include 深度都有硬上限；include 循环按栈检测并报完整链。
//! - **可回溯**：输出按行记录 [`crate::host::shader_preprocess::SourceLine`]（文件 + 原行号），
//!   wgpu 编译错误因此能指回原文件；
//!   指令行不产出输出行，映射保持一一对应。
//!
//! 本模块不读文件系统：include 的源由调用方以 `(名字, 源文本)` 表提供，单测因此不需要临时目录。

use std::collections::BTreeSet;
use std::fmt;

/// 默认嵌套上限（`#if` 层数）。
pub const DEFAULT_MAX_NESTING: usize = 16;
/// 默认 include 深度上限（含根文件）。
pub const DEFAULT_MAX_INCLUDE_DEPTH: usize = 8;

/// 预处理错误。全部带**原文件 + 行号**，不产生"没有位置的编译失败"。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreprocessError {
    /// `//#if`/`//#elif` 引用了不在 `known_features` 里的名字（防拼写漂移）。
    UnknownFeature {
        feature: String,
        file: String,
        line: usize,
    },
    /// feature 名不合法（空或含非 `[A-Za-z0-9_]`）。
    InvalidFeature {
        feature: String,
        file: String,
        line: usize,
    },
    /// `//#elif`/`//#else`/`//#endif` 没有对应的 `//#if`。
    StrayDirective {
        directive: String,
        file: String,
        line: usize,
    },
    /// `//#else` 之后还有 `//#elif`/`//#else`。
    MisplacedElse {
        directive: String,
        file: String,
        line: usize,
    },
    /// 文件结束时仍有未闭合的 `//#if`。
    UnclosedIf { file: String, line: usize },
    /// `//#` 后面是无法识别的指令（防"以为实现了"）。
    UnknownDirective {
        directive: String,
        file: String,
        line: usize,
    },
    /// `//#include` 的语法不是 `//#include "name"`。
    MalformedInclude { file: String, line: usize },
    /// include 的名字不在提供的源表里。
    IncludeNotFound {
        include: String,
        file: String,
        line: usize,
    },
    /// include 形成环。`chain` 形如 `a -> b -> a`。
    IncludeCycle { chain: String },
    /// 嵌套或 include 超过上限。
    TooDeep {
        what: &'static str,
        limit: usize,
        file: String,
        line: usize,
    },
}

impl fmt::Display for PreprocessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFeature {
                feature,
                file,
                line,
            } => write!(f, "{file}:{line}: 未知 shader feature `{feature}`"),
            Self::InvalidFeature {
                feature,
                file,
                line,
            } => write!(f, "{file}:{line}: 非法 feature 名 `{feature}`"),
            Self::StrayDirective {
                directive,
                file,
                line,
            } => write!(f, "{file}:{line}: `{directive}` 没有对应的 `//#if`"),
            Self::MisplacedElse {
                directive,
                file,
                line,
            } => write!(f, "{file}:{line}: `{directive}` 出现在 `//#else` 之后"),
            Self::UnclosedIf { file, line } => write!(f, "{file}:{line}: `//#if` 未闭合"),
            Self::UnknownDirective {
                directive,
                file,
                line,
            } => write!(f, "{file}:{line}: 未知指令 `{directive}`"),
            Self::MalformedInclude { file, line } => {
                write!(
                    f,
                    "{file}:{line}: `//#include` 必须是 `//#include \"name\"`"
                )
            }
            Self::IncludeNotFound {
                include,
                file,
                line,
            } => write!(f, "{file}:{line}: include `{include}` 不在源表里"),
            Self::IncludeCycle { chain } => write!(f, "include 循环：{chain}"),
            Self::TooDeep {
                what,
                limit,
                file,
                line,
            } => write!(f, "{file}:{line}: {what}超过上限 {limit}"),
        }
    }
}

impl std::error::Error for PreprocessError {}

/// 输出行 → 原文件行。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLine {
    pub file: String,
    /// 1-based，与编辑器/wgpu 报错口径一致。
    pub line: usize,
}

/// 预处理成功的结果。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PreprocessedShader {
    source: String,
    line_map: Vec<SourceLine>,
}

impl PreprocessedShader {
    /// 生成源（可直接交给 `wgpu::ShaderModuleDescriptor.source`）。
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// 每个输出行（0-based 下标即"输出第 n+1 行"）对应的原文件行。
    #[must_use]
    pub fn line_map(&self) -> &[SourceLine] {
        &self.line_map
    }

    /// 1-based 输出行号 → 原文件行。
    #[must_use]
    pub fn origin_of(&self, generated_line: usize) -> Option<&SourceLine> {
        generated_line
            .checked_sub(1)
            .and_then(|index| self.line_map.get(index))
    }
}

/// 预处理选项。
#[derive(Clone, Copy, Debug)]
pub struct PreprocessOptions<'a> {
    /// 已定义的 feature（大小写敏感，必须与 `known_features` 中的拼写一致）。
    pub features: &'a [&'a str],
    /// 合法 feature 全集（缺少的 `//#if` 名字按拼写错误处理）。
    pub known_features: &'a [&'a str],
    /// `(名字, 源文本)`：`//#include "名字"` 从这里取。
    pub include_sources: &'a [(&'a str, &'a str)],
    pub max_nesting: usize,
    pub max_include_depth: usize,
}

impl Default for PreprocessOptions<'_> {
    fn default() -> Self {
        Self {
            features: &[],
            known_features: &[],
            include_sources: &[],
            max_nesting: DEFAULT_MAX_NESTING,
            max_include_depth: DEFAULT_MAX_INCLUDE_DEPTH,
        }
    }
}

/// 预处理入口。`source` 是根文件文本。
pub fn preprocess(
    root_name: &str,
    root_source: &str,
    options: PreprocessOptions<'_>,
) -> Result<PreprocessedShader, PreprocessError> {
    let defines: BTreeSet<&str> = options.features.iter().copied().collect();
    let known: BTreeSet<&str> = options.known_features.iter().copied().collect();
    let mut state = State {
        options,
        defines,
        known,
        output: String::new(),
        line_map: Vec::new(),
        include_stack: vec![root_name.to_owned()],
    };
    state.expand(root_name, root_source, 1)?;
    Ok(PreprocessedShader {
        source: state.output,
        line_map: state.line_map,
    })
}

struct State<'a> {
    options: PreprocessOptions<'a>,
    defines: BTreeSet<&'a str>,
    known: BTreeSet<&'a str>,
    output: String,
    line_map: Vec<SourceLine>,
    /// 当前 include 链（根文件在栈底），用于循环检测。
    include_stack: Vec<String>,
}

struct CondFrame {
    /// 本分支是否激活（已与父级取与）。
    active: bool,
    /// 父级是否激活（`#else` 需要用它计算）。
    parent_active: bool,
    /// 是否有分支已命中（`#elif`/`#else` 据此关闭后续分支）。
    taken: bool,
    seen_else: bool,
    file: String,
    line: usize,
}

impl State<'_> {
    fn expand(
        &mut self,
        file: &str,
        source: &str,
        include_depth: usize,
    ) -> Result<(), PreprocessError> {
        let mut stack: Vec<CondFrame> = Vec::new();
        for (index, raw_line) in source.lines().enumerate() {
            let line = index + 1;
            let Some(directive) = parse_directive(raw_line) else {
                if if_active(&stack) {
                    self.output.push_str(raw_line);
                    self.output.push('\n');
                    self.line_map.push(SourceLine {
                        file: file.to_owned(),
                        line,
                    });
                }
                continue;
            };
            match directive {
                Directive::If(feature) => {
                    self.check_feature(feature, file, line)?;
                    if stack.len() >= self.options.max_nesting {
                        return Err(PreprocessError::TooDeep {
                            what: "`//#if` 嵌套",
                            limit: self.options.max_nesting,
                            file: file.to_owned(),
                            line,
                        });
                    }
                    let parent_active = if_active(&stack);
                    let cond = self.defines.contains(feature);
                    stack.push(CondFrame {
                        active: parent_active && cond,
                        parent_active,
                        taken: cond,
                        seen_else: false,
                        file: file.to_owned(),
                        line,
                    });
                }
                Directive::Elif(feature) => {
                    self.check_feature(feature, file, line)?;
                    let Some(frame) = stack.last_mut() else {
                        return Err(PreprocessError::StrayDirective {
                            directive: "//#elif".to_owned(),
                            file: file.to_owned(),
                            line,
                        });
                    };
                    if frame.seen_else {
                        return Err(PreprocessError::MisplacedElse {
                            directive: "//#elif".to_owned(),
                            file: file.to_owned(),
                            line,
                        });
                    }
                    let cond = !frame.taken && self.defines.contains(feature);
                    frame.active = frame.parent_active && cond;
                    frame.taken |= cond;
                }
                Directive::Else => {
                    let Some(frame) = stack.last_mut() else {
                        return Err(PreprocessError::StrayDirective {
                            directive: "//#else".to_owned(),
                            file: file.to_owned(),
                            line,
                        });
                    };
                    if frame.seen_else {
                        return Err(PreprocessError::MisplacedElse {
                            directive: "//#else".to_owned(),
                            file: file.to_owned(),
                            line,
                        });
                    }
                    frame.seen_else = true;
                    frame.active = frame.parent_active && !frame.taken;
                    frame.taken = true;
                }
                Directive::Endif => {
                    if stack.pop().is_none() {
                        return Err(PreprocessError::StrayDirective {
                            directive: "//#endif".to_owned(),
                            file: file.to_owned(),
                            line,
                        });
                    }
                }
                Directive::Include(include) => {
                    if if_active(&stack) {
                        self.include(file, line, include, include_depth)?;
                    }
                }
                Directive::MalformedInclude => {
                    return Err(PreprocessError::MalformedInclude {
                        file: file.to_owned(),
                        line,
                    });
                }
                Directive::Unknown(name) => {
                    return Err(PreprocessError::UnknownDirective {
                        directive: name.to_owned(),
                        file: file.to_owned(),
                        line,
                    });
                }
            }
        }
        if let Some(frame) = stack.last() {
            return Err(PreprocessError::UnclosedIf {
                file: frame.file.clone(),
                line: frame.line,
            });
        }
        Ok(())
    }

    fn check_feature(&self, feature: &str, file: &str, line: usize) -> Result<(), PreprocessError> {
        validate_feature(feature, file, line)?;
        if self.known.contains(feature) {
            Ok(())
        } else {
            Err(PreprocessError::UnknownFeature {
                feature: feature.to_owned(),
                file: file.to_owned(),
                line,
            })
        }
    }

    fn include(
        &mut self,
        file: &str,
        line: usize,
        include: &str,
        include_depth: usize,
    ) -> Result<(), PreprocessError> {
        if self.include_stack.iter().any(|name| name == include) {
            let mut chain = self.include_stack.join(" -> ");
            chain.push_str(" -> ");
            chain.push_str(include);
            return Err(PreprocessError::IncludeCycle { chain });
        }
        if include_depth >= self.options.max_include_depth {
            return Err(PreprocessError::TooDeep {
                what: "include 深度",
                limit: self.options.max_include_depth,
                file: file.to_owned(),
                line,
            });
        }
        let Some((_, source)) = self
            .options
            .include_sources
            .iter()
            .find(|(name, _)| *name == include)
        else {
            return Err(PreprocessError::IncludeNotFound {
                include: include.to_owned(),
                file: file.to_owned(),
                line,
            });
        };
        self.include_stack.push(include.to_owned());
        // 递归展开：include 的文件有自己的 `#if` 栈；`include_depth + 1` 让上限按链长计。
        let result = self.expand(include, source, include_depth + 1);
        self.include_stack.pop();
        result
    }
}

fn if_active(stack: &[CondFrame]) -> bool {
    stack.last().is_none_or(|frame| frame.active)
}

fn validate_feature(feature: &str, file: &str, line: usize) -> Result<(), PreprocessError> {
    let valid = !feature.is_empty()
        && feature
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if valid {
        Ok(())
    } else {
        Err(PreprocessError::InvalidFeature {
            feature: feature.to_owned(),
            file: file.to_owned(),
            line,
        })
    }
}

enum Directive<'a> {
    If(&'a str),
    Elif(&'a str),
    Else,
    Endif,
    Include(&'a str),
    MalformedInclude,
    Unknown(&'a str),
}

/// 只识别独占一行的 `//#...`；普通注释与含指令的代码行原样放过。
fn parse_directive(line: &str) -> Option<Directive<'_>> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("//#")?.trim();
    if rest.is_empty() {
        return Some(Directive::Unknown("//#"));
    }
    let (name, argument) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(name, argument)| (name, argument.trim()));
    match name {
        "if" => Some(Directive::If(argument)),
        "elif" => Some(Directive::Elif(argument)),
        "else" if argument.is_empty() => Some(Directive::Else),
        "else" => Some(Directive::Unknown(trimmed)),
        "endif" if argument.is_empty() => Some(Directive::Endif),
        "endif" => Some(Directive::Unknown(trimmed)),
        "include" => Some(parse_include(argument)),
        _ => Some(Directive::Unknown(trimmed)),
    }
}

fn parse_include(argument: &str) -> Directive<'_> {
    argument
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .filter(|name| !name.is_empty() && !name.contains('"'))
        .map_or(Directive::MalformedInclude, Directive::Include)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        source: &str,
        features: &[&str],
        known: &[&str],
    ) -> Result<PreprocessedShader, PreprocessError> {
        preprocess(
            "root.wgsl",
            source,
            PreprocessOptions {
                features,
                known_features: known,
                ..PreprocessOptions::default()
            },
        )
    }

    fn run_with_includes(
        root: &str,
        root_source: &str,
        features: &[&str],
        known: &[&str],
        includes: &[(&str, &str)],
    ) -> Result<PreprocessedShader, PreprocessError> {
        preprocess(
            root,
            root_source,
            PreprocessOptions {
                features,
                known_features: known,
                include_sources: includes,
                ..PreprocessOptions::default()
            },
        )
    }

    #[test]
    fn nested_if_elif_else_selects_one_branch() {
        let source = "\
//#if A
fn a() {}
//#if B
fn ab() {}
//#else
fn a_only() {}
//#endif
//#elif C
fn c() {}
//#else
fn none() {}
//#endif
";
        let known = ["A", "B", "C"];
        let ab = run(source, &["A", "B"], &known).unwrap();
        assert!(ab.source().contains("fn ab()"));
        assert!(!ab.source().contains("fn a_only()"));
        assert!(!ab.source().contains("fn c()"));

        let only_a = run(source, &["A"], &known).unwrap();
        assert!(only_a.source().contains("fn a_only()"));
        assert!(!only_a.source().contains("fn ab()"));

        let only_c = run(source, &["C"], &known).unwrap();
        assert!(only_c.source().contains("fn c()"));
        assert!(!only_c.source().contains("fn a()"));

        let none = run(source, &[], &known).unwrap();
        assert!(none.source().contains("fn none()"));
    }

    /// feature 名大小写敏感：`skin` 不能命中 `SKIN`，两个名字都在已知表里时按未定义处理。
    #[test]
    fn feature_names_are_case_sensitive() {
        let source = "//#if SKIN\nselected\n//#else\nfallback\n//#endif\n";
        let output = run(source, &["skin"], &["SKIN", "skin"]).unwrap();
        assert_eq!(output.source(), "fallback\n");
        let output = run(source, &["SKIN"], &["SKIN"]).unwrap();
        assert_eq!(output.source(), "selected\n");
    }

    #[test]
    fn unknown_or_invalid_features_are_errors_with_location() {
        let error = run("// a\n//#if SKN\nx\n//#endif\n", &[], &["SKIN"]).unwrap_err();
        assert!(matches!(
            error,
            PreprocessError::UnknownFeature { ref feature, line: 2, .. } if feature == "SKN"
        ));
        // 未激活的分支同样校验：语法错误优先于"分支未激活"。
        let error = run(
            "//#if OFF\n//#if BAD!\nx\n//#endif\n//#endif\n",
            &[],
            &["OFF"],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            PreprocessError::InvalidFeature { line: 2, .. }
        ));
        // 未激活分支里的拼写错误同样报错。
        let error = run(
            "//#if OFF\n//#if TYPO\nx\n//#endif\n//#endif\n",
            &[],
            &["OFF", "REAL"],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            PreprocessError::UnknownFeature { line: 2, .. }
        ));
    }

    #[test]
    fn stray_and_misplaced_directives_are_errors() {
        assert!(matches!(
            run("//#endif\n", &[], &[]).unwrap_err(),
            PreprocessError::StrayDirective { line: 1, .. }
        ));
        assert!(matches!(
            run("//#if A\n//#else\n//#else\n//#endif\n", &["A"], &["A"]).unwrap_err(),
            PreprocessError::MisplacedElse { line: 3, .. }
        ));
        assert!(matches!(
            run(
                "//#if A\n//#else\n//#elif B\n//#endif\n",
                &["A"],
                &["A", "B"]
            )
            .unwrap_err(),
            PreprocessError::MisplacedElse { line: 3, .. }
        ));
        assert!(matches!(
            run("//#if A\nx\n", &["A"], &["A"]).unwrap_err(),
            PreprocessError::UnclosedIf { line: 1, .. }
        ));
        assert!(matches!(
            run("//#ifdef A\n", &["A"], &["A"]).unwrap_err(),
            PreprocessError::UnknownDirective { .. }
        ));
        assert!(matches!(
            run("//#include x.wgsl\n", &[], &[]).unwrap_err(),
            PreprocessError::MalformedInclude { line: 1, .. }
        ));
        assert!(matches!(
            run("//#include \"\"\n", &[], &[]).unwrap_err(),
            PreprocessError::MalformedInclude { line: 1, .. }
        ));
    }

    #[test]
    fn include_expands_and_line_map_points_back_to_the_source() {
        let includes = [("lib.wgsl", "fn lib() {}\nfn lib2() {}\n")];
        let root = "fn root() {}\n//#include \"lib.wgsl\"\nfn tail() {}\n";
        let output = run_with_includes("root.wgsl", root, &[], &[], &includes).unwrap();
        assert_eq!(
            output.source(),
            "fn root() {}\nfn lib() {}\nfn lib2() {}\nfn tail() {}\n"
        );
        assert_eq!(
            output.line_map(),
            [
                SourceLine {
                    file: "root.wgsl".to_owned(),
                    line: 1
                },
                SourceLine {
                    file: "lib.wgsl".to_owned(),
                    line: 1
                },
                SourceLine {
                    file: "lib.wgsl".to_owned(),
                    line: 2
                },
                SourceLine {
                    file: "root.wgsl".to_owned(),
                    line: 3
                },
            ]
        );
        assert_eq!(output.origin_of(2).unwrap().file, "lib.wgsl");
        assert_eq!(output.origin_of(2).unwrap().line, 1);
        assert!(output.origin_of(0).is_none());
        assert!(output.origin_of(5).is_none());
    }

    #[test]
    fn include_cycle_reports_the_whole_chain() {
        let includes = [
            ("a.wgsl", "//#include \"b.wgsl\"\n"),
            ("b.wgsl", "//#include \"a.wgsl\"\n"),
        ];
        let error = run_with_includes("root.wgsl", "//#include \"a.wgsl\"\n", &[], &[], &includes)
            .unwrap_err();
        match error {
            PreprocessError::IncludeCycle { chain } => {
                assert_eq!(chain, "root.wgsl -> a.wgsl -> b.wgsl -> a.wgsl");
            }
            other => panic!("期望循环错误，得到 {other:?}"),
        }
    }

    #[test]
    fn include_not_found_and_inactive_include_is_skipped() {
        let error = run("//#include \"missing.wgsl\"\n", &[], &[]).unwrap_err();
        assert!(matches!(
            error,
            PreprocessError::IncludeNotFound { line: 1, .. }
        ));

        let output = run(
            "//#if USE_LIB\n//#include \"missing.wgsl\"\n//#endif\nfn ok() {}\n",
            &[],
            &["USE_LIB"],
        )
        .unwrap();
        assert_eq!(output.source(), "fn ok() {}\n");
    }

    #[test]
    fn depth_limits_are_enforced_with_location() {
        let mut nested = String::new();
        for _ in 0..(DEFAULT_MAX_NESTING + 1) {
            nested.push_str("//#if A\n");
        }
        for _ in 0..(DEFAULT_MAX_NESTING + 1) {
            nested.push_str("//#endif\n");
        }
        let error = run(&nested, &["A"], &["A"]).unwrap_err();
        assert!(matches!(
            error,
            PreprocessError::TooDeep {
                what: "`//#if` 嵌套",
                limit: DEFAULT_MAX_NESTING,
                ..
            }
        ));

        // include 深度：上限按链长计，根文件占 1。
        let includes = [
            ("a.wgsl", "//#include \"b.wgsl\"\n"),
            ("b.wgsl", "fn b() {}\n"),
        ];
        let options_error = preprocess(
            "root.wgsl",
            "//#include \"a.wgsl\"\n",
            PreprocessOptions {
                include_sources: &includes,
                max_include_depth: 2,
                ..PreprocessOptions::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            options_error,
            PreprocessError::TooDeep {
                what: "include 深度",
                limit: 2,
                ..
            }
        ));
    }

    #[test]
    fn directive_lines_do_not_consume_output_lines() {
        let output = run(
            "//#if A\n//#if B\nx\n//#endif\n//#endif\n",
            &["A", "B"],
            &["A", "B"],
        )
        .unwrap();
        assert_eq!(output.source(), "x\n");
        assert_eq!(
            output.line_map(),
            [SourceLine {
                file: "root.wgsl".to_owned(),
                line: 3
            }]
        );
    }

    #[test]
    fn ordinary_comments_and_indented_directives_are_handled() {
        let output = run(
            "  //#if A\n    x\n// plain comment\n//#endif\n",
            &["A"],
            &["A"],
        )
        .unwrap();
        assert_eq!(output.source(), "    x\n// plain comment\n");
    }
}
