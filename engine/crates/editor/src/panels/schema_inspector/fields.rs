use super::*;

/// 一行字段的输入缓冲 + 上次同步的文档取值。
///
/// 字段对 `panel.rs` 可见（P4-② 拆目录后由两个文件共用；这是同一模块内的拆文件，
/// 不是新的对外 API）。
#[derive(Clone)]
pub(crate) struct FieldBuffer {
    /// 上次从文档同步过来的文本（用于判断文档是否变了）。
    pub(crate) seed: String,
    /// 当前输入框内容。
    pub(crate) text: String,
    /// 上一帧是否持有焦点（持有焦点时不覆盖用户输入）。
    pub(crate) focused: bool,
}

/// 字段该用哪种控件（**纯函数**，因此"控件选择"可以脱离 egui 单测）。
///
/// 依据是 `FieldKind` + 字段名：颜色类字段（`color` / `tint` / `fill`）用取色器，
/// 数值用 `DragValue`，向量与 `宽x高` 用多个 `DragValue` 连排——这样"把某个组件的
/// 手写面板换成 schema 生成"就**不会降低操作体验**（手写面板用的也是这些控件），
/// 这是迁移面板的前提条件。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FieldWidget {
    /// 勾选框（`Bool`）。
    Checkbox,
    /// 下拉（`Enum`）。
    Choice,
    /// 单个数值（`Int`/`Number`）；`integer` 决定是否按整数步进。
    Number { integer: bool },
    /// 多分量（`Vec2/Vec3/Vec4`）；`size` 表示 `宽x高` 写法（`x` 分隔）。
    Vector { parts: usize, size: bool },
    /// 取色器（分量数 3 或 4）。
    Color { parts: usize },
    /// 文本框（`Text`，含路径/资源引用等带规则的字段）。
    Text,
}

/// 颜色类字段名（与引擎编码器里那些 0~1 分量的字段对应）。
pub(crate) const COLOR_FIELD_NAMES: &[&str] = &["color", "tint", "fill", "background", "bg"];

#[must_use]
pub(crate) fn field_widget(kind: FieldKind, field: &str) -> FieldWidget {
    // 候选值来自**项目资源**的字段（字体）即使类型是文本，也用下拉：这是"手写面板的字体下拉"
    // 在通用编辑器里的等价物。
    if field_choice_source(field) != ChoiceSource::Static {
        return FieldWidget::Choice;
    }
    let is_color = COLOR_FIELD_NAMES
        .iter()
        .any(|name| field.eq_ignore_ascii_case(name));
    match kind {
        FieldKind::Bool => FieldWidget::Checkbox,
        FieldKind::Enum => FieldWidget::Choice,
        FieldKind::Int => FieldWidget::Number { integer: true },
        FieldKind::Number => FieldWidget::Number { integer: false },
        FieldKind::Size => FieldWidget::Vector {
            parts: 2,
            size: true,
        },
        FieldKind::Color => FieldWidget::Color { parts: 4 },
        FieldKind::Vec2 | FieldKind::Vec3 | FieldKind::Vec4 if is_color => FieldWidget::Color {
            parts: match kind {
                FieldKind::Vec2 => 2,
                FieldKind::Vec3 => 3,
                _ => 4,
            },
        },
        FieldKind::Vec2 => FieldWidget::Vector {
            parts: 2,
            size: false,
        },
        FieldKind::Vec3 => FieldWidget::Vector {
            parts: 3,
            size: false,
        },
        FieldKind::Vec4 => FieldWidget::Vector {
            parts: 4,
            size: false,
        },
        FieldKind::Text => FieldWidget::Text,
    }
}
