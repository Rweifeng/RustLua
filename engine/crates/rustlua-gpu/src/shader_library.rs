//! P3-R05-1：shader 源的**解析层**——内置源（编译期 `include_str!`）+ 开发期覆盖目录。
//!
//! 解析顺序只有一条：项目 `shaders/<名字>.wgsl` 存在就用它，否则用内置源。内置源永远是
//! 完整的，所以覆盖目录删空就等价于回到发布行为（这是本包的回滚开关）。
//!
//! 这一层刻意**不碰 GPU、也不做校验**：文件读出来是什么就是什么，`naga` 解析、绑定接口比对
//! 与变体重建在 R05-2/3 里由渲染器负责。分开的理由是热重载失败的语义在渲染器一侧（"保留旧
//! 管线并报原因"），解析层如果替它做决定就会出现"文件读到了但被悄悄丢掉"这种查不出来的状态。

use std::{
    borrow::Cow,
    ffi::OsStr,
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};

use super::shader::{ALL_SHADERS, ShaderSource, fnv1a64};

/// 项目里放 shader 覆盖文件的目录名（计划 §6.5）。编辑器监听根、运行时默认路径、
/// 报错文案都读这一个名字——两边各写一遍 `"shaders"` 是"改了没反应"这类问题的温床。
///
/// 权威定义在 `rustlua-infra::paths`：无 GPU 的编辑器领域层（文件监听）也要读它，
/// 这里只做再导出，保证调用方路径不变。
pub use rustlua_infra::paths::SHADER_SOURCE_DIR;

/// `--shader-dir` 之外的开关：环境变量（脚本与 CI 里比参数好注入）。
pub const SHADER_DIR_ENV: &str = "RUSTLUA_SHADER_DIR";

/// 覆盖目录的轮询节奏：编辑器与桌面运行时共用这个去抖窗口。
///
/// 为什么不是一次事件一次重载：一次保存会写出多个中间态（先截断再写入），而一次重载要校验
/// **全部**变体；把窗口内的事件并成一次，作者看到的就是"改完 → 一份结果"。
pub const SHADER_RELOAD_DEBOUNCE: Duration = Duration::from_millis(150);

/// 覆盖目录的解析口径（编辑器与运行时共用）：显式参数 > 环境变量 > 宿主默认。
///
/// 空白值一律算"没给"：`RUSTLUA_SHADER_DIR=` 这种设法是常态，把它当目录会让重载去读一个
/// 不存在（更糟：恰好叫空名）的根，而报错会长得像作者写错了 shader。
/// 两个宿主的差别只在 `fallback`：编辑器 = 项目 `shaders/`（始终监听），运行时 = `None`（不监听）。
#[must_use]
pub fn shader_override_dir(
    explicit: Option<PathBuf>,
    env_value: Option<&OsStr>,
    fallback: Option<PathBuf>,
) -> Option<PathBuf> {
    explicit
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| {
            env_value
                .and_then(OsStr::to_str)
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
        })
        .or(fallback)
}

/// 源最终从哪来。面板与日志要能区分这两者：Tier 一样的名字、不同的真相。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShaderOrigin {
    /// 编译期内嵌的仓库源。
    Builtin,
    /// 开发期覆盖目录里的文件（带绝对路径，报错要能定位到它）。
    Override { path: PathBuf },
}

impl ShaderOrigin {
    #[must_use]
    pub const fn is_override(&self) -> bool {
        matches!(self, Self::Override { .. })
    }
}

/// 一次解析的结果。`source` 用 `Cow` 是为了让内置路径零拷贝（覆盖路径才分配）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedShader {
    /// 稳定短名（与 [`ShaderSource::name`] 同一个口径）。
    pub name: &'static str,
    /// 仓库内源路径（`engine/crates/rustlua-gpu/shaders/*.wgsl`），日志与"覆盖文件应该叫什么"都用它。
    pub repo_path: &'static str,
    /// 源文本。
    pub source: Cow<'static, str>,
    /// 源文本的 64 位 FNV-1a——"这份源变没变"的唯一判据，同一哈希不重复编译。
    pub hash: u64,
    pub origin: ShaderOrigin,
}

impl ResolvedShader {
    fn builtin(shader: &'static ShaderSource) -> Self {
        Self {
            name: shader.name,
            repo_path: shader.path,
            source: Cow::Borrowed(shader.source),
            hash: shader.hash,
            origin: ShaderOrigin::Builtin,
        }
    }
}

/// 覆盖文件存在但读不出来。与"文件不存在"（正常回退内置）严格分开：前者是用户的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderOverrideError {
    pub name: String,
    pub path: PathBuf,
    pub reason: &'static str,
}

impl fmt::Display for ShaderOverrideError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "shader 覆盖文件 {}（{}）：{}",
            self.reason,
            self.path.display(),
            self.name
        )
    }
}

impl std::error::Error for ShaderOverrideError {}

