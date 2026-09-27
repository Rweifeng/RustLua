//! 玩家数据目录（P3）：存档与玩家设置应该写到哪里。
//!
//! **为什么必须是引擎能力而不是游戏侧约定**：发布包被安装到
//! `C:\Program Files\...` 时，程序目录对普通用户**不可写**；即使可写，
//! 把存档写在安装目录里也意味着"重装/卸载即丢档"，并且违反 Windows 的应用
//! 数据约定（用户数据属于用户配置目录，程序文件属于安装目录）。
//!
//! 规则（只有一条，按项目形态选择根目录）：
//!
//! | 形态 | 判定 | 存档 | 玩家设置 |
//! |---|---|---|---|
//! | 发布包 | 项目根是 `<exe>/pack`（旁边有 `release.toml`）或 `--pack` | `<用户数据目录>/save.sav` | `<用户数据目录>/settings.toml` |
//! | 开发工程 | 其余情况 | `<project>/.data/save.sav` | `<project>/settings.toml` |
//!
//! 开发工程保留工程内路径是因为编辑器与验收样例要能直接检查这些文件（改键面板、
//! `jhyt_m20_*` 的 settings 往返断言）；发布包一律走用户目录。
//!
//! 用户数据目录：
//! * Windows：`%APPDATA%\<app>`（`APPDATA` 缺失时退回 `%LOCALAPPDATA%`、再退 `%USERPROFILE%`）；
//! * 其他平台：`$XDG_DATA_HOME/<app>`，否则 `$HOME/.local/share/<app>`；
//! * 环境变量 `RUSTLUA_USER_DIR` 可整体覆盖（测试与便携版分发用）。

use std::{
    env, fs,
    io::{Error, Result},
    path::{Path, PathBuf},
};

/// 覆盖用户数据目录根的环境变量（测试/便携分发）。
pub const USER_DIR_ENV: &str = "RUSTLUA_USER_DIR";

/// 应用名兜底（`project.toml`/`release.toml` 都没给名字时）。
pub const DEFAULT_APP_NAME: &str = "rustlua-game";

/// 存档与玩家设置的根目录（见模块文档的表）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataRoots {
    /// 存档目录：`save.sav`、`save_<slot>.sav` 写在这里。
    pub save_dir: PathBuf,
    /// 玩家设置目录：`settings.toml` 读写的根。
    pub settings_dir: PathBuf,
    /// 是否按发布包处理（决定上面两个字段是用户目录还是工程内路径）。
    pub packaged: bool,
    /// 应用名（用户目录的最后一段）。
    pub app_name: String,
}

impl DataRoots {
    /// 存档文件路径（默认槽位）。
    #[must_use]
    pub fn save_file(&self) -> PathBuf {
        self.save_dir.join("save.sav")
    }

    /// 玩家设置文件路径。
    #[must_use]
    pub fn settings_file(&self) -> PathBuf {
        self.settings_dir.join("settings.toml")
    }

    /// 运行期日志目录（`game.log`、`crash.log`）。
    ///
    /// 发布包形态下必须落在用户数据目录：安装目录（`C:\Program Files\...`）对普通用户
    /// 不可写，日志/崩溃报告写在那里会静默丢失，而且会在发布目录里留下运行期残留
    /// （M31 验收 ① 节明令禁止）。开发形态仍是工程内 `artifacts/logs`，由调用方决定。
    #[must_use]
    pub fn log_dir(&self) -> PathBuf {
        self.settings_dir.join("logs")
    }
}

/// 判定"项目根是发布包"：目录名是 `pack` 且旁边有 `release.toml`，或调用方
/// （`game --pack`）明确说是。
#[must_use]
pub fn is_release_pack(project_root: &Path, explicit_pack: bool) -> bool {
    if explicit_pack {
        return true;
    }
    let named_pack = project_root
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("pack"));
    if !named_pack {
        return false;
    }
    project_root
        .parent()
        .is_some_and(|parent| parent.join("release.toml").is_file())
}

/// 应用名：优先发布包的 `release.toml` 的 `publish_name`，其次 `project.toml`
/// 的 `name`，最后 [`DEFAULT_APP_NAME`]。结果会做文件名安全化（只留 ASCII
/// 字母数字、`-`、`_`，其余换成 `-`）。
#[must_use]
pub fn app_name(project_root: &Path) -> String {
    let raw = super::project_config::publish_name(project_root)
        .or_else(|| super::project_config::project_name(project_root))
        .unwrap_or_else(|| DEFAULT_APP_NAME.to_owned());
    sanitize_component(&raw)
}

