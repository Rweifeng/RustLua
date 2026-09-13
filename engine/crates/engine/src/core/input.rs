//! Platform-independent input snapshots for deterministic simulation.
//!
//! M20-02 追加**动作映射层**：把物理输入（键盘/鼠标/抽象手柄）映射到命名动作，
//! 支持重绑定与设置持久化（[`ActionMap::to_text`] / [`ActionMap::from_text`]）。
//! 手柄为**最小跨平台抽象**：宿主喂入按钮/轴即可，具体设备轮询（需要额外
//! 依赖）未接入，见任务 M20-02 的已知限制。

use std::collections::{BTreeMap, HashMap, HashSet};

/// Keyboard keys currently relevant to the engine's action layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputKey {
    W,
    A,
    S,
    D,
    Space,
    Escape,
    Other(u32),
}

impl InputKey {
    /// 方向键上（M20-02 UI 焦点/滚动导航）。
    ///
    /// 方向键用 ASCII 之外的稳定码位编码进 [`InputKey::Other`]，既有枚举变体
    /// 语义不变；宿主（game_app / 编辑器预览）把平台的 ArrowUp/Down/Left/Right
    /// 映射到这里，Lua 侧用 `input.is_down("up")` 等名字读取。绑定文本自
    /// M20-07 起是可读的 `key:up`/`key:down`/`key:left`/`key:right`（见
    /// [`InputKey::arrow_name`]；`key:other:<code>` 仍可解析，保证旧设置文件
    /// 不失效）。命名动作与手柄 DPad 的接线见 M20-07/M20-08。
    pub const ARROW_UP: Self = Self::Other(0x0010_0001);
    /// 方向键下。
    pub const ARROW_DOWN: Self = Self::Other(0x0010_0002);
    /// 方向键左。
    pub const ARROW_LEFT: Self = Self::Other(0x0010_0003);
    /// 方向键右。
    pub const ARROW_RIGHT: Self = Self::Other(0x0010_0004);

    /// Maps a single ASCII letter to an [`InputKey`]. Movement letters
    /// `w`/`a`/`s`/`d` keep their dedicated variants (used by `input.axis`);
    /// every other letter maps to `Other(<lowercase ascii code>)` so the game
    /// window, the editor preview, and `parse_key` agree on one key identity.
    /// This is what makes gameplay keys such as Q/E/R/T/F/G distinct in both
    /// the standalone game and the editor's Play preview.
    #[must_use]
    pub fn from_char(letter: char) -> InputKey {
        match letter.to_ascii_lowercase() {
            'w' => Self::W,
            'a' => Self::A,
            's' => Self::S,
            'd' => Self::D,
            other => Self::Other(other as u32),
        }
    }

    /// 方向键的可读名字（非方向键返回 `None`）。绑定文本用 `key:up` 这类
    /// 可读形式（与 Lua 侧 `input.is_down("up")` 同名），而不是内部码位。
    ///
    /// [`Self::ARROW_*`] 是 `Other` 关联常量而非独立变体，无法用常量模式直接
    /// 匹配，因此这里按码位字面量比对（与上面的常量定义保持一一对应）。
    #[must_use]
    pub const fn arrow_name(self) -> Option<&'static str> {
        match self {
            Self::Other(0x0010_0001) => Some("up"),
            Self::Other(0x0010_0002) => Some("down"),
            Self::Other(0x0010_0003) => Some("left"),
            Self::Other(0x0010_0004) => Some("right"),
            _ => None,
        }
    }
}

/// Mouse buttons exposed by the engine input layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u16),
}

/// 手柄轴方向（M20-02 抽象手柄绑定用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AxisDirection {
    Positive,
    Negative,
}

impl AxisDirection {
    /// 方向符号（+1 / -1）。
    #[must_use]
    pub const fn sign(self) -> f32 {
        match self {
            Self::Positive => 1.0,
            Self::Negative => -1.0,
        }
    }

    /// 文本形式（诊断与测试用）。
    #[must_use]
    pub const fn to_text(self) -> &'static str {
        match self {
            Self::Positive => "+",
            Self::Negative => "-",
        }
    }
}

/// 最近一次产生输入的**设备类别**（M22/W3）。
///
/// 键鼠同属 [`InputDevice::Keyboard`]（UI 提示只需要区分「键鼠说明」与「手柄说明」）。
/// 由宿主/设备层在**真实边沿**发生时写入，而不是每帧根据「有轴值」推断——手柄空闲
/// 时轴也每帧上报 0.0，按值推断会把提示永远锁在手柄上。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum InputDevice {
    /// 键盘或鼠标。
    #[default]
    Keyboard,
    /// 抽象手柄（按钮或摇杆）。
    Gamepad,
}

impl InputDevice {
    /// Lua `input.last_device()` 的返回文本。
    #[must_use]
    pub const fn to_text(self) -> &'static str {
        match self {
            Self::Keyboard => "keyboard",
            Self::Gamepad => "gamepad",
        }
    }
}

/// 一个命名动作的绑定项（M20-02）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Binding {
    Key(InputKey),
    Mouse(MouseButton),
    /// 抽象手柄按钮索引（宿主喂入；设备轮询未接入）。
    GamepadButton(u8),
    /// 抽象手柄轴索引 + 方向（宿主喂入；设备轮询未接入）。
    GamepadAxis(u8, AxisDirection),
}

impl Binding {
    /// 稳定文本形式（用于设置持久化与 UI 展示）。
    #[must_use]
    pub fn to_text(self) -> String {
        match self {
            Self::Key(InputKey::W) => "key:w".to_owned(),
            Self::Key(InputKey::A) => "key:a".to_owned(),
            Self::Key(InputKey::S) => "key:s".to_owned(),
            Self::Key(InputKey::D) => "key:d".to_owned(),
            Self::Key(InputKey::Space) => "key:space".to_owned(),
            Self::Key(InputKey::Escape) => "key:escape".to_owned(),
            Self::Key(InputKey::Other(code)) => {
                // 方向键用可读名字（与 Lua 侧 `input.is_down("up")` 同名），
                // 而不是内部码位 `key:other:1048577`——设置文件与重绑定面板
                // 都需要稳定的可读文本（M20-07 / M20-08）。
                if let Some(name) = InputKey::Other(code).arrow_name() {
                    return format!("key:{name}");
                }
                // 可打印字符按键显示为字符本身（与 InputKey::from_char 对称）。
                match u8::try_from(code).ok().map(char::from) {
                    Some(letter) if letter.is_ascii_graphic() => format!("key:{letter}"),
                    _ => format!("key:other:{code}"),
                }
            }
            Self::Mouse(MouseButton::Left) => "mouse:left".to_owned(),
            Self::Mouse(MouseButton::Right) => "mouse:right".to_owned(),
            Self::Mouse(MouseButton::Middle) => "mouse:middle".to_owned(),
            Self::Mouse(MouseButton::Other(index)) => format!("mouse:other:{index}"),
            Self::GamepadButton(index) => format!("pad:{index}"),
            Self::GamepadAxis(index, AxisDirection::Positive) => format!("padaxis:{index}+"),
            Self::GamepadAxis(index, AxisDirection::Negative) => format!("padaxis:{index}-"),
        }
    }

