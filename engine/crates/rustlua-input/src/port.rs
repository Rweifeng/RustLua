//! `input` 设置持久化的**端口**（`settings.toml` 的 `[input]` 段）。
//!
//! 为什么不在这里直接读盘：文件路径规则（发布包 → 用户数据目录、开发工程 → 项目根）、
//! `[input]` 段的宽松解析与「写回时保留其它段」都住在 `rustlua-gameplay`，而
//! `gameplay → input` 已经存在——反向 `use` 会成环。与 `audio` 的 `SettingsPort` 同一套解法：
//! 绑定只认端口，实现由组合根（`rustlua_gameplay::input_lua`）注入。

use std::path::PathBuf;

use crate::input::ActionMap;
use crate::nav::NavSettings;

/// 一次成功读取的结果。`map = None` 表示段内内容非法，由调用方决定保留还是回退默认。
#[derive(Debug, Clone)]
pub struct LoadedSettings {
    /// 实际读到的文件（诊断消息里要带它）。
    pub path: PathBuf,
    /// 解析出的动作映射；`None` = 不可用（原因见 [`Self::reason`]）。
    pub map: Option<ActionMap>,
    /// 同一段里的导航手感（缺省回默认）。
    pub nav: NavSettings,
    /// v1→v2 改键迁移说明（有损换算逐条）。
    pub notes: Vec<String>,
    /// `map = None` 的原因（`missing_key...` / `invalid_input_section` / 版本头不符…）。
    pub reason: Option<String>,
}

/// 读取失败的三类语义（与 `audio.load_settings` 的返回口径一一对应）。
#[derive(Debug, Clone)]
pub enum SettingsLoadError {
    /// 没有工程根：返回 `(false, "no_project_root")`。
    NoRoot,
    /// 文件不存在：保留当前绑定，返回 `(false, "missing")`。
    Missing,
    /// 读到一半失败：记一条诊断，返回 `(false, <io 错误文本>)`。
    ReadFailed { path: PathBuf, message: String },
}

/// 写入失败的分类：只有真正写不进去时才记诊断（建目录失败的历史行为是不记）。
#[derive(Debug, Clone)]
pub enum SettingsSaveError {
    /// 没有工程根：`(false, "no_project_root")`。
    NoRoot,
    /// 建父目录失败：`(false, <io 错误文本>)`，不记诊断。
    Directory { message: String },
    /// 落盘失败：记诊断 `"{path}: save failed: {message}"`，返回 `(false, message)`。
    Write { path: PathBuf, message: String },
}

/// `[input]` 段设置文件的读写端口。
///
/// 三个方法都接受**显式路径**（Lua 侧可选参数）：`None` = 用工程默认位置。
pub trait SettingsPort {
    /// 默认设置文件路径（`settings_path()` 的返回值；无工程根时 `None`）。
    fn path(&self) -> Option<PathBuf>;

    /// 读并解析（不做任何"应用"动作：合并默认、回退、诊断都由绑定侧决定，口径只有一处）。
    fn load(&self, path: Option<&str>) -> Result<LoadedSettings, SettingsLoadError>;

    /// 保序读改写：只动 `[input]` 段，其余段与注释原样保留。
    fn save(&self, path: Option<&str>, map: &ActionMap) -> Result<(), SettingsSaveError>;
}
