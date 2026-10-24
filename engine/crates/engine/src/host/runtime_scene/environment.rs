//! P3-R03-4：把场景里的 `Environment` 组件解析成运行时环境数据，并应用到渲染器。
//!
//! 环境是**场景级**状态：组件挂在哪都行，`RuntimeScene` 取第一个合法声明。`.hdr` 在运行时
//! 映射到同名 `.rlenv`（导入期烘焙产物）；读取失败/格式坏掉时保留原因（编辑器/日志可读），
//! 环境项回退为"没有环境"而不是把坏数据塞进 GPU。

use std::path::Path;

use rustlua_assets::env_asset::EnvironmentAsset;
use rustlua_render::environment::EnvironmentComponent;
use rustlua_render::render_settings::{
    EnvironmentLighting, EnvironmentSettings, IblMode, IblTier, RenderSettings, ToggleMode,
};
use rustlua_render::runtime_gpu::EnvironmentRenderer;
use rustlua_scene::scene::Scene;

use super::RuntimeScene;

/// 场景环境组件的加载结果（`error` 非空 ⇒ 没有 lighting/asset）。
#[derive(Clone, Debug)]
pub struct RuntimeSceneEnvironment {
    /// 作者写的组件原文（诊断用）。
    pub source: String,
    /// 解析成功的组件取值；解析失败时为 `None`。
    pub component: Option<EnvironmentComponent>,
    /// 运行时实际读取的路径（`.hdr` → 同名 `.rlenv`）。
    pub resolved_path: String,
    pub asset: Option<EnvironmentAsset>,
    pub lighting: Option<EnvironmentLighting>,
    /// 首个失败原因；成功时为 `None`。
    pub error: Option<String>,
}

impl RuntimeSceneEnvironment {
    /// 编辑器/日志用的读数（不克隆纹理数据）。
    #[must_use]
    pub fn report(&self) -> EnvironmentReport {
        EnvironmentReport {
            source: self.source.clone(),
            map: self
                .component
                .as_ref()
                .map(|component| component.map.clone()),
            resolved_path: self.resolved_path.clone(),
            intensity: self
                .component
                .as_ref()
                .map_or(1.0, |component| component.intensity),
            rotation: self
                .component
                .as_ref()
                .map_or(0.0, |component| component.rotation),
            face_size: self.asset.as_ref().map_or(0, |asset| asset.face_size),
            mip_count: self.asset.as_ref().map_or(0, |asset| asset.mip_count()),
            error: self.error.clone(),
        }
    }
}

/// 环境组件的只读报告（编辑器 Inspector/性能面板/验收用）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnvironmentReport {
    pub source: String,
    pub map: Option<String>,
    pub resolved_path: String,
    pub intensity: f32,
    pub rotation: f32,
    pub face_size: u32,
    pub mip_count: u32,
    pub error: Option<String>,
}

impl EnvironmentReport {
    /// 人类可读的一行状态（日志/面板）。
    #[must_use]
    pub fn status_line(&self) -> String {
        match &self.error {
            Some(error) => format!("环境不可用（{}）：{error}", self.resolved_path),
            None => format!(
                "环境 {}（{} 面 {} mip，intensity={}，rotation={}）",
                self.resolved_path, self.face_size, self.mip_count, self.intensity, self.rotation
            ),
        }
    }
}

/// 场景里第一个 `Environment` 组件（按实体 id 排序，决定确定性）。
#[must_use]
pub fn environment_payload(scene: &Scene) -> Option<String> {
    scene
        .entities
        .values()
        .find_map(|entity| entity.components.get("Environment").cloned())
}

/// 解析 + 读取 + 解码。没有声明时返回 `None`；声明了但坏掉时返回带 `error` 的报告。
#[must_use]
pub fn load(project_root: Option<&Path>, scene: &Scene) -> Option<RuntimeSceneEnvironment> {
    let source = environment_payload(scene)?;
    let mut report = RuntimeSceneEnvironment {
        source: source.clone(),
        component: None,
        resolved_path: String::new(),
        asset: None,
        lighting: None,
        error: None,
    };
    let component = match EnvironmentComponent::from_component_str(&source) {
        Ok(component) => component,
        Err(error) => {
            report.error = Some(format!("Environment 组件无效：{error}"));
            return Some(report);
        }
    };
    report.resolved_path = component.resolved_rlenv_path();
    report.component = Some(component.clone());
    let Some(root) = project_root else {
        report.error = Some("没有工程根目录，无法读取环境资产".to_owned());
        return Some(report);
    };
    let absolute = root.join(&report.resolved_path);
    let bytes = read_bytes(&absolute).map_err(|error| error.to_string());
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => {
            report.error = Some(format!("读取 {} 失败：{error}", absolute.display()));
            return Some(report);
        }
    };
    let asset = match EnvironmentAsset::decode(&bytes) {
        Ok(asset) => asset,
        Err(error) => {
            report.error = Some(format!("{} 不是合法的 .rlenv：{error}", absolute.display()));
            return Some(report);
        }
    };
    let lighting = EnvironmentLighting {
        sh: asset.sh,
        settings: EnvironmentSettings {
            intensity: component.intensity,
            rotation: component.rotation,
        },
    };
    report.asset = Some(asset);
    report.lighting = Some(lighting);
    Some(report)
}

