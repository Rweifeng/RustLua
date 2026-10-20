//! assetc 命令行入口。

use anyhow::anyhow;
use std::path::PathBuf;

fn usage() {
    eprintln!(
        "assetc — RustLua 资源构建工具\n\
         \n\
         用法: assetc <命令> [参数]\n\
         \n\
         命令:\n\
         \x20 assetc scan <项目目录>           扫描并注册资源到 ResourceDatabase\n\
         \x20 assetc import <项目目录>         增量导入资源到 ImportCache\n\
         \x20 assetc verify <目录形态包> <容器形态包>  逐条目比对两种打包形态（docs/67 P0-2）\n\
         \x20 assetc closure <发布根> [--plan <bundle_plan.json>] [--cdn <远端目录>] [--json]\n\
         \x20                              按计划表核对最终产物：容器存在、索引可解析、\n\
         \x20                              每条逻辑路径都在容器里（WebGL/PC/微信同一门）；\n\
         \x20                              --cdn 给远端容器目录（-AssetPack remote 的 <发布根>/assets/pack、\n\
         \x20                              或 -CdnPackDir 镜像）：本地找不到的容器在那里核，\n\
         \x20                              keep 条目也允许只存在于该目录（随资源包上 CDN）；\n\
         \x20                              keep 条目必须在目录里且不在任何容器中；\n\
         \x20                              --allow-cdn-skip 显式声明这次不查远端（日志仍打印跳过数）\n\
\x20 assetc inspect <包目录|.rlb>       打印容器索引摘要与启动闭包落在第几块（docs/67）\n\
         \x20 assetc list <项目目录>           列出已注册资源\n\
         \x20 assetc benchmark <项目目录>      输出各类资产压缩/解压基准\n\
         \x20 assetc package <项目目录> -o <输出目录>   打包发布资源并生成清单\n\
         \x20        [--bundle none|single|grouped]  none=目录形态（默认），single=单容器，grouped=按 bundles.json 分组（docs/67）\n\
         \x20        [--compress] [--encrypt] 仅目录形态有意义（单包由容器逐块压缩）\n\
         \x20        [--fast]                 单包用 zstd L3（开发构建）\n\
\x20        [--keep <前缀>]          容器形态下把匹配的条目留在目录里（微信字体：wx.loadFont 只吃真实路径）\n\
\x20        [--exclude-cdn]          grouped 本地包不携带 unit=cdn 的组（CDN 资源由发布器另行分发）\n\
         \x20        [--drop-undeclared-fonts] 剔除未声明的字体二进制\n\
         \x20 assetc --check-deps <项目目录>   检查依赖链路（列出 Invalid 断链）\n\
         \x20 assetc --check                   冒烟检查（临时项目闭环验证）\n\
         \x20 assetc --help                    显示本帮助\n\
         \n\
         退出码: 0 成功；1 失败"
    );
}

fn arg(args: &[String], index: usize, flag: &str) -> anyhow::Result<PathBuf> {
    args.get(index)
        .ok_or_else(|| anyhow!("缺少参数: {flag}"))
        .map(PathBuf::from)
}

