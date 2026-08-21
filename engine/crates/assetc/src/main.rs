//! assetc 命令行入口。

use std::path::PathBuf;

fn usage() {
    eprintln!(
        "assetc — P2 资源构建工具\n\
         \n\
         用法: assetc <命令> [参数]\n\
         \n\
         命令:\n\
         \x20 assetc scan <项目目录>           扫描并注册资源到 ResourceDatabase\n\
         \x20 assetc import <项目目录>         增量导入资源到 ImportCache\n\
         \x20 assetc clean <项目目录>          清空导入缓存\n\
         \x20 assetc list <项目目录>           列出已注册资源\n\
         \x20 assetc package <项目目录> -o <输出目录>   打包发布资源目录并生成清单\n\
         \x20 assetc --check                   冒烟检查（临时项目闭环验证）\n\
         \x20 assetc --help                    显示本帮助\n\
         \n\
         退出码: 0 成功；1 失败"
    );
}

fn arg(args: &[String], index: usize, flag: &str) -> Result<PathBuf, String> {
    args.get(index)
        .ok_or_else(|| format!("缺少参数: {flag}"))
        .map(PathBuf::from)
}

fn run(args: &[String]) -> Result<String, String> {
    match args.get(1).map(String::as_str) {
        Some("--check") => assetc::check(),
        Some("scan") => {
            let project = arg(args, 2, "项目目录")?;
            assetc::scan(&project).map(|entries| summarize_scan(&entries))
        }
        Some("import") => {
            let project = arg(args, 2, "项目目录")?;
            assetc::import(&project).map(|outcomes| summarize_import(&outcomes))
        }
        Some("clean") => {
            let project = arg(args, 2, "项目目录")?;
            assetc::clean(&project).map(|_| "assetc_clean_ok".to_owned())
        }
        Some("list") => {
            let project = arg(args, 2, "项目目录")?;
            assetc::list(&project).map(|entries| {
                let mut out = String::from("assetc_list");
                for entry in entries {
                    out.push_str(&format!(
                        "\n{}\t{}\t{}\t{}",
                        entry.path.display(),
                        entry.kind,
                        entry.status,
                        entry.asset_id.as_deref().unwrap_or("-")
                    ));
                }
                out
            })
        }
        Some("package") => {
            let project = arg(args, 2, "项目目录")?;
            let out = {
                let mut out_arg = None;
                let mut i = 3;
                while i < args.len() {
                    if args[i] == "-o" {
                        out_arg = Some(PathBuf::from(
                            args.get(i + 1)
                                .ok_or_else(|| "缺少参数: -o <输出目录>".to_owned())?,
                        ));
                        break;
                    }
                    i += 1;
                }
                out_arg.ok_or_else(|| "缺少参数: -o <输出目录>".to_owned())?
            };
            assetc::package(&project, &out)
                .map(|manifest| format!("assetc_package_ok files={}", manifest.entries.len()))
        }
        Some("--help") | None => {
            usage();
            Ok(String::new())
        }
        Some(other) => Err(format!("未知命令: {other}")),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match run(&args) {
        Ok(summary) => {
            if !summary.is_empty() {
                println!("{summary}");
            }
        }
        Err(error) => {
            eprintln!("assetc error: {error}\n");
            usage();
            std::process::exit(1);
        }
    }
}

fn summarize_scan(entries: &[assetc::ScanEntry]) -> String {
    let total = entries.len();
    let supported = entries.iter().filter(|e| e.status != "unsupported").count();
    let unsupported = total - supported;
    let failed = entries
        .iter()
        .filter(|e| e.error.is_some() && e.status != "unsupported")
        .count();
    let mut out = format!(
        "assetc_scan_ok total={total} supported={supported} unsupported={unsupported} failed={failed}"
    );
    for entry in entries.iter().take(20) {
        out.push_str(&format!(
            "\n{}\t{}\t{}\t{}",
            entry.path.display(),
            entry.kind,
            entry.status,
            entry.asset_id.as_deref().unwrap_or("-")
        ));
    }
    out
}

fn summarize_import(outcomes: &[assetc::ImportOutcome]) -> String {
    let total = outcomes.len();
    let imported = outcomes.iter().filter(|o| !o.reused).count();
    let reused = outcomes.iter().filter(|o| o.reused).count();
    let failed = outcomes.iter().filter(|o| o.error.is_some()).count();
    let mut out = format!(
        "assetc_import_ok total={total} imported={imported} reused={reused} failed={failed}"
    );
    for outcome in outcomes {
        out.push_str(&format!(
            "\n{}\t{}\t{}",
            outcome.path.display(),
            if outcome.reused { "reused" } else { "imported" },
            outcome.error.as_deref().unwrap_or("-")
        ));
    }
    out
}
