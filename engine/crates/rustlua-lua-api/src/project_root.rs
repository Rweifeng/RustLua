//! 宿主工程根：绑定把"工程内相对路径"落到磁盘的唯一入口。
//!
//! ## 为什么在契约层
//!
//! `data.load("data/items.json")` 这类绑定需要一棵工程根——它属于**宿主的运行环境**，
//! 不属于任何领域 crate。绑定下沉之后（`docs/62` P1 ⑤-4c），`rustlua-gameplay` 里的
//! 数据表绑定不能再借 `ScriptState.project_root`（那会把 scripting 拖回绑定层），
//! 于是把这条"绑定需要、与领域无关"的东西放进契约层：宿主持有一份
//! [`Handle<ProjectRoot>`](Handle)，绑定只读。
//!
//! 与 [`crate::Handle`] 同一条理由：`Handle` 让"换工程"（赋值）与"读路径"（绑定）
//! 看见同一份状态，而不是在装配时拷贝一个 `PathBuf` 快照——快照会让运行期的
//! `set_project_root` 对已经装好的绑定失效。

use std::path::{Path, PathBuf};

use crate::Handle;

/// 工程根（未设置时 [`path`](Self::path) 为 `None`）。
///
/// 只存"根在哪"，不解析任何领域路径：`scenes/<name>.scene`、`data/<table>.json`
/// 这些写法留在各自的绑定里，契约层不认领域。
#[derive(Debug, Default)]
pub struct ProjectRoot {
    root: Option<PathBuf>,
}

impl ProjectRoot {
    /// 建一份能力单元。`what` 只用于重入报错（与 `ScriptState` 的其它句柄同口径）。
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "project_root")
    }

    /// 设置 / 覆盖工程根（宿主切工程时调用）。
    pub fn set(&mut self, root: PathBuf) {
        self.root = Some(root);
    }

    /// 当前工程根；未设置时为 `None`。
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// 把工程内相对路径解析成磁盘路径；未设置工程根时为 `None`。
    #[must_use]
    pub fn resolve(&self, relative: impl AsRef<Path>) -> Option<PathBuf> {
        self.root.as_ref().map(|root| root.join(relative))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_root_resolves_to_none() {
        let handle = ProjectRoot::handle();
        assert!(handle.borrow().unwrap().path().is_none());
        assert!(
            handle
                .borrow()
                .unwrap()
                .resolve("data/items.json")
                .is_none()
        );
    }

    #[test]
    fn set_root_is_visible_through_every_clone_of_the_handle() {
        let handle = ProjectRoot::handle();
        let clone = handle.clone();
        handle
            .borrow_mut()
            .unwrap()
            .set(PathBuf::from("C:/projects/demo"));
        // 句柄是能力单元，不是拷贝：另一个克隆读到的必须是同一份。
        assert_eq!(
            clone.borrow().unwrap().resolve("data/items.json"),
            Some(PathBuf::from("C:/projects/demo").join("data/items.json"))
        );
        // 覆盖（换工程）同样对所有克隆生效。
        clone.borrow_mut().unwrap().set(PathBuf::from("D:/other"));
        assert_eq!(handle.borrow().unwrap().path(), Some(Path::new("D:/other")));
    }
}
