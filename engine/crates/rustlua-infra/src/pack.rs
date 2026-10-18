//! 发布资源编解码：可选 gzip 压缩 + chacha20poly1305 加密。
//!
//! 编码文件带 `RustLua` 头部 + 标志字节 +（加密时）nonce + payload；
//! `decode` 自动检测：非 `RustLua` 文件（明文/开发模式）原样返回，
//! 因此同一套运行时代码既能加载开发明文，也能加载发布加密包。

use chacha20poly1305::{
    ChaCha20Poly1305, Nonce,
    aead::{Aead, KeyInit},
};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use std::io::{Read, Write};

/// 打包文件魔数（7 字节 ASCII `RustLua`；长度由 `PACK_MAGIC.len()` 统一推导，
/// 因此加长不影响 `encode`/`decode` 的游标计算）。
pub const PACK_MAGIC: &[u8; 7] = b"RustLua";
const FLAG_COMPRESSED: u8 = 1;
const FLAG_ENCRYPTED: u8 = 2;
const NONCE_LEN: usize = 12;

/// 固定 32 字节密钥（内嵌于可执行文件；客户端加密的固有局限是密钥可被
/// 提取，仅提高门槛，防止随意读取）。
const KEY: [u8; 32] = [
    0x2a, 0x7f, 0x1b, 0x44, 0x91, 0x03, 0x5e, 0xcc, 0x88, 0x0d, 0x42, 0x6a, 0x13, 0xe0, 0x57, 0x29,
    0xb4, 0x6c, 0x01, 0x82, 0x3d, 0x97, 0x55, 0x0a, 0x60, 0x24, 0x8e, 0x71, 0xc9, 0x32, 0xf0, 0x4d,
];

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = GzDecoder::new(bytes);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|error| format!("解压失败: {error}"))?;
    Ok(out)
}

fn seal(plaintext: &[u8], nonce: &[u8; NONCE_LEN]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new((&KEY).into());
    cipher
        .encrypt(Nonce::from_slice(nonce), plaintext)
        .expect("encrypt")
}

fn open(ciphertext: &[u8], nonce: &[u8; NONCE_LEN]) -> Result<Vec<u8>, String> {
    let cipher = ChaCha20Poly1305::new((&KEY).into());
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| "解密失败（密钥不匹配或数据损坏）".to_owned())
}

/// 给**容器块级 AEAD**（`docs/67` §4.4 批 F）用的同一把密钥与同一个算法。
///
/// 为什么复用而不是另开一把：密钥内嵌在可执行文件里，多一把不会更安全，只会多一处
/// 要对齐的常量；而"同一把密钥保护两种发布形态"让 `--encrypt` 的语义保持一致
/// （目录形态是整文件 AEAD，容器形态是块级 AEAD）。
pub fn aead_key() -> [u8; 32] {
    KEY
}

/// 用固定密钥对一段字节做 AEAD 封装（附 AAD），供容器块使用。
pub fn seal_with_aad(plaintext: &[u8], nonce: &[u8; NONCE_LEN], aad: &[u8]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new((&KEY).into());
    cipher
        .encrypt(
            Nonce::from_slice(nonce),
            chacha20poly1305::aead::Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("encrypt")
}

/// [`seal_with_aad`] 的逆操作；失败统一成一个不泄露细节的错误（与 [`open`] 同口径）。
pub fn open_with_aad(
    ciphertext: &[u8],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
) -> Result<Vec<u8>, String> {
    let cipher = ChaCha20Poly1305::new((&KEY).into());
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            chacha20poly1305::aead::Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| "解密失败（密钥不匹配、AAD 不符或数据损坏）".to_owned())
}

/// 编码资源：按需压缩、加密。
pub fn encode(bytes: &[u8], compress: bool, encrypt: bool) -> Vec<u8> {
    if !compress && !encrypt {
        return bytes.to_vec();
    }
    let mut flags = 0u8;
    let mut payload = bytes.to_vec();
    if compress {
        payload = gzip(&payload);
        flags |= FLAG_COMPRESSED;
    }
    let mut nonce = [0u8; NONCE_LEN];
    if encrypt {
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce);
        payload = seal(&payload, &nonce);
        flags |= FLAG_ENCRYPTED;
    }
    let mut out = Vec::with_capacity(PACK_MAGIC.len() + 1 + NONCE_LEN + payload.len());
    out.extend_from_slice(PACK_MAGIC);
    out.push(flags);
    if encrypt {
        out.extend_from_slice(&nonce);
    }
    out.extend_from_slice(&payload);
    out
}

/// 解码资源；非 `RustLua` 文件（明文/开发模式）原样返回。
///
/// 例外：收到 [`crate::bundle::BUNDLE_MAGIC`]（RLB 资源包）**必须报错**而不是当明文透传。
/// 资源包是容器、不是资源；若把它原样交给调用方，Lua 加载器会把整包当脚本执行，
/// 症状是"语法错误出现在某个二进制行"——那种错误没有人能据此定位（`docs/67` §4.6）。
pub fn decode(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.starts_with(crate::bundle::BUNDLE_MAGIC) {
        return Err(
            "这是 RLB 资源包（容器），不是单个资源：请经 bundle 挂载后按条目读取".to_owned(),
        );
    }
    if !bytes.starts_with(PACK_MAGIC) {
        return Ok(bytes.to_vec());
    }
    if bytes.len() < PACK_MAGIC.len() + 1 {
        return Err("资源头损坏".to_owned());
    }
    let flags = bytes[PACK_MAGIC.len()];
    let cursor = PACK_MAGIC.len() + 1;
    let mut payload = bytes[cursor..].to_vec();
    if flags & FLAG_ENCRYPTED != 0 {
        if payload.len() < NONCE_LEN {
            return Err("加密资源缺少 nonce".to_owned());
        }
        let nonce: [u8; NONCE_LEN] = payload[..NONCE_LEN]
            .try_into()
            .expect("nonce length is fixed");
        payload = open(&payload[NONCE_LEN..], &nonce)?;
    }
    if flags & FLAG_COMPRESSED != 0 {
        payload = gunzip(&payload)?;
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_passes_through_decode() {
        let data = b"function update() return 'ok' end".to_vec();
        assert_eq!(decode(&data).unwrap(), data);
    }

    #[test]
    fn bundle_magic_is_rejected_not_passed_through() {
        // 把资源包交给单文件解码器必须响亮失败：否则整包会被当明文脚本执行。
        let mut bundle = crate::bundle::BUNDLE_MAGIC.to_vec();
        bundle.extend_from_slice(&[0u8; 32]);
        let error = decode(&bundle).expect_err("RLB 包不能当资源解码");
        assert!(error.contains("RLB"), "得到：{error}");
    }

    #[test]
    fn compress_encrypt_roundtrip() {
        let data = "local s = '重复的中文内容 for 压缩 测试 12345' "
            .repeat(50)
            .into_bytes();
        let encoded = encode(&data, true, true);
        assert!(encoded.starts_with(PACK_MAGIC));
        assert_ne!(encoded, data);
        assert_eq!(decode(&encoded).unwrap(), data);
    }

    #[test]
    fn encrypt_only_and_compress_only_roundtrip() {
        let data = "plain text payload".repeat(20).into_bytes();
        assert_eq!(decode(&encode(&data, false, true)).unwrap(), data);
        assert_eq!(decode(&encode(&data, true, false)).unwrap(), data);
    }
}
