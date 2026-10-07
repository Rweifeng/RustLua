// 引擎产物共用的 Windows **文件图标**：把 engine/assets/engine_icon.ico 编成 .res 交给链接器。
//
// 谁在用：editor / game / assetc 三个 bin crate 的 `build.rs` 各用一行 `include!` 引走本文件
// ——资源编译器的发现逻辑（RC 环境变量 → Windows SDK → vendored zig）只要一份，三处各自抄
// 一遍必然腐烂成三种行为。
//
// 图源：`py engine/tools/gen_engine_icon.py`（同一张规格表同时出窗口图标用的 PNG 和这里的 ICO）。
//
// 失败口径是刻意的两档：
//   * **找不到编译器** → `cargo:warning` 后跳过。这台机器/CI 之外的人不该因为没装 SDK 就编不过，
//     而且缺的只是 Explorer 里那个文件图标，窗口图标（走 PNG 的 `with_icon`）不受影响。
//   * **找到了但编译失败** → panic。那说明 .rc 或入库的 .ico 坏了，是我们自己的 bug，
//     必须响，不能悄悄产出一个没图标的 exe。

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../tools/winicon_build.rs");
    println!("cargo:rerun-if-changed=../../assets/engine_icon.ico");
    println!("cargo:rerun-if-env-changed=RC");

    let target = std::env::var("TARGET").unwrap_or_default();
    // 只有 MSVC 的 link.exe 吃裸 .res；windows-gnu 要的是 windres 产物，本仓库没有该目标。
    if !target.ends_with("-windows-msvc") {
        return;
    }
    embed_windows_icon();
}

fn embed_windows_icon() {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let ico = std::path::Path::new(&manifest_dir).join("../../assets/engine_icon.ico");
    let ico = ico.canonicalize().unwrap_or_else(|error| {
        panic!(
            "找不到 {}：{error}；先跑 py engine/tools/gen_engine_icon.py 出图",
            ico.display()
        )
    });

    let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR");
    let rc_path = std::path::Path::new(&out_dir).join("engine_icon.rc");
    let res_path = std::path::Path::new(&out_dir).join("engine_icon.res");
    // .rc 的字符串里反斜杠要转义，而 Win32 API 接受正斜杠，所以统一换掉。
    let ico_text = ico.display().to_string().replace('\\', "/");
    std::fs::write(&rc_path, format!("1 ICON \"{ico_text}\"\n")).expect("写 engine_icon.rc 失败");

    let Some(compiler) = find_resource_compiler() else {
        println!(
            "cargo:warning=没找到 rc.exe 也没找到 engine/.tools/zig 里的 zig rc：\
             跳过 exe 文件图标（窗口图标不受影响）。装 Windows SDK 10，或按 \
             engine/.tools 的缓存口径放一份 zig 即可。"
        );
        return;
    };

    let mut command = std::process::Command::new(&compiler.program);
    command
        .args(compiler.global_args)
        .args(["/nologo", "/fo"])
        .arg(&res_path)
        .arg(&rc_path);
    let output = command.output().unwrap_or_else(|error| {
        panic!("启动资源编译器 {} 失败：{error}", compiler.program.display())
    });
    if !output.status.success() {
        panic!(
            "把 engine_icon.ico 编成 .res 失败（status={}）：\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // link.exe 把裸 .res 当输入文件收，RT_GROUP_ICON 就这么进 PE 了。
    println!("cargo:rustc-link-arg={}", res_path.display());
}

struct ResourceCompiler {
    program: std::path::PathBuf,
    /// `zig rc` 的 `rc` 是子命令，不是参数开关，所以调用形态要一起带回来。
    global_args: &'static [&'static str],
}

fn find_resource_compiler() -> Option<ResourceCompiler> {
    if let Some(rc) = std::env::var_os("RC") {
        let program = std::path::PathBuf::from(&rc);
        if program.is_file() {
            return Some(ResourceCompiler { program, global_args: &[] });
        }
    }
    if let Some(rc) = windows_kits_rc() {
        return Some(ResourceCompiler { program: rc, global_args: &[] });
    }
    vendored_zig_rc().map(|zig| ResourceCompiler { program: zig, global_args: &["rc"] })
}

/// Windows SDK 的 `rc.exe`：`Program Files (x86)\Windows Kits\10\bin\<版本>\x64\rc.exe`。
/// 版本目录按数字比大小，字典序会把 `10.0.9xxx` 排到 `10.0.26100` 后面。
fn windows_kits_rc() -> Option<std::path::PathBuf> {
    let kits_bin = ["ProgramFiles(x86)", "ProgramFiles", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|root| std::path::PathBuf::from(root).join("Windows Kits/10/bin"))
        .find(|dir| dir.is_dir())?;
    let newest = std::fs::read_dir(&kits_bin)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|version| kits_bin.join(version).join("x64/rc.exe").is_file())
        .max_by_key(|version| {
            version
                .split('.')
                .map(|part| part.parse::<u32>().unwrap_or_default())
                .collect::<Vec<_>>()
        })?;
    Some(kits_bin.join(newest).join("x64/rc.exe"))
}

/// 仓库自带的 zig 缓存（`engine/.tools` 被 gitignore，装了才有）：`zig rc` 是 rc.exe 的替代实现。
fn vendored_zig_rc() -> Option<std::path::PathBuf> {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let zig_root = std::path::Path::new(&manifest_dir).join("../../.tools/zig");
    let extract = |entry: &std::path::Path| {
        let candidate = entry.join("zig.exe");
        candidate.is_file().then_some(candidate)
    };
    if let Some(direct) = extract(&zig_root) {
        return Some(direct);
    }
    std::fs::read_dir(&zig_root)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter_map(|path| extract(&path))
        .next()
}
