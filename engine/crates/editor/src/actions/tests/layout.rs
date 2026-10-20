//! 布局与 Sprite 排序的纯函数：对齐分布保持端点、排序只做最小相邻交换。

use super::layout_ops::{DistributionAxis, SpriteOrderTarget};
use super::*;

#[test]
fn distribute_targets_preserves_endpoints_and_even_spacing() {
    let targets = vec![
        (
            SceneEntityId(1),
            TransformFields {
                position_x: 40.0,
                position_y: 5.0,
                ..TransformFields::default()
            },
        ),
        (
            SceneEntityId(2),
            TransformFields {
                position_x: 0.0,
                position_y: 2.0,
                ..TransformFields::default()
            },
        ),
        (
            SceneEntityId(3),
            TransformFields {
                position_x: 10.0,
                position_y: 9.0,
                ..TransformFields::default()
            },
        ),
    ];
    let distributed = distribute_targets(&targets, DistributionAxis::Horizontal);
    let xs = distributed
        .iter()
        .map(|(_, transform)| transform.position_x)
        .collect::<Vec<_>>();
    assert_eq!(xs, vec![0.0, 20.0, 40.0]);
    let ys = distributed
        .iter()
        .map(|(_, transform)| transform.position_y)
        .collect::<Vec<_>>();
    assert_eq!(ys, vec![2.0, 9.0, 5.0]);
}

#[test]
fn sorted_sprite_order_updates_keep_min_order_and_follow_y() {
    let sprite = |order| SpriteComponent {
        texture: "assets/test.png".to_owned(),
        size: Vec2::new(32.0, 32.0),
        pivot: Vec2::new(0.5, 0.5),
        tint: [1.0; 4],
        sorting: rustlua_render::sprite::SpriteSorting {
            layer: "world".to_owned(),
            order,
        },

        flip_x: false,
        flip_y: false,
        z: 0.0,
        nine_slice: None,
    };
    let targets = vec![
        SpriteOrderTarget {
            id: SceneEntityId(1),
            position_x: 0.0,
            position_y: 20.0,
            order: 8,
            sprite: sprite(8),
        },
        SpriteOrderTarget {
            id: SceneEntityId(2),
            position_x: 0.0,
            position_y: 10.0,
            order: 5,
            sprite: sprite(5),
        },
        SpriteOrderTarget {
            id: SceneEntityId(3),
            position_x: 0.0,
            position_y: 30.0,
            order: 12,
            sprite: sprite(12),
        },
    ];
    let updates = sorted_sprite_order_updates(&targets, false);
    let orders = updates
        .iter()
        .map(|(_, value)| {
            SpriteComponent::from_component_str(value)
                .unwrap()
                .sorting
                .order
        })
        .collect::<Vec<_>>();
    assert_eq!(orders, vec![5, 6, 7]);
}
