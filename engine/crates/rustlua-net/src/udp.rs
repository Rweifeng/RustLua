//! 真实 UDP 传输：经中继（或直连）转发，自带**可靠有序**层与重传。
//!
//! 为什么不是"UDP 就是不可靠，直接把丢包交给上层"：帧同步的输入必须最终送达，
//! 但迟到的输入仍有价值（回滚可以重模拟），所以本层只保证"不丢、不重、按序交付"，
//! 不保证"准时"。上层的滞后补偿（预测/回滚）见 [`crate::rollback`]。
//!
//! 线上格式（小端）：
//!
//! ```text
//! [0..4]   magic  b"RLU1"
//! [4]      kind   1 = join（入房），2 = data（数据 / 纯 ACK）
//! [5..9]   seq    u32  发送序号（每发一条可靠消息 +1）
//! [9..13]  ack    u32  已连续收到的最大对端序号（0 表示还没收到过）
//! [13..21] bits   u64  ack 之前的 64 个序号位图（bit0 = ack-1）
//! [21..23] len    u16  负载长度
//! [23..]   payload
//! ```
//!
//! `kind = 1` 时负载是 `[room_len][room...][player]`；中继按房间把后续数据报原样
//! 转发给另一名成员。数据报的 seq/ack 由两端自己解释，中继不看内容。
//!
//! 设计边界：
//! - 单条消息必须装进一个数据报（默认上限 [`MAX_DATAGRAM`]，1200 字节，低于以太网 MTU），
//!   不做应用层分片——帧同步的输入与校验和都远小于这个尺寸。
//! - `poll` 非阻塞，同时负责收包、去重、按序交付与超时重传；调用频率决定了重传时延精度，
//!   游戏循环每帧调用一次即可（60Hz ≈ 16ms）。
//! - 中继本身不保证顺序，所以这里用序号 + 位图 ACK 自己兜底；重复包被丢弃。

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::net::{ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use crate::wire::{Message, PlayerId};
use crate::{NetError, Transport};

/// 单个数据报的硬上限，避免 IP 分片（分片丢失会放大丢包）。
pub const MAX_DATAGRAM: usize = 1200;

/// 重传超时。固定值而不是 RTT 自适应：帧同步的可靠层只承载小消息，150ms 已覆盖
/// 常见公网抖动；自适应 RTO 的复位逻辑在丢包突发时反而更容易把队列拖长。
pub const RETRANSMIT_AFTER: Duration = Duration::from_millis(150);

const MAGIC: [u8; 4] = *b"RLU1";
const HEADER: usize = 23;
const KIND_JOIN: u8 = 1;
const KIND_DATA: u8 = 2;
const MAX_ROOM: usize = 32;

/// 传输层统计：排障与测试共用（`retransmits > 0` 才证明丢包路径真的被走过）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UdpStats {
    pub sent: u64,
    pub received: u64,
    pub duplicate: u64,
    pub malformed: u64,
    pub retransmits: u64,
    pub delivered: u64,
}

struct Pending {
    payload: Vec<u8>,
    last_sent: Instant,
}

pub struct UdpTransport {
    player: PlayerId,
    socket: UdpSocket,
    room: String,
    next_seq: u32,
    /// 已发出但还没被对端 ACK 的数据报，按序号排序。
    outgoing: BTreeMap<u32, Pending>,
    /// 已到达但还不能交付（前面有缺口）的数据报。
    incoming: BTreeMap<u32, Message>,
    /// 下一条要按序交付的对端序号。
    next_deliver: u32,
    last_ack_sent: Option<Instant>,
    last_join_sent: Instant,
    joined: bool,
    stats: UdpStats,
}

