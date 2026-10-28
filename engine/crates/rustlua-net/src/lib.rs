//! 确定性帧同步网络：输入延迟、按帧放行、状态校验和与 desync 检测。
//!
//! 设计目标是"同一份输入序列在任意机器上产出同一帧序列"：
//! - [`Session`] 只放行**收齐所有玩家**的帧，输入按玩家号排序后交给模拟层；
//! - 校验和由模拟层按帧上报，对端不一致时在首个分歧帧报 [`Desync`]；
//! - [`LoopbackNet`] 提供确定性延迟/抖动用于测试与本地双端；
//! - [`TcpTransport`] 是真实 socket 的传输，读写线程让 `poll` 不阻塞。
//!
//! 刻意不做的事：预测回滚、插值、可靠 UDP。这些应由具体游戏在模拟层之上实现，
//! 本 crate 只保证"帧与输入的次序"这一层契约。

mod lockstep;
mod loopback;
mod tcp;
mod wire;

pub use lockstep::{Desync, FrameInputs, Session, SessionConfig};
pub use loopback::{LoopbackNet, LoopbackTransport};
pub use tcp::TcpTransport;
pub use wire::{Message, PlayerId};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum NetError {
    #[error("network io: {0}")]
    Io(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("config: {0}")]
    Config(String),
}

/// 传输层契约：`send` 立即入队，`poll` 取走所有已到达的消息（非阻塞）。
pub trait Transport {
    /// 发给 `peer`（`None` 表示广播）。TCP 是点对点，`peer` 被忽略。
    fn send(&mut self, peer: Option<PlayerId>, message: &Message) -> Result<(), NetError>;
    /// 取走已到达的消息：`(发送者, 消息)`。
    fn poll(&mut self) -> Result<Vec<(PlayerId, Message)>, NetError>;
}
