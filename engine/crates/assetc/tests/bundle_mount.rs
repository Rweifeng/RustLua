//! 批 B 的挂载门（`docs/67` §7.4 / §10.4 / §10.5）。
//!
//! 为什么这个文件在 `assetc/tests/` 而不是 infra 的单测：
//! - 它要**同一份工程**的两种产物（目录形态 + 单包形态）才能证明"容器不改变内容"，
//!   而写端只在 `assetc`（`bundle-write` feature）编译；
//! - `vfs::set_provider` / `vfs::set_lister` 是**进程级一次安装**，所以整个文件只装一次
//!   （一个集成测试二进制 = 一个进程），四道门在同一个 `#[test]` 里顺序跑完。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rustlua_infra::bundle::{BundleSet, MissReason};

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos()
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rustlua-bundle-mount-{label}-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 1x1 红色 PNG（与 assetc 单测里那份一致）。
const RED_PNG: [u8; 70] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// 建一个"文本 + 二进制 + 子目录 + require 依赖"的小工程。
fn fixture(root: &Path) {
    fs::create_dir_all(root.join("scripts/data")).unwrap();
    fs::create_dir_all(root.join("scenes")).unwrap();
    fs::create_dir_all(root.join("assets/textures")).unwrap();
    fs::write(
        root.join("project.toml"),
        "[project]\nname = \"mount\"\nversion = \"0.1.0\"\nengine = \"RustLua\"\n\
         main_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
    )
    .unwrap();
    fs::write(
        root.join("scripts/main.lua"),
        "local data = require('data.items')\nfunction update() return data.title end\n",
    )
    .unwrap();
    fs::write(
        root.join("scripts/data/items.lua"),
        "return { title = '挂载门' }\n",
    )
    .unwrap();
    fs::write(
        root.join("scenes/main.scene"),
        "scene_version=2\nname=main\ncomponent\t1\tSprite\ttexture=assets/textures/red.png;size=4x4\n",
    )
    .unwrap();
    fs::write(root.join("assets/textures/red.png"), RED_PNG).unwrap();
}

fn relative_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    out.sort();
    out
}

