//! P1-②：多级细节（LOD）。`LodGroup` 组件按相机距离选择 `MeshRenderer` 的网格。
//!
//! 对标 Godot/Cocos 的能力补齐：远处网格换低细节，顶点量随距离下降（docs/40
//! "3D 层级空间索引 + CPU culling"之后的渲染质量档）。
//!
//! 组件串：`LodGroup=mesh=<基础网格>;lod=<阈值>|<网格>;lod=<阈值>|<网格>...`
//!
//! - `mesh`：最近级（高细节）网格；
//! - `lod`：可重复；`阈值|网格`，阈值 = 从上一级切换到本级的相机距离（世界单位），
//!   必须**严格递增且为正**（否则解析报错）。
//!
//! 选择语义：`select(distance)` 返回**最后一个** `threshold <= distance` 的级；
//! 距离小于第一级阈值时返回基础级（与 Godot 的距离递减 LOD 同向）。
//!
//! 与蒙皮的关系：各级网格假设**同一骨骼拓扑**（网格换、骨骼矩阵同源）。实体带
//! `Animation3D` 时使用 LOD 必须保证各级网格骨骼数一致，否则骨骼矩阵越界——这是
//! 场景作者责任（引擎只做网格选择，不校验骨骼数）。

use crate::core::mesh3d;

/// 一个 LOD 级：从上一级切换到本级的距离 + 该级网格路径。
#[derive(Clone, Debug, PartialEq)]
pub struct LodLevel {
    /// 从上一级切换到本级的相机距离（世界单位）。`levels[0]` 的该字段恒 0（不使用）。
    pub threshold: f32,
    /// 项目相对路径（`.glb`/`.gltf`）。
    pub mesh: String,
}

/// 距离驱动 LOD 组：`levels[0]` 是基础级（高细节），越往后越远、越低细节。
#[derive(Clone, Debug, PartialEq)]
pub struct LodGroup {
    pub levels: Vec<LodLevel>,
}

/// 解析 `LodGroup` 组件串的错误。
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum LodGroupError {
    /// 字段不是 `key=value`。
    #[error("malformed LodGroup field: {0}")]
    MalformedField(String),
    /// 缺 `mesh=` 字段。
    #[error("missing LodGroup mesh field")]
    MissingMesh,
    /// 网格路径非法（绝对路径 / 逃出工程根）。
    #[error("invalid LodGroup mesh path: {0}")]
    InvalidMeshPath(String),
    /// 网格扩展名不是 `.glb`/`.gltf`。
    #[error("unsupported LodGroup mesh extension: {0}")]
    UnsupportedMeshExtension(String),
    /// `lod` 字段缺 `|`、阈值非有限或非正。
    #[error(
        "invalid LodGroup lod threshold: {0} (expected `distance|mesh` with a positive finite distance)"
    )]
    InvalidThreshold(String),
    /// 阈值未严格递增。
    #[error("LodGroup thresholds must strictly increase: {previous} >= {current}")]
    ThresholdNotIncreasing { previous: f32, current: f32 },
}

impl LodGroup {
    /// 解析编辑器写入的组件串：`LodGroup=mesh=<路径>;lod=<距离>|<路径>[;lod=...]`。
    pub fn from_component_str(value: &str) -> Result<Self, LodGroupError> {
        let mut mesh: Option<String> = None;
        let mut lods: Vec<(f32, String)> = Vec::new();
        for field in value.split(';') {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| LodGroupError::MalformedField(field.to_owned()))?;
            match key {
                "mesh" => {
                    mesh3d::validate_mesh_path(raw).map_err(|error| match error {
                        mesh3d::MeshRendererError::InvalidMeshPath(value) => {
                            LodGroupError::InvalidMeshPath(value)
                        }
                        mesh3d::MeshRendererError::UnsupportedMeshExtension(value) => {
                            LodGroupError::UnsupportedMeshExtension(value)
                        }
                        _ => LodGroupError::MissingMesh,
                    })?;
                    mesh = Some(raw.to_owned());
                }
                "lod" => {
                    let (threshold, path) = raw
                        .split_once('|')
                        .ok_or_else(|| LodGroupError::InvalidThreshold(raw.to_owned()))?;
                    let threshold: f32 = threshold
                        .trim()
                        .parse()
                        .map_err(|_| LodGroupError::InvalidThreshold(raw.to_owned()))?;
                    if !threshold.is_finite() || threshold <= 0.0 {
                        return Err(LodGroupError::InvalidThreshold(raw.to_owned()));
                    }
                    mesh3d::validate_mesh_path(path).map_err(|error| match error {
                        mesh3d::MeshRendererError::InvalidMeshPath(value) => {
                            LodGroupError::InvalidMeshPath(value)
                        }
                        mesh3d::MeshRendererError::UnsupportedMeshExtension(value) => {
                            LodGroupError::UnsupportedMeshExtension(value)
                        }
                        _ => LodGroupError::MissingMesh,
                    })?;
                    lods.push((threshold, path.to_owned()));
                }
                _ => return Err(LodGroupError::MalformedField(field.to_owned())),
            }
        }
        let mesh = mesh.ok_or(LodGroupError::MissingMesh)?;
        // 阈值严格递增（从基础级阈值 0 起）：第一级必须 > 0，之后每级必须 > 上一级。
        let mut previous = 0.0f32;
        for (threshold, _) in &lods {
            if *threshold <= previous {
                return Err(LodGroupError::ThresholdNotIncreasing {
                    previous,
                    current: *threshold,
                });
            }
            previous = *threshold;
        }
        let mut levels = Vec::with_capacity(lods.len() + 1);
        levels.push(LodLevel {
            threshold: 0.0,
            mesh,
        });
        for (threshold, path) in lods {
            levels.push(LodLevel {
                threshold,
                mesh: path,
            });
        }
        Ok(Self { levels })
    }

    /// 编码回编辑器的组件串格式。
    #[must_use]
    pub fn to_component_str(&self) -> String {
        let mut parts = vec![format!("mesh={}", self.levels[0].mesh)];
        for level in &self.levels[1..] {
            parts.push(format!("lod={}|{}", level.threshold, level.mesh));
        }
        parts.join(";")
    }

    /// 按到相机的距离选择网格路径：返回**最后一个** `threshold <= distance` 的级；
    /// 小于第一级阈值时返回基础级。
    #[must_use]
    pub fn select(&self, distance: f32) -> &str {
        let mut chosen = 0usize;
        for (index, level) in self.levels.iter().enumerate().skip(1) {
            if distance >= level.threshold {
                chosen = index;
            } else {
                // 阈值严格递增：一旦不满足，后面更远的级也不会满足。
                break;
            }
        }
        &self.levels[chosen].mesh
    }
}