/// 内置源 + 可选的开发期覆盖目录。
///
/// `override_dir == None` 时行为与 R04 完全一致（release/wasm 就是这个形态：没有入口，
/// 也就没有"带了覆盖目录"这种事故）。
#[derive(Clone, Debug)]
pub struct ShaderLibrary {
    override_dir: Option<PathBuf>,
}

impl ShaderLibrary {
    /// 只有内置源（默认/发布形态）。
    #[must_use]
    pub const fn builtin() -> Self {
        Self { override_dir: None }
    }

    /// 指定覆盖目录。目录不存在**不报错**：那是"还没写覆盖文件"，不是错误。
    #[must_use]
    pub fn with_override_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            override_dir: Some(dir.into()),
        }
    }

    #[must_use]
    pub fn override_dir(&self) -> Option<&Path> {
        self.override_dir.as_deref()
    }

    /// 某个 shader 的覆盖文件路径（即使该文件不存在也返回路径，供 watcher 与报错文案用）。
    #[must_use]
    pub fn override_path(&self, name: &str) -> Option<PathBuf> {
        let dir = self.override_dir.as_ref()?;
        let shader = ALL_SHADERS.iter().find(|shader| shader.name == name)?;
        let file_name = Path::new(shader.path)
            .file_name()
            .map(|name| name.to_owned())?;
        Some(dir.join(file_name))
    }

    /// 按稳定短名解析。未知名字返回 `None`（调用方据此报错，绝不静默换一份源）。
    ///
    /// 覆盖文件读失败/不是 UTF-8/为空都返回 `Err`：**不**回退内置源。热重载拿到 `Err` 时必须
    /// 保留旧管线并显示原因，否则作者会看到"我改了没生效、也没报错"。
    #[must_use = "返回值决定实际生效的 shader 源，丢掉它等于静默忽略覆盖文件"]
    pub fn resolve(&self, name: &str) -> Result<Option<ResolvedShader>, ShaderOverrideError> {
        let Some(shader) = ALL_SHADERS.iter().find(|shader| shader.name == name) else {
            return Ok(None);
        };
        let Some(path) = self.override_path(name) else {
            return Ok(Some(ResolvedShader::builtin(shader)));
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Some(ResolvedShader::builtin(shader)));
            }
            Err(_) => {
                return Err(self.override_error(name, &path, "读取失败"));
            }
        };
        if text.trim().is_empty() {
            return Err(self.override_error(name, &path, "内容为空"));
        }
        Ok(Some(ResolvedShader {
            name: shader.name,
            repo_path: shader.path,
            hash: fnv1a64(text.as_bytes()),
            source: Cow::Owned(text),
            origin: ShaderOrigin::Override { path },
        }))
    }

    /// 全部 shader 的解析结果（顺序与注册表一致）。启动期用它做一次"覆盖集扫描"，
    /// 让坏覆盖文件在第一次渲染前就报出来，而不是渲染到某个 pass 才发现。
    pub fn resolve_all(&self) -> Result<Vec<ResolvedShader>, ShaderOverrideError> {
        let mut out = Vec::with_capacity(ALL_SHADERS.len());
        for shader in ALL_SHADERS {
            if let Some(resolved) = self.resolve(shader.name)? {
                out.push(resolved);
            }
        }
        Ok(out)
    }

    fn override_error(&self, name: &str, path: &Path, reason: &'static str) -> ShaderOverrideError {
        ShaderOverrideError {
            name: name.to_owned(),
            path: path.to_path_buf(),
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shader::{MESH3D, MESH3D_PBR, TONEMAP};

    fn temp_dir(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let unique = format!(
            "rustlua-shader-library-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        dir.push(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// R05-1 的出口门：没有覆盖目录时，解析结果与内置源**逐字节**相同。
    #[test]
    fn builtin_library_is_byte_identical_to_the_embedded_sources() {
        let library = ShaderLibrary::builtin();
        for shader in ALL_SHADERS {
            let resolved = library.resolve(shader.name).unwrap().expect("已注册的名字");
            assert_eq!(resolved.source, shader.source, "{} 源漂移", shader.name);
            assert_eq!(resolved.hash, shader.hash, "{} 哈希漂移", shader.name);
            assert_eq!(resolved.origin, ShaderOrigin::Builtin);
            assert_eq!(resolved.repo_path, shader.path);
        }
        assert_eq!(library.resolve_all().unwrap().len(), ALL_SHADERS.len());
    }

    /// R05-4：覆盖目录的解析优先级（编辑器与运行时都读这条，所以只在引擎侧钉一次）。
    #[test]
    fn shader_override_dir_prefers_the_flag_then_the_env() {
        let flag = PathBuf::from("flag-dir");
        let env = PathBuf::from("env-dir");
        let project = PathBuf::from("project").join(SHADER_SOURCE_DIR);
        assert_eq!(
            shader_override_dir(
                Some(flag.clone()),
                Some(env.as_os_str()),
                Some(project.clone())
            ),
            Some(flag.clone()),
            "显式参数必须压过环境变量"
        );
        assert_eq!(
            shader_override_dir(None, Some(env.as_os_str()), Some(project.clone())),
            Some(env),
            "没有参数时环境变量生效（脚本/CI 的注入方式）"
        );
        assert_eq!(
            shader_override_dir(None, None, Some(project.clone())),
            Some(project.clone()),
            "编辑器的默认是项目 shaders/"
        );
        assert_eq!(
            shader_override_dir(None, None, None),
            None,
            "运行时的默认是没有：不监听 = 内置源"
        );
        // 空白环境变量算"没给"，而不是去读一个空名字目录。
        assert_eq!(
            shader_override_dir(None, Some(OsStr::new("   ")), Some(project.clone())),
            Some(project),
            "`RUSTLUA_SHADER_DIR=` 不该把默认顶掉"
        );
        assert_eq!(shader_override_dir(Some(PathBuf::new()), None, None), None);
    }

    /// 覆盖目录存在但里面没有这个 shader → 仍然内置（"只覆盖一份"是常态）。
    #[test]
    fn missing_override_file_falls_back_to_builtin_without_error() {
        let dir = temp_dir("missing");
        std::fs::write(
            dir.join("tonemap.wgsl"),
            "// 只有 tonemap 有覆盖\nfn main() {}\n",
        )
        .unwrap();
        let library = ShaderLibrary::with_override_dir(&dir);
        let mesh = library.resolve("mesh3d").unwrap().unwrap();
        assert_eq!(mesh.origin, ShaderOrigin::Builtin);
        assert_eq!(mesh.source, MESH3D.source);
        let tonemap = library.resolve("tonemap").unwrap().unwrap();
        assert!(tonemap.origin.is_override(), "覆盖文件必须被采纳");
        assert_eq!(tonemap.source, "// 只有 tonemap 有覆盖\nfn main() {}\n");
        assert_eq!(tonemap.hash, fnv1a64(tonemap.source.as_bytes()));
    }

    /// 覆盖文件名由仓库路径的 file_name 决定，所以 `mesh3d` 与 `mesh3d_pbr` 不会互相覆盖。
    #[test]
    fn override_path_uses_the_repository_file_name() {
        let library = ShaderLibrary::with_override_dir("shaders");
        assert_eq!(
            library.override_path("mesh3d").unwrap(),
            Path::new("shaders/mesh3d.wgsl")
        );
        assert_eq!(
            library.override_path("mesh3d_pbr").unwrap(),
            Path::new("shaders/mesh3d_pbr.wgsl")
        );
        assert_eq!(
            library.override_path("tonemap").unwrap(),
            Path::new("shaders/tonemap.wgsl")
        );
        assert!(library.override_path("nope").is_none(), "未知名字没有路径");
        assert!(
            ShaderLibrary::builtin().override_path("mesh3d").is_none(),
            "没有覆盖目录就没有覆盖路径"
        );
    }

    /// 坏覆盖文件是**错误**而不是静默回退：否则"改了没反应、也没报错"无法排查。
    #[test]
    fn unreadable_or_empty_override_is_an_error_not_a_silent_fallback() {
        let dir = temp_dir("broken");
        std::fs::write(dir.join("mesh3d.wgsl"), "   \n\t\n").unwrap();
        let library = ShaderLibrary::with_override_dir(&dir);
        let error = library.resolve("mesh3d").expect_err("空覆盖文件必须报错");
        assert_eq!(error.name, "mesh3d");
        assert_eq!(error.reason, "内容为空");
        assert!(error.path.ends_with("mesh3d.wgsl"));
        assert!(error.to_string().contains(&dir.display().to_string()));

        std::fs::write(dir.join("mesh3d_pbr.wgsl"), [0xff_u8, 0xfe, 0xfd]).unwrap();
        let error = library
            .resolve("mesh3d_pbr")
            .expect_err("非 UTF-8 必须报错");
        assert_eq!(error.reason, "读取失败");
        assert_eq!(
            library.resolve("tonemap").unwrap().unwrap().hash,
            TONEMAP.hash,
            "坏一份不影响其它 shader"
        );
    }

    /// 未知名字返回 `None`（而不是内置里"最接近的那份"）。
    #[test]
    fn unknown_name_is_none() {
        let library = ShaderLibrary::builtin();
        assert!(library.resolve("Mesh3D").unwrap().is_none());
        assert!(library.resolve("").unwrap().is_none());
    }

    /// 同一份内容哈希稳定、改一个字节就变——热重载的"要不要重编译"判据。
    #[test]
    fn content_hash_tracks_a_single_byte_change() {
        let dir = temp_dir("hash");
        let path = dir.join("mesh3d.wgsl");
        let library = ShaderLibrary::with_override_dir(&dir);
        std::fs::write(&path, "fn main() { }\n").unwrap();
        let first = library.resolve("mesh3d").unwrap().unwrap();
        let second = library.resolve("mesh3d").unwrap().unwrap();
        assert_eq!(first.hash, second.hash, "同一内容不该重复触发编译");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let third = library.resolve("mesh3d").unwrap().unwrap();
        assert_ne!(first.hash, third.hash);
        assert_ne!(third.hash, MESH3D_PBR.hash, "覆盖源与内置源各自独立");
    }
}
