//! `CanvasGroup`：整棵子树的透明度与交互开关（对齐 Unity `CanvasGroup`）。
//!
//! 三个字段与 Unity 同名同义：
//! * `alpha`：0..1，乘法继承——子节点最终 alpha = 自己 × 所有祖先；
//! * `interactable`：false 时整棵子树不参与命中（按钮变灰且点不动）；
//! * `blocks_raycasts`：false 时命中**穿过**整棵子树（用于纯装饰浮层）。
//!
//! 组件串：`alpha=1;interactable=true;blocks_raycasts=true`，字段全部可选。

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasGroup {
    pub alpha: f32,
    pub interactable: bool,
    pub blocks_raycasts: bool,
}

impl Default for CanvasGroup {
    fn default() -> Self {
        Self {
            alpha: 1.0,
            interactable: true,
            blocks_raycasts: true,
        }
    }
}

impl CanvasGroup {
    pub fn from_component_str(raw: &str) -> Result<Self, String> {
        let mut group = Self::default();
        let mut alpha_seen = false;
        let mut interactable_seen = false;
        let mut blocks_seen = false;
        for field in raw.split(';') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let Some((key, value)) = field.split_once('=') else {
                return Err(format!("expected key=value, got `{field}`"));
            };
            let value = value.trim();
            match key {
                "alpha" => {
                    if alpha_seen {
                        return Err("duplicate `alpha=`".to_owned());
                    }
                    alpha_seen = true;
                    let alpha = value
                        .parse::<f32>()
                        .map_err(|_| format!("invalid alpha `{value}`"))?;
                    if !alpha.is_finite() || !(0.0..=1.0).contains(&alpha) {
                        return Err(format!("alpha must be within 0..=1, got `{value}`"));
                    }
                    group.alpha = alpha;
                }
                "interactable" => {
                    if interactable_seen {
                        return Err("duplicate `interactable=`".to_owned());
                    }
                    interactable_seen = true;
                    group.interactable = parse_bool("interactable", value)?;
                }
                "blocks_raycasts" => {
                    if blocks_seen {
                        return Err("duplicate `blocks_raycasts=`".to_owned());
                    }
                    blocks_seen = true;
                    group.blocks_raycasts = parse_bool("blocks_raycasts", value)?;
                }
                other => return Err(format!("unknown CanvasGroup field `{other}`")),
            }
        }
        Ok(group)
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        format!(
            "alpha={};interactable={};blocks_raycasts={}",
            self.alpha, self.interactable, self.blocks_raycasts
        )
    }
}

fn parse_bool(key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("`{key}` must be true or false, got `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        assert_eq!(
            CanvasGroup::from_component_str("").unwrap(),
            CanvasGroup::default()
        );
        let group =
            CanvasGroup::from_component_str("alpha=0.5;interactable=false;blocks_raycasts=false")
                .unwrap();
        assert_eq!(group.alpha, 0.5);
        assert!(!group.interactable);
        assert!(!group.blocks_raycasts);
        assert_eq!(
            CanvasGroup::from_component_str(&group.to_component_str()).unwrap(),
            group
        );
    }

    #[test]
    fn rejects_out_of_range_and_unknown_fields() {
        for bad in [
            "alpha=1.5",
            "alpha=-0.1",
            "alpha=NaN",
            "alpha=0.5;alpha=0.6",
            "interactable=yes",
            "blocks_raycasts=1",
            "visible=false",
        ] {
            assert!(CanvasGroup::from_component_str(bad).is_err(), "{bad:?}");
        }
    }
}