    /// 解析 [`Binding::to_text`] 的输出。
    pub fn parse(text: &str) -> Result<Self, ActionMapError> {
        let (kind, rest) = text
            .split_once(':')
            .ok_or_else(|| ActionMapError::InvalidBinding(text.to_owned()))?;
        match (kind, rest) {
            ("key", value) => match value {
                "w" => Ok(Self::Key(InputKey::W)),
                "a" => Ok(Self::Key(InputKey::A)),
                "s" => Ok(Self::Key(InputKey::S)),
                "d" => Ok(Self::Key(InputKey::D)),
                "space" => Ok(Self::Key(InputKey::Space)),
                "escape" => Ok(Self::Key(InputKey::Escape)),
                "up" => Ok(Self::Key(InputKey::ARROW_UP)),
                "down" => Ok(Self::Key(InputKey::ARROW_DOWN)),
                "left" => Ok(Self::Key(InputKey::ARROW_LEFT)),
                "right" => Ok(Self::Key(InputKey::ARROW_RIGHT)),
                other => {
                    if let Some(index) = other.strip_prefix("other:") {
                        return index
                            .parse::<u32>()
                            .map(|code| Self::Key(InputKey::Other(code)))
                            .map_err(|_| ActionMapError::InvalidBinding(text.to_owned()));
                    }
                    let mut chars = other.chars();
                    match (chars.next(), chars.next()) {
                        (Some(letter), None) if letter.is_ascii_graphic() => {
                            Ok(Self::Key(InputKey::from_char(letter)))
                        }
                        _ => Err(ActionMapError::InvalidBinding(text.to_owned())),
                    }
                }
            },
            ("mouse", value) => match value {
                "left" => Ok(Self::Mouse(MouseButton::Left)),
                "right" => Ok(Self::Mouse(MouseButton::Right)),
                "middle" => Ok(Self::Mouse(MouseButton::Middle)),
                other => other
                    .strip_prefix("other:")
                    .and_then(|index| index.parse::<u16>().ok())
                    .map(|index| Self::Mouse(MouseButton::Other(index)))
                    .ok_or_else(|| ActionMapError::InvalidBinding(text.to_owned())),
            },
            ("pad", value) => value
                .parse::<u8>()
                .map(Self::GamepadButton)
                .map_err(|_| ActionMapError::InvalidBinding(text.to_owned())),
            ("padaxis", value) => {
                let (index, sign) = value
                    .split_at_checked(value.len().saturating_sub(1))
                    .ok_or_else(|| ActionMapError::InvalidBinding(text.to_owned()))?;
                let index = index
                    .parse::<u8>()
                    .map_err(|_| ActionMapError::InvalidBinding(text.to_owned()))?;
                match sign {
                    "+" => Ok(Self::GamepadAxis(index, AxisDirection::Positive)),
                    "-" => Ok(Self::GamepadAxis(index, AxisDirection::Negative)),
                    _ => Err(ActionMapError::InvalidBinding(text.to_owned())),
                }
            }
            _ => Err(ActionMapError::InvalidBinding(text.to_owned())),
        }
    }
}

/// 动作映射错误（重绑定/设置持久化）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionMapError {
    EmptyAction,
    DuplicateBinding { action: String, binding: String },
    InvalidBinding(String),
    UnsupportedVersion(String),
}

impl std::fmt::Display for ActionMapError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyAction => formatter.write_str("action name must not be empty"),
            Self::DuplicateBinding { action, binding } => {
                write!(formatter, "action {action} already binds {binding}")
            }
            Self::InvalidBinding(text) => write!(formatter, "invalid binding: {text}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported action map version: {version}")
            }
        }
    }
}

impl std::error::Error for ActionMapError {}

/// 跨动作冲突：一个物理输入被两个及以上动作占用（M20-08）。
///
/// `actions` 至少两项，按动作名字典序（[`ActionMap::actions`] 的顺序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionConflict {
    /// 被重复占用的物理输入。
    pub binding: Binding,
    /// 占用它的动作名（≥2 项）。
    pub actions: Vec<String>,
}

impl ActionConflict {
    /// 人可读的一行诊断（编辑器面板与日志共用；中文与 `ActionMapError` 的英文
    /// 消息分开，前者面向玩家、后者面向 API 调用方）。
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{} 被多个动作同时绑定：{}（请先解绑其中一个）",
            self.binding.to_text(),
            self.actions.join("、")
        )
    }

    /// 转成既有的 [`ActionMapError::DuplicateBinding`]，便于复用同一诊断出口。
    #[must_use]
    pub fn as_error(&self) -> ActionMapError {
        ActionMapError::DuplicateBinding {
            action: self
                .actions
                .first()
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned()),
            binding: self.binding.to_text(),
        }
    }
}

/// 动作映射的持久化版本头。
pub const ACTION_MAP_VERSION: &str = "rustlua-actions-v1";

/// 命名动作 → 绑定集合（M20-02）。表内按动作名稳定排序，便于对比与展示。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionMap {
    actions: BTreeMap<String, Vec<Binding>>,
}

