//! M76-02：`ScrollView` 运行期组件。
//!
//! 组件串只有轴向（`horizontal=true/false`，缺省竖向）；**内容尺寸不写在组件里**——
//! 运行期按布局树里子节点的实际包围盒算出来，与 `.ui.json` 的 `scroll_view` 一致
//! （设计器里也没有"内容尺寸"字段，内容由子节点决定）。轴向决定滚轮/拖动改哪个分量。

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScrollView {
    pub horizontal: bool,
}

impl ScrollView {
    pub fn from_component_str(raw: &str) -> Result<Self, String> {
        let mut horizontal: Option<bool> = None;
        for field in raw.split(';') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let Some((key, value)) = field.split_once('=') else {
                return Err(format!("expected key=value, got `{field}`"));
            };
            match key {
                "horizontal" => {
                    if horizontal.is_some() {
                        return Err("duplicate `horizontal=`".to_owned());
                    }
                    horizontal = Some(match value.trim() {
                        "true" => true,
                        "false" => false,
                        other => {
                            return Err(format!(
                                "`horizontal` must be true or false, got `{other}`"
                            ));
                        }
                    });
                }
                other => return Err(format!("unknown ScrollView field `{other}`")),
            }
        }
        Ok(Self {
            horizontal: horizontal.unwrap_or(false),
        })
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        format!("horizontal={}", self.horizontal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_axis_and_round_trips() {
        assert_eq!(
            ScrollView::from_component_str("").unwrap(),
            ScrollView { horizontal: false }
        );
        assert_eq!(
            ScrollView::from_component_str("horizontal=true").unwrap(),
            ScrollView { horizontal: true }
        );
        assert_eq!(
            ScrollView::from_component_str(&ScrollView { horizontal: true }.to_component_str())
                .unwrap(),
            ScrollView { horizontal: true }
        );
        for bad in [
            "horizontal=yes",
            "horizontal=true;horizontal=false",
            "vertical=true",
        ] {
            assert!(ScrollView::from_component_str(bad).is_err(), "{bad:?}");
        }
    }
}
