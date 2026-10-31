//! 按项目**实际用到的字符**重新子集化 `project.toml` 里声明的主字体。
//!
//! 发布流程（`assetc package`）用同一套 `char_collect` + `font_subset` 生成子集，这里做的是
//! **仓库里那份工程字体**：文案变了以后旧子集会缺字，屏幕上显示成方框
//! （`ui.measure_text(...).missing_count > 0` 就是这条判据）。
//!
//! `--source` 指全量字体（`engine/crates/editor/fonts/` 那一类），目标取 `project.toml` 的
//! `font =`。两者必须是不同文件：子集不可逆（未用字形被丢光；可变字体虽然保留
//! `fvar`/`avar`/`gvar`，但也只剩这一份裁过的字形），拿子集再子集一次就永远回不到全量。
//!
//! 源字体是可变字体时**不要**先手工实例化：`font_weight` 在运行时经 `set_variation`
//! 生效，轴被丢就会静默退回默认实例（Noto Sans SC 默认是 Thin，小字号笔画细而发虚）。
//!
//! 用法：`cargo run -q -p engine --example subset_project_font -- <project-root> --source <full.ttf>`
use std::{fs, path::PathBuf};

use rustlua_assets_import::char_collect::collect_project_chars;
use rustlua_render::font::project_font;
use rustlua_render::font_subset::subset_ttf;

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let root = args.next().ok_or_else(|| {
        "usage: subset_project_font <project-root> --source <full.ttf>".to_owned()
    })?;
    let root = PathBuf::from(root);
    let source = match args.next().as_deref() {
        Some("--source") => PathBuf::from(args.next().ok_or("--source 后面要跟路径")?),
        Some(other) => return Err(format!("未知参数 {other}（应为 --source <full.ttf>）")),
        None => return Err("必须用 --source 指明全量字体".to_owned()),
    };
    let target = root.join(project_font::primary_path(&root));
    if target == source {
        return Err(format!(
            "源与目标是同一个文件 {}：子集不可逆，第二次子集会把源字体毁掉",
            target.display()
        ));
    }

    let chars = collect_project_chars(&root);
    let source_bytes = fs::read(&source).map_err(|error| format!("read {source:?}: {error}"))?;
    let before = fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0);
    let subset = subset_ttf(&source_bytes, &chars)?;
    fs::write(&target, &subset).map_err(|error| format!("write {target:?}: {error}"))?;
    println!(
        "subset_project_font_ok chars={} before={before}B after={}B source={} target={}",
        chars.chars().count(),
        subset.len(),
        source.display(),
        target.display(),
    );
    Ok(())
}
