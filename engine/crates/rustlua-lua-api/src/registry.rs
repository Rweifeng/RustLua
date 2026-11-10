use mlua::{Lua, Result as LuaResult};
use std::cell::Cell;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

pub const API_PACK_VERSION: u32 = 1;

/// 一个 Lua 全局模块的装配函数（领域 crate 提供，组合根登记）。
///
/// 为什么是闭包而不是 trait：安装器要**捕获句柄**（`Handle<T>` 的克隆），闭包天然能做，
/// 而 trait 对象还得额外定义一个带泛型参数的实现类型。这里不需要多态——注册表只负责
/// 按名字收集、按顺序调用。
pub type Installer = Box<dyn Fn(&Lua) -> LuaResult<()>>;

/// API 包声明所属维度。它用于组合、诊断和依赖审查，不会代替 Cargo feature 做代码裁剪。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiDimension {
    Shared,
    TwoD,
    ThreeD,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ApiSurface {
    /// 安全基线：共享 + 2D API，不允许装入 3D 扩展包。
    #[default]
    TwoD,
    /// 3D API 面也包含共享和 2D 基础 API。
    ThreeD,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiPackInfo {
    pub id: String,
    pub version: String,
    pub api_version: u32,
    pub dimension: ApiDimension,
    pub dependencies: Vec<String>,
    pub modules: Vec<&'static str>,
}

/// 一个可静态链接的 Lua API 扩展包。
///
/// 包只提供模块安装器，不持有 VM 或全局 ECS 状态；安装器通过闭包显式捕获宿主授权的句柄。
pub struct ApiPack {
    id: String,
    version: String,
    api_version: u32,
    dimension: ApiDimension,
    dependencies: Vec<String>,
    modules: Vec<(&'static str, Installer)>,
}

impl ApiPack {
    #[must_use]
    pub fn new(id: impl Into<String>, version: impl Into<String>, dimension: ApiDimension) -> Self {
        Self {
            id: id.into(),
            version: version.into(),
            api_version: API_PACK_VERSION,
            dimension,
            dependencies: Vec::new(),
            modules: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_api_version(mut self, api_version: u32) -> Self {
        self.api_version = api_version;
        self
    }

    #[must_use]
    pub fn with_dependency(mut self, id: impl Into<String>) -> Self {
        self.dependencies.push(id.into());
        self
    }

    pub fn module(
        &mut self,
        name: &'static str,
        install: impl Fn(&Lua) -> LuaResult<()> + 'static,
    ) {
        self.modules.push((name, Box::new(install)));
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub const fn api_version(&self) -> u32 {
        self.api_version
    }

    #[must_use]
    pub const fn dimension(&self) -> ApiDimension {
        self.dimension
    }

    #[must_use]
    pub fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    #[must_use]
    pub fn module_names(&self) -> Vec<&'static str> {
        self.modules.iter().map(|(name, _)| *name).collect()
    }

    #[must_use]
    pub fn info(&self) -> ApiPackInfo {
        ApiPackInfo {
            id: self.id.clone(),
            version: self.version.clone(),
            api_version: self.api_version,
            dimension: self.dimension,
            dependencies: self.dependencies.clone(),
            modules: self.module_names(),
        }
    }
}

/// 绑定集合的注册表：**组合根（engine）填，运行时（scripting）消费**。
///
/// 这是依赖倒置的落点：`rustlua-scripting` 从"自己 `use` 13 个领域 crate 并逐个 register"
/// 变成"拿一个注册表，按名字装"。谁提供绑定由组合根决定，VM 不再认识具体 crate。
///
/// 名字是**稳定标识**而不是装饰：重复登记直接报错（同一个全局表被装两遍，Lua 里后者静默
/// 覆盖前者，症状是"某个函数行为变了但没人改它"）。
#[derive(Default)]
pub struct Registry {
    entries: Vec<(&'static str, Installer)>,
    packs: Vec<ApiPack>,
    surface: ApiSurface,
    /// 宿主工程根目录（`settings.toml` 所在目录）。
    ///
    /// 为什么放在这里：设置持久化类绑定（`audio.load_settings` 等）需要它，而它在**运行时
    /// 被晚于装配地设置**（宿主先建 VM、再定工程根）。装机器与宿主共享这个单元，就能在
    /// 调用时读到最新值，不必把根目录顺着 120+ 个调用点的手工参数传下去。
    ///
    /// `None` = 尚未确定（发布包启动早期、纯脚本测试）。
    root: Rc<RefCell<Option<PathBuf>>>,
    /// 宿主帧事实：本步固定步长（秒）与指针在视口内的位置。
    ///
    /// 为什么也放注册表：`input` 的连发节奏要用步长推进逻辑时间、`camera3d.mouse_ray` 要
    /// "指针在哪"，而这两样都由宿主逐帧写、由**不同**的领域绑定读。放进注册表后，写入方
    /// （宿主侧的输入门面）与读取方（各领域绑定）只共享一个小单元，不需要谁去认识谁的 crate
    /// （P1 阶段 3 的 `input` 下沉靠它解耦）。
    ///
    /// `Rc<Cell<_>>` 而不是裸 `Cell`：宿主门面要**直接持有**这两个单元（它不该为了读一个
    /// `f32` 而抱着整个注册表），`cell()` 系列出口就是为了让它们能被共享出去。
    delta_seconds: Rc<Cell<f32>>,
    pointer: Rc<Cell<Option<(f32, f32)>>>,
}

impl Registry {
    /// 创建 2D 基础 API 注册表。3D 宿主应显式选择 [`ApiSurface::ThreeD`]。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn for_surface(surface: ApiSurface) -> Self {
        Self {
            surface,
            ..Self::default()
        }
    }

    #[must_use]
    pub const fn surface(&self) -> ApiSurface {
        self.surface
    }

    /// 登记一个模块的安装器。顺序即装配顺序（模块之间互不读取，顺序不影响行为）。
    pub fn add(&mut self, name: &'static str, install: impl Fn(&Lua) -> LuaResult<()> + 'static) {
        self.entries.push((name, Box::new(install)));
    }

    /// 登记一个独立 API 包。依赖在 VM 装配前拓扑排序；无效清单会在执行任何安装器前失败。
    pub fn add_pack(&mut self, pack: ApiPack) {
        self.packs.push(pack);
    }

    #[must_use]
    pub fn packs(&self) -> Vec<ApiPackInfo> {
        self.packs.iter().map(ApiPack::info).collect()
    }

    /// 写入宿主工程根。运行时的 `set_project_root` / `set_settings_root` 转发到这里，
    /// 于是"设置持久化"类绑定与宿主看到的是同一个根。
    pub fn set_root(&self, root: Option<PathBuf>) {
        *self.root.borrow_mut() = root;
    }

    /// 当前工程根（端口实现每次调用都读，不缓存——根目录可能在装配之后才确定）。
    #[must_use]
    pub fn root(&self) -> Option<PathBuf> {
        self.root.borrow().clone()
    }

    /// 共享单元本身：端口实现捕获它，就能拿到**之后**写进来的根。
    #[must_use]
    pub fn root_cell(&self) -> Rc<RefCell<Option<PathBuf>>> {
        Rc::clone(&self.root)
    }

    /// 写入本步固定步长（秒）。运行时的 `set_delta` 转发到这里；非有限/负值按 0 处理。
    pub fn set_delta_seconds(&self, seconds: f32) {
        self.delta_seconds.set(if seconds.is_finite() {
            seconds.max(0.0)
        } else {
            0.0
        });
    }

    /// 本步固定步长（秒，未设置时为 0）。
    #[must_use]
    pub fn delta_seconds(&self) -> f32 {
        self.delta_seconds.get()
    }

    /// 步长单元的共享句柄（宿主门面直接持有，免得为一个 `f32` 抱着整个注册表）。
    #[must_use]
    pub fn delta_cell(&self) -> Rc<Cell<f32>> {
        Rc::clone(&self.delta_seconds)
    }

    /// 写入指针位置（视口坐标，`None` = 指针不在视口内）。
    pub fn set_pointer(&self, pointer: Option<(f32, f32)>) {
        self.pointer.set(pointer);
    }

    /// 指针位置（视口坐标）；`camera3d.mouse_ray` 等绑定读它做屏幕→世界换算。
    #[must_use]
    pub fn pointer(&self) -> Option<(f32, f32)> {
        self.pointer.get()
    }

    /// 指针单元的共享句柄。
    #[must_use]
    pub fn pointer_cell(&self) -> Rc<Cell<Option<(f32, f32)>>> {
        Rc::clone(&self.pointer)
    }

    /// 已登记的模块名（宿主自检、诊断输出用）。
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.entries
            .iter()
            .map(|(name, _)| *name)
            .chain(
                self.packs
                    .iter()
                    .flat_map(|pack| pack.modules.iter().map(|(name, _)| *name)),
            )
            .collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
            + self
                .packs
                .iter()
                .map(|pack| pack.modules.len())
                .sum::<usize>()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.packs.iter().all(|pack| pack.modules.is_empty())
    }

    /// 先校验全部清单，再按 legacy 模块和依赖拓扑顺序装配 API 包。
    pub fn install(&self, lua: &Lua) -> LuaResult<()> {
        let order = self.validate()?;
        for (_, install) in &self.entries {
            install(lua)?;
        }
        for index in order {
            for (_, install) in &self.packs[index].modules {
                install(lua)?;
            }
        }
        Ok(())
    }

    fn validate(&self) -> LuaResult<Vec<usize>> {
        let mut seen_modules = Vec::with_capacity(self.len());
        for (name, _) in &self.entries {
            if seen_modules.contains(name) {
                return Err(crate::api_error(format!(
                    "Lua 模块重复登记：{name}（同一张全局表被装两遍会静默覆盖）"
                )));
            }
            seen_modules.push(*name);
        }

        let mut pack_ids = Vec::with_capacity(self.packs.len());
        for pack in &self.packs {
            if pack.id.trim().is_empty() || pack.version.trim().is_empty() {
                return Err(crate::api_error(
                    "Lua API 包的 id 和 version 不能为空".to_owned(),
                ));
            }
            if pack.api_version != API_PACK_VERSION {
                return Err(crate::api_error(format!(
                    "Lua API 包 {} 的契约版本不匹配：期望 {}，实际 {}",
                    pack.id, API_PACK_VERSION, pack.api_version
                )));
            }
            if pack.dimension == ApiDimension::ThreeD && self.surface != ApiSurface::ThreeD {
                return Err(crate::api_error(format!(
                    "当前 Lua API 面为 2D，拒绝安装 3D 包：{}",
                    pack.id
                )));
            }
            if pack.modules.is_empty() {
                return Err(crate::api_error(format!(
                    "Lua API 包 {} 没有登记任何模块",
                    pack.id
                )));
            }
            if pack_ids.contains(&pack.id.as_str()) {
                return Err(crate::api_error(format!(
                    "Lua API 包 id 重复登记：{}",
                    pack.id
                )));
            }
            pack_ids.push(pack.id.as_str());
            for (name, _) in &pack.modules {
                if name.trim().is_empty() || seen_modules.contains(name) {
                    return Err(crate::api_error(format!(
                        "Lua 模块重复或为空：{name}（API 包 {}）",
                        pack.id
                    )));
                }
                seen_modules.push(*name);
            }
        }

        let mut indegree = vec![0usize; self.packs.len()];
        for (index, pack) in self.packs.iter().enumerate() {
            let mut seen_dependencies: Vec<&str> = Vec::with_capacity(pack.dependencies.len());
            for dependency in &pack.dependencies {
                if dependency == &pack.id || seen_dependencies.contains(&dependency.as_str()) {
                    return Err(crate::api_error(format!(
                        "Lua API 包 {} 的依赖无效：{}",
                        pack.id, dependency
                    )));
                }
                if !pack_ids.contains(&dependency.as_str()) {
                    return Err(crate::api_error(format!(
                        "Lua API 包 {} 依赖未登记的包：{}",
                        pack.id, dependency
                    )));
                }
                seen_dependencies.push(dependency.as_str());
                indegree[index] += 1;
            }
        }

        let mut order = Vec::with_capacity(self.packs.len());
        let mut emitted = vec![false; self.packs.len()];
        while order.len() < self.packs.len() {
            let Some(index) =
                (0..self.packs.len()).find(|&index| !emitted[index] && indegree[index] == 0)
            else {
                return Err(crate::api_error("Lua API 包依赖存在循环".to_owned()));
            };
            emitted[index] = true;
            order.push(index);
            let completed_id = &self.packs[index].id;
            for (dependent_index, dependent) in self.packs.iter().enumerate() {
                if !emitted[dependent_index]
                    && dependent
                        .dependencies
                        .iter()
                        .any(|dependency| dependency == completed_id)
                {
                    indegree[dependent_index] -= 1;
                }
            }
        }
        Ok(order)
    }
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Registry")
            .field("names", &self.names())
            .field("packs", &self.packs())
            .field("surface", &self.surface)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn api_packs_install_in_dependency_order() {
        let installed = Rc::new(RefCell::new(Vec::new()));
        let mut dependent =
            ApiPack::new("dependent", "1.0", ApiDimension::ThreeD).with_dependency("base");
        {
            let installed = Rc::clone(&installed);
            dependent.module("dependent_api", move |_| {
                installed.borrow_mut().push("dependent");
                Ok(())
            });
        }
        let mut base = ApiPack::new("base", "1.0", ApiDimension::Shared);
        {
            let installed = Rc::clone(&installed);
            base.module("base_api", move |_| {
                installed.borrow_mut().push("base");
                Ok(())
            });
        }

        let mut registry = Registry::for_surface(ApiSurface::ThreeD);
        registry.add_pack(dependent);
        registry.add_pack(base);
        registry.install(&Lua::new()).expect("依赖应可解析");

        assert_eq!(*installed.borrow(), ["base", "dependent"]);
        assert_eq!(registry.names(), ["dependent_api", "base_api"]);
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn invalid_pack_graph_fails_before_running_installers() {
        let installed = Rc::new(RefCell::new(false));
        let mut pack = ApiPack::new("missing-dependency", "1.0", ApiDimension::TwoD)
            .with_dependency("not-registered");
        {
            let installed = Rc::clone(&installed);
            pack.module("should_not_install", move |_| {
                *installed.borrow_mut() = true;
                Ok(())
            });
        }

        let mut registry = Registry::new();
        registry.add_pack(pack);
        let error = registry.install(&Lua::new()).expect_err("缺失依赖必须报错");
        assert!(error.to_string().contains("未登记的包"));
        assert!(!*installed.borrow());
    }

    #[test]
    fn duplicate_module_names_across_packs_are_rejected_before_install() {
        let mut first = ApiPack::new("first", "1.0", ApiDimension::Shared);
        first.module("duplicate", |_| Ok(()));
        let mut second = ApiPack::new("second", "1.0", ApiDimension::ThreeD);
        second.module("duplicate", |_| Ok(()));

        let mut registry = Registry::for_surface(ApiSurface::ThreeD);
        registry.add_pack(first);
        registry.add_pack(second);
        let error = registry
            .install(&Lua::new())
            .expect_err("模块名冲突必须报错");
        assert!(error.to_string().contains("重复或为空"));
    }

    #[test]
    fn cyclic_pack_dependencies_are_rejected_before_install() {
        let mut first =
            ApiPack::new("first", "1.0", ApiDimension::Shared).with_dependency("second");
        first.module("first_api", |_| Ok(()));
        let mut second =
            ApiPack::new("second", "1.0", ApiDimension::ThreeD).with_dependency("first");
        second.module("second_api", |_| Ok(()));

        let mut registry = Registry::for_surface(ApiSurface::ThreeD);
        registry.add_pack(first);
        registry.add_pack(second);
        let error = registry.install(&Lua::new()).expect_err("依赖循环必须报错");
        assert!(error.to_string().contains("依赖存在循环"));
    }

    #[test]
    fn unsupported_pack_api_version_is_rejected() {
        let mut pack = ApiPack::new("future", "1.0", ApiDimension::TwoD).with_api_version(2);
        pack.module("future_api", |_| Ok(()));

        let mut registry = Registry::new();
        registry.add_pack(pack);
        let error = registry
            .install(&Lua::new())
            .expect_err("不支持的契约版本必须报错");
        assert!(error.to_string().contains("契约版本不匹配"));
    }

    #[test]
    fn two_dimensional_surface_rejects_three_dimensional_packs_before_install() {
        let installed = Rc::new(RefCell::new(false));
        let mut pack = ApiPack::new("rendering-3d", "1.0", ApiDimension::ThreeD);
        {
            let installed = Rc::clone(&installed);
            pack.module("rendering_3d", move |_| {
                *installed.borrow_mut() = true;
                Ok(())
            });
        }

        let mut registry = Registry::for_surface(ApiSurface::TwoD);
        registry.add_pack(pack);
        let error = registry
            .install(&Lua::new())
            .expect_err("2D 注册表必须拒绝 3D 包");
        assert!(error.to_string().contains("拒绝安装 3D 包"));
        assert!(!*installed.borrow());
    }
}
