//! `Toggle`：复选/单选开关（对齐 Unity `Toggle`）。
//!
//! 组件串：`on=true;group=Sound;checkmark=Checkmark`（三个字段都可省）。
//!
//! * `on`：当前状态；默认 `true` 与 Unity 新建 Toggle 一致。
//! * `group`：非空时是单选组名——同组内至多一个 `on`，宿主在切换时关掉兄弟项。
//! * `checkmark`：子实体名，`on` 时铺满控件矩形，`off` 时收成零尺寸。
//!
//! 控件自身只占一块布局矩形；勾选图形由子实体画，与 Unity 的 `graphic` 字段同一分工。

use crate::ui::{escape_text, split_fields, unescape_text};
use crate::ui_layout::UiRect;
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::world::World;

#[derive(Clone, Debug, PartialEq)]
pub struct Toggle {
    pub is_on: bool,
    pub group: String,
    pub checkmark: String,
}

impl Default for Toggle {
    fn default() -> Self {
        Self {
            is_on: true,
            group: String::new(),
            checkmark: String::new(),
        }
    }
}

impl Toggle {
    pub fn from_component_str(raw: &str) -> Result<Self, String> {
        let mut toggle = Self::default();
        let mut on_seen = false;
        let mut group_seen = false;
        let mut checkmark_seen = false;
        for field in split_fields(raw) {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let Some((key, value)) = field.split_once('=') else {
                return Err(format!("expected key=value, got `{field}`"));
            };
            let value = value.trim();
            match key {
                "on" => {
                    if on_seen {
                        return Err("duplicate `on=`".to_owned());
                    }
                    on_seen = true;
                    toggle.is_on = match value {
                        "true" => true,
                        "false" => false,
                        other => return Err(format!("`on` must be true or false, got `{other}`")),
                    };
                }
                "group" => {
                    if group_seen {
                        return Err("duplicate `group=`".to_owned());
                    }
                    group_seen = true;
                    toggle.group = unescape_text(value);
                }
                "checkmark" => {
                    if checkmark_seen {
                        return Err("duplicate `checkmark=`".to_owned());
                    }
                    checkmark_seen = true;
                    toggle.checkmark = unescape_text(value);
                }
                other => return Err(format!("unknown Toggle field `{other}`")),
            }
        }
        Ok(toggle)
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        let mut encoded = format!("on={}", self.is_on);
        if !self.group.is_empty() {
            encoded.push_str(&format!(";group={}", escape_text(&self.group)));
        }
        if !self.checkmark.is_empty() {
            encoded.push_str(&format!(";checkmark={}", escape_text(&self.checkmark)));
        }
        encoded
    }

    #[must_use]
    pub fn normalized(&self) -> f32 {
        if self.is_on { 1.0 } else { 0.0 }
    }

    /// 勾选图形矩形：开=控件矩形，关=控件原点上的零尺寸矩形
    /// （不挪到屏幕原点，避免零尺寸四边形在左上角留下一个点）。
    #[must_use]
    pub fn checkmark_rect(&self, control: UiRect) -> UiRect {
        if self.is_on {
            control
        } else {
            UiRect::new(control.x, control.y, 0.0, 0.0)
        }
    }

    /// 返回状态是否真的变了（宿主据此决定要不要发 `value_changed`）。
    pub fn set_on(&mut self, on: bool) -> bool {
        let changed = self.is_on != on;
        self.is_on = on;
        changed
    }
}