impl UdpTransport {
    /// 以 `player` 身份加入 `relay` 上的 `room`。桌面传输用 `udp://` 前缀的服务器地址
    /// （`net` Lua 模块负责选路，这里只收 `host:port`）。
    pub fn connect(
        relay: impl ToSocketAddrs,
        room: &str,
        player: PlayerId,
    ) -> Result<Self, NetError> {
        if room.is_empty() || room.len() > MAX_ROOM || !room.is_ascii() {
            return Err(NetError::Config(format!(
                "room must be 1..={MAX_ROOM} ascii bytes, got {}",
                room.len()
            )));
        }
        let relay = relay
            .to_socket_addrs()
            .map_err(io_error)?
            .next()
            .ok_or_else(|| NetError::Config("relay address resolved to nothing".to_owned()))?;
        let bind = if relay.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = UdpSocket::bind(bind).map_err(io_error)?;
        socket.set_nonblocking(true).map_err(io_error)?;
        // connect 之后内核只收这个对端的包，避免被随机来源打满队列。
        socket.connect(relay).map_err(io_error)?;

        let mut transport = Self {
            player,
            socket,
            room: room.to_owned(),
            next_seq: 1,
            outgoing: BTreeMap::new(),
            incoming: BTreeMap::new(),
            next_deliver: 1,
            last_ack_sent: None,
            last_join_sent: Instant::now(),
            joined: false,
            stats: UdpStats::default(),
        };
        transport.send_join(room)?;
        Ok(transport)
    }

    /// 入房包可能被丢弃：在收到对端第一条消息前每 500ms 重发一次。
    fn resend_join_if_needed(&mut self) -> Result<(), NetError> {
        if self.joined || self.last_join_sent.elapsed() < Duration::from_millis(500) {
            return Ok(());
        }
        let room = self.room.clone();
        self.send_join(&room)?;
        self.last_join_sent = Instant::now();
        Ok(())
    }

    pub fn player(&self) -> PlayerId {
        self.player
    }

    pub fn stats(&self) -> UdpStats {
        self.stats
    }

    #[cfg(test)]
    pub fn debug_gap(&self) -> (u32, Vec<u32>, Vec<u32>) {
        (
            self.next_deliver,
            self.incoming.keys().copied().take(10).collect(),
            self.outgoing.keys().copied().take(10).collect(),
        )
    }

    fn send_join(&mut self, room: &str) -> Result<(), NetError> {
        let mut payload = Vec::with_capacity(2 + room.len());
        payload.push(room.len() as u8);
        payload.extend_from_slice(room.as_bytes());
        payload.push(self.player);
        let mut datagram = vec![0u8; HEADER];
        datagram[0..4].copy_from_slice(&MAGIC);
        datagram[4] = KIND_JOIN;
        datagram[21..23].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        datagram.extend_from_slice(&payload);
        self.socket.send(&datagram).map_err(io_error)?;
        self.stats.sent += 1;
        Ok(())
    }

    fn send_datagram(&mut self, seq: u32, payload: &[u8]) -> Result<(), NetError> {
        // ACK 只确认**连续**收到到哪里；乱序到达的更高序号进位图，避免发送方把中间
        // 被丢掉的那条误判为已确认（那会让重传永远不发生、接收方永久卡在缺口上）。
        let ack = self.next_deliver.saturating_sub(1);
        let mut bits = 0u64;
        if ack > 0 {
            for offset in 1..=64u32 {
                if self.incoming.contains_key(&ack.wrapping_add(offset)) {
                    bits |= 1u64 << (offset - 1);
                }
            }
        }
        let mut datagram = Vec::with_capacity(HEADER + payload.len());
        datagram.extend_from_slice(&MAGIC);
        datagram.push(KIND_DATA);
        datagram.extend_from_slice(&seq.to_le_bytes());
        datagram.extend_from_slice(&ack.to_le_bytes());
        datagram.extend_from_slice(&bits.to_le_bytes());
        datagram.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        datagram.extend_from_slice(payload);
        self.socket.send(&datagram).map_err(io_error)?;
        self.stats.sent += 1;
        Ok(())
    }