#[test]
fn bundle_mount_roundtrip_and_loud_misses() {
    let project = temp_dir("project");
    fixture(&project);
    let loose = temp_dir("loose");
    let packed = temp_dir("packed");

    // 两种形态：目录形态（对照）与单包形态（被测）。
    assetc::package(&project, &loose, false, false, false).expect("目录形态打包");
    let report =
        assetc::package_bundled(&project, &packed, false, false, false, &[]).expect("单包打包");
    assert!(report.entries >= 4, "条目数：{report:?}");

    // ---------------- 门 1：挂载往返（同一工程，两形态逐条目字节相等）----------------
    let bundle_path = packed.join(&report.bundle_path);
    assert!(
        bundle_path.is_file(),
        "容器必须存在：{}",
        bundle_path.display()
    );

    let mut set = BundleSet::new(&packed);
    let mounted = set
        .mount_file(&bundle_path)
        .unwrap_or_else(|error| panic!("挂载 {}: {error}", bundle_path.display()));
    assert_eq!(mounted as u64, report.entries);

    let mut compared = 0;
    for relative in relative_files(&loose) {
        if relative == "manifest.txt" {
            continue; // 两形态的清单本来就不同
        }
        let expected = fs::read(loose.join(&relative)).unwrap();
        let actual = set
            .read(&relative)
            .expect("读容器")
            .unwrap_or_else(|| panic!("容器缺少 {relative}"));
        assert_eq!(actual, expected, "条目 {relative} 字节不一致");
        compared += 1;
    }
    assert_eq!(compared as u64, report.entries, "条目数必须守恒");
    // 反查：容器里的每个条目在目录形态里都存在
    for path in set.paths() {
        assert!(
            loose.join(path).is_file(),
            "容器里多出目录形态没有的条目 {path}"
        );
    }

    // 逐条目全量校验（`--check` 的那条路径）
    set.verify_all().expect("全量校验");

    // ---------------- 门 2：缺件三态必须能分开（§7.4）----------------
    // ① 在已挂载的包里 → Found
    assert_eq!(set.explain_miss("scripts/main.lua"), MissReason::Found);
    // ② 谁都没有 → NotInAnyBundle（与"文件不存在"同义）
    assert_eq!(
        set.explain_miss("assets/textures/nope.png"),
        MissReason::NotInAnyBundle
    );
    // ③ 已知但未挂载 → InUnmountedBundle(包名)，这是 bundle 化引入的新失效模式
    set.declare_pending("art-ui.rlb", ["assets/textures/later.png".to_owned()]);
    assert_eq!(
        set.explain_miss("assets/textures/later.png"),
        MissReason::InUnmountedBundle {
            bundle: "art-ui.rlb".to_owned()
        }
    );
    let described = set.explain_miss("assets/textures/later.png").describe();
    assert!(
        described.contains("art-ui.rlb") && described.contains("未挂载"),
        "诊断必须给出包名与原因：{described}"
    );
    // 三种归因互不相同（这条是"响亮"的可证部分）
    assert_ne!(
        set.explain_miss("scripts/main.lua"),
        set.explain_miss("assets/textures/later.png")
    );

    // ---------------- 门 3：vfs 接线（provider + lister），只装一次 ----------------
    let set = Arc::new(Mutex::new(set));
    rustlua_infra::bundle::install(Arc::clone(&set)).expect("安装 bundle provider");

    // 3a provider 命中：vfs 读到的字节就是条目字节（`read` 返回原文，decode 对明文透传）
    let via_vfs = rustlua_infra::vfs::read(&packed.join("scripts/main.lua")).expect("vfs 读脚本");
    assert_eq!(
        via_vfs,
        fs::read(loose.join("scripts/main.lua")).unwrap(),
        "vfs 通道必须与目录形态字节相同"
    );
    assert!(rustlua_infra::vfs::is_file(
        &packed.join("assets/textures/red.png")
    ));

    // 3b lister：`scripts/` 在磁盘上不存在（只在容器里），但递归枚举必须能列出来
    assert!(
        !packed.join("scripts").is_dir(),
        "单包形态下 scripts/ 不该落在磁盘上"
    );
    let listed = rustlua_infra::vfs::list_files_recursive(&packed.join("scripts"));
    let listed_rel: Vec<String> = listed
        .iter()
        .map(|path| {
            path.strip_prefix(&packed)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(
        listed_rel,
        vec!["scripts/data/items.lua", "scripts/main.lua"],
        "枚举必须来自容器条目"
    );

    // 3c 未挂载条目经 vfs 读 → 拿不到字节（provider 返回 None，回落文件系统也失败）
    assert!(
        rustlua_infra::vfs::read(&packed.join("assets/textures/later.png")).is_err(),
        "未挂载条目不得静默返回字节"
    );
    // 但归因仍然是对的（模型里能问出"在哪个包里"）
    let guard = set.lock().unwrap();
    assert_eq!(
        guard.explain_miss("assets/textures/later.png"),
        MissReason::InUnmountedBundle {
            bundle: "art-ui.rlb".to_owned()
        }
    );

    // ---------------- 门 4：数据损坏必须报错（不许读出错字节）----------------
    drop(guard);
    let mut corrupted = fs::read(&bundle_path).unwrap();
    // 改块区第一个字节：store 块的块内数据被改 → 条目 FNV 失配
    let payload_start = 32;
    corrupted[payload_start] ^= 0xff;
    let broken = packed.join("corrupt.rlb");
    fs::write(&broken, &corrupted).unwrap();
    let mut corrupted_set = BundleSet::new(&packed);
    corrupted_set.mount_file(&broken).unwrap();
    let error = corrupted_set
        .read("scripts/main.lua")
        .expect_err("改过字节的容器必须报错");
    // 压缩块先炸在 zstd 帧上，store 块先炸在条目校验和上——两种都必须是**错误**而不是错字节。
    assert!(
        matches!(
            error,
            rustlua_infra::bundle::BundleError::Checksum { .. }
                | rustlua_infra::bundle::BundleError::BlockDecode { .. }
        ),
        "得到 {error:?}"
    );

    for dir in [&project, &loose, &packed] {
        let _ = fs::remove_dir_all(dir);
    }
}

/// 批 C 门：分组形态——计划覆盖每一个条目、每个容器都能挂上、字节与目录形态相等。
#[test]
fn grouped_pack_is_covered_by_plan_and_mounts() {
    use rustlua_infra::bundle::plan::BundlePlan;
    use rustlua_infra::bundle::{BundleIndex, BundleReader, MemSource};

    let project = temp_dir("grouped");
    fixture(&project);
    let loose = temp_dir("grouped-loose");
    let packed = temp_dir("grouped-packed");

    assetc::package(&project, &loose, false, false, false).expect("目录形态打包");
    let report = assetc::package_grouped(&project, &packed, false, false).expect("分组打包");

    // ① 计划表存在、格式可解析、组数与报告一致
    let plan_bytes = fs::read(packed.join("bundle_plan.json")).expect("计划表必须存在");
    let plan = BundlePlan::parse(&plan_bytes).expect("计划表必须可解析");
    assert_eq!(
        plan.groups.len(),
        report.groups.len(),
        "计划与报告的组数必须一致"
    );

    // ② 计划覆盖每一个条目，且每条路径只出现一次
    let mut planned: Vec<String> = plan
        .groups
        .iter()
        .flat_map(|group| group.paths.clone())
        .collect();
    let planned_count = planned.len();
    planned.sort();
    planned.dedup();
    assert_eq!(planned_count, planned.len(), "计划里出现重复条目");
    assert_eq!(planned.len() as u64, report.entries, "计划必须覆盖全部条目");

    // ③ 每个容器都能挂上，并且组内条目字节与目录形态相等
    let mut compared = 0u64;
    for group in &plan.groups {
        let path = packed.join(&group.bundle);
        assert!(
            path.is_file(),
            "计划声明的容器不存在：{}（unit={}）",
            group.bundle,
            group.unit
        );
        let bytes = fs::read(&path).unwrap();
        assert_eq!(
            bytes.len() as u64,
            group.bundle_bytes,
            "计划里的落盘字节与容器实际大小不一致：{}",
            group.bundle
        );
        let mut reader =
            BundleReader::new(BundleIndex::parse(&bytes).unwrap(), MemSource::new(bytes));
        assert_eq!(
            reader.index().len() as u64,
            group.entries,
            "组 {} 的条目数不一致",
            group.name
        );
        for relative in &group.paths {
            let expected = fs::read(loose.join(relative)).unwrap();
            let actual = reader
                .read(relative)
                .unwrap()
                .unwrap_or_else(|| panic!("容器 {} 缺少 {relative}", group.bundle));
            assert_eq!(actual, expected, "条目 {relative} 字节不一致");
            compared += 1;
        }
        reader.verify_all().unwrap();
    }
    assert_eq!(compared, report.entries, "条目数必须守恒");

    // ④ 启动必需的组都在 main 单元（否则 Web 上 require 同步会炸）
    let main_paths: Vec<&str> = plan
        .groups
        .iter()
        .filter(|group| group.unit == "main")
        .flat_map(|group| group.paths.iter().map(String::as_str))
        .collect();
    for required in ["project.toml", "scripts/main.lua", "scenes/main.scene"] {
        assert!(
            main_paths.contains(&required),
            "{required} 必须在 main 单元里：{main_paths:?}"
        );
    }

    // ⑤ 骨架形态：数据目录不落盘
    for gone in ["scenes", "scripts", "assets"] {
        assert!(!packed.join(gone).exists(), "{gone}/ 不该留在输出根");
    }

    for dir in [&project, &loose, &packed] {
        let _ = fs::remove_dir_all(dir);
    }
}

/// 批 E 收尾门：`assetc verify` 必须能证明"容器内容 == 目录形态内容"，
/// 并且**改一个字节就必须红**（否则它只是一个永远绿的装饰）。
#[test]
fn verify_bundle_equivalence_detects_mismatch() {
    let project = temp_dir("verify-equiv");
    fixture(&project);
    let loose = temp_dir("verify-loose");
    let packed = temp_dir("verify-packed");

    assetc::package(&project, &loose, false, false, false).expect("目录形态打包");
    assetc::package_grouped(&project, &packed, false, false).expect("分组打包");

    let compared =
        assetc::verify_bundle_equivalence(&loose, &packed).expect("两种形态必须逐字节相等");
    assert!(compared >= 4, "至少要比对夹具里的几个条目：{compared}");

    // 改坏目录形态里的一个字节：verify 必须报错并指出条目
    let victim = loose.join("assets/textures/red.png");
    let mut bytes = fs::read(&victim).unwrap();
    bytes[0] ^= 0xff;
    fs::write(&victim, &bytes).unwrap();
    let error = assetc::verify_bundle_equivalence(&loose, &packed).expect_err("字节不同必须报错");
    assert!(
        error.to_string().contains("red.png"),
        "错误必须点名到条目：{error}"
    );

    for dir in [&project, &loose, &packed] {
        let _ = fs::remove_dir_all(dir);
    }
}
