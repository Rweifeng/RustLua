//! 资源通道的原生回归：**provider 优先、解码不漏、配置读得到**。
//!
//! 为什么要有这个文件，而不是塞进 `src/` 里的单元测试：`vfs::set_provider` 是**进程级一次安装**的。
//! Cargo 给每个集成测试文件单独一个进程，所以这里的 provider 不会波及其他用例；反过来，单元测试里
//! 装它会污染同进程的其他测试——这正是这条判据在前两轮"只有浏览器侧证据"的原因。
//! 本文件只放**一个** `#[test]`：同一进程里的第二个用例会撞上"provider 已安装"（那是设计，不是失败）。

use std::path::Path;

/// 进程内唯一的临时根。
fn temp_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("rustlua-vfs-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("建临时目录");
    root
}

#[test]
fn the_resource_channel_reads_through_the_provider_and_decodes() {
    let root = temp_root("channel");

    // provider 提供的路径（桌面上文件系统里没有它），内容带打包编码（压缩 ⇒ 字节与明文不同）。
    let in_pack = root.join("assets/data/hero.json");
    let packed = rustlua_infra::pack::encode(br#"{"source":"pack"}"#, true, false);
    assert_ne!(
        packed, br#"{"source":"pack"}"#,
        "前提：压缩后字节必须与明文不同，否则下面的断言没有区分力"
    );
    // `project.toml` 也只在 provider 里（= Web/微信的形态）。
    let toml = root.join("project.toml");
    let toml_bytes = b"name = \"Web\"\nfont = \"assets/fonts/Pack.ttf\"\n".to_vec();
    // provider 不认、但磁盘上真实存在的路径（回落文件系统）。
    let on_disk = root.join("notes.txt");
    std::fs::write(&on_disk, b"from disk").unwrap();

    let table = [(in_pack.clone(), packed), (toml.clone(), toml_bytes)];
    rustlua_infra::vfs::set_provider(Box::new(move |path: &Path| {
        table
            .iter()
            .find(|(candidate, _)| candidate == path)
            .map(|(_, bytes)| bytes.clone())
    }))
    .expect("本进程首个 provider");

    // 1) 读资源 = provider 优先 + 自动解码（provider 返回的是**未解码**字节）。
    let decoded = rustlua_infra::vfs::read_resource(&in_pack).expect("provider 路径可读");
    assert_eq!(
        String::from_utf8(decoded).unwrap(),
        r#"{"source":"pack"}"#,
        "provider 的字节必须被自动解码"
    );

    // 2) `read` 必须保持"未解码"语义（与 `std::fs::read` 同口径）；
    //    否则 `read_resource` 的解码会变成双重解码，或解码被偷偷做两遍。
    let raw = rustlua_infra::vfs::read(&in_pack).expect("provider 路径可读");
    assert_ne!(
        String::from_utf8_lossy(&raw),
        r#"{"source":"pack"}"#,
        "`read` 必须返回打包原文，解码只发生在 `read_resource`"
    );

    // 3) provider 不认的路径回落文件系统，同样解码（明文原样返回 ⇒ 安全）。
    let from_disk = rustlua_infra::vfs::read_resource(&on_disk).expect("磁盘路径可读");
    assert_eq!(String::from_utf8(from_disk).unwrap(), "from disk");

    // 4) 两边都没有的路径：报错，不静默给空。
    assert!(rustlua_infra::vfs::read_resource(&root.join("nope.txt")).is_err());

    // 5) 工程配置走同一条通道：`project.toml` 只在 provider 里也能读出键。
    assert_eq!(
        rustlua_infra::project_config::read_key(&root, "font").as_deref(),
        Some("assets/fonts/Pack.ttf")
    );
    assert_eq!(
        rustlua_infra::project_config::read_key(&root, "name").as_deref(),
        Some("Web")
    );

    // 6) 存在性问 provider（工程根在磁盘上不存在也不影响）。
    assert!(rustlua_infra::vfs::is_file(&toml));
    assert!(!rustlua_infra::vfs::is_file(&root.join("nope.txt")));

    let _ = std::fs::remove_dir_all(&root);
}
