use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use mlua::Result as LuaResult;

use crate::reentrant_borrow;

/// 宿主拥有的**能力单元**：绑定与宿主共享同一份状态，但类型上没有环形依赖。
///
/// 语义上等价于 `Rc<RefCell<T>>`，差别只有两点，都是踩过坑才加的：
///
/// 1. **重入不 panic，而是 Lua 错误**（[`reentrant_borrow`]）。`RefCell` 默认那一下 panic
///    在 release + strip 的产物上只剩 `unreachable`，看不出是哪个能力、更看不出是重入。
/// 2. **带名字**。`what` 只用于报错（`"audio"`、`"input"`……），让"哪个单元被重入"当场可读。
///
/// 为什么用 `Rc` 而不是 `Arc`：Lua 侧本来就被钉在主线程（`mlua` 的 `Lua` 是 `!Send`），
/// 能力单元跟它同生命周期，加原子计数只会多付成本而不换来任何跨线程能力。
pub struct Handle<T> {
    inner: Rc<RefCell<T>>,
    what: &'static str,
}

impl<T> Handle<T> {
    /// 宿主建一份能力单元。`what` 是报错里用的短名（小写、与 Lua 模块同名的写法最省事）。
    pub fn new(value: T, what: &'static str) -> Self {
        Self {
            inner: Rc::new(RefCell::new(value)),
            what,
        }
    }

    /// 借用（只读）。
    pub fn borrow(&self) -> LuaResult<Ref<'_, T>> {
        self.inner
            .try_borrow()
            .map_err(|error| reentrant_borrow(self.what, error))
    }

    /// 借用（可写）。
    pub fn borrow_mut(&self) -> LuaResult<RefMut<'_, T>> {
        self.inner
            .try_borrow_mut()
            .map_err(|error| reentrant_borrow(self.what, error))
    }

    /// 宿主的免报错只读访问。
    ///
    /// 宿主侧（每帧驱动、编辑器面板）**不在 Lua 调用栈里**，重入在结构上不可能发生；
    /// 要求它写 `?` 会把"宿主驱动"与"Lua 绑定"两种场景混成一种错误处理，反而掩盖真问题。
    /// 这里仍然用 `try_*`，失败时给一条带单元名的 panic（这是宿主逻辑错误，不是用户错误）。
    pub fn with<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        match self.inner.try_borrow() {
            Ok(value) => read(&value),
            Err(error) => panic!("{} 被借用期间宿主再次只读访问：{error}", self.what),
        }
    }

    /// 宿主的免报错可写访问（语义同 [`Handle::with`]）。
    pub fn with_mut<R>(&self, write: impl FnOnce(&mut T) -> R) -> R {
        match self.inner.try_borrow_mut() {
            Ok(mut value) => write(&mut value),
            Err(error) => panic!("{} 被借用期间宿主再次可写访问：{error}", self.what),
        }
    }

    /// 是否与另一个句柄指向同一份状态（测试与去重用）。
    #[must_use]
    pub fn same(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }
}

impl<T: Default> Default for Handle<T> {
    /// 无名的默认单元（`what` = `"<unnamed>"`）。
    ///
    /// **只为 `#[derive(Default)]` 的宿主状态服务**：像 `ScriptState` 这种三十多个字段的状态，
    /// 手写 `Default` 会在每次加字段时漏一个（编译器能抓，但没人愿意改三十行），派生才是对的。
    /// 代价是重入报错里的单元名不好看，所以真正的装配点应当用 `Handle::new(state, "名字")`
    /// 覆盖它——`ScriptState` 的运行时构造点就是这么做的。
    fn default() -> Self {
        Self::new(T::default(), "<unnamed>")
    }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
            what: self.what,
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Handle<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Handle")
            .field("what", &self.what)
            .finish_non_exhaustive()
    }
}
