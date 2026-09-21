//! 临时工具：对工程里的 Lua 文件做**语法编译检查**（`load` 只编译不执行）。
//!
//! 用法：`cargo run -q -p engine --example lua_check -- <project-root> <file.lua>...`
//! 失败（有文件编译不过）退出码 2。演示工程大量新增脚本时，它比跑整套引擎测试快得多；
//! 真正的运行期验证仍由 `game_api` 的 headless 场景门负责。

use engine::core::script::LuaRuntime;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = args
        .next()
        .expect("usage: lua_check <project-root> <file.lua>...");
    let files: Vec<String> = args.collect();
    assert!(!files.is_empty(), "no input files");

    let mut runtime = LuaRuntime::new().expect("lua vm");
    runtime.set_project_root(std::path::PathBuf::from(&root));

    let mut failed = 0;
    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(source) => source,
            Err(error) => {
                println!("READ-FAIL {file}: {error}");
                failed += 1;
                continue;
            }
        };
        if source.contains("]==========]") {
            println!("SKIP {file}: source contains the long-bracket delimiter");
            continue;
        }
        let wrapped = format!(
            "local chunk, err = load([==========[\n{source}\n]==========], \"@{name}\")\n\
             if not chunk then error(err, 0) end\n\
             return \"ok\"",
            name = file.replace('\\', "/")
        );
        match runtime.execute_console(&wrapped) {
            Ok(_) => println!("ok   {file}"),
            Err(error) => {
                println!("FAIL {file}: {error}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        println!("{failed} file(s) failed to compile");
        std::process::exit(2);
    }
    println!("all {count} file(s) compile", count = files.len());
}