/// 解析存档与设置根目录（用户数据根按 `RUSTLUA_USER_DIR` → 平台约定目录）。
#[must_use]
pub fn data_roots(project_root: &Path, explicit_pack: bool) -> DataRoots {
    data_roots_at(project_root, explicit_pack, &data_base())
}

/// 与 [`data_roots`] 相同，但**显式**给出平台数据根目录（其下再拼应用名）。
/// 测试与便携分发用这个版本：不依赖进程环境变量，因此多个测试可以并行跑。
#[must_use]
pub fn data_roots_at(project_root: &Path, explicit_pack: bool, data_base: &Path) -> DataRoots {
    let app = app_name(project_root);
    if is_release_pack(project_root, explicit_pack) {
        let user = data_base.join(&app);
        DataRoots {
            save_dir: user.clone(),
            settings_dir: user,
            packaged: true,
            app_name: app,
        }
    } else {
        DataRoots {
            save_dir: project_root.join(super::PROJECT_WORK_DIR),
            settings_dir: project_root.to_path_buf(),
            packaged: false,
            app_name: app,
        }
    }
}

/// 用户数据目录（`<基目录>/<app>`）。基目录见 [`data_base`]。
#[must_use]
pub fn user_data_dir(app: &str) -> PathBuf {
    data_base().join(app)
}

/// 用户数据基目录：`RUSTLUA_USER_DIR`（便携版/CI 覆盖）优先，否则平台约定目录。
#[must_use]
pub fn data_base() -> PathBuf {
    env::var_os(USER_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map_or_else(platform_data_dir, PathBuf::from)
}

fn platform_data_dir() -> PathBuf {
    if cfg!(windows) {
        for key in ["APPDATA", "LOCALAPPDATA"] {
            if let Some(value) = env::var_os(key).filter(|value| !value.is_empty()) {
                return PathBuf::from(value);
            }
        }
        if let Some(profile) = env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
            return PathBuf::from(profile).join("AppData").join("Roaming");
        }
    } else if let Some(value) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(value);
    } else if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join(".local").join("share");
    }
    env::temp_dir().join("rustlua-user")
}

/// 首次运行时把发布包里的 `settings.toml` 复制到用户目录作为**默认值**，
/// 之后用户目录里的文件就是唯一真相（升级发布包不会覆盖玩家的改键/音量）。
///
/// 返回 `Ok(true)` 表示这次真的播种了。用户文件已存在 → `Ok(false)`（不动它）。
/// 目录创建或复制失败返回错误（调用方决定是警告还是失败，不静默吞掉）。
pub fn seed_settings(roots: &DataRoots, pack_root: &Path) -> Result<bool> {
    if !roots.packaged {
        return Ok(false);
    }
    let target = roots.settings_file();
    if target.is_file() {
        return Ok(false);
    }
    let source = pack_root.join("settings.toml");
    if !source.is_file() {
        return Ok(false);
    }
    fs::create_dir_all(&roots.settings_dir).map_err(|error| {
        Error::other(format!(
            "无法创建用户数据目录 {}: {error}（可用 {USER_DIR_ENV} 指定其他位置）",
            roots.settings_dir.display()
        ))
    })?;
    fs::copy(&source, &target).map_err(|error| {
        Error::other(format!(
            "无法把 {} 播种到 {}: {error}",
            source.display(),
            target.display()
        ))
    })?;
    Ok(true)
}

/// 创建存档目录（发布包首次运行的用户目录、开发工程的 `.cache` 都可能还不存在）。
pub fn ensure_save_dir(roots: &DataRoots) -> Result<()> {
    fs::create_dir_all(&roots.save_dir).map_err(|error| {
        Error::other(format!(
            "无法创建存档目录 {}: {error}（可用 {USER_DIR_ENV} 指定其他位置）",
            roots.save_dir.display()
        ))
    })
}

