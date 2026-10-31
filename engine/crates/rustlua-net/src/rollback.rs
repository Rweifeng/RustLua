//! 预测回滚账本：保管"每帧输入 + 模拟前快照"，把迟到输入翻译成"从哪一帧重模拟"。
//!
//! 本模块刻意不碰模拟层：快照是模拟层给的**不透明字节**（Lua 侧通常是自己序列化
//! 出来的状态串），本模块只负责三件事：
//!
//! 1. **预测**：前一帧还没收到某玩家的输入时，沿用他最近一次确认输入；
//! 2. **修正**：迟到的确认输入如果与当时用的预测不同，就要求从那一帧起重模拟；
//! 3. **窗口**：只保留最近 `window` 帧的快照，更早的迟到输入被判 `TooLate`。
//!
//! 典型调用顺序（每帧）：
//!
//! ```text
//! let frame = rb.next_frame();
//! rb.save_snapshot(frame, world.snapshot())?;      // 模拟前
//! let inputs = rb.inputs_for(frame);               // 预测已补齐
//! simulate(frame, &inputs);
//! rb.commit_frame(frame)?;
//! // 收包：
//! if let RollbackStep::Resimulate { from } = rb.accept_remote(player, frame, payload) {
//!     let snap = rb.rollback_to(from)?;            // 恢复到 from 之前的快照
//!     restore(snap);
//!     // 重新模拟 from .. rb.next_frame()
//! }
//! ```
//!
//! 回滚的代价是重模拟窗口内的帧；`stats()` 把重模拟次数与帧数暴露出来，便于决定
//! 预测窗口与输入延迟。

use std::collections::BTreeMap;

use crate::NetError;
use crate::wire::PlayerId;

/// 回滚窗口参数。`window` 是"最多往回追多少帧"，也是快照环的容量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RollbackConfig {
    pub players: PlayerId,
    pub local: PlayerId,
    pub window: u32,
}

impl RollbackConfig {
    pub fn new(players: PlayerId, local: PlayerId, window: u32) -> Result<Self, NetError> {
        if players == 0 {
            return Err(NetError::Config("players must be at least 1".to_owned()));
        }
        if local >= players {
            return Err(NetError::Config(format!(
                "local player {local} is outside 0..{players}"
            )));
        }
        if window == 0 {
            return Err(NetError::Config(
                "rollback window must be at least 1".to_owned(),
            ));
        }
        Ok(Self {
            players,
            local,
            window,
        })
    }
}

