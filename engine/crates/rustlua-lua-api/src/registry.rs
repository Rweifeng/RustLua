use mlua::{Lua, Result as LuaResult};
use std::cell::Cell;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// 一个 Lua 全局模块的装配函数（领域 crate 提供，组合根登记）。
///
/// 为什么是闭包而不是 trait：安装器要**捕获句柄**（`Handle<T>` 的克隆），闭包天然能做，
/// 而 trait 对象还得额外定义一个带泛型参数的实现类型。这里不需要多态——注册表只负责
/// 按名字收集、按顺序调用。
pub type Installer = Box<dyn Fn(&Lua) -> LuaResult<()>>;

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
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个模块的安装器。顺序即装配顺序（模块之间互不读取，顺序不影响行为）。
    pub fn add(&mut self, name: &'static str, install: impl Fn(&Lua) -> LuaResult<()> + 'static) {
        self.entries.push((name, Box::new(install)));
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
        self.entries.iter().map(|(name, _)| *name).collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 按登记顺序装配全部模块。重名在**装配时**报错（含两个名字，便于定位是谁重复登记）。
    pub fn install(&self, lua: &Lua) -> LuaResult<()> {
        let mut seen: Vec<&'static str> = Vec::with_capacity(self.entries.len());
        for (name, install) in &self.entries {
            if seen.contains(name) {
                return Err(crate::api_error(format!(
                    "Lua 模块重复登记：{name}（同一张全局表被装两遍会静默覆盖）"
                )));
            }
            seen.push(name);
            install(lua)?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Registry")
            .field("names", &self.names())
            .finish()
    }
}