    /// 只发 ACK（没有待发数据时用于及时确认，避免对端白等一个 RTO）。
    fn send_ack(&mut self) -> Result<(), NetError> {
        self.send_datagram(0, &[])?;
        self.last_ack_sent = Some(Instant::now());
        Ok(())
    }

    fn apply_ack(&mut self, ack: u32, bits: u64) {
        if ack == 0 {
            return;
        }
        // `ack` 表示对端已按序收到到 ack 为止，之前的一定也已交付。
        let acked: Vec<u32> = self.outgoing.range(..=ack).map(|(seq, _)| *seq).collect();
        for seq in acked {
            self.outgoing.remove(&seq);
        }
        for offset in 1..=64u32 {
            if bits & (1u64 << (offset - 1)) != 0 {
                self.outgoing.remove(&ack.wrapping_sub(offset));
            }
        }
    }

    /// 解析一条数据报；返回 `true` 表示有新数据需要按序交付。
    fn on_datagram(&mut self, bytes: &[u8]) -> Result<bool, NetError> {
        if bytes.len() < HEADER || bytes[0..4] != MAGIC {
            self.stats.malformed += 1;
            return Ok(false);
        }
        let len = u16::from_le_bytes([bytes[21], bytes[22]]) as usize;
        if bytes.len() != HEADER + len {
            self.stats.malformed += 1;
            return Ok(false);
        }
        match bytes[4] {
            KIND_DATA => {
                let seq = u32::from_le_bytes(bytes[5..9].try_into().expect("seq"));
                let ack = u32::from_le_bytes(bytes[9..13].try_into().expect("ack"));
                let bits = u64::from_le_bytes(bytes[13..21].try_into().expect("bits"));
                self.apply_ack(ack, bits);
                if seq == 0 {
                    return Ok(false);
                }
                if seq < self.next_deliver || self.incoming.contains_key(&seq) {
                    self.stats.duplicate += 1;
                    return Ok(false);
                }
                self.stats.received += 1;
                self.joined = true;
                let message = Message::decode(&bytes[HEADER..])?;
                self.incoming.insert(seq, message);
                Ok(true)
            }
            KIND_JOIN => Ok(false),
            _ => {
                self.stats.malformed += 1;
                Ok(false)
            }
        }
    }