impl ActionMap {
    /// 空映射。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 引擎默认映射：与当前 `game_jhyt` 键位一致，并附基础手柄默认绑定。
    ///
    /// 游戏/编辑器用 [`Self::merge_missing`] 把它作为「未重绑定项的兜底」，
    /// 保证用户重绑定后其余动作仍可玩。
    ///
    /// M20-08 扩表：`game_jhyt` 剩余的物理键直读（建造 B / 放置 F / 拆除 X /
    /// 使用·升级 U / 任务 L / 重开 R / 选种子 C / 音量 ± / 数字键直选 1..9）
    /// 全部升级为命名动作，玩家在编辑器「项目设置 → 输入」里可以整体改键。
    /// 数字键用 `slot_1..slot_9` 而不是一个动作多绑定，因为「按了第几个」本身
    /// 就是动作语义。
    ///
    /// **每个物理输入只出现在一个动作里**（唯一例外是同一动作的多绑定）：
    /// 否则默认表自身就会命中 [`Self::conflicts`]，编辑器一打开面板就报冲突。
    /// 游戏里 F/U/Q/I/P/L 等键的**上下文复用**（同一动作在不同模式下有不同
    /// 语义）由玩法层决定，不在这里重复绑定。
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut map = Self::new();
        let defaults: &[(&str, &[Binding])] = &[
            ("move_up", &[Binding::Key(InputKey::W)]),
            ("move_down", &[Binding::Key(InputKey::S)]),
            ("move_left", &[Binding::Key(InputKey::A)]),
            ("move_right", &[Binding::Key(InputKey::D)]),
            ("interact", &[Binding::Key(InputKey::from_char('e'))]),
            ("attack", &[Binding::Key(InputKey::from_char('j'))]),
            ("skill", &[Binding::Key(InputKey::from_char('k'))]),
            ("gift", &[Binding::Key(InputKey::from_char('g'))]),
            ("sleep", &[Binding::Key(InputKey::from_char('t'))]),
            ("practice", &[Binding::Key(InputKey::from_char('q'))]),
            ("inventory", &[Binding::Key(InputKey::from_char('i'))]),
            ("pause", &[Binding::Key(InputKey::from_char('p'))]),
            // 建造/采集/任务/重开（M20-08）：game_jhyt 迁移动作层后的新动作。
            ("build", &[Binding::Key(InputKey::from_char('b'))]),
            ("place", &[Binding::Key(InputKey::from_char('f'))]),
            ("demolish", &[Binding::Key(InputKey::from_char('x'))]),
            ("use", &[Binding::Key(InputKey::from_char('u'))]),
            ("quest", &[Binding::Key(InputKey::from_char('l'))]),
            ("respawn", &[Binding::Key(InputKey::from_char('r'))]),
            ("seed", &[Binding::Key(InputKey::from_char('c'))]),
            ("volume_up", &[Binding::Key(InputKey::from_char('+'))]),
            ("volume_down", &[Binding::Key(InputKey::from_char('-'))]),
            // 数字键 1..9 直选（对话选项/商店行/面板行）——键盘专属快捷方式，
            // 手柄侧走 ui_* + confirm。
            ("slot_1", &[Binding::Key(InputKey::from_char('1'))]),
            ("slot_2", &[Binding::Key(InputKey::from_char('2'))]),
            ("slot_3", &[Binding::Key(InputKey::from_char('3'))]),
            ("slot_4", &[Binding::Key(InputKey::from_char('4'))]),
            ("slot_5", &[Binding::Key(InputKey::from_char('5'))]),
            ("slot_6", &[Binding::Key(InputKey::from_char('6'))]),
            ("slot_7", &[Binding::Key(InputKey::from_char('7'))]),
            ("slot_8", &[Binding::Key(InputKey::from_char('8'))]),
            ("slot_9", &[Binding::Key(InputKey::from_char('9'))]),
            (
                "confirm",
                &[Binding::Key(InputKey::Space), Binding::GamepadButton(0)],
            ),
            (
                "cancel",
                &[Binding::Key(InputKey::Escape), Binding::GamepadButton(1)],
            ),
            // 菜单/焦点导航（M20-02 面板 + M20-07 动作层）：方向键与左摇杆
            // **按方向拆成四个动作**，这样 `input.action_axis("ui_down","ui_up")`
            // 能产出带符号的模拟量（把两个方向塞进同一动作会让正负相互抵消；
            // `action_axis` 的**首参是负方向动作**，垂直轴即 down→up）。
            // M20-08 追加 DPad 按钮 12..15（M20-06 的设备契约），键盘手感不变。
            (
                "ui_up",
                &[
                    Binding::Key(InputKey::ARROW_UP),
                    Binding::GamepadAxis(1, AxisDirection::Positive),
                    Binding::GamepadButton(12),
                ],
            ),
            (
                "ui_down",
                &[
                    Binding::Key(InputKey::ARROW_DOWN),
                    Binding::GamepadAxis(1, AxisDirection::Negative),
                    Binding::GamepadButton(13),
                ],
            ),
            (
                "ui_left",
                &[
                    Binding::Key(InputKey::ARROW_LEFT),
                    Binding::GamepadAxis(0, AxisDirection::Negative),
                    Binding::GamepadButton(14),
                ],
            ),
            (
                "ui_right",
                &[
                    Binding::Key(InputKey::ARROW_RIGHT),
                    Binding::GamepadAxis(0, AxisDirection::Positive),
                    Binding::GamepadButton(15),
                ],
            ),
        ];
        for &(action, bindings) in defaults {
            // bindings 是 &&[Binding]（defaults 本身是切片引用），这里要 iter() 自动解引用，
            // 直接 `for binding in bindings` 会编不过（&&[T] 不是 IntoIterator）。
            for binding in bindings.iter() {
                // 默认表由常量构造，重复即代码缺陷；忽略返回值会让问题静默。
                map.actions
                    .entry(action.to_owned())
                    .or_default()
                    .push(*binding);
            }
        }
        debug_assert!(
            map.conflicts().is_empty(),
            "默认表不得含跨动作重复绑定：{:?}",
            map.conflicts()
        );
        map
    }

    /// 跨动作的物理输入冲突：同一绑定被两个及以上动作占用（M20-08）。
    ///
    /// 与 [`Self::bind`] 的 `DuplicateBinding`（同一动作内重复绑定同一输入）互补：
    /// 那个是「无意义重复」，这个是「同一按键触发两个动作」，玩法上几乎总是配置
    /// 错误，编辑器重绑定面板必须能看见并阻止保存。返回顺序按绑定文本排序，
    /// 保证 UI/测试稳定。
    #[must_use]
    pub fn conflicts(&self) -> Vec<ActionConflict> {
        // `Binding` 没有 `Ord`：先用文本做键（也顺便让输出有序），再映射回绑定本身。
        let mut by_text: BTreeMap<String, (Binding, Vec<String>)> = BTreeMap::new();
        for action in self.actions() {
            for binding in self.bindings(action) {
                let entry = by_text
                    .entry(binding.to_text())
                    .or_insert_with(|| (*binding, Vec::new()));
                entry.1.push(action.to_owned());
            }
        }
        by_text
            .into_values()
            .filter(|(_, actions)| actions.len() > 1)
            .map(|(binding, actions)| ActionConflict { binding, actions })
            .collect()
    }

    /// 保存前校验：存在跨动作冲突时返回首个冲突（可读诊断）——沿用
    /// [`ActionMapError::DuplicateBinding`]，消息形态与 `input.rebind` 一致。
    pub fn validate_bindings(&self) -> Result<(), ActionMapError> {
        match self.conflicts().into_iter().next() {
            None => Ok(()),
            Some(conflict) => Err(conflict.as_error()),
        }
    }

    /// 绑定一个物理输入到动作；同一动作重复绑定同一输入报错。
    pub fn bind(&mut self, action: &str, binding: Binding) -> Result<(), ActionMapError> {
        if action.trim().is_empty() {
            return Err(ActionMapError::EmptyAction);
        }
        let bindings = self.actions.entry(action.to_owned()).or_default();
        if bindings.contains(&binding) {
            return Err(ActionMapError::DuplicateBinding {
                action: action.to_owned(),
                binding: binding.to_text(),
            });
        }
        bindings.push(binding);
        Ok(())
    }

    /// 解绑；返回是否确实移除。
    pub fn unbind(&mut self, action: &str, binding: Binding) -> bool {
        let Some(bindings) = self.actions.get_mut(action) else {
            return false;
        };
        let before = bindings.len();
        bindings.retain(|candidate| *candidate != binding);
        let removed = bindings.len() != before;
        if bindings.is_empty() {
            self.actions.remove(action);
        }
        removed
    }

    /// 清空一个动作的全部绑定。
    pub fn clear(&mut self, action: &str) {
        self.actions.remove(action);
    }

    /// 动作的绑定列表（未定义时为空）。
    #[must_use]
    pub fn bindings(&self, action: &str) -> &[Binding] {
        self.actions.get(action).map_or(&[], Vec::as_slice)
    }

    /// 全部动作名（字典序）。
    #[must_use]
    pub fn actions(&self) -> Vec<&str> {
        self.actions.keys().map(String::as_str).collect()
    }

    /// 是否没有任何绑定。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// 仅补齐缺失动作（不覆盖用户已有的重绑定）——用于「恢复默认」之外的兜底。
    pub fn merge_missing(&mut self, defaults: &ActionMap) {
        for (action, bindings) in &defaults.actions {
            self.actions
                .entry(action.clone())
                .or_insert_with(|| bindings.clone());
        }
    }

    /// 序列化为设置文本（含版本头，逐行 `action=binding,binding`）。
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = String::from(ACTION_MAP_VERSION);
        for (action, bindings) in &self.actions {
            if bindings.is_empty() {
                continue;
            }
            let list = bindings
                .iter()
                .map(|binding| binding.to_text())
                .collect::<Vec<_>>()
                .join(",");
            text.push('\n');
            text.push_str(action);
            text.push('=');
            text.push_str(&list);
        }
        text
    }

    /// 解析 [`Self::to_text`] 的输出；版本头不符/绑定非法均返回错误。
    pub fn from_text(text: &str) -> Result<Self, ActionMapError> {
        let mut lines = text.lines();
        let header = lines.next().unwrap_or_default().trim();
        if header != ACTION_MAP_VERSION {
            return Err(ActionMapError::UnsupportedVersion(header.to_owned()));
        }
        let mut map = Self::new();
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (action, list) = line
                .split_once('=')
                .ok_or_else(|| ActionMapError::InvalidBinding(line.to_owned()))?;
            for item in list.split(',').filter(|item| !item.trim().is_empty()) {
                let binding = Binding::parse(item.trim())?;
                map.bind(action.trim(), binding)?;
            }
        }
        Ok(map)
    }
}

/// 谓词（模拟量）：手柄轴取模拟值，其余绑定为 0/1。
fn binding_axis_value(
    binding: Binding,
    held_keys: &HashSet<InputKey>,
    held_mouse: &HashSet<MouseButton>,
    gamepad_buttons: &HashSet<u8>,
    gamepad_axes: &HashMap<u8, f32>,
    axis_digital: &HashMap<(u8, AxisDirection), bool>,
) -> f32 {
    match binding {
        Binding::GamepadAxis(index, direction) => {
            let raw = gamepad_axes.get(&index).copied().unwrap_or(0.0);
            (raw * direction.sign()).max(0.0)
        }
        other => f32::from(u8::from(binding_is_down(
            other,
            held_keys,
            held_mouse,
            gamepad_buttons,
            gamepad_axes,
            axis_digital,
        ))),
    }
}

/// 谓词：绑定当前是否处于「按下」状态。
///
/// 手柄轴优先读**滞回数字态**（设备层每帧写入，见
/// [`InputSnapshot::set_gamepad_axis_digital`]）；没有该状态时（测试直接
/// `set_gamepad_axis` 的路径）回落到 `|value| >= AXIS_DIGITAL_ON`，语义与 M20 一致。
fn binding_is_down(
    binding: Binding,
    held_keys: &HashSet<InputKey>,
    held_mouse: &HashSet<MouseButton>,
    gamepad_buttons: &HashSet<u8>,
    gamepad_axes: &HashMap<u8, f32>,
    axis_digital: &HashMap<(u8, AxisDirection), bool>,
) -> bool {
    match binding {
        Binding::Key(key) => held_keys.contains(&key),
        Binding::Mouse(button) => held_mouse.contains(&button),
        Binding::GamepadButton(index) => gamepad_buttons.contains(&index),
        Binding::GamepadAxis(index, direction) => match axis_digital.get(&(index, direction)) {
            Some(down) => *down,
            None => gamepad_axes
                .get(&index)
                .is_some_and(|value| *value * direction.sign() >= AXIS_DIGITAL_ON),
        },
    }
}