/// 只保留 ASCII 字母数字与 `-`/`_`，其余替换成 `-`；空结果退回默认名。
fn sanitize_component(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        DEFAULT_APP_NAME.to_owned()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = env::temp_dir().join(format!("rustlua-userdir-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// 造一个"发布包安装目录"：`<install>/pack` + 旁边 `release.toml`，返回 pack 目录。
    fn install_pack(tag: &str, publish_name: &str) -> PathBuf {
        let install = temp_root(tag);
        let pack = install.join("pack");
        fs::create_dir_all(&pack).unwrap();
        fs::write(
            install.join("release.toml"),
            format!("publish_name = \"{publish_name}\"\n"),
        )
        .unwrap();
        fs::write(pack.join("project.toml"), "name = \"DemoGame\"\n").unwrap();
        pack
    }

    fn install_of(pack: &Path) -> PathBuf {
        pack.parent().map(Path::to_path_buf).unwrap_or_default()
    }

    /// 开发工程：设置留在工程根（编辑器与 `jhyt_m20_*` 直接检查这个文件），
    /// 存档进工程工作目录 `.cache`（与宿主既有行为一致）。
    #[test]
    fn source_project_keeps_data_inside_the_project() {
        let root = temp_root("dev");
        fs::write(root.join("project.toml"), "name = \"Demo\"\n").unwrap();
        let base = temp_root("devbase");
        let roots = data_roots_at(&root, false, &base);
        assert!(!roots.packaged);
        assert_eq!(roots.settings_file(), root.join("settings.toml"));
        assert_eq!(roots.save_file(), root.join(".cache").join("save.sav"));
        assert!(!roots.save_dir.starts_with(&base), "开发工程不碰用户目录");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&base);
    }

    /// 发布包：`<exe>/pack` + 旁边 `release.toml` ⇒ 用户数据目录，且**不写进程序目录**。
    #[test]
    fn release_pack_writes_into_the_user_data_directory() {
        let pack = install_pack("pack", "Demo");
        let base = temp_root("packbase");
        let roots = data_roots_at(&pack, false, &base);
        assert!(roots.packaged, "release layout must be detected");
        assert_eq!(roots.app_name, "Demo");
        assert_eq!(roots.save_dir, base.join("Demo"));
        assert_eq!(roots.settings_dir, base.join("Demo"));
        assert!(!roots.save_dir.starts_with(&pack), "不得写进程序目录");
        // 日志/崩溃报告同样落在用户数据目录（安装目录可能只读，且发布目录不容运行期残留）。
        assert_eq!(roots.log_dir(), base.join("Demo").join("logs"));
        assert!(
            roots
                .log_dir()
                .join("game.log")
                .to_string_lossy()
                .contains("Demo")
        );
        let _ = fs::remove_dir_all(install_of(&pack));
        let _ = fs::remove_dir_all(&base);
    }

    /// `--pack` 明确指定时即使目录不叫 `pack` 也按发布包处理；目录名叫 `pack`
    /// 但旁边没有 `release.toml`（例如开发时手动 `assetc package` 的输出）按开发工程处理。
    #[test]
    fn pack_detection_requires_release_marker_or_explicit_flag() {
        let root = temp_root("detect");
        let pack = root.join("pack");
        fs::create_dir_all(&pack).unwrap();
        assert!(!is_release_pack(&pack, false));
        assert!(is_release_pack(&pack, true));
        fs::write(root.join("release.toml"), "publish_name = \"Demo\"\n").unwrap();
        fs::write(
            root.join(crate::core::project_config::PROJECT_TOML),
            "name = \"Demo\"\n",
        )
        .unwrap();
        assert!(is_release_pack(&pack, false));
        assert!(!is_release_pack(&root, false), "非 pack 目录不算发布包");
        // 显式 `--pack` 且目录不叫 pack：仍按发布包处理。应用名退回 `project.toml`
        // 的 name——这个布局下没有与 `pack/` 同级的 `release.toml` 标记可读。
        let base = temp_root("detectbase");
        let explicit = data_roots_at(&root, true, &base);
        assert!(explicit.packaged);
        assert_eq!(explicit.save_dir, base.join("Demo"));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&base);
    }

    /// 首次运行播种设置：包里的 `settings.toml` → 用户目录；此后用户文件是唯一真相，
    /// 再次调用（模拟升级发布包）**不得**覆盖玩家改过的设置。
    #[test]
    fn first_run_seeds_settings_and_never_overwrites_user_edits() {
        let pack = install_pack("seed", "Demo");
        fs::write(pack.join("settings.toml"), "[audio]\nmusic = 0.5\n").unwrap();
        let base = temp_root("seedbase");
        let roots = data_roots_at(&pack, false, &base);
        assert!(seed_settings(&roots, &pack).unwrap(), "首次运行应播种");
        assert_eq!(
            fs::read_to_string(roots.settings_file()).unwrap(),
            "[audio]\nmusic = 0.5\n"
        );
        // 玩家改了设置，发布包也换了默认值 → 用户文件必须原样保留。
        fs::write(roots.settings_file(), "[audio]\nmusic = 0.1\n").unwrap();
        fs::write(pack.join("settings.toml"), "[audio]\nmusic = 0.9\n").unwrap();
        assert!(
            !seed_settings(&roots, &pack).unwrap(),
            "已有用户文件不再播种"
        );
        assert_eq!(
            fs::read_to_string(roots.settings_file()).unwrap(),
            "[audio]\nmusic = 0.1\n",
            "升级不得覆盖玩家设置"
        );
        // 开发工程不播种：工程内的 settings.toml 就是玩家设置本身。
        let dev = temp_root("seeddev");
        fs::write(dev.join("project.toml"), "name = \"Demo\"\n").unwrap();
        fs::write(dev.join("settings.toml"), "[audio]\nmusic = 0.5\n").unwrap();
        let dev_roots = data_roots_at(&dev, false, &base);
        assert!(!seed_settings(&dev_roots, &dev).unwrap());
        let _ = fs::remove_dir_all(install_of(&pack));
        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&dev);
    }

    /// 包内没有 `settings.toml` 时不播种（不能凭空造文件）；存档目录按需创建；
    /// 应用名做文件名安全化（发布名含空格/非 ASCII 时不能生成有问题的路径）。
    #[test]
    fn seeding_is_skipped_without_defaults_and_save_dir_is_created() {
        let pack = install_pack("sanitize", "江湖 有田");
        let base = temp_root("sanitizebase");
        let roots = data_roots_at(&pack, false, &base);
        assert!(!seed_settings(&roots, &pack).unwrap(), "包内无默认设置");
        assert!(!roots.settings_file().exists());
        assert!(
            roots.app_name.is_ascii(),
            "应用名必须安全化: {}",
            roots.app_name
        );
        assert!(!roots.app_name.contains(' '));
        assert!(!roots.save_dir.exists(), "解析本身不建目录");
        ensure_save_dir(&roots).unwrap();
        assert!(roots.save_dir.is_dir());
        let _ = fs::remove_dir_all(install_of(&pack));
        let _ = fs::remove_dir_all(&base);
    }

    /// 环境变量覆盖（便携版分发 / CI）：`RUSTLUA_USER_DIR` 决定用户数据根。
    /// 这是**唯一**读写该环境变量的测试（其余测试一律走 `data_roots_at` 的显式
    /// 基目录），因此不会与并行测试互相踩。
    #[test]
    fn user_dir_env_var_overrides_the_platform_directory() {
        let pack = install_pack("env", "Demo");
        let base = temp_root("envbase");
        unsafe {
            env::set_var(USER_DIR_ENV, &base);
        }
        let roots = data_roots(&pack, false);
        assert_eq!(roots.save_dir, base.join("Demo"));
        assert_eq!(user_data_dir("Demo"), base.join("Demo"));
        unsafe {
            env::remove_var(USER_DIR_ENV);
        }
        let _ = fs::remove_dir_all(install_of(&pack));
        let _ = fs::remove_dir_all(&base);
    }

    /// 没有 `publish_name`/`name` 时退回默认应用名（不能让用户目录变成空路径）。
    #[test]
    fn missing_names_fall_back_to_default_app_name() {
        let root = temp_root("noname");
        fs::create_dir_all(root.join("pack")).unwrap();
        fs::write(root.join("release.toml"), "# empty\n").unwrap();
        assert_eq!(app_name(&root.join("pack")), DEFAULT_APP_NAME);
        assert_eq!(sanitize_component("   "), DEFAULT_APP_NAME);
        assert_eq!(sanitize_component("a b"), "a-b");
        let _ = fs::remove_dir_all(&root);
    }
}