fn run(args: &[String]) -> anyhow::Result<String> {
    match args.get(1).map(String::as_str) {
        Some("--check") => assetc::check(),
        Some("--check-deps") => {
            let project = arg(args, 2, "项目目录")?;
            assetc::check_dependencies(&project).map(|links| summarize_links(&links))
        }
        Some("scan") => {
            let project = arg(args, 2, "项目目录")?;
            assetc::scan(&project).map(|entries| summarize_scan(&entries))
        }
        Some("import") => {
            let project = arg(args, 2, "项目目录")?;
            assetc::import(&project).map(|outcomes| summarize_import(&outcomes))
        }
        Some("verify") => {
            let loose = arg(args, 2, "目录形态包")?;
            let container = arg(args, 3, "容器形态包")?;
            assetc::verify_bundle_equivalence(&loose, &container)
                .map(|entries| format!("assetc_verify_ok entries={entries}"))
        }
        Some("inspect") => {
            let target = arg(args, 2, "包目录或 .rlb 文件")?;
            assetc::inspect_bundles(&target)
        }
        Some("closure") => {
            let base = arg(args, 2, "发布根")?;
            let plan = args
                .iter()
                .position(|value| value == "--plan")
                .and_then(|index| args.get(index + 1))
                .map(PathBuf::from);
            // `--cdn <目录>`：装容器的**远端目录**。两种用途：
            // ① 瘦首包（`-AssetPack remote`）把 `pack/` 裁成骨架，所有组的容器都在
            //    `<发布根>/assets/pack/…`——本地找不到的组在这里核；
            // ② `-CdnPackDir` 镜像 / `cdn` 组：给定时所有 `cdn` 组都会在这里再核一遍。
            // 都没给而计划里又有 `cdn` 组、或本地缺件时，那些组会被记成"未检查"问题（不是静默跳过）。
            let cdn = args
                .iter()
                .position(|value| value == "--cdn")
                .and_then(|index| args.get(index + 1))
                .map(PathBuf::from);
            let options = assetc::closure::ClosureOptions {
                cdn: cdn.as_deref(),
                allow_cdn_skip: args.iter().any(|value| value == "--allow-cdn-skip"),
            };
            let report =
                assetc::closure::check_release_closure_with(&base, plan.as_deref(), options)?;
            if args.iter().any(|value| value == "--json") {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_owned())
                );
                if report.problems.is_empty() {
                    return Ok(String::new());
                }
                return Err(anyhow!("assetc_closure_failed {}", report.summary()));
            }
            if report.problems.is_empty() {
                Ok(format!("assetc_closure_ok {}", report.summary()))
            } else {
                Err(anyhow!(
                    "assetc_closure_failed {}：\n  - {}",
                    report.summary(),
                    report.problems.join("\n  - ")
                ))
            }
        }
        Some("layout") => {
            let target = arg(args, 2, "包目录或 bundle_plan.json")?;
            if args.iter().any(|value| value == "--json") {
                assetc::distribution_layout_json(&target)
            } else {
                assetc::distribution_layout_report(&target)
            }
        }
        Some("bundles") if args.get(2).map(String::as_str) == Some("init") => {
            let project = arg(args, 3, "项目目录")?;
            let force = args.iter().any(|value| value == "--force");
            assetc::init_bundles(&project, force)
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
        Some("benchmark") => {
            let project = arg(args, 2, "项目目录")?;
            assetc::benchmark_compression(&project).map(|entries| summarize_benchmark(&entries))
        }
        Some("package") => {
            let project = arg(args, 2, "项目目录")?;
            let mut out = None;
            let mut compress = false;
            let mut encrypt = false;
            let mut drop_undeclared_fonts = false;
            let mut mode = assetc::BundleMode::Loose;
            let mut fast = false;
            let mut keep: Vec<String> = Vec::new();
            let mut exclude_cdn = false;
            let mut block_size_kb: u32 = 0;
            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "-o" => {
                        out = Some(PathBuf::from(
                            args.get(i + 1)
                                .ok_or_else(|| anyhow!("缺少参数: -o <输出目录>"))?,
                        ));
                        i += 1;
                    }
                    "--compress" => compress = true,
                    "--encrypt" => encrypt = true,
                    "--drop-undeclared-fonts" => drop_undeclared_fonts = true,
                    "--fast" => fast = true,
                    "--exclude-cdn" => exclude_cdn = true,
                    "--block-size" => {
                        // 块大小实验口（docs/67 §9）：默认 256 KB，量压缩率与启动解压的权衡。
                        let value = args
                            .get(i + 1)
                            .ok_or_else(|| anyhow!("缺少参数: --block-size <KB>"))?;
                        block_size_kb = value.parse::<u32>().map_err(|_| {
                            anyhow!("--block-size 只认整数（单位 KB），收到 {value}")
                        })?;
                        i += 1;
                    }
                    "--keep" => {
                        // 容器形态下故意留在目录里的前缀（微信 `wx.loadFont` 只吃真实文件路径）。
                        let value = args
                            .get(i + 1)
                            .ok_or_else(|| anyhow!("缺少参数: --keep <路径前缀>"))?;
                        keep.push(value.clone());
                        i += 1;
                    }
                    "--bundle" => {
                        let value = args
                            .get(i + 1)
                            .ok_or_else(|| anyhow!("缺少参数: --bundle <none|single|grouped>"))?;
                        mode = assetc::BundleMode::parse(value).ok_or_else(|| {
                            anyhow!("--bundle 只认 none|single|grouped，收到 {value}")
                        })?;
                        i += 1;
                    }
                    other => return Err(anyhow!("package 未知参数: {other}")),
                }
                i += 1;
            }
            let out = out.ok_or_else(|| anyhow!("缺少参数: -o <输出目录>"))?;
            match mode {
                assetc::BundleMode::Loose => {
                    assetc::package(&project, &out, compress, encrypt, drop_undeclared_fonts).map(
                        |manifest| {
                            format!(
                                "assetc_package_ok mode=none files={} compress_assets={compress} encrypt_code={encrypt} drop_undeclared_fonts={drop_undeclared_fonts}",
                                manifest.entries.len()
                            )
                        },
                    )
                }
                assetc::BundleMode::Single => {
                    if compress {
                        eprintln!("提示: --bundle single 下 --compress 无效果（压缩由容器块负责）");
                    }
                    assetc::package_bundled_with(&project, &out, drop_undeclared_fonts, fast, encrypt, &keep, block_size_kb * 1024).map(
                        |report| {
                            format!(
                                "assetc_package_ok mode=single files={} entries={} blocks={} raw_kb={} bundle_kb={} index_kb={} ratio={:.3} bundle={}",
                                report.files,
                                report.entries,
                                report.blocks,
                                report.raw_bytes / 1024,
                                report.bundle_bytes / 1024,
                                report.index_bytes / 1024,
                                report.ratio(),
                                report.bundle_path
                            )
                        },
                    )
                }
                assetc::BundleMode::Grouped => {
                    if compress {
                        eprintln!("提示: --bundle grouped 下 --compress 无效果（压缩由容器块负责）");
                    }
                    assetc::package_grouped_keep_with(&project, &out, drop_undeclared_fonts, fast, encrypt, &keep, block_size_kb * 1024, exclude_cdn).map(
                        |report| {
                            let groups: Vec<String> = report
                                .groups
                                .iter()
                                .map(|group| {
                                    format!(
                                        "{}:{}={}KB",
                                        group.name,
                                        group.unit,
                                        group.bundle_bytes / 1024
                                    )
                                })
                                .collect();
                            format!(
                                "assetc_package_ok mode=grouped files={} entries={} groups={} blocks={} raw_kb={} bundle_kb={} ratio={:.3} plan=[{}]",
                                report.files,
                                report.entries,
                                report.groups.len(),
                                report.blocks,
                                report.raw_bytes / 1024,
                                report.bundle_bytes / 1024,
                                report.ratio(),
                                groups.join(", ")
                            )
                        },
                    )
                }
            }
        }
        Some("--help") | None => {
            usage();
            Ok(String::new())
        }
        Some(other) => Err(anyhow!("未知命令: {other}")),
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

fn summarize_links(links: &[assetc::BrokenLink]) -> String {
    if links.is_empty() {
        return "assetc_check_deps_ok".to_owned();
    }
    let mut out = format!("assetc_check_deps_found={}", links.len());
    for link in links {
        let chain: Vec<String> = link
            .chain
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        out.push_str(&format!("\n{} ({})", chain.join(" → "), link.reason));
    }
    out
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

fn summarize_benchmark(entries: &[assetc::CompressionBenchmarkEntry]) -> String {
    let mut out = String::from(
        "assetc_benchmark_compression\nkind\tfiles\toriginal_bytes\tcompressed_bytes\tratio\tcompress_ms\tdecompress_ms",
    );
    for entry in entries {
        out.push_str(&format!(
            "\n{}\t{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}",
            entry.kind,
            entry.files,
            entry.original_bytes,
            entry.compressed_bytes,
            entry.ratio(),
            entry.compress_time.as_secs_f64() * 1000.0,
            entry.decompress_time.as_secs_f64() * 1000.0,
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