/// A stable input state consumed by simulation and scripts.
#[derive(Debug, Clone, PartialEq)]
pub struct InputSnapshot {
    held_keys: HashSet<InputKey>,
    pressed_keys: HashSet<InputKey>,
    released_keys: HashSet<InputKey>,
    held_mouse: HashSet<MouseButton>,
    pressed_mouse: HashSet<MouseButton>,
    released_mouse: HashSet<MouseButton>,
    /// 抽象手柄按钮（M20-02：宿主喂入，设备轮询见 M20-06）。
    held_gamepad: HashSet<u8>,
    pressed_gamepad: HashSet<u8>,
    released_gamepad: HashSet<u8>,
    /// 抽象手柄轴值（-1..=1）。
    gamepad_axes: HashMap<u8, f32>,
    /// 轴的**滞回数字态**（M22/W3）：(轴索引, 方向) → 当前是否越过阈值。
    ///
    /// 设备层每帧写全量（与按钮的按住状态同一套约定）；没有条目的 (轴, 方向)
    /// 回落到按模拟值判定，保证直接 `set_gamepad_axis` 的既有调用语义不变。
    axis_digital: HashMap<(u8, AxisDirection), bool>,
    /// 轴的数字按下/松开边沿（M22/W3，只在变化帧写入）。
    pressed_axes: HashSet<(u8, AxisDirection)>,
    released_axes: HashSet<(u8, AxisDirection)>,
    /// 最近一次产生真实输入的设备类别（M22/W3）。
    last_device: InputDevice,
    cursor_position: (f64, f64),
    cursor_delta: (f64, f64),
    scroll_delta: (f32, f32),
    focused: bool,
}

impl Default for InputSnapshot {
    fn default() -> Self {
        Self {
            held_keys: HashSet::new(),
            pressed_keys: HashSet::new(),
            released_keys: HashSet::new(),
            held_mouse: HashSet::new(),
            pressed_mouse: HashSet::new(),
            released_mouse: HashSet::new(),
            held_gamepad: HashSet::new(),
            pressed_gamepad: HashSet::new(),
            released_gamepad: HashSet::new(),
            gamepad_axes: HashMap::new(),
            axis_digital: HashMap::new(),
            pressed_axes: HashSet::new(),
            released_axes: HashSet::new(),
            last_device: InputDevice::default(),
            cursor_position: (0.0, 0.0),
            cursor_delta: (0.0, 0.0),
            scroll_delta: (0.0, 0.0),
            focused: true,
        }
    }
}

impl InputSnapshot {
    /// Records a keyboard state transition without losing a quick press/release.
    pub fn key_event(&mut self, key: InputKey, is_down: bool) {
        if is_down {
            if self.held_keys.insert(key) {
                self.pressed_keys.insert(key);
                // M22/W3：真实按下才切换「最后使用的设备」，供 UI 提示选说明文案。
                self.last_device = InputDevice::Keyboard;
            }
        } else if self.held_keys.remove(&key) {
            self.released_keys.insert(key);
        }
    }

    /// Records a mouse-button state transition without losing a quick click.
    pub fn mouse_event(&mut self, button: MouseButton, is_down: bool) {
        if is_down {
            if self.held_mouse.insert(button) {
                self.pressed_mouse.insert(button);
                self.last_device = InputDevice::Keyboard;
            }
        } else if self.held_mouse.remove(&button) {
            self.released_mouse.insert(button);
        }
    }

    /// Updates the cursor position and accumulates movement until the next frame boundary.
    pub fn cursor_moved(&mut self, x: f64, y: f64) {
        self.cursor_delta.0 += x - self.cursor_position.0;
        self.cursor_delta.1 += y - self.cursor_position.1;
        self.cursor_position = (x, y);
    }

