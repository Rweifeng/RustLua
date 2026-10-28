//! 真实 TCP 传输：单条连接、长度前缀分帧，读写各一个后台线程。
//!
//! 线程模型让 `poll` 完全非阻塞：游戏循环每帧调用一次即可，不会因为对端慢而卡住。
//! 连接断开时读线程结束、`poll` 自然变空；写线程在通道关闭后刷新并退出。

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::wire::{Message, PlayerId};
use crate::{NetError, Transport};

/// 单帧上限：与本 crate 的输入负载上限保持同一量级。
const MAX_FRAME: usize = 64 * 1024;

pub struct TcpTransport {
    player: PlayerId,
    outgoing: Sender<Vec<u8>>,
    incoming: Receiver<Message>,
}

impl TcpTransport {
    /// 客户端：连接到 `addr`，以 `player` 身份入座。
    pub fn connect(addr: impl ToSocketAddrs, player: PlayerId) -> Result<Self, NetError> {
        let stream = TcpStream::connect(addr).map_err(io_error)?;
        Self::spawn(stream, player)
    }

    /// 服务端：接受一条连接，以 `player` 身份入座。
    pub fn accept(listener: &TcpListener, player: PlayerId) -> Result<Self, NetError> {
        let (stream, _) = listener.accept().map_err(io_error)?;
        Self::spawn(stream, player)
    }

    pub fn player(&self) -> PlayerId {
        self.player
    }

    fn spawn(stream: TcpStream, player: PlayerId) -> Result<Self, NetError> {
        stream.set_nodelay(true).map_err(io_error)?;
        let mut reader = stream.try_clone().map_err(io_error)?;
        let mut writer = stream;
        let (outgoing, outgoing_rx) = mpsc::channel::<Vec<u8>>();
        let (incoming_tx, incoming) = mpsc::channel::<Message>();

        thread::Builder::new()
            .name("rustlua-net-writer".to_owned())
            .spawn(move || {
                while let Ok(bytes) = outgoing_rx.recv() {
                    let header = (bytes.len() as u32).to_le_bytes();
                    if writer
                        .write_all(&header)
                        .and_then(|()| writer.write_all(&bytes))
                        .is_err()
                    {
                        break;
                    }
                }
                let _ = writer.flush();
            })
            .map_err(io_error)?;

        thread::Builder::new()
            .name("rustlua-net-reader".to_owned())
            .spawn(move || {
                loop {
                    let mut header = [0u8; 4];
                    if reader.read_exact(&mut header).is_err() {
                        break;
                    }
                    let len = u32::from_le_bytes(header) as usize;
                    if len == 0 || len > MAX_FRAME {
                        break;
                    }
                    let mut body = vec![0u8; len];
                    if reader.read_exact(&mut body).is_err() {
                        break;
                    }
                    match Message::decode(&body) {
                        Ok(message) => {
                            if incoming_tx.send(message).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(io_error)?;

        Ok(Self {
            player,
            outgoing,
            incoming,
        })
    }
}

impl Transport for TcpTransport {
    fn send(&mut self, _peer: Option<PlayerId>, message: &Message) -> Result<(), NetError> {
        // 单条连接即一对一：`peer` 只用于 loopback 的定向发送。
        self.outgoing
            .send(message.encode())
            .map_err(|_| NetError::Io("tcp transport is closed".to_owned()))
    }

    fn poll(&mut self) -> Result<Vec<(PlayerId, Message)>, NetError> {
        let mut out = Vec::new();
        while let Ok(message) = self.incoming.try_recv() {
            let from = message.player();
            out.push((from, message));
        }
        Ok(out)
    }
}

fn io_error(error: std::io::Error) -> NetError {
    match error.kind() {
        ErrorKind::Interrupted => NetError::Io("interrupted".to_owned()),
        _ => NetError::Io(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::lockstep::{FrameInputs, Session, SessionConfig};

    fn payload(player: u8, frame: u32) -> Vec<u8> {
        let mut out = vec![player];
        out.extend_from_slice(&frame.to_le_bytes());
        out
    }

    /// 在真实 socket 上跑满 `frames` 帧；返回本端放行的每一帧输入。
    fn run_side<T: Transport>(transport: T, local: PlayerId, frames: u32) -> Vec<FrameInputs> {
        let config = SessionConfig::new(2, local, 2).expect("config");
        let mut session = Session::new(config, transport);
        session.prime(Vec::new(), 2).expect("prime");
        let mut log = Vec::new();
        let mut submitted = 0;
        let deadline = Instant::now() + Duration::from_secs(10);
        while log.len() < frames as usize {
            while submitted < frames {
                session.submit(payload(local, submitted)).expect("submit");
                submitted += 1;
            }
            session.poll().expect("poll");
            while let Some(frame) = session.try_advance() {
                session
                    .record_checksum(frame.frame, frame.checksum())
                    .expect("checksum");
                log.push(frame);
            }
            assert!(
                Instant::now() < deadline,
                "tcp session stalled at {} / {frames} frames",
                log.len()
            );
            thread::sleep(Duration::from_millis(1));
        }
        // 把最后几帧的校验和也交换完，再断言没有 desync。
        let drain = Instant::now() + Duration::from_millis(200);
        while Instant::now() < drain {
            session.poll().expect("drain");
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            session.first_desync(),
            None,
            "checksums must match over TCP"
        );
        log
    }

    #[test]
    fn tcp_sessions_converge_over_a_real_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");

        let host = thread::spawn(move || {
            let transport = TcpTransport::accept(&listener, 0).expect("accept");
            run_side(transport, 0, 80)
        });
        let client_transport = TcpTransport::connect(addr, 1).expect("connect");
        let client = run_side(client_transport, 1, 80);
        let host = host.join().expect("host thread");

        // 80 帧提交 + 2 帧预热（delay）都会被放行。
        assert_eq!(client.len(), 82, "client advanced every frame");
        assert_eq!(host.len(), 82, "host advanced every frame");
        assert_eq!(client, host, "both ends simulated identical inputs");
    }
}