/// 桌面走文件系统、Web/发布包走 VFS（与工程脚本的存在性判断同一口径）。
fn read_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(fs_error) => rustlua_infra::vfs::read(path).map_err(|vfs_error| {
            std::io::Error::new(vfs_error.kind(), format!("{fs_error}; vfs: {vfs_error}"))
        }),
    }
}

/// 把场景环境应用到渲染器：声明了环境就打开 PBR/HDR/IBL，并优先上传 cubemap；
/// 上传失败或 Tier C 时回退 SH-only，且把原因留在报告里。
///
/// 没有声明 `Environment` 时**不动**渲染设置（保持宿主默认），只清掉上一次的环境，
/// 因此旧场景的像素不受影响。
pub fn apply_to_renderer(
    renderer: &mut impl EnvironmentRenderer,
    scene: &RuntimeScene,
    report: &EnvironmentReport,
) -> EnvironmentApplyReport {
    let mut apply = EnvironmentApplyReport {
        declared: report.map.is_some(),
        ibl_mode: renderer.ibl_mode(),
        tier: renderer.ibl_tier(),
        map_size: renderer.environment_map_info(),
        fallback_reason: None,
    };
    let Some(lighting) = scene.environment_lighting() else {
        renderer.clear_environment();
        apply.fallback_reason = report.error.clone();
        apply.ibl_mode = renderer.ibl_mode();
        return apply;
    };
    let settings = RenderSettings {
        hdr: ToggleMode::On,
        pbr: true,
        ibl: ToggleMode::On,
        variants: renderer.render_settings().variants,
    };
    renderer.set_render_settings(settings);
    let mut fallback = None;
    if let Some(asset) = scene.environment_asset() {
        let environment_settings = lighting.settings;
        if let Err(error) = renderer.set_environment_asset(asset, environment_settings) {
            fallback = Some(format!("环境 cubemap 上传失败（{error}），回退 SH-only"));
        }
    } else {
        fallback = Some("环境 cubemap 不可用，回退 SH-only".to_owned());
    }
    if fallback.is_some() {
        renderer.set_environment(lighting);
    }
    apply.ibl_mode = renderer.ibl_mode();
    apply.tier = renderer.ibl_tier();
    apply.map_size = renderer.environment_map_info();
    if !apply.tier.supports_specular() {
        let reason = format!(
            "Tier {}：Rgba16Float cubemap 不可用，镜面 IBL 关闭（保留 SH9 漫反射）",
            apply.tier.as_str()
        );
        fallback = Some(match fallback {
            Some(existing) => format!("{existing}；{reason}"),
            None => reason,
        });
    }
    apply.fallback_reason = fallback;
    apply
}

/// 一次环境应用后的实际档位读数。
#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentApplyReport {
    pub declared: bool,
    pub ibl_mode: IblMode,
    pub tier: IblTier,
    pub map_size: Option<(u32, u32)>,
    pub fallback_reason: Option<String>,
}

impl RuntimeScene {
    /// 场景声明的环境组件（解析成功时）。用于编辑器面板/验收读数。
    #[must_use]
    pub fn environment_component(&self) -> Option<&EnvironmentComponent> {
        self.environment.as_ref()?.component.as_ref()
    }

    /// 环境光照（SH + intensity/rotation）；资产坏掉时 `None`。
    #[must_use]
    pub fn environment_lighting(&self) -> Option<EnvironmentLighting> {
        self.environment.as_ref()?.lighting
    }

    /// 预过滤 cubemap 资产（供渲染器上传）。
    #[must_use]
    pub fn environment_asset(&self) -> Option<&EnvironmentAsset> {
        self.environment.as_ref()?.asset.as_ref()
    }