    /// Updates focus. Losing focus clears held buttons to prevent stuck input.
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.held_keys.clear();
            self.held_mouse.clear();
            // M20-02：失焦同时清手柄按住状态，避免焦点回来后「粘键」。
            self.held_gamepad.clear();
            self.gamepad_axes.clear();
            // M22/W3：轴的数字态同样清空（含边沿）；设备层会在下一帧按实际杆位
            // 重建数字态，因此回来后仍按住的杆会重新产生一次按下边沿。
            self.axis_digital.clear();
            self.pressed_axes.clear();
            self.released_axes.clear();
        }
    }

    /// 喂入抽象手柄按钮状态（M20-02；设备轮询由宿主负责）。
    ///
    /// 语义是**帧内事件**：从松开到按下产生 `just_pressed`，按下到松开产生
    /// `just_released`。宿主每帧重建快照时（编辑器 Play 预览），重复喂「按住」
    /// 会反复产生按下边沿；这种「每帧断言当前按住状态」请用
    /// [`Self::set_gamepad_button_held`]。
    pub fn gamepad_button_event(&mut self, button: u8, is_down: bool) {
        if is_down {
            if self.held_gamepad.insert(button) {
                self.pressed_gamepad.insert(button);
                // M22/W3：真实按下才切换设备类别（空闲帧不写，避免提示被锁死）。
                self.last_device = InputDevice::Gamepad;
            }
        } else {
            self.held_gamepad.remove(&button);
            // 宿主每帧重建快照时「按住」状态本来就是空的，此时仍必须产生松开边沿；
            // 重复喂同一松开事件会重复触发，由设备层只在变化帧喂事件来保证唯一。
            self.released_gamepad.insert(button);
        }
    }

    /// 直接设置手柄按钮的**按住状态**（不产生按下/松开边沿）。
    ///
    /// `down = true` 只保证 `gamepad_button`/`action_is_down` 为真；`down = false`
    /// 清掉按住状态但不产生 `just_released`（松开边沿由
    /// [`Self::gamepad_button_event`] 负责）。设备层用它在同一帧里「重建」持续
    /// 按住的按钮，避免每帧重复触发一次性动作。
    pub fn set_gamepad_button_held(&mut self, button: u8, down: bool) {
        if down {
            self.held_gamepad.insert(button);
        } else {
            self.held_gamepad.remove(&button);
        }
    }

    /// 喂入抽象手柄轴值（钳制到 -1..=1）。
    ///
    /// M22/W3 起轴也有**数字态与边沿**：本方法只写模拟量，
    /// [`Self::set_gamepad_axis_digital`] / [`Self::gamepad_axis_event`] 负责数字态
    /// 与边沿（设备层每帧调用，见 `host::gamepad::GamepadPoller`）。只调用本方法的
    /// 既有测试路径按 `|value| >= AXIS_DIGITAL_ON` 回落判定，语义不变。
    pub fn set_gamepad_axis(&mut self, axis: u8, value: f32) {
        self.gamepad_axes.insert(axis, value.clamp(-1.0, 1.0));
    }

    /// 直接设置轴的**数字按住状态**（不产生边沿；设备层每帧写全量）。
    pub fn set_gamepad_axis_digital(&mut self, axis: u8, direction: AxisDirection, down: bool) {
        self.axis_digital.insert((axis, direction), down);
    }

    /// 喂入轴的按下/松开**边沿**（M22/W3；设备层只在变化帧调用）。
    ///
    /// 同时更新数字按住状态与边沿集合：`is_down = true` 产生 `just_pressed`，
    /// `false` 产生 `just_released`。语义与 [`Self::gamepad_button_event`] 一致
    /// （快速推杆/回中不会丢边沿，边沿保留到本逻辑帧被消费）。
    pub fn gamepad_axis_event(&mut self, axis: u8, direction: AxisDirection, is_down: bool) {
        if is_down {
            if !self
                .axis_digital
                .get(&(axis, direction))
                .copied()
                .unwrap_or(false)
            {
                self.pressed_axes.insert((axis, direction));
            }
            self.axis_digital.insert((axis, direction), true);
            self.last_device = InputDevice::Gamepad;
        } else {
            self.axis_digital.insert((axis, direction), false);
            self.released_axes.insert((axis, direction));
        }
    }

    /// 当前轴数字态（无条目时为 `None`，表示按模拟值回落判定）。
    #[must_use]
    pub fn gamepad_axis_digital(&self, axis: u8, direction: AxisDirection) -> Option<bool> {
        self.axis_digital.get(&(axis, direction)).copied()
    }

    /// 最近一次产生真实输入的设备类别（M22/W3）。
    #[must_use]
    pub const fn last_device(&self) -> InputDevice {
        self.last_device
    }

    #[must_use]
    pub fn gamepad_button(&self, button: u8) -> bool {
        self.held_gamepad.contains(&button)
    }

    #[must_use]
    pub fn gamepad_axis(&self, axis: u8) -> f32 {
        self.gamepad_axes.get(&axis).copied().unwrap_or(0.0)
    }

    /// 动作是否处于按下状态（任一绑定命中）。
    #[must_use]
    pub fn action_is_down(&self, map: &ActionMap, action: &str) -> bool {
        map.bindings(action).iter().any(|binding| {
            binding_is_down(
                *binding,
                &self.held_keys,
                &self.held_mouse,
                &self.held_gamepad,
                &self.gamepad_axes,
                &self.axis_digital,
            )
        })
    }

    /// 动作本帧是否刚按下（任一绑定命中）。
    ///
    /// M22/W3 起**手柄轴也能产生按下边沿**（设备层按滞回阈值合成），因此
    /// `input.action_pressed("ui_up")` 对键盘方向键、DPad 与左摇杆同义。
    #[must_use]
    pub fn action_just_pressed(&self, map: &ActionMap, action: &str) -> bool {
        map.bindings(action).iter().any(|binding| match binding {
            Binding::Key(key) => self.pressed_keys.contains(key),
            Binding::Mouse(button) => self.pressed_mouse.contains(button),
            Binding::GamepadButton(index) => self.pressed_gamepad.contains(index),
            Binding::GamepadAxis(index, direction) => {
                self.pressed_axes.contains(&(*index, *direction))
            }
        })
    }

    /// 动作本帧是否刚松开（任一绑定命中）。
    #[must_use]
    pub fn action_just_released(&self, map: &ActionMap, action: &str) -> bool {
        map.bindings(action).iter().any(|binding| match binding {
            Binding::Key(key) => self.released_keys.contains(key),
            Binding::Mouse(button) => self.released_mouse.contains(button),
            Binding::GamepadButton(index) => self.released_gamepad.contains(index),
            Binding::GamepadAxis(index, direction) => {
                self.released_axes.contains(&(*index, *direction))
            }
        })
    }

    /// 动作是否由**模拟量绑定**（手柄轴过阈值）保持按下（M22/W3）。
    ///
    /// `input.action_repeat` 用它区分「摇杆按住 → 连发」与「键盘/DPad 按住 → 只走
    /// 一步」：数字量没有持续量，保持连发会让一次按键在菜单里连续滚动。
    #[must_use]
    pub fn action_held_by_analog(&self, map: &ActionMap, action: &str) -> bool {
        map.bindings(action).iter().any(|binding| match binding {
            Binding::GamepadAxis(index, direction) => {
                match self.axis_digital.get(&(*index, *direction)) {
                    Some(down) => *down,
                    None => self
                        .gamepad_axes
                        .get(index)
                        .is_some_and(|value| *value * direction.sign() >= AXIS_DIGITAL_ON),
                }
            }
            _ => false,
        })
    }

    /// 两个动作合成的轴（-1..=1）；手柄轴绑定取模拟量，键位取数字量。
    #[must_use]
    pub fn action_axis(&self, map: &ActionMap, negative: &str, positive: &str) -> f32 {
        let mut value = 0.0f32;
        for binding in map.bindings(negative) {
            value -= binding_axis_value(
                *binding,
                &self.held_keys,
                &self.held_mouse,
                &self.held_gamepad,
                &self.gamepad_axes,
                &self.axis_digital,
            );
        }
        for binding in map.bindings(positive) {
            value += binding_axis_value(
                *binding,
                &self.held_keys,
                &self.held_mouse,
                &self.held_gamepad,
                &self.gamepad_axes,
                &self.axis_digital,
            );
        }
        value.clamp(-1.0, 1.0)
    }

    /// Records a scroll-wheel event (accumulated until the next frame).
    pub fn scroll_event(&mut self, dx: f32, dy: f32) {
        self.scroll_delta.0 += dx;
        self.scroll_delta.1 += dy;
    }

    /// Clears edge transitions and cursor/scroll deltas after the frame.
    pub fn finish_frame(&mut self) {
        self.pressed_keys.clear();
        self.released_keys.clear();
        self.pressed_mouse.clear();
        self.released_mouse.clear();
        self.pressed_gamepad.clear();
        self.released_gamepad.clear();
        self.pressed_axes.clear();
        self.released_axes.clear();
        self.cursor_delta = (0.0, 0.0);
        self.scroll_delta = (0.0, 0.0);
    }

    /// Clears held and transient state, used when pausing or losing focus.
    pub fn clear_all(&mut self) {
        self.held_keys.clear();
        self.held_mouse.clear();
        self.held_gamepad.clear();
        self.gamepad_axes.clear();
        self.axis_digital.clear();
        self.clear_transitions();
    }

    /// Clears transient transitions, used when entering a paused state.
    pub fn clear_transitions(&mut self) {
        self.pressed_keys.clear();
        self.released_keys.clear();
        self.pressed_mouse.clear();
        self.released_mouse.clear();
        self.pressed_gamepad.clear();
        self.released_gamepad.clear();
        self.pressed_axes.clear();
        self.released_axes.clear();
        self.cursor_delta = (0.0, 0.0);
        self.scroll_delta = (0.0, 0.0);
    }

    #[must_use]
    pub fn is_down(&self, key: InputKey) -> bool {
        self.held_keys.contains(&key)
    }

    #[must_use]
    pub fn just_pressed(&self, key: InputKey) -> bool {
        self.pressed_keys.contains(&key)
    }

    #[must_use]
    pub fn just_released(&self, key: InputKey) -> bool {
        self.released_keys.contains(&key)
    }

    #[must_use]
    pub fn mouse_is_down(&self, button: MouseButton) -> bool {
        self.held_mouse.contains(&button)
    }

    #[must_use]
    pub fn mouse_just_pressed(&self, button: MouseButton) -> bool {
        self.pressed_mouse.contains(&button)
    }

    #[must_use]
    pub fn mouse_just_released(&self, button: MouseButton) -> bool {
        self.released_mouse.contains(&button)
    }

    #[must_use]
    pub fn axis(&self, axis: &str) -> f32 {
        match axis {
            "horizontal" => {
                self.is_down(InputKey::D) as i32 as f32 - self.is_down(InputKey::A) as i32 as f32
            }
            "vertical" => {
                self.is_down(InputKey::W) as i32 as f32 - self.is_down(InputKey::S) as i32 as f32
            }
            _ => 0.0,
        }
    }

    #[must_use]
    pub const fn cursor_position(&self) -> (f64, f64) {
        self.cursor_position
    }

    #[must_use]
    pub const fn cursor_delta(&self) -> (f64, f64) {
        self.cursor_delta
    }

    #[must_use]
    pub const fn scroll_delta(&self) -> (f32, f32) {
        self.scroll_delta
    }

    #[must_use]
    pub const fn is_focused(&self) -> bool {
        self.focused
    }
}

// ---------------------------------------------------------------------------
// 手柄死区与轴判定（M22：自 `host::gamepad` 下沉）
//
// 这些是**纯值类型与纯函数**，不依赖任何平台 API；原先放在 `host/gamepad.rs`
// 导致 `core::settings` 反向依赖 `host`（docs/37 P1-1）。下沉后 core 可以独立编译
// 到 wasm/headless-only 目标，`host::gamepad` 只做重导出以保持既有路径可用。
// ---------------------------------------------------------------------------

/// 左/右摇杆默认死区。
pub const STICK_DEADZONE: f32 = 0.15;
/// 扳机默认死区。
pub const TRIGGER_DEADZONE: f32 = 0.05;
/// 死区可配置上限：必须严格小于 1（等于 1 会把满量程也归零）。
pub const MAX_DEADZONE: f32 = 0.9;

/// 轴判定的按下阈值（模拟量绝对值达到它才算「按下」）。
///
/// 与 [`AXIS_DIGITAL_OFF`] 一起构成**滞回**：进入用 ON、退出用 OFF，避免摇杆在
/// 阈值附近抖动时反复产生按下/松开边沿（M22 起轴也有数字边沿，见
/// [`InputSnapshot::gamepad_axis_event`]）。
pub const AXIS_DIGITAL_ON: f32 = 0.5;
/// 轴判定的松开阈值（严格小于 [`AXIS_DIGITAL_ON`]）。
pub const AXIS_DIGITAL_OFF: f32 = 0.4;
/// 滞回宽度：可配置进入阈值时，退出阈值 = 进入阈值 − 本常量（下限 0）。
pub const AXIS_HYSTERESIS: f32 = AXIS_DIGITAL_ON - AXIS_DIGITAL_OFF;

/// 摇杆/扳机死区（M20-08：可在 `settings.toml` 的 `[input]` 段读写；默认值仍是
/// [`STICK_DEADZONE`] / [`TRIGGER_DEADZONE`]）。
///
/// 语义与 [`apply_stick_deadzone`] / [`apply_stick_deadzone_radial`] /
/// [`apply_trigger_deadzone`] 一致：死区内归零（消除回中漂移），死区外按比例
/// 重映射到 0..1。非有限值与超界值在构造时就被钳制，因此从设置文件读进来的脏
/// 数据不会产生非法曲线。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GamepadDeadzone {
    /// 左/右摇杆死区（0..=MAX_DEADZONE）。
    pub stick: f32,
    /// 扳机死区（0..=MAX_DEADZONE）。
    pub trigger: f32,
}

