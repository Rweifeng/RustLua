//! 按项目**实际用到的字符**重新子集化 `project.toml` 里声明的主字体。
//!
//! 发布流程（`assetc package`）用同一套 `char_collect` + `font_subset` 生成子集，这里做的是
//! **仓库里那份工程字体**：文案变了以后旧子集会缺字，屏幕上显示成方框
//! （`ui.measure_text(...).missing_count > 0` 就是这条判据）。
//!
//! `--source` 指全量字体（`engine/crates/editor/fonts/` 那一类），目标取 `project.toml` 的
//! `font =`。两者必须是不同文件：子集不可逆（`subset_ttf` 会丢掉 `gvar/fvar` 与全部未用字形），
//! 拿子集再子集一次就把源字体毁了，所以这里直接拒。
//!
//! 用法：`cargo run -q -p engine --example subset_project_font -- <project-root> --source <full.ttf>`
use std::{fs, path::PathBuf};

use rustlua_render::font::project_font;
use rustlua_render::font_subset::subset_ttf;
use rustlua_scene::char_collect::collect_project_chars;

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
