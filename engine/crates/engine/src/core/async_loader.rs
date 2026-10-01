//! P0-② 异步资源加载：后台读盘 + CPU 解码，主线程小步 GPU 上传。
//!
//! 背景（docs/40 WP1 的异步资源队列）：此前纹理与网格在渲染器热路径上**同步**读盘解码
//! （`TextureAsset::load_png` / `MeshAsset::from_gltf_path`），首次引用或场景切换时主线程
//! 被 CPU 解码冻住。本模块把「读盘 + 解码」移出主线程：
//!
//! - **Async 模式**（游戏运行，`Renderer::new`）：常驻 worker 线程消费请求队列；主线程
//!   每帧 [`crate::core::async_loader::AsyncLoader::drain_ready`] 取走已解码产物做 GPU 上传（wgpu 的 `queue.write_texture`
//!   提交本身不阻塞，真正的同步开销是 CPU 解码，因此收益集中在解码下移）。
//! - **Sync 模式**（golden / 编辑器离屏预览，`Renderer::new_offscreen`）：`request_*` 立即
//!   读盘解码并入队，保证确定性输出（异步加载永远不会出现在 golden 的像素里）。
//! - **wasm**：`std::thread` 不可用，worker 数恒为 0，`Async` 请求自动退化为同步完成
//!   （编译期判定，行为诚实：Web 端不假装有后台线程）。
//!
//! 关键契约：
//! 1. **幂等去重**：同一 key 在「排队 / 解码中 / 已就绪未取走 / 已取走」期间只解码一次；
//!    失败同样只试一次（读盘/解码失败通常不会自己好，回传 `error` 由调用方用占位资源，
//!    如白贴图，不自动重试）。
//! 2. **失败不崩溃**：任何失败都变成 `DecodedAsset.error`，worker 不 panic。
//! 3. **确定性**：Sync 与 Async 的解码产物逐字节一致（有测试断言同一 PNG 两种模式结果相同）。
//! 4. **降级即同步**：worker 线程创建失败时请求走同步完成路径，绝不丢请求、绝不死锁。

use std::{
    collections::{HashMap, VecDeque},
    fmt,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

use super::{
    mesh3d::MeshAsset,
    texture::{TextureAsset, TextureColorSpace},
};

/// 加载模式：游戏运行（后台线程）或确定性输出（同步完成）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadMode {
    /// 常驻 worker 线程读盘解码；wasm 下自动退化为同步。
    Async,
    /// 请求立即读盘解码，本帧即可用（golden / 编辑器离屏预览）。
    Sync,
}

/// 资源家族（与 `ResourceKind` 的说法一致，但只覆盖运行时会异步解码的两类）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadKind {
    /// PNG 贴图（`TextureAsset::load_png`，sRGB）。
    TexturePng,
    /// glTF/GLB 网格（`MeshAsset::from_gltf_path`）。
    GltfMesh,
}

/// 一个解码完成的资源：成功时 `texture` / `meshes` 恰有一个为 `Some`，
/// 失败时 `error` 为 `Some` 且两个产物均为 `None`。
#[derive(Clone, Debug)]
pub struct DecodedAsset {
    /// 请求时的 key（渲染器的路径字符串；同一 key 恒同一请求）。
    pub key: String,
    pub kind: LoadKind,
    pub texture: Option<TextureAsset>,
    pub meshes: Option<Vec<MeshAsset>>,
    pub error: Option<String>,
}

/// 解码请求（worker 消费）。`Clone` 仅为 send 失败时回退同步解码（不 clone 数据，只 clone 任务描述）。
#[derive(Clone)]
enum Job {
    Texture { key: String, path: PathBuf },
    Gltf { key: String, path: PathBuf },
}

/// 请求生命周期（只被主线程读写；worker 仅通过结果通道回传）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoadState {
    /// 已入队 / 解码中。
    Pending,
    /// 解码完成，产物在 `ready` 队列里等主线程取走。
    Ready,
    /// 已取走（成功或失败），不再重复解码。
    Done,
}

