//! 线上消息格式：定长头 + 小端整数，便于跨平台对拍与手工排查。
//!
//! 每条消息都自带 `player`（发送者），传输层不需要额外维护"谁发的"：
//! TCP 只在单条连接上收发，loopback 由 bus 记录来源，两边解码路径完全一致。

use crate::NetError;

/// 单条消息的负载上限（16 位长度字段），超过即协议错误。
pub const MAX_PAYLOAD: usize = u16::MAX as usize;

/// 发送者编号；`0..players`。
pub type PlayerId = u8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// 握手：声明自己的玩家号与总人数（TCP 侧用于未来协商）。
    Hello { player: PlayerId, players: PlayerId },
    /// 第 `frame` 帧、玩家 `player` 的输入。
    Input {
        player: PlayerId,
        frame: u32,
        payload: Vec<u8>,
    },
    /// 第 `frame` 帧模拟完成后玩家 `player` 的状态校验和。
    Checksum {
        player: PlayerId,
        frame: u32,
        value: u64,
    },
}

impl Message {
    pub fn player(&self) -> PlayerId {
        match self {
            Self::Hello { player, .. }
            | Self::Input { player, .. }
            | Self::Checksum { player, .. } => *player,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Hello { player, players } => vec![1, *player, *players],
            Self::Input {
                player,
                frame,
                payload,
            } => {
                let mut out = Vec::with_capacity(8 + payload.len());
                out.push(2);
                out.push(*player);
                out.extend_from_slice(&frame.to_le_bytes());
                out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
                out.extend_from_slice(payload);
                out
            }
            Self::Checksum {
                player,
                frame,
                value,
            } => {
                let mut out = Vec::with_capacity(14);
                out.push(3);
                out.push(*player);
                out.extend_from_slice(&frame.to_le_bytes());
                out.extend_from_slice(&value.to_le_bytes());
                out
            }
        }
    }

    /// 解码一条完整消息；长度不精确（含多余字节）也算协议错误，避免悄悄吞数据。
    pub fn decode(bytes: &[u8]) -> Result<Self, NetError> {
        let tag = *bytes
            .first()
            .ok_or_else(|| NetError::Protocol("empty message".to_owned()))?;
        match tag {
            1 if bytes.len() == 3 => Ok(Self::Hello {
                player: bytes[1],
                players: bytes[2],
            }),
            2 if bytes.len() >= 8 => {
                let player = bytes[1];
                let frame = u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
                let len = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
                if bytes.len() != 8 + len {
                    return Err(NetError::Protocol(format!(
                        "input length mismatch: header says {len}, got {}",
                        bytes.len() - 8
                    )));
                }
                Ok(Self::Input {
                    player,
                    frame,
                    payload: bytes[8..].to_vec(),
                })
            }
            3 if bytes.len() == 14 => {
                let player = bytes[1];
                let frame = u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
                let value = u64::from_le_bytes([
                    bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11], bytes[12],
                    bytes[13],
                ]);
                Ok(Self::Checksum {
                    player,
                    frame,
                    value,
                })
            }
            other => Err(NetError::Protocol(format!(
                "unknown message tag {other} (len {})",
                bytes.len()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_round_trips() {
        let messages = [
            Message::Hello {
                player: 1,
                players: 2,
            },
            Message::Input {
                player: 0,
                frame: 42,
                payload: vec![9, 8, 7],
            },
            Message::Input {
                player: 1,
                frame: 0,
                payload: Vec::new(),
            },
            Message::Checksum {
                player: 1,
                frame: 7,
                value: 0xDEAD_BEEF_1234_5678,
            },
        ];
        for message in messages {
            let encoded = message.encode();
            assert_eq!(Message::decode(&encoded), Ok(message));
        }
    }

    #[test]
    fn malformed_messages_are_rejected() {
        assert!(Message::decode(&[]).is_err());
        assert!(Message::decode(&[9, 0, 0]).is_err());
        // 头部声称 4 字节负载、实际只有 3 字节。
        assert!(Message::decode(&[2, 0, 1, 0, 0, 0, 4, 0, 1, 2, 3]).is_err());
        assert!(Message::decode(&[1, 0]).is_err());
    }
}