    fn drain(&mut self) -> Result<(), NetError> {
        let mut buffer = [0u8; 2048];
        loop {
            match self.socket.recv(&mut buffer) {
                Ok(size) => {
                    self.on_datagram(&buffer[..size])?;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                // 对端未启动时 Windows 会报 ConnectionReset，按"I/O 暂时不可用"处理。
                Err(error) if error.kind() == ErrorKind::ConnectionReset => break,
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(())
    }

    fn retransmit_due(&mut self) -> Result<(), NetError> {
        let now = Instant::now();
        let due: Vec<u32> = self
            .outgoing
            .iter()
            .filter(|(_, pending)| now.duration_since(pending.last_sent) >= RETRANSMIT_AFTER)
            .map(|(seq, _)| *seq)
            .collect();
        for seq in due {
            let payload = self
                .outgoing
                .get(&seq)
                .map(|pending| pending.payload.clone())
                .expect("due entry exists");
            self.send_datagram(seq, &payload)?;
            if let Some(pending) = self.outgoing.get_mut(&seq) {
                pending.last_sent = now;
            }
            self.stats.retransmits += 1;
        }
        Ok(())
    }

    /// 主动发一条不可靠数据报（不进重传队列）。用于快照/诊断这类"过期就作废"的消息。
    pub fn send_unreliable(&mut self, message: &Message) -> Result<(), NetError> {
        let payload = message.encode();
        if payload.len() + HEADER > MAX_DATAGRAM {
            return Err(NetError::Config(format!(
                "unreliable payload is {} bytes, datagram limit {}",
                payload.len(),
                MAX_DATAGRAM - HEADER
            )));
        }
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        self.send_datagram(seq, &payload)
    }
}

impl Transport for UdpTransport {
    fn send(&mut self, _peer: Option<PlayerId>, message: &Message) -> Result<(), NetError> {
        let payload = message.encode();
        if payload.len() + HEADER > MAX_DATAGRAM {
            return Err(NetError::Config(format!(
                "udp payload is {} bytes, datagram limit {} — raise the fragment budget or send state out of band",
                payload.len(),
                MAX_DATAGRAM - HEADER
            )));
        }
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        self.send_datagram(seq, &payload)?;
        self.outgoing.insert(
            seq,
            Pending {
                payload,
                last_sent: Instant::now(),
            },
        );
        Ok(())
    }

    fn poll(&mut self) -> Result<Vec<(PlayerId, Message)>, NetError> {
        self.resend_join_if_needed()?;
        self.drain()?;
        let mut out = Vec::new();
        while let Some(message) = self.incoming.remove(&self.next_deliver) {
            self.next_deliver = self.next_deliver.wrapping_add(1);
            self.stats.delivered += 1;
            out.push((message.player(), message));
        }
        let ack_due = self.last_ack_sent.map_or(!out.is_empty(), |sent| {
            sent.elapsed() >= Duration::from_millis(10)
        });
        if ack_due {
            self.send_ack()?;
        }
        self.retransmit_due()?;
        Ok(out)
    }
}

fn io_error(error: std::io::Error) -> NetError {
    NetError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::lockstep::{Session, SessionConfig};

    /// 进程内中继：按房间配对两个地址并互转数据报；`drop_every > 0` 时周期性丢包，
    /// 用来证明重传真的兜住了（否则"不丢包"的测试绿灯什么也证明不了）。
    fn spawn_relay(drop_every: u64) -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("bind relay");
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .expect("timeout");
        let addr = socket.local_addr().expect("relay addr");
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(AtomicU64::new(0));
        let stop_thread = stop.clone();
        thread::spawn(move || {
            let mut rooms: BTreeMap<String, Vec<SocketAddr>> = BTreeMap::new();
            let mut buffer = [0u8; 2048];
            while !stop_thread.load(Ordering::Relaxed) {
                let (size, from) = match socket.recv_from(&mut buffer) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if size < HEADER || buffer[0..4] != MAGIC {
                    continue;
                }
                if buffer[4] == KIND_JOIN {
                    let room_len = buffer[HEADER] as usize;
                    let room = String::from_utf8_lossy(&buffer[HEADER + 1..HEADER + 1 + room_len])
                        .into_owned();
                    let members = rooms.entry(room).or_default();
                    if !members.contains(&from) {
                        members.push(from);
                    }
                    continue;
                }
                // 数据报：找到同房间的另一个成员转发。
                let mut target = None;
                for members in rooms.values() {
                    if members.contains(&from) {
                        target = members.iter().find(|other| **other != from).copied();
                        break;
                    }
                }
                let Some(target) = target else { continue };
                let count = seen.fetch_add(1, Ordering::Relaxed);
                // 伪随机丢包：按固定步长丢会和"整批发送 + 固定重传节奏"形成相位对齐，
                // 某些序号会被永久丢掉——那是测试中继的假象，不是可靠层的问题。
                if drop_every > 0 {
                    let mixed = count.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29);
                    if mixed.is_multiple_of(drop_every) {
                        continue;
                    }
                }
                let _ = socket.send_to(&buffer[..size], target);
            }
        });
        addr
    }

    fn payload(player: u8, frame: u32) -> Vec<u8> {
        let mut out = vec![player];
        out.extend_from_slice(&frame.to_le_bytes());
        out
    }

    #[test]
    fn udp_sessions_converge_over_a_lossy_loopback() {
        let relay = spawn_relay(4);
        let a = UdpTransport::connect(relay, "test", 0).expect("host connect");
        let b = UdpTransport::connect(relay, "test", 1).expect("client connect");
        let mut host = Session::new(SessionConfig::new(2, 0, 2).expect("config"), a);
        let mut client = Session::new(SessionConfig::new(2, 1, 2).expect("config"), b);
        host.prime(Vec::new(), 2).expect("prime");
        client.prime(Vec::new(), 2).expect("prime");
        for frame in 0..60u32 {
            host.submit(payload(0, frame)).expect("host submit");
            client.submit(payload(1, frame)).expect("client submit");
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut host_log = Vec::new();
        let mut client_log = Vec::new();
        while host_log.len() < 62 || client_log.len() < 62 {
            host.poll().expect("host poll");
            client.poll().expect("client poll");
            while let Some(frame) = host.try_advance() {
                host.record_checksum(frame.frame, frame.checksum())
                    .expect("checksum");
                host_log.push(frame);
            }
            while let Some(frame) = client.try_advance() {
                client
                    .record_checksum(frame.frame, frame.checksum())
                    .expect("checksum");
                client_log.push(frame);
            }
            assert!(
                Instant::now() < deadline,
                "udp session stalled: host {} client {} host_next {} client_next {} host {:?} gap {:?} client {:?} gap {:?}",
                host_log.len(),
                client_log.len(),
                host.next_frame(),
                client.next_frame(),
                host.transport().stats(),
                host.transport().debug_gap(),
                client.transport().stats(),
                client.transport().debug_gap()
            );
            thread::sleep(Duration::from_millis(1));
        }
        let drain = Instant::now() + Duration::from_millis(200);
        while Instant::now() < drain {
            host.poll().expect("host drain");
            client.poll().expect("client drain");
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(host.first_desync(), None);
        assert_eq!(client.first_desync(), None);
        assert_eq!(host_log.len(), 62);
        assert_eq!(host_log, client_log);
    }

    #[test]
    fn udp_retransmits_when_datagrams_are_dropped() {
        // 丢包中继下必然发生重传，否则说明"可靠层"根本没在工作。
        let relay = spawn_relay(3);
        let host_transport = UdpTransport::connect(relay, "lossy", 0).expect("host connect");
        let mut peer = UdpTransport::connect(relay, "lossy", 1).expect("peer connect");
        let mut session =
            Session::new(SessionConfig::new(2, 0, 2).expect("config"), host_transport);
        session.prime(Vec::new(), 2).expect("prime");
        for frame in 0..40u32 {
            session.submit(payload(0, frame)).expect("submit");
        }
        for frame in 0..42u32 {
            peer.send(
                None,
                &Message::Input {
                    player: 1,
                    frame,
                    payload: Vec::new(),
                },
            )
            .expect("peer send");
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut advanced = 0;
        while advanced < 42 {
            session.poll().expect("poll");
            while session.try_advance().is_some() {
                advanced += 1;
            }
            peer.poll().expect("peer poll");
            assert!(
                Instant::now() < deadline,
                "lossy session stalled at {advanced} / 42 frames"
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            session.transport().stats().retransmits > 0,
            "lossy relay must trigger retransmission"
        );
    }

    #[test]
    fn udp_rejects_oversized_messages() {
        let relay = spawn_relay(0);
        let mut transport = UdpTransport::connect(relay, "big", 0).expect("connect");
        let message = Message::Input {
            player: 0,
            frame: 0,
            payload: vec![0u8; MAX_DATAGRAM],
        };
        assert!(matches!(
            transport.send(None, &message),
            Err(NetError::Config(_))
        ));
    }

    #[test]
    fn udp_rejects_bad_rooms() {
        let relay = spawn_relay(0);
        assert!(matches!(
            UdpTransport::connect(relay, "", 0),
            Err(NetError::Config(_))
        ));
        assert!(matches!(
            UdpTransport::connect(relay, &"a".repeat(MAX_ROOM + 1), 0),
            Err(NetError::Config(_))
        ));
    }
}