/// 后台读盘 + CPU 解码的异步加载器。
///
/// 使用方式：调用方（渲染器）在资源 MISS 时 `request_*`，每帧 `drain_ready()` 取走
/// 完成品并做 GPU 上传。`request_*` 永远不阻塞（Sync 模式除外——那是它的定义）。
pub struct AsyncLoader {
    mode: LoadMode,
    /// key → 状态（去重 + 生命周期）。
    inflight: HashMap<String, LoadState>,
    /// 已解码、等主线程取走的产物（Sync 与 Async 共用一个队列）。
    ready: VecDeque<DecodedAsset>,
    /// 每 worker 一个请求通道（Async 且至少一个 worker 时存在）。
    job_senders: Vec<mpsc::Sender<Job>>,
    /// 轮询分发游标（请求轮流发给下一个 worker，天然负载均衡）。
    next_worker: usize,
    /// worker 结果通道（Async 且至少一个 worker 时存在）。
    result_rx: Option<mpsc::Receiver<DecodedAsset>>,
    /// 实际工作的 worker 数（0 = 无后台，请求同步完成）。
    workers: usize,
}

/// 默认后台解码线程数：解码与磁盘 IO 交错，2 个足够；线程过多只会加剧真机帧抖动。
const DEFAULT_WORKERS: usize = 2;

impl AsyncLoader {
    /// 以默认 worker 数构造（Async 在 wasm 下 worker 数恒为 0）。
    #[must_use]
    pub fn new(mode: LoadMode) -> Self {
        Self::with_workers(mode, DEFAULT_WORKERS)
    }

    /// 指定 worker 数构造。`workers == 0` 时 Async 也退化为同步完成。
    #[must_use]
    pub fn with_workers(mode: LoadMode, workers: usize) -> Self {
        // wasm 没有 std::thread；无论调用方要多少 worker，实际恒为 0（同步完成）。
        #[cfg(target_arch = "wasm32")]
        let _ = workers; // consume the parameter before shadowing it
        #[cfg(target_arch = "wasm32")]
        let workers = 0;
        // 结果通道：多 worker 共用一个 sender（mpsc 天然多生产者），主线程持 receiver。
        let (result_tx, result_rx) = mpsc::channel::<DecodedAsset>();
        let mut job_senders = Vec::new();
        if mode == LoadMode::Async {
            for _ in 0..workers {
                // 每 worker 一条独立请求通道（mpsc 是单消费者，不能共享 receiver）。
                let (job_tx, job_rx) = mpsc::channel::<Job>();
                let result_tx = result_tx.clone();
                if thread::Builder::new()
                    .name("rustlua-asset".to_string())
                    .spawn(move || {
                        while let Ok(job) = job_rx.recv() {
                            // 解码失败以 `error` 回传，worker 本身不 panic。
                            if result_tx.send(decode_job(job)).is_err() {
                                break;
                            }
                        }
                    })
                    .is_ok()
                {
                    job_senders.push(job_tx);
                }
                // 线程起不来就少一个 worker；全起不来时请求走同步完成（降级即同步）。
            }
        }
        let workers = job_senders.len();
        Self {
            mode,
            inflight: HashMap::new(),
            ready: VecDeque::new(),
            job_senders,
            next_worker: 0,
            result_rx: (workers > 0).then_some(result_rx),
            workers,
        }
    }

    /// 实际生效的加载模式（worker 数为 0 时 Async 也按同步完成行为）。
    #[must_use]
    pub const fn mode(&self) -> LoadMode {
        if self.workers == 0 {
            LoadMode::Sync
        } else {
            self.mode
        }
    }

    /// 当前实际 worker 数（wasm / 线程创建失败时为 0）。
    #[must_use]
    pub const fn workers(&self) -> usize {
        self.workers
    }

