//! 表现层组件 codec 的**装表入口**（`docs/62` P1 第 4 条）。
//!
//! `rustlua-scene` 不再依赖 `rustlua-render` / `rustlua-ui`：它把 `Sprite`/`Text` 这类组件的
//! 载荷当**不透明文本**搬运，合法性交给归属 crate 的 codec 校验（见
//! `rustlua_component_codec` 的模块文档）。那些 codec 由组合根装一次表。
//!
//! 本模块就是那个"组合根的一处"：`engine` 同时看见渲染层与 UI 层，`editor` 又依赖 `engine`，
//! 所以双方都调这里，**不会各装一套**（重复装会被注册表拒绝，本函数因此做成幂等：
//! 第二次起直接返回 `false`）。
//!
//! 谁需要调它：任何会**编译场景文本**的入口——`RuntimeScene::init`（场景实例化的唯一入口，
//! 预制体套用与运行时校验都在下游）、编辑器场景视图、`validate_component_payload` 的调用方、
//! 跑场景校验的 examples/tests。不编译场景的进程（纯运行时帧循环）不调也行：
//! 它读的是 ECS 里的类型化组件，不是字符串。

/// 期望装进全局表的 codec（渲染层 6 个 + UI 层 15 个 = 21 个）。
fn expected() -> Vec<&'static rustlua_component_codec::ComponentCodec> {
    let mut all: Vec<&'static rustlua_component_codec::ComponentCodec> =
        rustlua_render::scene_codec::scene_codecs().to_vec();
    all.extend_from_slice(rustlua_ui::scene_codec::scene_codecs());
    all
}

/// 装表结果：`installed_now=false` 表示**别的入口已经装过**且内容完整（幂等成功，不是错误）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstallReport {
    pub installed_now: bool,
    pub codecs: usize,
}

/// 装表失败的原因。与 [`rustlua_component_codec::CodecInstallError`] 分开：本函数还要能
/// 报"表被装过但缺项"——那说明某个入口先装了一张不完整的表，继续跑会把缺 codec 的组件
/// 静默丢掉。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InstallError {
    #[error("组件 codec 表拒绝安装：{0}")]
    Rejected(String),
    #[error(
        "已装表的 codec 不完整，缺 {missing:?}：场景里的这些组件会被静默丢弃。\
         全局表只允许在组合根装一次完整表"
    )]
    Incomplete { missing: Vec<&'static str> },
}

/// 确保表现层 codec 表已装且**完整**，把"装没装成"变成可观察的返回值。
///
/// `RuntimeScene::init` 用这个而不是 [`install`]：装表失败继续往下跑，表现为场景里
/// `Sprite`/`UiLayout` 等组件无声消失——那正是要消灭的静默回退。
pub fn ensure_installed() -> Result<InstallReport, InstallError> {
    // 检查与装表必须是一个原子动作：并行测试（以及并行编辑器入口）会同时看到"未装"，
    // 后到的那次被注册表拒绝，于是完整表反而报成装表失败。锁只在这一小段持有。
    static GUARD: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _guard = GUARD
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let expected = expected();
    if rustlua_scene::component_codec::is_installed() {
        let installed = rustlua_scene::component_codec::global();
        let missing: Vec<&'static str> = expected
            .iter()
            .map(|codec| codec.name())
            .filter(|name| installed.get(name).is_none())
            .collect();
        if !missing.is_empty() {
            return Err(InstallError::Incomplete { missing });
        }
        return Ok(InstallReport {
            installed_now: false,
            codecs: installed.len(),
        });
    }
    let registry = rustlua_component_codec::ComponentCodecRegistry::from_codecs(&expected);
    match rustlua_component_codec::install(registry) {
        Ok(()) => Ok(InstallReport {
            installed_now: true,
            codecs: expected.len(),
        }),
        Err(error) => Err(InstallError::Rejected(error.to_string())),
    }
}

/// 装一次表现层组件 codec 表（渲染层 6 个 + UI 层 15 个 = 21 个）。
///
/// 幂等：已经装过（或别的入口先装了）就返回 `false`，不报错。**不要**用它来检测"是不是我装的"
/// ——需要那个语义时用 `rustlua_scene::component_codec::is_installed()`。
///
/// 失败时不再静默：错误会以 `error` 级别写进日志（`RuntimeScene::init` 则会直接失败）。
pub fn install() -> bool {
    match ensure_installed() {
        Ok(report) => report.installed_now,
        Err(error) => {
            tracing::error!(target: "scene", error = %error, "[scene] component codec install failed");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 装表必须**幂等**：多个入口（编辑器主窗口 + 场景视图 + 校验器）都会调，
    /// 若第二次就报错，装配顺序会变成隐性依赖。
    #[test]
    fn install_is_idempotent() {
        let first = install();
        let second = install();
        assert!(!second, "第二次装表必须返回 false（幂等）");
        let _ = first;
        assert!(rustlua_scene::component_codec::is_installed());
        assert_eq!(
            rustlua_scene::component_codec::global().len(),
            21,
            "渲染层 6 个 + UI 层 15 个 = 21（M80-04 新增 Toggle/Slider/Progress/Scrollbar；\
             ScrollView 为 UI Studio 导出链补齐）"
        );
    }

    /// 可观察入口：不管本次是不是自己装的，都必须报告**完整**表；缺项要报错而不是 false。
    #[test]
    fn ensure_installed_reports_a_complete_table() {
        let report = ensure_installed().expect("完整表必须通过");
        assert_eq!(report.codecs, 21);
        let names: Vec<&'static str> = expected().iter().map(|codec| codec.name()).collect();
        assert_eq!(names.len(), 21);
        for name in [
            "Sprite",
            "Text",
            "UiLayout",
            "Toggle",
            "Slider",
            "Progress",
            "Scrollbar",
            "ScrollView",
        ] {
            assert!(names.contains(&name), "期望表缺 {name}: {names:?}");
        }
    }

    /// 装表之后，带表现层组件的场景文本必须能编译（这条是端到端的：codec 真的接上了）。
    #[test]
    fn a_scene_with_sprite_and_text_compiles_after_install() {
        install();
        let mut scene = rustlua_scene::scene::Scene::new("codec-smoke");
        let id = rustlua_scene::scene::SceneEntityId(1);
        scene.add_entity(id, "hero", None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        entity
            .components
            .insert("Sprite".to_owned(), "texture=a.png;size=8x8".to_owned());
        entity
            .components
            .insert("Text".to_owned(), "text=Hi;font_size=24".to_owned());
        let typed = rustlua_scene::typed_scene::TypedScene::compile_v2(&scene)
            .expect("装表后 Sprite/Text 必须能编译");
        assert_eq!(typed.entities[0].components.len(), 2);
    }
}
