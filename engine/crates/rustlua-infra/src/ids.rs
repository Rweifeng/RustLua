//! 跨层共享的**标识类型**。
//!
//! 为什么放在 `rustlua-infra`（最底层）而不是各自的领域 crate：
//!
//! `AssetId` 被 `rustlua-assets`（数据库）、`rustlua-scene`（prefab/scene 引用）、
//! `rustlua-scripting`（`scene_spawn` 的句柄表）和编辑器同时使用。它是个**纯值对象**
//! （128 位、可 `Copy`、无任何 I/O 或领域逻辑），但曾经定义在 `rustlua-assets` 里，
//! 于是 `rustlua-scripting` 为了两个字段类型就得多依赖一整个资源库 crate
//! （`docs/62` 的 P1 依赖倒置：绑定层不该为了类型拖着领域实现走）。
//!
//! 挪到最底层后：`rustlua-assets` 仍然 `pub use` 它（对外路径不变），而 `scripting`
//! 只依赖 `infra`——**类型下沉、实现留在原处**，这是这个 crate 里唯一的"共享词汇"。
//!
//! 红线：这里是**值对象**的容身处，不要往里塞需要 I/O、日志或领域规则的东西。
//! 每加一个类型都要问一遍"它真的被两层以上共享，且不带来实现依赖吗"。

use std::fmt;
use std::str::FromStr;

/// 稳定 128 位资源标识，按 UUID 形式渲染。
///
/// 由 [`AssetId::from_seed`] 从种子确定性生成：同一路径 + 导入器版本永远得到同一个 id，
/// 所以 id 可以持久化在资源库、场景文件与存档里。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AssetId([u8; 16]);

impl AssetId {
    /// 从唯一种子生成确定性 UUID 形状的 id。
    #[must_use]
    pub fn from_seed(seed: &str) -> Self {
        let mut bytes = [0u8; 16];
        for (index, byte) in seed.as_bytes().iter().enumerate() {
            let slot = index % 16;
            bytes[slot] = bytes[slot]
                .wrapping_add(*byte)
                .rotate_left((index % 8) as u32);
            let other = (slot * 7 + 3) % 16;
            bytes[other] ^= byte.wrapping_mul(31);
        }
        // UUID version 4 shape 与 RFC 4122 variant；identity 由资源库持久化。
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(bytes)
    }

    /// 原始 16 字节（资源库落盘、哈希前使用）。
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                write!(formatter, "-")?;
            }
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// [`AssetId`] 的解析错误。
///
/// 单独一个类型（不是 `ResourceDbError`）是为了让本 crate 不反向依赖资源库；
/// `rustlua-assets` 用 `From` 把它并进自己的错误枚举，调用点看到的错误类型不变。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetIdParseError(pub String);

impl fmt::Display for AssetIdParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid asset UUID: {}", self.0)
    }
}

impl std::error::Error for AssetIdParseError {}

impl FromStr for AssetId {
    type Err = AssetIdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let compact: String = value
            .chars()
            .filter(|character| *character != '-')
            .collect();
        if compact.len() != 32
            || !compact
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Err(AssetIdParseError(value.to_owned()));
        }
        let mut bytes = [0u8; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&compact[index * 2..index * 2 + 2], 16)
                .map_err(|_| AssetIdParseError(value.to_owned()))?;
        }
        Ok(Self(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::AssetId;

    #[test]
    fn seed_is_deterministic_and_shape_is_uuid_v4() {
        let first = AssetId::from_seed("assets/hero.png:texture-1");
        let second = AssetId::from_seed("assets/hero.png:texture-1");
        assert_eq!(first, second, "同一种子必须得到同一个 id（要能持久化）");
        assert_ne!(first, AssetId::from_seed("assets/hero.png:texture-2"));
        let rendered = first.to_string();
        assert_eq!(rendered.len(), 36, "{rendered}");
        assert_eq!(rendered.as_bytes()[8], b'-');
        assert_eq!(rendered.as_bytes()[14], b'4', "版本位应为 4：{rendered}");
    }

    #[test]
    fn text_round_trip_and_rejects_garbage() {
        let id = AssetId::from_seed("ui/title.png");
        assert_eq!(id.to_string().parse::<AssetId>().unwrap(), id);
        // 大小写与分隔符都归一：不带分隔符的紧凑写法也认
        let compact = id.to_string().replace('-', "");
        assert_eq!(compact.parse::<AssetId>().unwrap(), id);
        for bad in [
            "",
            "nope",
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
            &"0".repeat(31),
        ] {
            assert!(bad.parse::<AssetId>().is_err(), "{bad:?} 不该被当成合法 id");
        }
    }
}