impl GamepadDeadzone {
    /// 引擎默认死区（0.15 / 0.05）。
    pub const DEFAULT: Self = Self {
        stick: STICK_DEADZONE,
        trigger: TRIGGER_DEADZONE,
    };

    /// 构造并钳制到 `0..=MAX_DEADZONE`（非有限值回落默认值）。
    #[must_use]
    pub fn new(stick: f32, trigger: f32) -> Self {
        Self {
            stick: sanitize_deadzone(stick, STICK_DEADZONE),
            trigger: sanitize_deadzone(trigger, TRIGGER_DEADZONE),
        }
    }

    /// 与默认值比较（编辑器「死区是否被改过」判断用）。
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::DEFAULT
    }
}

impl Default for GamepadDeadzone {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn sanitize_deadzone(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, MAX_DEADZONE)
    } else {
        fallback
    }
}

/// 单轴死区：死区内归零，死区外按比例映射到 0..1（保持符号）。
#[must_use]
pub fn apply_stick_deadzone(value: f32, deadzone: f32) -> f32 {
    apply_axis_deadzone(value, deadzone)
}

/// 扳机死区（输入 0..1）。
#[must_use]
pub fn apply_trigger_deadzone(value: f32, deadzone: f32) -> f32 {
    apply_axis_deadzone(value, deadzone)
}

fn apply_axis_deadzone(value: f32, deadzone: f32) -> f32 {
    let clamped = value.clamp(-1.0, 1.0);
    if clamped.abs() <= deadzone {
        return 0.0;
    }
    let span = (1.0 - deadzone).max(f32::EPSILON);
    let magnitude = (clamped.abs() - deadzone) / span;
    magnitude.clamp(0.0, 1.0) * clamped.signum()
}