    /// 环境加载报告（含失败原因）；场景没有声明时 `None`。
    #[must_use]
    pub fn environment_report(&self) -> Option<EnvironmentReport> {
        self.environment
            .as_ref()
            .map(RuntimeSceneEnvironment::report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_scene::scene::SceneEntityId;

    fn game_root() -> std::path::PathBuf {
        rustlua_infra::games_dir().join("examples").join("game_api")
    }

    fn scene_with_environment(payload: &str) -> Scene {
        let mut scene = Scene::new("env-test");
        let entity = SceneEntityId(1);
        scene.add_entity(entity, "World", None).expect("add entity");
        scene
            .entities
            .get_mut(&entity)
            .unwrap()
            .components
            .insert("Environment".into(), payload.into());
        scene
    }

    /// 环境是场景级状态：组件挂在哪都行，多份声明按实体 id 取第一份（确定性）。
    #[test]
    fn environment_belongs_to_the_scene_and_first_id_wins() {
        let mut scene = Scene::new("two-envs");
        for (id, payload) in [
            (5, "map=assets/env/second.rlenv"),
            (2, "map=assets/env/first.rlenv"),
        ] {
            scene.add_entity(SceneEntityId(id), "Env", None).unwrap();
            scene
                .entities
                .get_mut(&SceneEntityId(id))
                .unwrap()
                .components
                .insert("Environment".into(), payload.into());
        }
        assert_eq!(
            environment_payload(&scene).as_deref(),
            Some("map=assets/env/first.rlenv")
        );
        assert!(environment_payload(&Scene::new("empty")).is_none());
    }

    /// `.hdr` 是作者写的来源路径，运行时读同名 `.rlenv`；SH 与强度/旋转原样进光照。
    #[test]
    fn load_reads_the_committed_rlenv_and_builds_lighting() {
        let root = game_root();
        let scene = scene_with_environment("map=assets/env/studio.hdr;intensity=1.5;rotation=0.25");
        let loaded = load(Some(&root), &scene).expect("declared environment");
        assert_eq!(loaded.error, None, "{:?}", loaded.error);
        let report = loaded.report();
        assert_eq!(report.resolved_path, "assets/env/studio.rlenv");
        assert_eq!(report.face_size, 128);
        assert_eq!(report.mip_count, 5);
        assert_eq!(report.intensity, 1.5);
        assert_eq!(report.rotation, 0.25);
        let asset = loaded.asset.expect("decoded asset");
        let lighting = loaded.lighting.expect("lighting");
        assert_eq!(lighting.sh, asset.sh);
        assert_eq!(lighting.settings.intensity, 1.5);
        assert_eq!(lighting.settings.rotation, 0.25);
        assert!(
            report.status_line().contains("studio.rlenv"),
            "{}",
            report.status_line()
        );
    }

    /// 缺文件/坏文件都只让环境项为空，组件与原因保留（编辑器能显示为什么没环境）。
    #[test]
    fn load_reports_missing_and_corrupt_assets_without_lighting() {
        let root = game_root();
        let missing = scene_with_environment("map=assets/env/does-not-exist.rlenv");
        let loaded = load(Some(&root), &missing).expect("declared environment");
        let error = loaded
            .error
            .as_deref()
            .expect("missing asset must be reported");
        assert!(error.contains("does-not-exist.rlenv"), "{error}");
        assert!(loaded.lighting.is_none());
        assert!(loaded.asset.is_none());
        assert!(loaded.component.is_some());
        assert!(
            loaded.report().status_line().contains("环境不可用"),
            "{}",
            loaded.report().status_line()
        );

        let temp = std::env::temp_dir().join(format!("rustlua-env-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(temp.join("assets/env")).expect("temp dir");
        std::fs::write(temp.join("assets/env/bad.rlenv"), b"not an rlenv").expect("write");
        let corrupt = scene_with_environment("map=assets/env/bad.rlenv");
        let loaded = load(Some(&temp), &corrupt).expect("declared environment");
        let error = loaded
            .error
            .as_deref()
            .expect("corrupt asset must be reported");
        assert!(error.contains("不是合法的 .rlenv"), "{error}");
        assert!(loaded.lighting.is_none());
        std::fs::remove_dir_all(&temp).ok();
    }

    #[test]
    fn load_without_project_root_explains_the_missing_root() {
        let scene = scene_with_environment("map=assets/env/studio.rlenv");
        let loaded = load(None, &scene).expect("declared environment");
        assert!(
            loaded
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("工程根目录")
        );
        assert!(loaded.lighting.is_none());
    }

    /// 坏组件是**可见错误**而不是静默忽略：报告里点名组件无效。
    #[test]
    fn invalid_component_is_reported_instead_of_silently_ignored() {
        let scene = scene_with_environment("map=assets/env/studio.png");
        let loaded = load(None, &scene).expect("declared environment");
        assert!(
            loaded
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("组件无效")
        );
        assert!(loaded.report().map.is_none());
    }

    /// 场景实例化的唯一入口必须把环境一并装进运行时（编辑器/游戏宿主只读这一份）。
    #[test]
    fn runtime_scene_surfaces_the_loaded_environment() {
        let root = game_root();
        let scene = scene_with_environment("map=assets/env/studio.rlenv;intensity=0.75");
        let runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime");
        let report = runtime.environment_report().expect("environment report");
        assert_eq!(report.error, None);
        assert_eq!(report.face_size, 128);
        assert!(runtime.environment_asset().is_some());
        assert_eq!(
            runtime
                .environment_component()
                .map(|component| component.intensity),
            Some(0.75)
        );
        assert_eq!(
            runtime
                .environment_lighting()
                .expect("lighting")
                .settings
                .intensity,
            0.75
        );
    }
}
