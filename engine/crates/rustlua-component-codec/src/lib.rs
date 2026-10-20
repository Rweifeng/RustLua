//! 场景组件载荷契约的**叶子 crate**：`ComponentCodec` trait 形态 + 注册表 + 装表入口。
//!
//! ## 为什么需要它（`docs/62` P1 第 4 条）
//!
//! 场景文本里的组件是一个**字符串契约**：`Sprite:texture=a.png;w=16;h=16`。这份文法的实现
//! 天然住在组件的**归属 crate**——`Sprite` 的字段语义只有 `rustlua-render` 说得清，
//! `Text` 的字号/颜色规则只有 `rustlua-ui` 说得清。但 `rustlua-scene` 是这两层的**上游**：
//! 它要能解析、校验、并把原文带进 `TypedScene`。于是过去它直接 `use rustlua_render::*`，
//! 换来一条 `scene → render/ui` 的编译边。
//!
//! 那条边的代价：`rustlua-scene` 是**几乎所有人都要编**的 crate（`assetc` 导场景、
//! `editor` 读场景、`game` 装世界），于是任何一次改 `ui` 的 Text 布局，都会把 scene 及其
//! 全部下游拖进同一次重编；反过来，`scene` 的测试也必须拉起整套渲染/UI。
//!
//! ## 三方怎么接起来
//!
//! ```text
//!    rustlua-render ─┐                        ┌─→ 靠 ComponentCodec 把"校验函数"交出来
//!    rustlua-ui ─────┼→ rustlua-component-codec（叶子：trait + 注册表 + 装表）
//!    rustlua-scene ──┘                        └─→ 查表校验，自己不认类型
//!                                  ↑
//!                         engine / editor 装一次表
//! ```
//!
//! 场景层只认**名字 + 一段不透明文本**：`ComponentId` 仍是场景层的知识（组件名是场景格式的
//! 一部分），载荷**校验**由归属 crate 的 [`ComponentCodec`] 负责，`TypedScene` 对这类组件存
//! `Opaque`（原文 + 解析出的资源引用），类型化消费方（渲染器、编辑器）拿原文回各自 crate 解析。
//!
//! ## 未装表时的语义：**硬错误**，不是静默跳过
//!
//! 与导入层注册表（少一个导入器 = 该类型报"不支持"）不同，也刻意不同于"未知组件留在源场景"
//! 的宽容：**已知名字但缺 codec** 说明程序装配漏了，此时把 `Sprite` 当未知组件丢掉，工程里
//! 的精灵会**无声消失**。所以查表失败要返回带指引的 [`CodecError`]，严格编译判红、
//! 宽容编译记一条诊断——两种编译都能被人看见。
//!
//! [`CodecError`]: 见 [`ComponentCodecRegistry::require`]
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// 一个表现层组件的编解码入口。
///
/// 只有函数指针，没有状态：codec 的实现必须是无副作用的纯解析，才能被注册表共享。
#[derive(Clone, Copy)]
pub struct ComponentCodec {
    name: &'static str,
    validate: fn(&str) -> Result<(), String>,
    asset_paths: fn(&str) -> Vec<String>,
}

impl ComponentCodec {
    /// 只有校验的 codec（组件不引用任何资源，如 `RenderMode`）。
    #[must_use]
    pub const fn new(name: &'static str, validate: fn(&str) -> Result<(), String>) -> Self {
        Self {
            name,
            validate,
            asset_paths: no_assets,
        }
    }

    /// 附带"这段载荷引用了哪些源资源路径"（`MeshRenderer.mesh`、`Text.font`…）。
    ///
    /// 场景层拿这些路径问 `AssetResolver` 要 `AssetId`，用于装载期依赖收集。
    /// 路径解析失败必须返回**空**而不是报错：资源缺失的可见性归资源库的依赖图，
    /// 场景编译不重复报同一件事。
    #[must_use]
    pub const fn with_assets(self, asset_paths: fn(&str) -> Vec<String>) -> Self {
        Self {
            asset_paths,
            ..self
        }
    }

    /// 组件名，与场景/Prefab 文件里写的那个名字逐字节相同。
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// 校验载荷；`Err` 是**诊断原文**（归属 crate 的解析器错误，含行内定位信息）。
    pub fn validate(&self, value: &str) -> Result<(), String> {
        (self.validate)(value)
    }

    /// 这段载荷引用的源资源路径（默认空）。
    #[must_use]
    pub fn asset_paths(&self, value: &str) -> Vec<String> {
        (self.asset_paths)(value)
    }
}

/// 不引用资源的默认实现。
fn no_assets(_value: &str) -> Vec<String> {
    Vec::new()
}

impl std::fmt::Debug for ComponentCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentCodec")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// 组件名 → codec 的不可变表。
#[derive(Clone, Debug, Default)]
pub struct ComponentCodecRegistry {
    map: BTreeMap<&'static str, &'static ComponentCodec>,
}

impl ComponentCodecRegistry {
    /// 空表：只认场景层自己的组件（Transform/Camera/Tilemap/…），表现层组件一律报"未装表"。
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// 从若干 codec 建表。**同名重复是错误**：静默后者覆盖前者会让两处实现各说一套。
    #[must_use]
    pub fn from_codecs(codecs: &[&'static ComponentCodec]) -> Self {
        let mut map = BTreeMap::new();
        for codec in codecs {
            let previous = map.insert(codec.name(), *codec);
            debug_assert!(
                previous.is_none(),
                "组件 codec 重名注册：{}（每个组件名只能有一个 codec）",
                codec.name()
            );
        }
        Self { map }
    }

    /// 查表；`None` = 没有为这个名字装 codec。
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&'static ComponentCodec> {
        self.map.get(name).copied()
    }