/// **径向**摇杆死区（M22）：按 (x, y) 的模长判定，而不是逐轴判定。
///
/// 逐轴死区会让斜向比正向灵敏（x、y 各自刚过死区就同时输出），径向死区按杆的
/// 实际偏移量处理：模长 ≤ `deadzone` 归零，否则以 `(mag - dz) / (1 - dz)` 缩放并
/// **保持方向**。返回值已钳制到 `-1..=1`。
#[must_use]
pub fn apply_stick_deadzone_radial(x: f32, y: f32, deadzone: f32) -> (f32, f32) {
    let x = x.clamp(-1.0, 1.0);
    let y = y.clamp(-1.0, 1.0);
    let magnitude = (x * x + y * y).sqrt();
    if magnitude <= deadzone {
        return (0.0, 0.0);
    }
    let span = (1.0 - deadzone).max(f32::EPSILON);
    let scale = ((magnitude - deadzone) / span).clamp(0.0, 1.0) / magnitude;
    ((x * scale).clamp(-1.0, 1.0), (y * scale).clamp(-1.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::{
        ACTION_MAP_VERSION, AXIS_DIGITAL_OFF, AXIS_DIGITAL_ON, ActionMap, ActionMapError,
        AxisDirection, Binding, GamepadDeadzone, InputDevice, InputKey, InputSnapshot, MouseButton,
        apply_stick_deadzone_radial,
    };

    #[test]
    fn from_char_gives_each_letter_a_distinct_key() {
        assert_eq!(InputKey::from_char('w'), InputKey::W);
        assert_eq!(InputKey::from_char('a'), InputKey::A);
        assert_eq!(InputKey::from_char('s'), InputKey::S);
        assert_eq!(InputKey::from_char('d'), InputKey::D);
        // 其余字母键按小写 ASCII 码区分，Q/E/R/T/F/G 互不相同。
        assert_eq!(InputKey::from_char('q'), InputKey::Other(113));
        assert_eq!(InputKey::from_char('Q'), InputKey::Other(113));
        assert_eq!(InputKey::from_char('e'), InputKey::Other(101));
        assert_eq!(InputKey::from_char('t'), InputKey::Other(116));
        assert!(InputKey::from_char('q') != InputKey::from_char('e'));
    }

    #[test]
    fn preserves_press_and_release_until_frame_boundary() {
        let mut input = InputSnapshot::default();
        input.key_event(InputKey::Space, true);
        input.key_event(InputKey::Space, false);
        assert!(input.just_pressed(InputKey::Space));
        assert!(input.just_released(InputKey::Space));
        assert!(!input.is_down(InputKey::Space));
        input.finish_frame();
        assert!(!input.just_pressed(InputKey::Space));
        assert!(!input.just_released(InputKey::Space));
    }

    #[test]
    fn computes_wasd_axes_from_held_state() {
        let mut input = InputSnapshot::default();
        input.key_event(InputKey::D, true);
        input.key_event(InputKey::S, true);
        assert_eq!(input.axis("horizontal"), 1.0);
        assert_eq!(input.axis("vertical"), -1.0);
        input.key_event(InputKey::A, true);
        assert_eq!(input.axis("horizontal"), 0.0);
    }

    #[test]
    fn focus_loss_clears_held_state_and_pause_clears_edges() {
        let mut input = InputSnapshot::default();
        input.key_event(InputKey::W, true);
        input.mouse_event(MouseButton::Left, true);
        input.set_focused(false);
        assert!(!input.is_down(InputKey::W));
        assert!(!input.mouse_is_down(MouseButton::Left));
        input.key_event(InputKey::Space, true);
        input.clear_transitions();
        assert!(!input.just_pressed(InputKey::Space));
    }

    #[test]
    fn accumulates_and_resets_scroll_delta() {
        let mut input = InputSnapshot::default();
        input.scroll_event(0.0, 1.0);
        input.scroll_event(0.0, 2.0);
        assert_eq!(input.scroll_delta(), (0.0, 3.0));
        input.finish_frame();
        assert_eq!(input.scroll_delta(), (0.0, 0.0));
    }

    #[test]
    fn accumulates_cursor_delta() {
        let mut input = InputSnapshot::default();
        input.cursor_moved(10.0, 20.0);
        input.cursor_moved(13.0, 18.0);
        assert_eq!(input.cursor_position(), (13.0, 18.0));
        assert_eq!(input.cursor_delta(), (13.0, 18.0));
        input.finish_frame();
        assert_eq!(input.cursor_delta(), (0.0, 0.0));
    }

    #[test]
    fn default_action_map_covers_game_keys_and_gamepad() {
        let map = ActionMap::with_defaults();
        assert_eq!(
            map.bindings("interact"),
            [Binding::Key(InputKey::from_char('e'))]
        );
        assert_eq!(map.bindings("move_up"), [Binding::Key(InputKey::W)]);
        // confirm 同时绑定空格与手柄 A 键（基础手柄抽象）。
        assert!(
            map.bindings("confirm")
                .contains(&Binding::Key(InputKey::Space))
        );
        assert!(map.bindings("confirm").contains(&Binding::GamepadButton(0)));
        // 菜单导航按方向拆成四个动作：方向键 + 左摇杆方向 + DPad（可合成带符号轴）。
        assert_eq!(map.bindings("ui_up").len(), 3);
        assert!(
            map.bindings("ui_up")
                .contains(&Binding::Key(InputKey::ARROW_UP))
        );
        assert!(
            map.bindings("ui_up")
                .contains(&Binding::GamepadAxis(1, AxisDirection::Positive))
        );
        assert!(
            map.bindings("ui_up").contains(&Binding::GamepadButton(12)),
            "DPad 上 = 按钮 12（M20-06 契约）"
        );
        assert!(
            map.bindings("ui_down")
                .contains(&Binding::Key(InputKey::ARROW_DOWN))
        );
        assert!(
            map.bindings("ui_down")
                .contains(&Binding::GamepadButton(13))
        );
        assert!(
            map.bindings("ui_left")
                .contains(&Binding::GamepadButton(14))
        );
        assert!(
            map.bindings("ui_right")
                .contains(&Binding::GamepadButton(15))
        );
        assert!(map.actions().contains(&"pause"));
        // M20-08 扩表：建造/采集/任务/重开/选种子/音量/数字键全部成为命名动作，
        // 且默认表**自身零冲突**（否则编辑器面板一打开就报冲突）。
        for action in [
            "build",
            "place",
            "demolish",
            "use",
            "quest",
            "respawn",
            "seed",
            "volume_up",
            "volume_down",
            "slot_1",
            "slot_9",
        ] {
            assert!(
                !map.bindings(action).is_empty(),
                "M20-08 默认动作缺失：{action}"
            );
        }
        assert_eq!(
            map.bindings("build"),
            [Binding::Key(InputKey::from_char('b'))]
        );
        assert_eq!(
            map.bindings("volume_up"),
            [Binding::Key(InputKey::from_char('+'))]
        );
        assert_eq!(
            map.bindings("slot_3"),
            [Binding::Key(InputKey::from_char('3'))]
        );
        assert_eq!(map.actions().len(), 36, "默认动作总数");
        assert_eq!(map.conflicts(), Vec::new(), "默认绑定不得跨动作重复");
        assert_eq!(map.validate_bindings(), Ok(()));
    }

    #[test]
    fn cross_action_duplicate_bindings_are_reported_and_block_validation() {
        let mut map = ActionMap::with_defaults();
        // 同动作内重复 → `bind` 直接报 DuplicateBinding（既有语义）。
        assert_eq!(
            map.bind("interact", Binding::Key(InputKey::from_char('e'))),
            Err(ActionMapError::DuplicateBinding {
                action: "interact".into(),
                binding: "key:e".into(),
            })
        );
        // 跨动作重复 → `bind` 允许（一次 rebind 覆盖后可能撞车），但 conflicts/
        // validate_bindings 必须报出来，编辑器据此阻止保存。
        map.bind("attack", Binding::Key(InputKey::from_char('e')))
            .expect("跨动作重复本身不阻止写入");
        let conflicts = map.conflicts();
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert_eq!(conflicts[0].binding, Binding::Key(InputKey::from_char('e')));
        assert_eq!(conflicts[0].actions, vec!["attack", "interact"]);
        assert!(conflicts[0].describe().contains("key:e"), "{conflicts:?}");
        assert_eq!(
            map.validate_bindings(),
            Err(ActionMapError::DuplicateBinding {
                action: "attack".into(),
                binding: "key:e".into(),
            })
        );
        // 解绑冲突项后校验恢复通过；输出顺序按绑定文本稳定排序。
        map.bind("skill", Binding::Key(InputKey::from_char('e')))
            .expect("第二处重复");
        assert!(map.unbind("attack", Binding::Key(InputKey::from_char('e'))));
        let conflicts = map.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].actions, vec!["interact", "skill"]);
        assert!(map.unbind("skill", Binding::Key(InputKey::from_char('e'))));
        assert!(map.conflicts().is_empty());
        assert_eq!(map.validate_bindings(), Ok(()));
    }

    #[test]
    fn action_queries_follow_bindings_and_survive_rebinding() {
        let mut map = ActionMap::with_defaults();
        let mut input = InputSnapshot::default();
        input.key_event(InputKey::from_char('e'), true);
        assert!(input.action_is_down(&map, "interact"));
        assert!(input.action_just_pressed(&map, "interact"));
        assert!(!input.action_is_down(&map, "attack"));

        // 重绑定：把 interact 从 E 改到 F，旧键失效、新键生效。
        assert!(map.unbind("interact", Binding::Key(InputKey::from_char('e'))));
        map.bind("interact", Binding::Key(InputKey::from_char('f')))
            .unwrap();
        assert!(!input.action_is_down(&map, "interact"));
        input.key_event(InputKey::from_char('f'), true);
        assert!(input.action_is_down(&map, "interact"));
        assert!(input.action_just_pressed(&map, "interact"));

        // 重复绑定同一输入 → 明确错误。
        assert_eq!(
            map.bind("interact", Binding::Key(InputKey::from_char('f'))),
            Err(ActionMapError::DuplicateBinding {
                action: "interact".into(),
                binding: "key:f".into(),
            })
        );
        assert_eq!(
            map.bind("  ", Binding::Key(InputKey::W)),
            Err(ActionMapError::EmptyAction)
        );
    }

    #[test]
    fn action_map_round_trips_through_settings_text() {
        let mut map = ActionMap::with_defaults();
        map.unbind("interact", Binding::Key(InputKey::from_char('e')));
        map.bind("interact", Binding::Key(InputKey::Other(102)))
            .unwrap();
        map.bind(
            "menu_confirm",
            Binding::GamepadAxis(1, AxisDirection::Negative),
        )
        .unwrap();
        let text = map.to_text();
        assert!(text.starts_with(ACTION_MAP_VERSION));
        let parsed = ActionMap::from_text(&text).unwrap();
        assert_eq!(parsed, map);

        // 版本头不符 / 绑定非法 → 明确错误，不静默放过。
        assert_eq!(
            ActionMap::from_text("rustlua-actions-v0\ninteract=key:e\n"),
            Err(ActionMapError::UnsupportedVersion(
                "rustlua-actions-v0".into()
            ))
        );
        assert!(matches!(
            ActionMap::from_text("rustlua-actions-v1\ninteract=joystick:1\n"),
            Err(ActionMapError::InvalidBinding(_))
        ));

        // 部分映射 + 默认兜底：用户重绑定不被覆盖。
        let mut partial = ActionMap::new();
        partial
            .bind("interact", Binding::Key(InputKey::from_char('f')))
            .unwrap();
        partial.merge_missing(&ActionMap::with_defaults());
        assert_eq!(
            partial.bindings("interact"),
            [Binding::Key(InputKey::from_char('f'))]
        );
        assert_eq!(partial.bindings("move_up"), [Binding::Key(InputKey::W)]);
    }

    #[test]
    fn arrow_keys_use_readable_binding_text_and_round_trip() {
        let map = ActionMap::with_defaults();
        // 默认表里方向键绑定必须是可读文本，不是内部码位。
        assert_eq!(map.bindings("ui_up")[0].to_text(), "key:up");
        assert_eq!(map.bindings("ui_down")[0].to_text(), "key:down");
        assert_eq!(map.bindings("ui_left")[0].to_text(), "key:left");
        assert_eq!(map.bindings("ui_right")[0].to_text(), "key:right");
        assert_eq!(InputKey::ARROW_UP.arrow_name(), Some("up"));
        assert_eq!(InputKey::from_char('e').arrow_name(), None);

        // 文本往返后仍是同一映射（M20-08 起 ui_* 还带 DPad 按钮 12..15）。
        let text = map.to_text();
        assert!(text.contains("ui_up=key:up,padaxis:1+,pad:12"), "{text}");
        assert_eq!(ActionMap::from_text(&text).unwrap(), map);

        // 旧设置文件里的码位写法仍可解析（向后兼容），并归一化为可读文本。
        let legacy = ActionMap::from_text("rustlua-actions-v1\nui_up=key:other:1048577\n").unwrap();
        assert_eq!(legacy.bindings("ui_up"), [Binding::Key(InputKey::ARROW_UP)]);
        assert_eq!(legacy.to_text(), "rustlua-actions-v1\nui_up=key:up");
    }

    #[test]
    fn gamepad_bindings_drive_actions_and_axes() {
        let map = ActionMap::with_defaults();
        let mut input = InputSnapshot::default();
        input.gamepad_button_event(0, true);
        assert!(input.action_is_down(&map, "confirm"));
        assert!(input.action_just_pressed(&map, "confirm"));
        input.finish_frame();
        input.gamepad_button_event(0, false);
        assert!(input.action_just_released(&map, "confirm"));
        assert!(!input.action_is_down(&map, "confirm"));

        // 轴：0.5 判定阈值，符号由绑定方向决定（默认映射按方向拆分动作）。
        input.set_gamepad_axis(1, 0.8);
        assert!(input.action_is_down(&map, "ui_up"));
        assert!(!input.action_is_down(&map, "ui_down"));
        input.set_gamepad_axis(1, 0.2);
        assert!(!input.action_is_down(&map, "ui_up"));
        input.set_gamepad_axis(1, 2.5);
        assert_eq!(input.gamepad_axis(1), 1.0, "轴值需钳制到 -1..=1");

        // 摇杆合成轴：一对相反方向的轴动作相加（模拟量，而非 0/1）——
        // 这正是默认 `ui_up`/`ui_down` 的用法。
        let mut stick_map = ActionMap::with_defaults();
        stick_map.clear("ui_up");
        stick_map.clear("ui_down");
        stick_map
            .bind("stick_up", Binding::GamepadAxis(1, AxisDirection::Negative))
            .unwrap();
        stick_map
            .bind(
                "stick_down",
                Binding::GamepadAxis(1, AxisDirection::Positive),
            )
            .unwrap();
        input.set_gamepad_axis(1, 0.8);
        assert!((input.action_axis(&stick_map, "stick_up", "stick_down") - 0.8).abs() < 1e-6);
        input.set_gamepad_axis(1, -0.6);
        assert!((input.action_axis(&stick_map, "stick_up", "stick_down") + 0.6).abs() < 1e-6);

        // 默认映射直接可用：不清理绑定也能得到带符号的垂直轴。
        // 约定：`action_axis(negative, positive)` 首参是**负向**动作，
        // 因此垂直方向写作 ("ui_down", "ui_up")（上为 +）。
        input.set_gamepad_axis(1, 0.7);
        assert!((input.action_axis(&map, "ui_down", "ui_up") - 0.7).abs() < 1e-6);
        input.set_gamepad_axis(1, -0.7);
        assert!((input.action_axis(&map, "ui_down", "ui_up") + 0.7).abs() < 1e-6);
        // 水平方向：左负右正，直接按 (left, right) 组合。
        input.set_gamepad_axis(0, 0.6);
        assert!((input.action_axis(&map, "ui_left", "ui_right") - 0.6).abs() < 1e-6);
    }

    #[test]
    fn focus_loss_clears_gamepad_state_and_key_axes_combine() {
        let map = ActionMap::with_defaults();
        let mut input = InputSnapshot::default();
        input.gamepad_button_event(0, true);
        input.set_gamepad_axis(1, 0.9);
        input.set_focused(false);
        assert!(!input.action_is_down(&map, "confirm"));
        assert_eq!(input.gamepad_axis(1), 0.0);
        assert!(!input.is_focused());

        // 键位轴：相反方向同时按下归零，单方向为 ±1。
        let mut input = InputSnapshot::default();
        input.key_event(InputKey::D, true);
        assert_eq!(input.action_axis(&map, "move_left", "move_right"), 1.0);
        input.key_event(InputKey::A, true);
        assert_eq!(input.action_axis(&map, "move_left", "move_right"), 0.0);
        input.key_event(InputKey::D, false);
        assert_eq!(input.action_axis(&map, "move_left", "move_right"), -1.0);
    }

    /// M22/W3：轴的**数字态 + 边沿**驱动动作，语义与键盘/按钮一致。
    #[test]
    fn axis_digital_state_and_edges_drive_actions() {
        let map = ActionMap::with_defaults();
        let mut input = InputSnapshot::default();

        // 设备层：轴值 + 越过阈值的数字态与按下边沿。
        input.set_gamepad_axis(1, 0.8);
        input.gamepad_axis_event(1, AxisDirection::Positive, true);
        assert!(input.action_is_down(&map, "ui_up"));
        assert!(input.action_just_pressed(&map, "ui_up"));
        assert!(!input.action_is_down(&map, "ui_down"));
        assert_eq!(
            input.gamepad_axis_digital(1, AxisDirection::Positive),
            Some(true)
        );

        // 按住期间：边沿只出现一次（快照保留到本逻辑帧被消费）。
        input.finish_frame();
        assert!(input.action_is_down(&map, "ui_up"));
        assert!(!input.action_just_pressed(&map, "ui_up"));

        // 回中：松开边沿，动作不再按下。
        input.set_gamepad_axis(1, 0.1);
        input.gamepad_axis_event(1, AxisDirection::Positive, false);
        assert!(input.action_just_released(&map, "ui_up"));
        assert!(!input.action_is_down(&map, "ui_up"));
        input.finish_frame();
        assert!(!input.action_just_released(&map, "ui_up"));

        // 负方向同理（轴值 -0.9 → ui_down）。
        input.set_gamepad_axis(1, -0.9);
        input.gamepad_axis_event(1, AxisDirection::Negative, true);
        assert!(input.action_is_down(&map, "ui_down"));
        assert!(input.action_just_pressed(&map, "ui_down"));
        assert!(!input.action_is_down(&map, "ui_up"));
    }

    /// 没有设备层数字态时（测试/宿主直接 `set_gamepad_axis`），回落按阈值判定，
    /// 且**不产生边沿**——M20 的既有语义不变。
    #[test]
    fn axis_without_device_state_falls_back_to_analog_threshold() {
        let map = ActionMap::with_defaults();
        let mut input = InputSnapshot::default();
        input.set_gamepad_axis(1, 0.8);
        assert!(input.action_is_down(&map, "ui_up"));
        assert!(!input.action_just_pressed(&map, "ui_up"));
        assert_eq!(input.gamepad_axis_digital(1, AxisDirection::Positive), None);

        input.set_gamepad_axis(1, 0.49);
        assert!(!input.action_is_down(&map, "ui_up"), "0.49 未过 0.5 阈值");
        // 阈值常量与 Lua 侧文档一致（编译期断言：避免被当成运行时条件）。
        const { assert!(AXIS_DIGITAL_ON == 0.5) };
        const { assert!(AXIS_DIGITAL_OFF < AXIS_DIGITAL_ON) };
    }

    /// M22/W3：径向死区按 (x, y) 模长判定，消除「斜向比正向灵敏」。
    #[test]
    fn radial_deadzone_scales_by_magnitude_and_keeps_direction() {
        // 死区内（含斜向小位移）归零。
        assert_eq!(
            apply_stick_deadzone_radial(0.0, 0.0, GamepadDeadzone::DEFAULT.stick),
            (0.0, 0.0)
        );
        assert_eq!(apply_stick_deadzone_radial(0.1, 0.1, 0.15), (0.0, 0.0));
        // 正向：与逐轴结果一致（模长 = 单轴绝对值）。
        let (x, y) = apply_stick_deadzone_radial(0.6, 0.0, 0.15);
        assert!((x - 0.529_411_8).abs() < 1e-4, "x={x}");
        assert_eq!(y, 0.0);
        // 斜向：与同模长的单轴输入得到相同**幅度**（逐轴死区下斜向会更大）。
        let diagonal = (0.6_f32, 0.6_f32);
        let magnitude = (diagonal.0 * diagonal.0 + diagonal.1 * diagonal.1).sqrt();
        let (dx, dy) = apply_stick_deadzone_radial(diagonal.0, diagonal.1, 0.15);
        let (axis_x, _) = apply_stick_deadzone_radial(magnitude, 0.0, 0.15);
        let diagonal_magnitude = (dx * dx + dy * dy).sqrt();
        assert!(
            (diagonal_magnitude - axis_x).abs() < 1e-4,
            "斜向模长 {diagonal_magnitude} 应与同模长单轴 {axis_x} 一致"
        );
        assert!((dx - dy).abs() < 1e-6, "方向保持（45° 输入仍 45°）");
        // 极值不越界：满量程斜向仍落在单位圆内。
        let (mx, my) = apply_stick_deadzone_radial(1.0, 1.0, 0.15);
        assert!(mx * mx + my * my <= 1.0 + 1e-6, "({mx}, {my}) 不得越界");
    }

    /// M22/W3：`last_device` 只在**真实边沿**上切换，空闲帧（轴每帧上报 0.0）不改写。
    #[test]
    fn last_device_follows_real_input_edges_only() {
        let mut input = InputSnapshot::default();
        assert_eq!(input.last_device(), InputDevice::Keyboard, "默认键鼠");

        input.gamepad_button_event(0, true);
        assert_eq!(input.last_device(), InputDevice::Gamepad);

        input.key_event(InputKey::W, true);
        assert_eq!(input.last_device(), InputDevice::Keyboard);
        assert_eq!(input.last_device().to_text(), "keyboard");

        // 轴事件（真实推杆）切到手柄；单纯写模拟量不算输入。
        input.gamepad_axis_event(1, AxisDirection::Positive, true);
        assert_eq!(input.last_device(), InputDevice::Gamepad);
        input.set_gamepad_axis(1, 0.0);
        input.gamepad_axis_event(1, AxisDirection::Positive, false);
        assert_eq!(input.last_device(), InputDevice::Gamepad, "松开不切回键鼠");
        assert_eq!(input.last_device().to_text(), "gamepad");

        input.mouse_event(MouseButton::Left, true);
        assert_eq!(input.last_device(), InputDevice::Keyboard, "鼠标算键鼠一侧");
    }

    /// M22/W3：失焦清空轴数字态与边沿（不粘杆）。
    #[test]
    fn focus_loss_clears_axis_digital_state() {
        let map = ActionMap::with_defaults();
        let mut input = InputSnapshot::default();
        input.set_gamepad_axis(1, 0.9);
        input.gamepad_axis_event(1, AxisDirection::Positive, true);
        assert!(input.action_is_down(&map, "ui_up"));

        input.set_focused(false);
        assert_eq!(input.gamepad_axis_digital(1, AxisDirection::Positive), None);
        assert!(!input.action_is_down(&map, "ui_up"), "失焦后不粘杆");
        assert!(!input.action_just_pressed(&map, "ui_up"));
    }
}