    /// 请求异步解码一张 PNG 贴图（sRGB，与渲染器既有路径一致）。幂等。
    pub fn request_texture(&mut self, key: impl Into<String>, path: impl AsRef<Path>) {
        let key = key.into();
        self.request(
            key.clone(),
            LoadKind::TexturePng,
            path.as_ref().to_path_buf(),
        );
        // 请求总是带着 key 本身（下面 request 里用），上面一行仅为占位不再使用。
        let _ = key;
    }

    /// 请求异步解析一个 glTF/GLB 网格。幂等。
    pub fn request_gltf(&mut self, key: impl Into<String>, path: impl AsRef<Path>) {
        let key = key.into();
        self.request(key, LoadKind::GltfMesh, path.as_ref().to_path_buf());
    }

    /// 请求加载：Async 下入队后立即返回；Sync 下立即解码并入 `ready`。
    fn request(&mut self, key: String, kind: LoadKind, path: PathBuf) {
        if self.inflight.contains_key(&key) {
            return; // 已排队 / 解码中 / 已就绪 / 已取走：一律不重复。
        }
        self.inflight.insert(key.clone(), LoadState::Pending);
        let job = match kind {
            LoadKind::TexturePng => Job::Texture {
                key: key.clone(),
                path,
            },
            LoadKind::GltfMesh => Job::Gltf {
                key: key.clone(),
                path,
            },
        };
        // 轮询分发给下一个 worker；无 worker 或发送失败（通道断开）→ 同步完成，请求永不丢失。
        let sent = if self.job_senders.is_empty() {
            false
        } else {
            let index = self.next_worker % self.job_senders.len();
            self.next_worker += 1;
            self.job_senders[index].send(job.clone()).is_ok()
        };
        if sent {
            return; // worker 接手，解码完成后经结果通道回传。
        }
        // 无 worker（Sync 模式 / wasm / 线程创建失败）：同步完成，保证请求永不丢失。
        let asset = decode_job(job);
        self.inflight.insert(key, LoadState::Ready);
        self.ready.push_back(asset);
    }

    /// 主线程每帧调用：先收 worker 结果，再取走全部已解码产物。
    ///
    /// 取走即标记 `Done`（失败也一样），调用方负责上传与占位回退。
    pub fn drain_ready(&mut self) -> Vec<DecodedAsset> {
        if let Some(result_rx) = &self.result_rx {
            while let Ok(asset) = result_rx.try_recv() {
                let key = asset.key.clone();
                if self.inflight.get(&key) == Some(&LoadState::Pending) {
                    self.inflight.insert(key, LoadState::Ready);
                }
                self.ready.push_back(asset);
            }
        }
        let mut out = Vec::with_capacity(self.ready.len());
        while let Some(asset) = self.ready.pop_front() {
            if let Some(state) = self.inflight.get_mut(&asset.key) {
                *state = LoadState::Done;
            }
            out.push(asset);
        }
        out
    }

    /// 尚未被取走的请求数（排队 + 解码中 + 已就绪）。测试与诊断用。
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.inflight
            .values()
            .filter(|s| **s != LoadState::Done)
            .count()
    }

    /// 已取走的请求数（含失败）。测试与诊断用。
    #[must_use]
    pub fn completed_count(&self) -> usize {
        self.inflight
            .values()
            .filter(|s| **s == LoadState::Done)
            .count()
    }
}

impl Default for AsyncLoader {
    fn default() -> Self {
        Self::new(LoadMode::Async)
    }
}

/// 在调用方线程里执行一个解码请求（worker 与同步兜底共用）。
fn decode_job(job: Job) -> DecodedAsset {
    match job {
        Job::Texture { key, path } => {
            let kind = LoadKind::TexturePng;
            match TextureAsset::load_png(&path, TextureColorSpace::Srgb) {
                Ok(texture) => DecodedAsset {
                    key,
                    kind,
                    texture: Some(texture),
                    meshes: None,
                    error: None,
                },
                Err(error) => DecodedAsset {
                    key,
                    kind,
                    texture: None,
                    meshes: None,
                    error: Some(error.to_string()),
                },
            }
        }
        Job::Gltf { key, path } => {
            let kind = LoadKind::GltfMesh;
            match MeshAsset::from_gltf_path(&path) {
                Ok(meshes) => DecodedAsset {
                    key,
                    kind,
                    texture: None,
                    meshes: Some(meshes),
                    error: None,
                },
                Err(error) => DecodedAsset {
                    key,
                    kind,
                    texture: None,
                    meshes: None,
                    error: Some(error.to_string()),
                },
            }
        }
    }
}

