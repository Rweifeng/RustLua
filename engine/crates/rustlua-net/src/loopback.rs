//! 内存传输：确定性的延迟与抖动，不碰操作系统网络。
//!
//! 用途有两个：单元测试里稳定复现"乱序/晚到"，以及本地双 VM 自检。
//! 时间单位是**帧**而不是毫秒，所以同一组参数在桌面与浏览器里都得到同一条时间线。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::wire::{Message, PlayerId};
use crate::{NetError, Transport};

struct Bus {
    frame: u32,
    latency: u32,
    jitter: u32,
    rng: u64,
    /// 按接收者分组：`(到期帧, 发送者, 编码后的消息)`。
    queues: Vec<VecDeque<(u32, PlayerId, Vec<u8>)>>,
    inboxes: Vec<Vec<(PlayerId, Vec<u8>)>>,
}

impl Bus {
    /// xorshift64*：结果只取决于种子与调用顺序，保证抖动可复现。
    fn next_random(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D)) >> 32) as u32
    }

    fn send(
        &mut self,
        from: PlayerId,
        to: Option<PlayerId>,
        message: &Message,
    ) -> Result<(), NetError> {
        let bytes = message.encode();
        let due = self.frame + self.latency + self.next_random() % (self.jitter + 1);
        for index in 0..self.queues.len() {
            if PlayerId::try_from(index).ok() == Some(from) {
                continue;
            }
            if to.is_some_and(|target| usize::from(target) != index) {
                continue;
            }
            self.queues[index].push_back((due, from, bytes.clone()));
        }
        Ok(())
    }

    fn step(&mut self) {
        self.frame += 1;
        for index in 0..self.queues.len() {
            let mut still_waiting = VecDeque::with_capacity(self.queues[index].len());
            while let Some((due, from, bytes)) = self.queues[index].pop_front() {
                if due <= self.frame {
                    self.inboxes[index].push((from, bytes));
                } else {
                    still_waiting.push_back((due, from, bytes));
                }
            }
            self.queues[index] = still_waiting;
        }
    }
}

/// 一组互相连通的 loopback 端点；`step()` 推进一帧并投递到期消息。
pub struct LoopbackNet {
    bus: Rc<RefCell<Bus>>,
}

impl LoopbackNet {
    /// `latency` 与 `jitter` 单位是帧：消息最早在 `帧 + latency` 到达，
    /// 抖动再叠加 `0..=jitter` 帧（由 `seed` 决定）。
    pub fn new(players: usize, latency: u32, jitter: u32, seed: u64) -> Result<Self, NetError> {
        if players == 0 || players > usize::from(u8::MAX) {
            return Err(NetError::Config(format!(
                "players must be 1..=255 (got {players})"
            )));
        }
        Ok(Self {
            bus: Rc::new(RefCell::new(Bus {
                frame: 0,
                latency,
                jitter,
                rng: seed | 1,
                queues: (0..players).map(|_| VecDeque::new()).collect(),
                inboxes: (0..players).map(|_| Vec::new()).collect(),
            })),
        })
    }

    /// 取一个端点；同一个 `player` 只能取一次（由调用方保证）。
    pub fn endpoint(&self, player: PlayerId) -> LoopbackTransport {
        LoopbackTransport {
            player,
            bus: Rc::clone(&self.bus),
        }
    }

    /// 推进一帧：所有 `到期帧 <= 当前帧` 的消息进入接收者信箱。
    pub fn step(&self) {
        self.bus.borrow_mut().step();
    }

    pub fn frame(&self) -> u32 {
        self.bus.borrow().frame
    }
}

pub struct LoopbackTransport {
    player: PlayerId,
    bus: Rc<RefCell<Bus>>,
}

impl Transport for LoopbackTransport {
    fn send(&mut self, peer: Option<PlayerId>, message: &Message) -> Result<(), NetError> {
        let mut bus = self.bus.borrow_mut();
        bus.send(self.player, peer, message)
    }

