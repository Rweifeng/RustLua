//! 帧同步会话：输入按帧定序，收齐所有玩家才放行；校验和互发用来抓 desync。

use std::collections::BTreeMap;

use crate::wire::{Message, PlayerId};
use crate::{NetError, Transport};

/// 会话参数。`players` 含本机；`local` 是本机玩家号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionConfig {
    pub players: PlayerId,
    pub local: PlayerId,
    /// 输入延迟（帧）。本机在第 F 帧提交的输入用于第 F+delay 帧。
    pub delay: u32,
}

impl SessionConfig {
    pub fn new(players: PlayerId, local: PlayerId, delay: u32) -> Result<Self, NetError> {
        if players == 0 {
            return Err(NetError::Config("players must be at least 1".to_owned()));
        }
        if local >= players {
            return Err(NetError::Config(format!(
                "local player {local} is outside 0..{players}"
            )));
        }
        Ok(Self {
            players,
            local,
            delay,
        })
    }
}

/// 一帧被放行时的完整输入（按玩家号升序）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameInputs {
    pub frame: u32,
    pub inputs: Vec<(PlayerId, Vec<u8>)>,
}

impl FrameInputs {
    /// 按玩家号取输入；缺失返回 `None`。
    pub fn input(&self, player: PlayerId) -> Option<&[u8]> {
        self.inputs
            .iter()
            .find(|(who, _)| *who == player)
            .map(|(_, payload)| payload.as_slice())
    }

    /// 把一帧的输入与玩家号喂给确定性滚动哈希（FNV-1a），用于跨端对拍。
    pub fn checksum(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        eat(&self.frame.to_le_bytes());
        for (player, payload) in &self.inputs {
            eat(&[*player]);
            eat(&(payload.len() as u32).to_le_bytes());
            eat(payload);
        }
        hash
    }
}

/// 首个不一致的校验和。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Desync {
    pub frame: u32,
    pub remote_player: PlayerId,
    pub local: u64,
    pub remote: u64,
}

pub struct Session<T: Transport> {
    config: SessionConfig,
    transport: T,
    next_submit: u32,
    next_frame: u32,
    pending: BTreeMap<(u32, PlayerId), Vec<u8>>,
    local_checksums: BTreeMap<u32, u64>,
    remote_checksums: BTreeMap<(u32, PlayerId), u64>,
    desync: Option<Desync>,
}

impl<T: Transport> Session<T> {
    pub fn new(config: SessionConfig, transport: T) -> Self {
        Self {
            config,
            transport,
            next_submit: 0,
            next_frame: 0,
            pending: BTreeMap::new(),
            local_checksums: BTreeMap::new(),
            remote_checksums: BTreeMap::new(),
            desync: None,
        }
    }

    pub fn config(&self) -> SessionConfig {
        self.config
    }

    /// 下一帧要提交的输入序号（不是被模拟的帧号：两者相差 `delay`）。
    pub fn next_submit_frame(&self) -> u32 {
        self.next_submit
    }

    /// 下一个等待放行的帧号。
    pub fn next_frame(&self) -> u32 {
        self.next_frame
    }

    /// 提交一帧本机输入，返回它生效的帧号。
    ///
    /// 单人会话没有"等待别人"的语义，`delay` 被忽略（否则前 delay 帧永远凑不齐）。
    pub fn submit(&mut self, payload: Vec<u8>) -> Result<u32, NetError> {
        if payload.len() > crate::wire::MAX_PAYLOAD {
            return Err(NetError::Config(format!(
                "input payload is {} bytes, limit {}",
                payload.len(),
                crate::wire::MAX_PAYLOAD
            )));
        }
        let delay = if self.config.players == 1 {
            0
        } else {
            self.config.delay
        };
        let frame = self.next_submit + delay;
        self.next_submit += 1;
        let message = Message::Input {
            player: self.config.local,
            frame,
            payload: payload.clone(),
        };
        self.pending.insert((frame, self.config.local), payload);
        self.broadcast(&message)?;
        Ok(frame)
    }

    /// 预热：把本机 `0..count` 帧先填上同一份输入（通常是空输入）。
    ///
    /// `delay` 让前 `delay` 帧没有"已提交的输入"，会话会永远停在帧 0；
    /// 开局调用一次 `prime` 把缺口补上，之后 `submit` 的输入正好落在帧 `delay..`。
    pub fn prime(&mut self, payload: Vec<u8>, count: u32) -> Result<(), NetError> {
        for frame in 0..count {
            self.pending
                .insert((frame, self.config.local), payload.clone());
            self.broadcast(&Message::Input {
                player: self.config.local,
                frame,
                payload: payload.clone(),
            })?;
        }
        Ok(())
    }

    /// 收下已到达的远端消息（输入与校验和）。
    pub fn poll(&mut self) -> Result<(), NetError> {
        for (from, message) in self.transport.poll()? {
            match message {
                Message::Input {
                    player,
                    frame,
                    payload,
                } => {
                    if player != from {
                        return Err(NetError::Protocol(format!(
                            "message from {from} claims player {player}"
                        )));
                    }
                    if player == self.config.local {
                        continue;
                    }
                    self.pending.insert((frame, player), payload);
                }
                Message::Checksum {
                    player,
                    frame,
                    value,
                } => {
                    if player == self.config.local {
                        continue;
                    }
                    self.remote_checksums.insert((frame, player), value);
                    if let Some(local) = self.local_checksums.get(&frame).copied()
                        && local != value
                        && self.desync.is_none()
                    {
                        self.desync = Some(Desync {
                            frame,
                            remote_player: player,
                            local,
                            remote: value,
                        });
                    }
                }
                Message::Hello { .. } => {}
            }
        }
        Ok(())
    }

    /// 若下一帧的输入已收齐，放行并返回；否则返回 `None`（调用方下一轮再试）。
    pub fn try_advance(&mut self) -> Option<FrameInputs> {
        let frame = self.next_frame;
        let mut inputs = Vec::with_capacity(self.config.players as usize);
        for player in 0..self.config.players {
            let payload = self.pending.get(&(frame, player))?.clone();
            inputs.push((player, payload));
        }
        for player in 0..self.config.players {
            self.pending.remove(&(frame, player));
        }
        self.next_frame += 1;
        Some(FrameInputs { frame, inputs })
    }

    /// 记录本帧状态校验和并发给对端；对端若已先到，立刻比较。
    pub fn record_checksum(&mut self, frame: u32, value: u64) -> Result<(), NetError> {
        self.local_checksums.insert(frame, value);
        if let Some(remote) = self
            .remote_checksums
            .iter()
            .find(|((remote_frame, player), remote)| {
                *remote_frame == frame && *player != self.config.local && **remote != value
            })
            .map(|((_, player), remote)| (*player, *remote))
            && self.desync.is_none()
        {
            self.desync = Some(Desync {
                frame,
                remote_player: remote.0,
                local: value,
                remote: remote.1,
            });
        }
        self.broadcast(&Message::Checksum {
            player: self.config.local,
            frame,
            value,
        })
    }

    pub fn first_desync(&self) -> Option<Desync> {
        self.desync
    }

    fn broadcast(&mut self, message: &Message) -> Result<(), NetError> {
        if self.config.players == 1 {
            return Ok(());
        }
        self.transport.send(None, message)
    }
}
