//! 图集打包 CLI（批 D，`docs/63`）：把一组散图 PNG 装箱为 `sheet-{i}.png` +
//! `sheet-{i}.atlas` 成对产物。
//!
//! 用法：`cargo run -q -p game --example atlas_pack -- <输出目录> <前缀> <输入 png ...>`
//!
//! `.atlas` 里的 `texture` 沿用调用者传入的 `<输出目录>` 形式（传项目根相对路径即得相对引用），
//! 分隔符统一为 `/`。区域名取文件名词干（stem）；同名后者覆盖前者并给出警告。
//! 装箱算法与门的确定性由 `rustlua-assets::atlas_pack` 的 golden 测试钉住。

use engine::core::atlas_pack::{AtlasPackerOptions, PackSource, compose_page, pack_atlas};
use std::{fs, path::PathBuf};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        return Err("用法: atlas_pack <输出目录> <前缀> <输入 png ...>".to_owned());
    }
    let out_dir = PathBuf::from(&args[0]);
    let prefix = &args[1];
    fs::create_dir_all(&out_dir).map_err(|error| format!("创建输出目录失败: {error}"))?;
    let mut sources: Vec<PackSource> = Vec::new();
    for input in &args[2..] {
        let path = PathBuf::from(input);
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("无法从路径取区域名: {input}"))?
            .to_owned();
        if sources.iter().any(|existing| existing.name == name) {
            eprintln!("警告: 区域名 {name:?} 重复，后出现的源会覆盖先前装箱");
            sources.retain(|existing| existing.name != name);
        }
        sources.push(PackSource { name, path });
    }
    let options = AtlasPackerOptions::default();
    let pages = pack_atlas(&sources, &options).map_err(|error| error.to_string())?;
    for (index, page) in pages.iter().enumerate() {
        let png = out_dir.join(format!("{prefix}-{index}.png"));
        let atlas = out_dir.join(format!("{prefix}-{index}.atlas"));
        compose_page(page, &sources, &png, &atlas).map_err(|error| error.to_string())?;
        println!(
            "atlas_pack_ok page={index} size={}x{} regions={} png={} atlas={}",
            page.width,
            page.height,
            page.regions.len(),
            png.display(),
            atlas.display()
        );
    }
    Ok(())
}
