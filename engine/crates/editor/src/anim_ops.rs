//! P3-E01-4：`.anim` 写盘——工程内路径约束 + 临时文件原子替换。
//!
//! `AnimationPlayer.clip=` 是工程相对路径。这里统一解析与落盘，避免面板把文件写到工程外：
//!
//! - 只接受 `assets/...` 这样的普通相对路径；绝对路径、盘符、`..`、`.`、空路径一律拒绝；
//! - 扩展名必须是 `.anim`（防止误把场景/材质文本写进别的资产）；
//! - 先写同目录的 `*.anim.tmp`，再原子替换目标；替换失败时清理临时文件，旧文件保持原样。

use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum AnimSaveError {
    #[error("动画片段路径必须是工程内相对路径，got `{0}`")]
    InvalidPath(String),
    #[error("动画片段必须使用 .anim 扩展名，got `{0}`")]
    NotAnim(String),
    #[error("写动画片段 `{path}` 失败：{source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// 解析工程内相对路径。调用方拿到的路径保证在 `root` 之下。
pub fn resolve_asset_path(root: &Path, relative: &str) -> Result<PathBuf, AnimSaveError> {
    let candidate = Path::new(relative);
    let is_plain_relative = !relative.is_empty()
        && relative.trim() == relative
        && candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if !is_plain_relative {
        return Err(AnimSaveError::InvalidPath(relative.to_owned()));
    }
    Ok(root.join(candidate))
}

/// 把规范文本原子写入工程内的 `.anim` 文件，返回最终绝对路径。
pub fn save_clip(root: &Path, relative: &str, text: &str) -> Result<PathBuf, AnimSaveError> {
    let path = resolve_asset_path(root, relative)?;
    let is_anim = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("anim"));
    if !is_anim {
        return Err(AnimSaveError::NotAnim(relative.to_owned()));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| AnimSaveError::InvalidPath(relative.to_owned()))?;
    std::fs::create_dir_all(parent).map_err(|source| AnimSaveError::Io {
        path: parent.display().to_string(),
        source,
    })?;
    let temp = path.with_extension("anim.tmp");
    if let Err(source) = std::fs::write(&temp, text) {
        let _ = std::fs::remove_file(&temp);
        return Err(AnimSaveError::Io {
            path: temp.display().to_string(),
            source,
        });
    }
    if let Err(source) = std::fs::rename(&temp, &path) {
        let _ = std::fs::remove_file(&temp);
        return Err(AnimSaveError::Io {
            path: path.display().to_string(),
            source,
        });
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const CLIP: &str = "\
version=1
name=hero
duration=1
looped=false
track=Transform.position;interp=linear;keys=0:0,0;1:10,0
";

    fn temp_root(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "rustlua-anim-ops-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    #[test]
    fn save_clip_writes_parseable_text_and_leaves_no_temp() {
        let root = temp_root("roundtrip");
        let path = save_clip(&root, "assets/anim/hero.anim", CLIP).expect("save");
        assert_eq!(path, root.join("assets/anim/hero.anim"));
        let text = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(text, CLIP);
        rustlua_animation::anim_asset::AnimClip::from_text("assets/anim/hero.anim", &text)
            .expect("saved clip parses");
        assert!(
            !path.with_extension("anim.tmp").exists(),
            "临时文件必须被替换掉"
        );
    }

    #[test]
    fn save_clip_replaces_existing_content() {
        let root = temp_root("replace");
        let first = CLIP;
        let second = CLIP.replace("name=hero", "name=hero_v2");
        save_clip(&root, "assets/anim/hero.anim", first).expect("first save");
        save_clip(&root, "assets/anim/hero.anim", &second).expect("second save");
        assert_eq!(
            std::fs::read_to_string(root.join("assets/anim/hero.anim")).expect("read"),
            second
        );
    }

    #[test]
    fn rejects_paths_outside_the_project_or_with_foreign_extensions() {
        let root = temp_root("reject");
        let old = CLIP;
        save_clip(&root, "assets/anim/hero.anim", old).expect("seed");

        for relative in [
            "../escape.anim",
            "assets/../../escape.anim",
            "assets\\..\\escape.anim",
            "",
            " assets/anim/hero.anim",
        ] {
            assert!(
                matches!(
                    resolve_asset_path(&root, relative),
                    Err(AnimSaveError::InvalidPath(_))
                ),
                "必须拒绝 `{relative}`"
            );
        }
        let absolute = root.join("assets/anim/absolute.anim");
        assert!(matches!(
            resolve_asset_path(&root, &absolute.to_string_lossy()),
            Err(AnimSaveError::InvalidPath(_))
        ));
        assert!(matches!(
            save_clip(&root, "assets/anim/hero.txt", old),
            Err(AnimSaveError::NotAnim(_))
        ));
        assert!(!absolute.exists());
        assert_eq!(
            std::fs::read_to_string(root.join("assets/anim/hero.anim")).expect("read"),
            old,
            "被拒绝的写盘不得触碰已有文件"
        );
    }

    #[test]
    fn creates_missing_asset_directories() {
        let root = temp_root("mkdir");
        let path = save_clip(&root, "assets/anim/new/clip.anim", CLIP).expect("save");
        assert!(path.is_file());
    }
}
