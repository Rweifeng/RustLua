//! `.sgraph` 资源读写：工程内路径约束、校验和原子替换。

use rustlua_visual_script::shader_graph::ShaderGraph;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ShaderGraphIoError {
    #[error("Shader Graph 路径必须是工程内相对路径：{0}")]
    InvalidPath(String),
    #[error("Shader Graph 必须使用 .sgraph 扩展名：{0}")]
    NotShaderGraph(String),
    #[error("Shader Graph 校验失败：{0}")]
    Invalid(String),
    #[error("{action} `{path}` 失败：{source}")]
    Io {
        action: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
}

pub fn resolve_shader_graph_path(
    root: &Path,
    relative: &str,
) -> Result<PathBuf, ShaderGraphIoError> {
    let candidate = Path::new(relative);
    let valid = !relative.is_empty()
        && relative.trim() == relative
        && candidate
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if !valid {
        return Err(ShaderGraphIoError::InvalidPath(relative.to_owned()));
    }
    let ext = candidate
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or_default();
    if !ext.eq_ignore_ascii_case("sgraph") {
        return Err(ShaderGraphIoError::NotShaderGraph(relative.to_owned()));
    }
    Ok(root.join(candidate))
}

pub fn load_shader_graph(root: &Path, relative: &str) -> Result<ShaderGraph, ShaderGraphIoError> {
    let path = resolve_shader_graph_path(root, relative)?;
    let text = std::fs::read_to_string(&path).map_err(|source| ShaderGraphIoError::Io {
        action: "读取 Shader Graph",
        path: path.display().to_string(),
        source,
    })?;
    ShaderGraph::from_json(&text).map_err(|error| ShaderGraphIoError::Invalid(error.to_string()))
}

pub fn save_shader_graph(
    root: &Path,
    relative: &str,
    graph: &ShaderGraph,
) -> Result<PathBuf, ShaderGraphIoError> {
    let path = resolve_shader_graph_path(root, relative)?;
    graph
        .validate()
        .map_err(|error| ShaderGraphIoError::Invalid(error.to_string()))?;
    let text = graph.to_json();
    let parent = path
        .parent()
        .ok_or_else(|| ShaderGraphIoError::InvalidPath(relative.to_owned()))?;
    std::fs::create_dir_all(parent).map_err(|source| ShaderGraphIoError::Io {
        action: "创建 Shader Graph 目录",
        path: parent.display().to_string(),
        source,
    })?;
    let temp = path.with_extension("sgraph.tmp");
    if let Err(source) = std::fs::write(&temp, &text) {
        let _ = std::fs::remove_file(&temp);
        return Err(ShaderGraphIoError::Io {
            action: "写 Shader Graph",
            path: temp.display().to_string(),
            source,
        });
    }
    if let Err(source) = std::fs::rename(&temp, &path) {
        let _ = std::fs::remove_file(&temp);
        return Err(ShaderGraphIoError::Io {
            action: "替换 Shader Graph",
            path: path.display().to_string(),
            source,
        });
    }
    Ok(path)
}

/// 把生成物 WGSL 导出到工程内 `.wgsl`，供审阅、接管线或移植到别的渲染器。
///
/// 注意：**它不会自动生效**。运行时的 shader 覆盖只认 `shaders/<内置文件名>.wgsl`
/// （`ShaderLibrary` 的解析口径），导出文件放在 `assets/fx/` 下是"可审阅产物"而不是
/// "已生效着色器"；把它接成真正的自定义材质需要渲染器侧的新 pass。
pub fn save_shader_source(
    root: &Path,
    relative: &str,
    source: &str,
) -> Result<PathBuf, ShaderGraphIoError> {
    let candidate = Path::new(relative);
    let valid = !relative.is_empty()
        && relative.trim() == relative
        && candidate
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if !valid {
        return Err(ShaderGraphIoError::InvalidPath(relative.to_owned()));
    }
    let ext = candidate
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or_default();
    if !ext.eq_ignore_ascii_case("wgsl") {
        return Err(ShaderGraphIoError::NotShaderGraph(relative.to_owned()));
    }
    if source.trim().is_empty() {
        return Err(ShaderGraphIoError::Invalid("生成的 WGSL 为空".into()));
    }
    let path = root.join(candidate);
    let parent = path
        .parent()
        .ok_or_else(|| ShaderGraphIoError::InvalidPath(relative.to_owned()))?;
    std::fs::create_dir_all(parent).map_err(|source| ShaderGraphIoError::Io {
        action: "创建 WGSL 目录",
        path: parent.display().to_string(),
        source,
    })?;
    let temp = path.with_extension("wgsl.tmp");
    if let Err(source) = std::fs::write(&temp, source) {
        let _ = std::fs::remove_file(&temp);
        return Err(ShaderGraphIoError::Io {
            action: "写 WGSL",
            path: temp.display().to_string(),
            source,
        });
    }
    if let Err(source) = std::fs::rename(&temp, &path) {
        let _ = std::fs::remove_file(&temp);
        return Err(ShaderGraphIoError::Io {
            action: "替换 WGSL",
            path: path.display().to_string(),
            source,
        });
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_visual_script::shader_graph::ShaderTemplate;
    #[test]
    fn save_load_round_trip_is_atomic() {
        let root = std::env::temp_dir().join(format!("rustlua-sgraph-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let graph = ShaderTemplate::Hologram.graph();
        let path = save_shader_graph(&root, "assets/fx/hologram.sgraph", &graph).unwrap();
        assert_eq!(
            load_shader_graph(&root, "assets/fx/hologram.sgraph").unwrap(),
            graph
        );
        assert!(!path.with_extension("sgraph.tmp").exists());
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn rejects_escape_and_bad_extension() {
        let root = Path::new("C:/project");
        assert!(matches!(
            resolve_shader_graph_path(root, "../x.sgraph"),
            Err(ShaderGraphIoError::InvalidPath(_))
        ));
        assert!(matches!(
            resolve_shader_graph_path(root, "x.material"),
            Err(ShaderGraphIoError::NotShaderGraph(_))
        ));
    }

    #[test]
    fn exports_generated_wgsl_and_rejects_wrong_extension() {
        let root = std::env::temp_dir().join(format!("rustlua-wgsl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let wgsl = "fn shader_graph_color() -> vec4<f32> { return vec4<f32>(1.0); }\n";
        let path = save_shader_source(&root, "assets/fx/demo.wgsl", wgsl).expect("export");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), wgsl);
        assert!(!path.with_extension("wgsl.tmp").exists());
        assert!(matches!(
            save_shader_source(&root, "assets/fx/demo.sgraph", wgsl),
            Err(ShaderGraphIoError::NotShaderGraph(_))
        ));
        assert!(matches!(
            save_shader_source(&root, "../escape.wgsl", wgsl),
            Err(ShaderGraphIoError::InvalidPath(_))
        ));
        let _ = std::fs::remove_dir_all(root);
    }
}