impl fmt::Debug for AsyncLoader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncLoader")
            .field("mode", &self.mode())
            .field("workers", &self.workers)
            .field("inflight", &self.inflight.len())
            .field("ready", &self.ready.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{AsyncLoader, DecodedAsset, LoadKind, LoadMode};
    use crate::core::texture::{TextureAsset, TextureColorSpace};
    use std::path::PathBuf;

    fn sample_png(name: &str) -> PathBuf {
        // 每测试独立文件：并行测试若共用同一路径，会互相覆盖写坏 PNG。
        let dir = std::env::temp_dir().join(format!(
            "rustlua-async-loader-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.png");
        if !path.exists() {
            // 用依赖里的 image crate 生成合法 PNG（2×2，便于逐像素断言）。
            let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([180, 90, 40, 255]));
            image.save(&path).expect("write sample png");
        }
        path
    }

    fn pixel(asset: &DecodedAsset) -> [u8; 4] {
        let texture = asset.texture.as_ref().expect("decoded texture");
        assert_eq!(texture.width(), 2);
        assert_eq!(texture.height(), 2);
        let bytes = texture.pixels();
        assert_eq!(bytes.len(), 16);
        [bytes[0], bytes[1], bytes[2], bytes[3]]
    }

    /// Sync 与 Async 对同一 PNG 给出逐字节一致的解码结果（确定性契约）。
    #[test]
    fn sync_and_async_decode_the_same_png_identically() {
        let png = sample_png("sync-async");

        let mut sync = AsyncLoader::new(LoadMode::Sync);
        sync.request_texture("hero.png", &png);
        assert_eq!(sync.mode(), LoadMode::Sync);
        assert_eq!(sync.pending_count(), 1);
        let sync_out = sync.drain_ready();
        assert_eq!(sync_out.len(), 1);
        assert!(sync_out[0].error.is_none());
        let sync_pixel = pixel(&sync_out[0]);
        assert_eq!(sync.pending_count(), 0);

        let mut async_loader = AsyncLoader::new(LoadMode::Async);
        async_loader.request_texture("hero.png", &png);
        assert_eq!(async_loader.mode(), LoadMode::Async);
        // 轮询直到 worker 解码完成（有上限，避免测试挂死）。
        let mut async_out = Vec::new();
        for _ in 0..200 {
            async_out = async_loader.drain_ready();
            if !async_out.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(async_out.len(), 1, "worker 应在限时内解码完成");
        assert!(async_out[0].error.is_none());
        assert_eq!(pixel(&async_out[0]), sync_pixel);
    }

    /// 同一 key 重复请求只解码一次（幂等去重）。
    #[test]
    fn duplicate_request_decodes_once() {
        let png = sample_png("duplicate");
        let mut loader = AsyncLoader::new(LoadMode::Sync);
        loader.request_texture("hero.png", &png);
        loader.request_texture("hero.png", &png); // 去重：不重复解码
        loader.request_texture("hero.png", &png);
        let out = loader.drain_ready();
        assert_eq!(out.len(), 1);
        assert_eq!(loader.completed_count(), 1);
    }

    /// 取走后再请求同一 key 也不会重复解码（renderer 的纹理缓存每帧都会命中，但防御边界仍要钉住）。
    #[test]
    fn drained_key_is_not_redecoded() {
        let png = sample_png("drained");
        let mut loader = AsyncLoader::new(LoadMode::Sync);
        loader.request_texture("hero.png", &png);
        assert_eq!(loader.drain_ready().len(), 1);
        loader.request_texture("hero.png", &png);
        assert_eq!(loader.drain_ready().len(), 0);
    }

    /// 缺失文件 → 失败回传 error，且不 panic、不重复排队。
    #[test]
    fn missing_file_reports_error_without_panic() {
        let missing = std::env::temp_dir().join(format!(
            "rustlua-async-loader-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&missing);
        let mut loader = AsyncLoader::new(LoadMode::Sync);
        loader.request_texture("ghost.png", &missing);
        let out = loader.drain_ready();
        assert_eq!(out.len(), 1);
        assert!(out[0].texture.is_none());
        assert!(out[0].meshes.is_none());
        let error = out[0].error.as_deref().expect("失败必须带 error");
        assert!(!error.is_empty());
        assert_eq!(out[0].kind, LoadKind::TexturePng);
    }

    /// 同一 key 先失败后（key 相同）不会自动重试；不同 key 正常各自解码。
    #[test]
    fn failed_key_is_not_retried_but_other_keys_work() {
        let png = sample_png("failed-other");
        let missing = std::env::temp_dir().join(format!(
            "rustlua-async-loader-missing-2-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&missing);
        let mut loader = AsyncLoader::new(LoadMode::Sync);
        loader.request_texture("bad.png", &missing);
        loader.request_texture("good.png", &png);
        let out = loader.drain_ready();
        assert_eq!(out.len(), 2);
        let bad = out.iter().find(|a| a.key == "bad.png").unwrap();
        assert!(bad.error.is_some());
        let good = out.iter().find(|a| a.key == "good.png").unwrap();
        assert!(good.error.is_none());
    }

    /// 空路径与合法路径：空路径会失败（读盘失败），合法路径成功——不 panic。
    #[test]
    fn empty_path_fails_cleanly() {
        let mut loader = AsyncLoader::new(LoadMode::Sync);
        loader.request_texture("empty.png", "");
        let out = loader.drain_ready();
        assert_eq!(out.len(), 1);
        assert!(out[0].error.is_some());
    }

    /// Async 模式下多个请求最终全部到达且顺序无关（逐个 drain 直到清空）。
    #[test]
    fn async_batch_requests_all_arrive() {
        let png = sample_png("batch");
        let mut loader = AsyncLoader::new(LoadMode::Async);
        for index in 0..16 {
            loader.request_texture(format!("t{index}.png"), &png);
        }
        let mut received = std::collections::HashSet::new();
        for _ in 0..500 {
            for asset in loader.drain_ready() {
                received.insert(asset.key);
            }
            if received.len() == 16 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(received.len(), 16, "16 个请求应全部解码完成");
        assert_eq!(loader.pending_count(), 0);
    }

    /// wasm 语义（无线程）由 `with_workers(_, 0)` 模拟：请求同步完成、产物立即可取。
    #[test]
    fn zero_workers_falls_back_to_sync_completion() {
        let png = sample_png("zero-workers");
        let mut loader = AsyncLoader::with_workers(LoadMode::Async, 0);
        assert_eq!(loader.workers(), 0);
        assert_eq!(loader.mode(), LoadMode::Sync);
        loader.request_texture("hero.png", &png);
        let out = loader.drain_ready();
        assert_eq!(out.len(), 1, "无 worker 时请求必须同步完成，绝不丢失");
        assert!(out[0].error.is_none());
    }

    /// 与现有同步入口（TextureAsset::load_png）的结果逐字节一致。
    #[test]
    fn matches_the_existing_sync_entry_point() {
        let png = sample_png("existing-entry");
        let reference =
            TextureAsset::load_png(&png, TextureColorSpace::Srgb).expect("reference decode");
        let mut loader = AsyncLoader::new(LoadMode::Sync);
        loader.request_texture("hero.png", &png);
        let out = loader.drain_ready();
        let decoded = out[0].texture.as_ref().unwrap();
        assert_eq!(decoded, &reference);
    }
}
