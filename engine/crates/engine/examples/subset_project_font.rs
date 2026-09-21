//! 临时工具：按项目**实际用到的字符**重新子集化示例工程的主字体（用完即删）。
//!
//! 工程的 `assets/fonts/NotoSansSC-VF.ttf` 是发布流程（`assetc package`）用同一套
//! `char_collect` + `font_subset` 生成的子集。示例工程重建后文案变了，旧子集缺字会
//! 在屏幕上显示成方框（`ui.measure_text(...).missing_count > 0` 就是这条判据）。
//!
//! 用法：`cargo run -q -p engine --example subset_project_font -- <project-root>`
use std::{fs, path::PathBuf};

use engine::core::{char_collect::collect_project_chars, font_subset::subset_ttf};

fn main() {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: subset_project_font <project-root>"),
    );
    let source_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
    let target = root.join("assets/fonts/NotoSansSC-VF.ttf");

    let chars = collect_project_chars(&root);
    let source = fs::read(&source_path).expect("read source font");
    let before = fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0);
    let subset = subset_ttf(&source, &chars).expect("subset font");
    fs::write(&target, &subset).expect("write subset font");
    println!(
        "subset_project_font_ok chars={} before={}B after={}B",
        chars.chars().count(),
        before,
        subset.len()
    );
}
