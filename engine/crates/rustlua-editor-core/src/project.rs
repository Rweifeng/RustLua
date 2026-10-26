use std::{
    fs,
    path::{Path, PathBuf},
};

pub const REQUIRED_DIRECTORIES: &[&str] = &["engine", "game", "assets", "docs"];
pub const GAME_PROJECT_REQUIRED_DIRECTORIES: &[&str] = &["scripts"];
pub const GAME_PROJECT_OPTIONAL_DIRECTORIES: &[&str] = &["scenes", "assets"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub root: PathBuf,
    pub version: String,
    pub directories: Vec<DirectoryStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryStatus {
    pub name: String,
    pub exists: bool,
    pub is_directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProjectError {
    #[error("项目路径不存在：{}", .0.display())]
    NotFound(PathBuf),
    #[error("项目路径不是目录：{}", .0.display())]
    NotDirectory(PathBuf),
    #[error("项目缺少必需目录：{name}（项目根目录：{}）", root.display())]
    MissingDirectory { root: PathBuf, name: String },
    #[error("读取项目路径失败：{}：{message}", path.display())]
    Io { path: PathBuf, message: String },
}

pub fn open_project(path: impl AsRef<Path>) -> Result<ProjectInfo, ProjectError> {
    let requested = path.as_ref();
    let root = fs::canonicalize(requested).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ProjectError::NotFound(requested.to_path_buf())
        } else {
            ProjectError::Io {
                path: requested.to_path_buf(),
                message: error.to_string(),
            }
        }
    })?;
    let metadata = fs::metadata(&root).map_err(|error| ProjectError::Io {
        path: root.clone(),
        message: error.to_string(),
    })?;
    if !metadata.is_dir() {
        return Err(ProjectError::NotDirectory(root));
    }

    let standalone_game = root.join("project.toml").is_file();
    let directory_names: &[&str] = if standalone_game {
        GAME_PROJECT_REQUIRED_DIRECTORIES
    } else {
        REQUIRED_DIRECTORIES
    };
    let mut directories =
        Vec::with_capacity(directory_names.len() + GAME_PROJECT_OPTIONAL_DIRECTORIES.len());
    for name in directory_names {
        let path = root.join(name);
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ProjectError::MissingDirectory {
                    root: root.clone(),
                    name: (*name).to_owned(),
                });
            }
            Err(error) => {
                return Err(ProjectError::Io {
                    path: path.clone(),
                    message: error.to_string(),
                });
            }
        };
        directories.push(DirectoryStatus {
            name: (*name).to_owned(),
            exists: true,
            is_directory: metadata.is_dir(),
        });
        if !metadata.is_dir() {
            return Err(ProjectError::MissingDirectory {
                root: root.clone(),
                name: (*name).to_owned(),
            });
        }
    }
    if standalone_game {
        for name in GAME_PROJECT_OPTIONAL_DIRECTORIES {
            let path = root.join(name);
            directories.push(DirectoryStatus {
                name: (*name).to_owned(),
                exists: path.is_dir(),
                is_directory: path.is_dir(),
            });
        }
    }

    let version = read_baseline_version(&root);
    Ok(ProjectInfo {
        root,
        version,
        directories,
    })
}

fn read_baseline_version(root: &Path) -> String {
    let mut command = std::process::Command::new("git");
    command
        .args(["describe", "--tags", "--abbrev=0"])
        .current_dir(root);
    #[cfg(windows)]
    {
        // 编辑器是 GUI 程序：git 是控制台程序，不设 CREATE_NO_WINDOW 会闪现终端窗口。
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command.output();
    match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        _ => "未检测到 Git 标签".to_owned(),
    }
}

pub fn check_project(path: impl AsRef<Path>) -> Result<String, ProjectError> {
    let info = open_project(path)?;
    let mut output = format!(
        "editor_project_ok root={} baseline={}\n",
        info.root.display(),
        info.version
    );
    for directory in info.directories {
        output.push_str(&format!("directory={} status=present\n", directory.name));
    }
    // 组件 schema 审计（docs/55 §1 第 3 条）：已注册组件的字段级漂移会被逐条列出，
    // 未注册组件单独计数（不是错误——扩展组件本来就可以没有 schema）。
    match crate::schema_audit::audit_project(&info.root) {
        Ok(audit) => {
            for line in audit.report_lines() {
                output.push_str(&line);
                output.push('\n');
            }
        }
        Err(error) => output.push_str(&format!("schema_audit_failed reason={error}\n")),
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_project() -> PathBuf {
        // 用 crate 级唯一令牌：并行测试下也不会有两个测试拿到同一目录。
        let root =
            std::env::temp_dir().join(format!("rustlua-editor-test-{}", crate::test_token()));
        fs::create_dir_all(&root).expect("temporary project should be created");
        root
    }

    #[test]
    fn opens_project_and_reports_required_directories() {
        let root = temp_project();
        for name in REQUIRED_DIRECTORIES {
            fs::create_dir(root.join(name)).expect("required directory should be created");
        }
        let info = open_project(&root).expect("project should open");
        assert_eq!(info.directories.len(), REQUIRED_DIRECTORIES.len());
        assert!(
            check_project(&root)
                .expect("check should pass")
                .contains("editor_project_ok")
        );
        fs::remove_dir_all(root).expect("temporary project should be removed");
    }

    #[test]
    fn opens_standalone_game_project_without_engine_directories() {
        let root = temp_project();
        fs::create_dir(root.join("scripts")).expect("scripts directory should be created");
        fs::write(root.join("project.toml"), "[project]\nname = \"demo\"\n")
            .expect("manifest should be created");
        let info = open_project(&root).expect("standalone game project should open");
        assert_eq!(info.directories[0].name, "scripts");
        assert!(!info.directories.iter().any(|entry| entry.name == "engine"));
        fs::remove_dir_all(root).expect("temporary project should be removed");
    }

    #[test]
    fn rejects_missing_project() {
        let path = std::env::temp_dir().join("rustlua-editor-project-that-does-not-exist");
        assert!(matches!(open_project(path), Err(ProjectError::NotFound(_))));
    }

    #[test]
    fn rejects_project_with_missing_required_directory() {
        let root = temp_project();
        fs::create_dir(root.join("engine")).expect("directory should be created");
        assert!(matches!(
            open_project(&root),
            Err(ProjectError::MissingDirectory { .. })
        ));
        fs::remove_dir_all(root).expect("temporary project should be removed");
    }
}
