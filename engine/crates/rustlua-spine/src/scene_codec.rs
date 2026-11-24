//! `SpineSkeleton` 组件的载荷契约（`docs/62` P1 第 4 条）。
//!
//! 文法属于本 crate：字段语义与跨字段校验只有 [`SpineSkeletonComponent::from_component_str`]
//! 一份实现。`rustlua-scene` 不认这里的类型，它通过 `rustlua_component_codec` 注册表拿到
//! **校验函数**，把原文原样带进 `TypedScene`；宿主侧再由 `script_packs` 的装配器解析成
//! `SpinePlayer`。
//!
//! **装表**：`engine::host::scene_codecs` 把 [`scene_codecs()`] 与渲染层/UI 层一起装表。

use rustlua_component_codec::ComponentCodec;

use crate::model::SpineSkeletonComponent;

/// `SpineSkeleton`：`.spine` 清单 + 动画/皮肤/速度/缩放/深度/循环/开关/排序层/次序。
///
/// 资源路径 = `data`（那份 `.spine` 清单）。清单里的骨骼/图集/页由导入层的依赖边展开，
/// 这里不重复列——否则同一份 `.skel` 会在依赖图里被登记两次。
fn validate_spine(value: &str) -> Result<(), String> {
    SpineSkeletonComponent::from_component_str(value)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn spine_assets(value: &str) -> Vec<String> {
    // 解析失败返回空：资源缺失的可见性归资源库依赖图，场景编译不重复报同一件事。
    SpineSkeletonComponent::from_component_str(value)
        .map(|component| component.asset_paths())
        .unwrap_or_default()
}

static SPINE_SKELETON: ComponentCodec =
    ComponentCodec::new("SpineSkeleton", validate_spine).with_assets(spine_assets);

/// 本 crate 负责的组件 codec（目前只有 `SpineSkeleton`）。
#[must_use]
pub fn scene_codecs() -> &'static [&'static ComponentCodec] {
    static CODECS: [&ComponentCodec; 1] = [&SPINE_SKELETON];
    &CODECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codecs_cover_the_spine_component_name() {
        let names: Vec<&str> = scene_codecs().iter().map(|codec| codec.name()).collect();
        assert_eq!(names, vec!["SpineSkeleton"]);
    }

    /// 校验与归属 crate 的解析器同源：合法的过、坏的拒。
    #[test]
    fn validation_matches_the_owner_crate_parser() {
        assert!(
            SPINE_SKELETON
                .validate("data=assets/spine/hero.spine")
                .is_ok()
        );
        assert!(
            SPINE_SKELETON
                .validate("data=assets/spine/hero.spine;animation=idle;speed=1.5")
                .is_ok()
        );
        // 缺 data 是硬错：没有清单就没有骨骼数据来源。
        assert!(SPINE_SKELETON.validate("animation=idle").is_err());
        assert!(SPINE_SKELETON.validate("data=").is_err());
        // scale 必须 > 0，speed 必须 >= 0。
        assert!(SPINE_SKELETON.validate("data=a.spine;scale=0").is_err());
        assert!(SPINE_SKELETON.validate("data=a.spine;speed=-1").is_err());
        assert!(SPINE_SKELETON.validate("data=a.spine;nonsense=1").is_err());
    }

    /// 资源路径要能被场景层拿去问资源库；解析不了时返回空而不是报错。
    #[test]
    fn asset_paths_are_reported_or_empty_never_error() {
        assert_eq!(
            SPINE_SKELETON.asset_paths("data=assets/spine/hero.spine"),
            vec!["assets/spine/hero.spine".to_owned()]
        );
        assert!(SPINE_SKELETON.asset_paths("not a payload").is_empty());
    }
}