/// 一次输入到达的判定结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollbackStep {
    /// 帧还没模拟过，或与当时预测一致：无需重模拟。
    Accepted,
    /// 已模拟过的帧收到不同的确认输入：从 `from` 帧（含）起重模拟。
    Resimulate { from: u32 },
    /// 修正输入早于保留窗口：无法回滚，记账为 `stale`。
    TooLate { frame: u32 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RollbackStats {
    /// 触发的重模拟次数。
    pub resimulations: u64,
    /// 累计需要重模拟的帧数（每次触发时 `next_frame - from` 的累加）。
    pub resimulated_frames: u64,
    /// 落在窗口之外、无法回滚的迟到输入条数。
    pub stale_inputs: u64,
    /// 用预测输入模拟过的帧次数（每帧每缺一个玩家计 1）。
    pub predicted_inputs: u64,
}

pub struct Rollback {
    config: RollbackConfig,
    /// 已确认（含本机提交）的输入：frame -> player -> payload。
    confirmed: BTreeMap<u32, BTreeMap<PlayerId, Vec<u8>>>,
    /// 模拟该帧时实际用过的输入（确认或预测），用于判断"修正是否真的改了结果"。
    used: BTreeMap<(u32, PlayerId), Vec<u8>>,
    /// 最近一次确认输入，作为预测值。
    last_known: BTreeMap<PlayerId, Vec<u8>>,
    /// 每个待模拟帧之前的状态快照。
    snapshots: BTreeMap<u32, Vec<u8>>,
    /// 所有 `< next_frame` 的帧都已模拟完。
    next_frame: u32,
    stats: RollbackStats,
}

impl Rollback {
    pub fn new(config: RollbackConfig) -> Self {
        Self {
            config,
            confirmed: BTreeMap::new(),
            used: BTreeMap::new(),
            last_known: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            next_frame: 0,
            stats: RollbackStats::default(),
        }
    }

    pub fn config(&self) -> RollbackConfig {
        self.config
    }

    pub fn stats(&self) -> RollbackStats {
        self.stats
    }

    /// 下一个要模拟的帧号。
    pub fn next_frame(&self) -> u32 {
        self.next_frame
    }

    /// 记录本机输入（应立即生效，不算预测）。
    pub fn record_local(&mut self, frame: u32, payload: Vec<u8>) -> Result<(), NetError> {
        self.insert_confirmed(self.config.local, frame, payload);
        Ok(())
    }

    /// 收到远端输入。返回是否需要重模拟——`from` 是**最早**受影响的帧。
    pub fn accept_remote(
        &mut self,
        player: PlayerId,
        frame: u32,
        payload: Vec<u8>,
    ) -> Result<RollbackStep, NetError> {
        if player == self.config.local {
            return Err(NetError::Protocol(
                "remote input claims the local player".to_owned(),
            ));
        }
        if player >= self.config.players {
            return Err(NetError::Protocol(format!(
                "player {player} is outside 0..{}",
                self.config.players
            )));
        }
        if frame < self.next_frame {
            let already_used = self.used.get(&(frame, player));
            if already_used.is_some_and(|used| used == &payload) {
                self.insert_confirmed(player, frame, payload);
                return Ok(RollbackStep::Accepted);
            }
            if !self.snapshots.contains_key(&frame) {
                self.stats.stale_inputs += 1;
                self.insert_confirmed(player, frame, payload);
                return Ok(RollbackStep::TooLate { frame });
            }
            let next = self.next_frame;
            self.insert_confirmed(player, frame, payload);
            self.stats.resimulations += 1;
            self.stats.resimulated_frames += u64::from(next - frame);
            return Ok(RollbackStep::Resimulate { from: frame });
        }
        self.insert_confirmed(player, frame, payload);
        Ok(RollbackStep::Accepted)
    }

    fn insert_confirmed(&mut self, player: PlayerId, frame: u32, payload: Vec<u8>) {
        self.last_known.insert(player, payload.clone());
        self.confirmed
            .entry(frame)
            .or_default()
            .insert(player, payload);
    }

    /// 取一帧要喂给模拟层的完整输入（按玩家号升序）。缺失的输入用最近确认值预测，
    /// 并记进 `used` 供后续对账。
    pub fn inputs_for(&mut self, frame: u32) -> Vec<(PlayerId, Vec<u8>)> {
        let mut out = Vec::with_capacity(self.config.players as usize);
        for player in 0..self.config.players {
            let confirmed = self
                .confirmed
                .get(&frame)
                .and_then(|inputs| inputs.get(&player))
                .cloned();
            let payload = match confirmed {
                Some(payload) => payload,
                None => {
                    self.stats.predicted_inputs += 1;
                    self.last_known.get(&player).cloned().unwrap_or_default()
                }
            };
            self.used.insert((frame, player), payload.clone());
            out.push((player, payload));
        }
        out
    }

    /// 模拟第 `frame` 帧之前调用：存入快照并淘汰窗口外的旧快照。
    pub fn save_snapshot(&mut self, frame: u32, bytes: Vec<u8>) {
        if frame != self.next_frame {
            return;
        }
        self.snapshots.insert(frame, bytes);
        let keep_from = frame.saturating_sub(self.config.window.saturating_sub(1));
        self.snapshots = self.snapshots.split_off(&keep_from);
    }

    /// 第 `frame` 帧模拟完成。
    pub fn commit_frame(&mut self, frame: u32) -> Result<(), NetError> {
        if frame != self.next_frame {
            return Err(NetError::Config(format!(
                "commit frame {frame} out of order: expected {}",
                self.next_frame
            )));
        }
        self.next_frame += 1;
        Ok(())
    }

    /// 回滚：返回 `from` 帧模拟前的快照，并把时钟倒回该帧。
    ///
    /// `from` 之后已存的快照与"实际用过输入"会被丢弃——它们会在重模拟里重新生成；
    /// 已确认输入保留（重模拟正是要用它们）。
    pub fn rollback_to(&mut self, from: u32) -> Result<Vec<u8>, NetError> {
        let snapshot =
            self.snapshots.get(&from).cloned().ok_or_else(|| {
                NetError::Config(format!("no snapshot retained for frame {from}"))
            })?;
        self.snapshots.retain(|frame, _| *frame <= from);
        self.used.retain(|(frame, _), _| *frame < from);
        self.next_frame = from;
        Ok(snapshot)
    }

    /// 该帧快照是否还在窗口里（测试与诊断用）。
    pub fn has_snapshot(&self, frame: u32) -> bool {
        self.snapshots.contains_key(&frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RollbackConfig {
        RollbackConfig::new(2, 1, 4).expect("config")
    }

    /// 模拟"跑到第 n 帧"：每帧存快照、取输入、提交。
    fn simulate(session: &mut Rollback, through: u32) {
        while session.next_frame() < through {
            let frame = session.next_frame();
            session.save_snapshot(frame, format!("snap:{frame}").into_bytes());
            let _ = session.inputs_for(frame);
            session.commit_frame(frame).expect("commit");
        }
    }

    #[test]
    fn prediction_fills_missing_remote_and_is_replaced_by_confirmation() {
        let mut session = Rollback::new(config());
        session.record_local(0, b"L0".to_vec()).expect("local");
        session.accept_remote(0, 0, b"R0".to_vec()).expect("remote");
        let inputs = session.inputs_for(0);
        assert_eq!(inputs, vec![(0, b"R0".to_vec()), (1, b"L0".to_vec())]);
        session.commit_frame(0).expect("commit");

        // 第 1 帧远端还没到：用上一帧确认值 R0 预测（本机输入照常提交）。
        session.record_local(1, b"L1".to_vec()).expect("local");
        let inputs = session.inputs_for(1);
        assert_eq!(inputs[0], (0, b"R0".to_vec()));
        assert_eq!(session.stats().predicted_inputs, 1);
    }

    #[test]
    fn late_input_after_simulation_requests_resimulation() {
        let mut session = Rollback::new(config());
        session.record_local(0, b"L0".to_vec()).expect("local");
        simulate(&mut session, 3);
        let step = session.accept_remote(0, 1, b"R1".to_vec()).expect("accept");
        assert_eq!(step, RollbackStep::Resimulate { from: 1 });
        assert_eq!(session.stats().resimulations, 1);
        assert_eq!(session.stats().resimulated_frames, 2);
        let snapshot = session.rollback_to(1).expect("rollback");
        assert_eq!(snapshot, b"snap:1");
        assert_eq!(session.next_frame(), 1);
    }

    #[test]
    fn matching_late_input_does_not_resimulate() {
        let mut session = Rollback::new(config());
        session.record_local(0, b"L0".to_vec()).expect("local");
        session.accept_remote(0, 0, b"R0".to_vec()).expect("remote");
        simulate(&mut session, 3);
        // 预测用的就是 R0，迟到确认与原预测一致。
        let step = session.accept_remote(0, 2, b"R0".to_vec()).expect("accept");
        assert_eq!(step, RollbackStep::Accepted);
        assert_eq!(session.stats().resimulations, 0);
    }

    #[test]
    fn input_older_than_window_is_stale() {
        let mut session = Rollback::new(config());
        session.record_local(0, b"L".to_vec()).expect("local");
        simulate(&mut session, 8);
        assert!(
            !session.has_snapshot(0),
            "window=4 keeps only the last frames"
        );
        let step = session
            .accept_remote(0, 0, b"late".to_vec())
            .expect("accept");
        assert_eq!(step, RollbackStep::TooLate { frame: 0 });
        assert_eq!(session.stats().stale_inputs, 1);
        assert!(session.rollback_to(0).is_err());
    }

    #[test]
    fn rollback_retains_confirmed_inputs_but_drops_later_snapshots() {
        let mut session = Rollback::new(config());
        session.record_local(0, b"L0".to_vec()).expect("local");
        session.accept_remote(0, 0, b"R0".to_vec()).expect("remote");
        session.accept_remote(0, 2, b"R2".to_vec()).expect("remote");
        simulate(&mut session, 4);
        session.rollback_to(1).expect("rollback");
        assert!(!session.has_snapshot(2), "later snapshots are regenerated");
        let inputs = session.inputs_for(2);
        assert_eq!(inputs[0], (0, b"R2".to_vec()), "confirmed input survives");
    }

    #[test]
    fn commit_out_of_order_is_rejected() {
        let mut session = Rollback::new(config());
        assert!(session.commit_frame(1).is_err());
        session.commit_frame(0).expect("commit 0");
        assert!(session.commit_frame(0).is_err());
    }

    #[test]
    fn rejects_remote_input_for_local_player() {
        let mut session = Rollback::new(config());
        assert!(session.accept_remote(1, 0, Vec::new()).is_err());
        assert!(session.accept_remote(2, 0, Vec::new()).is_err());
    }
}