    /// 已装表的组件名（诊断与自检用）。
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.map.keys().copied()
    }

    /// 表里有多少条。
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// 空表判定。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// 查表，缺失时报**带指引**的错误。
    ///
    /// 场景层用这个而不是 `get(...).is_some()`：未装表是装配错误，必须让写工程的人一眼看到
    /// "该装哪两个函数"，而不是得到一句"未知组件"。
    pub fn require(&self, name: &str) -> Result<&'static ComponentCodec, CodecError> {
        self.get(name).ok_or_else(|| CodecError::NotInstalled {
            name: name.to_owned(),
        })
    }
}

/// 查表/装表相关的错误。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CodecError {
    /// 这个名字有已知的组件语义，但进程里没装它的 codec。
    #[error(
        "组件 `{name}` 的 codec 未安装：表现层组件的文法由归属 crate 提供，\
         需在组合根装一次表（engine/editor 启动时已装 \
         `rustlua_render::scene_codecs()` + `rustlua_ui::scene_codecs()`）"
    )]
    NotInstalled {
        /// 缺 codec 的组件名。
        name: String,
    },
}

/// 装表失败的原因。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CodecInstallError {
    /// 已经装过一张表。**不做合并**：合并会让"谁该装哪些 codec"变成隐式的，
    /// 而且两次装表的先后顺序决定结果——那是调试噩梦。
    #[error("组件 codec 已经装过一张表（len={installed}），不能重复装；请在组合根统一装一次")]
    AlreadyInstalled {
        /// 已装表里 codec 的条数。
        installed: usize,
    },
}

static GLOBAL: OnceLock<ComponentCodecRegistry> = OnceLock::new();

/// 装表。**只允许成功一次**，重复装返回 [`CodecInstallError::AlreadyInstalled`]。
///
/// 为什么不能重复装/合并：装表是"这个进程认识哪些表现层组件"的唯一答案。允许合并就会出现
/// "编辑器装了 12 个、游戏装了 12 个但少一个 Sprite"这种只在某个入口才复现的缺组件问题。
pub fn install(registry: ComponentCodecRegistry) -> Result<(), CodecInstallError> {
    GLOBAL
        .set(registry)
        .map_err(|previous| CodecInstallError::AlreadyInstalled {
            installed: previous.len(),
        })
}

/// 全局表。未装表时返回空表（于是表现层组件走"未装表"诊断，而不是 panic）。
#[must_use]
pub fn global() -> &'static ComponentCodecRegistry {
    static EMPTY: OnceLock<ComponentCodecRegistry> = OnceLock::new();
    GLOBAL
        .get()
        .unwrap_or_else(|| EMPTY.get_or_init(ComponentCodecRegistry::empty))
}

/// 是否已经装过表（自检/诊断用）。
#[must_use]
pub fn is_installed() -> bool {
    GLOBAL.get().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_codec(value: &str) -> Result<(), String> {
        if value.is_empty() {
            Err("空载荷".to_owned())
        } else {
            Ok(())
        }
    }

    fn mesh_assets(value: &str) -> Vec<String> {
        if value.starts_with("mesh=") {
            vec![value.trim_start_matches("mesh=").to_owned()]
        } else {
            Vec::new()
        }
    }

    static FAKE: ComponentCodec = ComponentCodec::new("Fake", ok_codec);
    static FAKE_MESH: ComponentCodec =
        ComponentCodec::new("FakeMesh", ok_codec).with_assets(mesh_assets);

    #[test]
    fn lookup_is_by_exact_name_and_unknown_is_none() {
        let registry = ComponentCodecRegistry::from_codecs(&[&FAKE, &FAKE_MESH]);
        assert_eq!(registry.len(), 2);
        assert!(!registry.is_empty());
        assert_eq!(registry.get("Fake").map(ComponentCodec::name), Some("Fake"));
        assert!(
            registry.get("fake").is_none(),
            "组件名大小写敏感：文件里写的就是查表的键"
        );
        assert!(registry.get("Sprite").is_none());
        assert_eq!(
            registry.names().collect::<Vec<_>>(),
            vec!["Fake", "FakeMesh"]
        );
    }

    #[test]
    fn empty_registry_answers_nothing() {
        let registry = ComponentCodecRegistry::empty();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.get("Sprite").is_none());
        assert_eq!(registry.names().count(), 0);
    }

    #[test]
    fn validate_and_assets_are_delegated_to_the_owner_crate() {
        let registry = ComponentCodecRegistry::from_codecs(&[&FAKE_MESH]);
        let codec = registry.get("FakeMesh").unwrap();
        assert!(codec.validate("x").is_ok());
        assert_eq!(codec.validate(""), Err("空载荷".to_owned()));
        assert_eq!(codec.asset_paths("mesh=a.glb"), vec!["a.glb".to_owned()]);
        // 不引用资源的 codec 默认返回空，不 panic。
        assert!(FAKE.asset_paths("whatever").is_empty());
    }

    #[test]
    fn global_registry_is_empty_before_install_or_after_a_single_install() {
        // 这条测试不装表（装了会影响同进程的其它测试）：只钉住"未装表 = 空表 + 不 panic"。
        if !is_installed() {
            assert!(global().is_empty());
            assert!(global().get("Sprite").is_none());
        }
    }
}