/// 切换一个 Toggle；`group` 非空且打开时把同组兄弟关掉（Unity `ToggleGroup` 语义）。
///
/// 返回**实际变化**的实体（自己在前，兄弟在后）。宿主与 Lua 写值都走这一处，
/// 否则"鼠标点是单选、脚本写值是复选"会成为第二套语义。
pub fn set_toggle_on(world: &mut World, id: EntityId, on: bool) -> Vec<EntityId> {
    let group = world
        .get::<Toggle>(id)
        .ok()
        .flatten()
        .map(|toggle| toggle.group.clone())
        .unwrap_or_default();
    let mut changed = Vec::new();
    if let Ok(Some(toggle)) = world.get_mut::<Toggle>(id)
        && toggle.set_on(on)
    {
        changed.push(id);
    }
    if !on || group.is_empty() {
        return changed;
    }
    let siblings: Vec<EntityId> = world
        .entities_with::<Toggle>()
        .into_iter()
        .filter(|other| *other != id)
        .filter(|other| {
            world
                .get::<Toggle>(*other)
                .ok()
                .flatten()
                .is_some_and(|toggle| toggle.is_on && toggle.group == group)
        })
        .collect();
    for sibling in siblings {
        if let Ok(Some(toggle)) = world.get_mut::<Toggle>(sibling)
            && toggle.set_on(false)
        {
            changed.push(sibling);
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_on_and_round_trips() {
        assert_eq!(Toggle::from_component_str("").unwrap(), Toggle::default());
        assert!(Toggle::from_component_str("").unwrap().is_on);

        let toggle = Toggle {
            is_on: false,
            group: "Sound;Music".to_owned(),
            checkmark: "Check;mark".to_owned(),
        };
        let encoded = toggle.to_component_str();
        assert!(encoded.contains("group=Sound\\;Music"), "{encoded}");
        assert!(encoded.contains("checkmark=Check\\;mark"), "{encoded}");
        assert_eq!(
            Toggle::from_component_str(&encoded).unwrap(),
            toggle,
            "{encoded}"
        );
    }

    #[test]
    fn omits_empty_targets_and_rejects_duplicates() {
        let encoded = Toggle::default().to_component_str();
        assert_eq!(encoded, "on=true");
        assert_eq!(
            Toggle {
                is_on: false,
                group: String::new(),
                checkmark: String::new(),
            }
            .to_component_str(),
            "on=false"
        );
        for bad in [
            "on=yes",
            "on=true;on=false",
            "checkmark=A;checkmark=B",
            "is_on=true",
        ] {
            assert!(Toggle::from_component_str(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn checkmark_rect_follows_state() {
        let control = UiRect::new(20.0, 30.0, 40.0, 24.0);
        let on = Toggle::default();
        assert_eq!(on.checkmark_rect(control), control);
        let off = Toggle {
            is_on: false,
            ..Toggle::default()
        };
        assert_eq!(
            off.checkmark_rect(control),
            UiRect::new(20.0, 30.0, 0.0, 0.0)
        );
    }

    #[test]
    fn set_on_reports_only_real_changes() {
        let mut toggle = Toggle::default();
        assert!(!toggle.set_on(true));
        assert!(toggle.set_on(false));
        assert!(!toggle.is_on);
        assert!(!toggle.set_on(false));
    }

    #[test]
    fn group_turns_siblings_off_and_reports_every_change() {
        let mut world = World::new();
        let first = world.spawn();
        let second = world.spawn();
        let other = world.spawn();
        world
            .insert(
                first,
                Toggle {
                    is_on: false,
                    group: "Sound".to_owned(),
                    checkmark: String::new(),
                },
            )
            .unwrap();
        world
            .insert(
                second,
                Toggle {
                    is_on: true,
                    group: "Sound".to_owned(),
                    checkmark: String::new(),
                },
            )
            .unwrap();
        world
            .insert(
                other,
                Toggle {
                    is_on: true,
                    group: "Music".to_owned(),
                    checkmark: String::new(),
                },
            )
            .unwrap();

        let changed = set_toggle_on(&mut world, first, true);
        assert_eq!(changed, vec![first, second]);
        assert!(world.get::<Toggle>(first).unwrap().unwrap().is_on);
        assert!(!world.get::<Toggle>(second).unwrap().unwrap().is_on);
        assert!(
            world.get::<Toggle>(other).unwrap().unwrap().is_on,
            "不同组不得被关掉"
        );

        // 已经打开时重复写 true 不产生事件。
        assert!(set_toggle_on(&mut world, first, true).is_empty());
        // 关掉自己不影响兄弟。
        assert_eq!(set_toggle_on(&mut world, first, false), vec![first]);
        assert!(!world.get::<Toggle>(first).unwrap().unwrap().is_on);
    }
}