    fn poll(&mut self) -> Result<Vec<(PlayerId, Message)>, NetError> {
        let mut bus = self.bus.borrow_mut();
        let index = usize::from(self.player);
        let raw = bus
            .inboxes
            .get_mut(index)
            .ok_or_else(|| NetError::Config(format!("unknown loopback player {}", self.player)))?;
        let drained = std::mem::take(raw);
        drained
            .into_iter()
            .map(|(from, bytes)| {
                Message::decode(&bytes)
                    .map(|message| (from, message))
                    .map_err(|error| NetError::Protocol(format!("from {from}: {error}")))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lockstep::{Session, SessionConfig};

    fn payload(player: u8, frame: u32) -> Vec<u8> {
        let mut out = vec![player];
        out.extend_from_slice(&frame.to_le_bytes());
        out
    }

    /// 两端在**有延迟与抖动**的链路上跑同一份输入，放行帧数、每帧输入都与对端一致。
    #[test]
    fn loopback_sessions_converge_under_latency_and_jitter() {
        let net = LoopbackNet::new(2, 2, 2, 0xC0FFEE).expect("loopback");
        let config = |local| SessionConfig::new(2, local, 2).expect("config");
        let mut left = Session::new(config(0), net.endpoint(0));
        let mut right = Session::new(config(1), net.endpoint(1));
        left.prime(Vec::new(), 2).expect("left prime");
        right.prime(Vec::new(), 2).expect("right prime");

        let mut left_log = Vec::new();
        let mut right_log = Vec::new();
        for tick in 0..180 {
            if tick < 120 {
                left.submit(payload(0, tick)).expect("left submit");
                right.submit(payload(1, tick)).expect("right submit");
            }
            left.poll().expect("left poll");
            right.poll().expect("right poll");
            while let Some(frame) = left.try_advance() {
                left.record_checksum(frame.frame, frame.checksum())
                    .expect("left checksum");
                left_log.push(frame);
            }
            while let Some(frame) = right.try_advance() {
                right
                    .record_checksum(frame.frame, frame.checksum())
                    .expect("right checksum");
                right_log.push(frame);
            }
            net.step();
        }
        // 抖动的最后一跳可能还在路上：空转几帧让它们落地。
        for _ in 0..8 {
            left.poll().expect("left drain");
            right.poll().expect("right drain");
            net.step();
        }

        assert!(
            left_log.len() > 100,
            "only {} frames advanced",
            left_log.len()
        );
        assert_eq!(
            left_log.len(),
            right_log.len(),
            "both peers advanced the same frames"
        );
        assert_eq!(left_log, right_log, "per-frame inputs must be identical");
        assert_eq!(left.first_desync(), None);
        assert_eq!(right.first_desync(), None);
    }

    /// 输入延迟：第 F 帧提交的输入用于第 F+delay 帧。
    #[test]
    fn input_delay_shifts_the_effective_frame() {
        let net = LoopbackNet::new(2, 0, 0, 1).expect("loopback");
        let mut session = Session::new(
            SessionConfig::new(2, 0, 2).expect("config"),
            net.endpoint(0),
        );
        assert_eq!(session.submit(vec![7]).expect("submit"), 2);
        assert_eq!(session.submit(vec![8]).expect("submit"), 3);
        assert_eq!(session.try_advance(), None, "frame 0 has no input yet");
    }

    /// 单人会话没有对端可等：`delay` 被忽略，否则前 `delay` 帧永远凑不齐。
    #[test]
    fn single_player_ignores_delay() {
        let net = LoopbackNet::new(1, 0, 0, 1).expect("loopback");
        let mut session = Session::new(
            SessionConfig::new(1, 0, 2).expect("config"),
            net.endpoint(0),
        );
        assert_eq!(session.submit(vec![9]).expect("submit"), 0);
        let frame = session
            .try_advance()
            .expect("single player advances immediately");
        assert_eq!(frame.input(0), Some(&[9u8][..]));
    }

    /// 校验和不一致时，首个分歧帧被记录（不是静默继续）。
    #[test]
    fn mismatched_checksums_surface_as_desync() {
        let net = LoopbackNet::new(2, 1, 0, 7).expect("loopback");
        let mut left = Session::new(
            SessionConfig::new(2, 0, 1).expect("config"),
            net.endpoint(0),
        );
        let mut right = Session::new(
            SessionConfig::new(2, 1, 1).expect("config"),
            net.endpoint(1),
        );
        left.prime(Vec::new(), 1).expect("left prime");
        right.prime(Vec::new(), 1).expect("right prime");
        for tick in 0..4 {
            left.submit(payload(0, tick)).expect("left");
            right.submit(payload(1, tick)).expect("right");
        }
        for _ in 0..4 {
            left.poll().expect("left poll");
            right.poll().expect("right poll");
            net.step();
        }
        // 两边都放行同一帧，但故意报不同的校验和。
        let frame = left.try_advance().expect("left frame");
        let _ = right.try_advance().expect("right frame");
        left.record_checksum(frame.frame, 111)
            .expect("left checksum");
        right
            .record_checksum(frame.frame, 222)
            .expect("right checksum");
        for _ in 0..4 {
            left.poll().expect("left poll");
            right.poll().expect("right poll");
            net.step();
        }
        let desync = left.first_desync().expect("left sees desync");
        assert_eq!(desync.frame, frame.frame);
        assert_eq!(desync.local, 111);
        assert_eq!(desync.remote, 222);
        assert!(right.first_desync().is_some(), "the other side sees it too");
    }
}