#[cfg(test)]
mod tests {
    use super::{LodGroup, LodGroupError};

    #[test]
    fn parse_round_trip_preserves_levels() {
        let value = "mesh=a.glb;lod=8|b.glb;lod=32|c.glb";
        let group = LodGroup::from_component_str(value).unwrap();
        assert_eq!(group.levels.len(), 3);
        assert_eq!(group.levels[0].mesh, "a.glb");
        assert_eq!(group.levels[1].threshold, 8.0);
        assert_eq!(group.levels[1].mesh, "b.glb");
        assert_eq!(group.levels[2].threshold, 32.0);
        assert_eq!(group.levels[2].mesh, "c.glb");
        assert_eq!(group.to_component_str(), value);
    }

    /// 只有基础级：合法，select 恒返回它。
    #[test]
    fn single_level_is_valid_and_always_selected() {
        let group = LodGroup::from_component_str("mesh=a.glb").unwrap();
        assert_eq!(group.levels.len(), 1);
        assert_eq!(group.select(0.0), "a.glb");
        assert_eq!(group.select(1.0e9), "a.glb");
    }

    /// 选择语义：距离 < 阈值 → 基础级；>= 阈值 → 该级；多个阈值满足时取最后一个。
    #[test]
    fn select_picks_the_last_threshold_at_or_below_distance() {
        let group = LodGroup::from_component_str("mesh=a.glb;lod=8|b.glb;lod=32|c.glb").unwrap();
        assert_eq!(group.select(0.0), "a.glb");
        assert_eq!(group.select(7.999), "a.glb");
        assert_eq!(group.select(8.0), "b.glb");
        assert_eq!(group.select(31.999), "b.glb");
        assert_eq!(group.select(32.0), "c.glb");
        assert_eq!(group.select(1.0e6), "c.glb");
    }

    /// 阈值必须严格递增且为正。
    #[test]
    fn thresholds_must_strictly_increase() {
        // lod=0 与基础级（threshold 0）重叠：解析阶段按非正阈值直接拒绝。
        assert!(matches!(
            LodGroup::from_component_str("mesh=a.glb;lod=0|b.glb"),
            Err(LodGroupError::InvalidThreshold(_))
        ));
        assert!(matches!(
            LodGroup::from_component_str("mesh=a.glb;lod=8|b.glb;lod=8|c.glb"),
            Err(LodGroupError::ThresholdNotIncreasing { .. })
        ));
        assert!(matches!(
            LodGroup::from_component_str("mesh=a.glb;lod=8|b.glb;lod=4|c.glb"),
            Err(LodGroupError::ThresholdNotIncreasing { .. })
        ));
    }

    /// 缺 mesh、非法路径、非 glb/gltf 扩展、非有限/非数字阈值都报错。
    #[test]
    fn rejects_bad_fields() {
        assert_eq!(
            LodGroup::from_component_str("lod=8|b.glb").unwrap_err(),
            LodGroupError::MissingMesh
        );
        assert_eq!(
            LodGroup::from_component_str("mesh=../evil.glb;lod=8|b.glb").unwrap_err(),
            LodGroupError::InvalidMeshPath("../evil.glb".to_owned())
        );
        assert_eq!(
            LodGroup::from_component_str("mesh=a.glb;lod=8|b.txt").unwrap_err(),
            LodGroupError::UnsupportedMeshExtension("b.txt".to_owned())
        );
        assert_eq!(
            LodGroup::from_component_str("mesh=a.glb;lod=nan|b.glb").unwrap_err(),
            LodGroupError::InvalidThreshold("nan|b.glb".to_owned())
        );
        assert_eq!(
            LodGroup::from_component_str("mesh=a.glb;lod=abc|b.glb").unwrap_err(),
            LodGroupError::InvalidThreshold("abc|b.glb".to_owned())
        );
        assert_eq!(
            LodGroup::from_component_str("mesh=a.glb;bogus=x").unwrap_err(),
            LodGroupError::MalformedField("bogus=x".to_owned())
        );
    }

    /// 地面哨兵网格（`__ground_plane__`）在 LOD 里同样放行（与 MeshRenderer 一致）。
    #[test]
    fn ground_plane_sentinel_is_allowed() {
        let group = LodGroup::from_component_str("mesh=__ground_plane__;lod=8|b.glb").unwrap();
        assert_eq!(group.levels[0].mesh, "__ground_plane__");
    }
}
