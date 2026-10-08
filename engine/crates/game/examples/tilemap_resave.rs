//! M72-01 编码迁移：把 `.scene` 里的 `Tilemap` 组件串重存成**当前编码**。
//!
//! 为什么需要一个工具而不是手工改：一格一个 ASCII 数字的旧编码（`TILES_FORMAT_LEGACY`）
//! 表达不了图集索引 10 以上，而新旧编码的差别只在**载荷的写法**，不在数据本身。
//! 本工具做的是一个纯函数 `旧串 → Tilemap → 新串`，因此它的正确性判据非常硬：
//! **重画出来的像素必须一格都不变**（由 `tools/preview_scene.py` 的前后合成图证明）。
//!
//! 只改 `Tilemap` 行：其余行按**原始字节**原样写回（含 CRLF、含其他组件的载荷规范形态），
//! 因为 `migrate_project` 那条通道不改写组件载荷，混在一起会把无关 diff 也带进来。
//!
//! Usage:
//!   cargo run -p game --example tilemap_resave --release -- [<project-root>...]
//!
//! 不带参数时只处理 `game_jhyt`（M72 的八张场景）。工具是**幂等**的：新编码再跑一遍
//! 得到同一份字节，所以可以安全地重复执行。

use engine::core::tilemap::{TILES_FORMAT_ATLAS, Tilemap};
use std::{env, fs, path::PathBuf, process};

fn main() {
    if let Err(error) = run() {
        eprintln!("tilemap_resave: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let roots: Vec<PathBuf> = if env::args().nth(1).is_some() {
        env::args()
            .skip(1)
            .map(|arg| Ok(PathBuf::from(arg).canonicalize()?))
            .collect::<Result<_, Box<dyn std::error::Error>>>()?
    } else {
        vec![engine::core::games_dir().join("examples").join("game_jhyt")]
    };
    let mut files = 0usize;
    let mut rewritten = 0usize;
    let mut changed_lines = 0usize;
    for root in roots {
        if !root.join("project.toml").is_file() {
            return Err(format!("not a game project: {}", root.display()).into());
        }
        let scenes_dir = root.join("scenes");
        if !scenes_dir.is_dir() {
            eprintln!("  (no scenes dir: {})", scenes_dir.display());
            continue;
        }
        let mut entries: Vec<_> = fs::read_dir(&scenes_dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "scene"))
            .collect();
        entries.sort();
        for path in entries {
            files += 1;
            let bytes = fs::read(&path)?;
            let (output, line_changes) = resave_bytes(&bytes)?;
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if line_changes == 0 {
                println!("  {name}: 已是当前编码（{} B 不变）", bytes.len());
                continue;
            }
            fs::write(&path, &output)?;
            rewritten += 1;
            changed_lines += line_changes;
            println!(
                "  {name}: 重存 {line_changes} 行 Tilemap（{} B → {} B）",
                bytes.len(),
                output.len()
            );
        }
    }
    println!(
        "共 {files} 张场景，重存 {rewritten} 张 / {changed_lines} 行（tiles_format={TILES_FORMAT_ATLAS}）"
    );
    if changed_lines > 0 {
        println!(
            "下一步：py tools/preview_scene.py scenes/<name>.scene 与迁移前基线逐字节对比，\n\
             不一致即为编码迁移写错，不是「美术顺手改了」。"
        );
    }
    Ok(())
}

/// 逐行重存 `.scene` 字节流；返回 `(新字节, 改动的行数)`。
///
/// 行分隔符按文件里实际出现的写法还原（工程内是 CRLF）：只用 `\n` 切、再统一用
/// `\r\n` 拼，会把 LF 文件也顺手改掉，diff 里混进无关的换行符噪声。
fn resave_bytes(bytes: &[u8]) -> Result<(Vec<u8>, usize), Box<dyn std::error::Error>> {
    let text = std::str::from_utf8(bytes)?;
    let mut output = String::with_capacity(text.len());
    let mut changes = 0usize;
    for line in split_preserving(text) {
        if !line.ends_with('\n') {
            // 末行无换行：原样写出，但同样计入改动数。
            let rewritten = resave_line(line)?;
            if rewritten != line {
                changes += 1;
            }
            output.push_str(&rewritten);
            continue;
        }
        let body = line.strip_suffix('\n').unwrap_or(line);
        let (body, ending) = if let Some(stripped) = body.strip_suffix('\r') {
            (stripped, "\r\n")
        } else {
            (body, "\n")
        };
        let rewritten = resave_line(body)?;
        if rewritten != body {
            changes += 1;
        }
        output.push_str(&rewritten);
        output.push_str(ending);
    }
    Ok((output.into_bytes(), changes))
}

fn split_preserving(text: &str) -> Vec<&str> {
    // 保留分隔符的切分：`split_inclusive('\n')` 正是这件事。
    text.split_inclusive('\n').collect()
}

fn resave_line(line: &str) -> Result<String, Box<dyn std::error::Error>> {
    // 组件行：`component\t<entity id>\t<ComponentName>\t<载荷>`。
    let mut fields = line.split('\t');
    if fields.next() != Some("component") {
        return Ok(line.to_owned());
    }
    let rest: Vec<&str> = fields.collect();
    if rest.len() < 3 || rest[1] != "Tilemap" {
        return Ok(line.to_owned());
    }
    let payload = rest[2..].join("\t");
    let map = Tilemap::from_component_str(&payload).map_err(|error| {
        // 只取前 60 个**字符**（按字节切会在中文场景名上越过 UTF-8 边界直接 panic）。
        let head: String = line.chars().take(60).collect();
        format!("{head} 行的 Tilemap 解析失败：{error}")
    })?;
    Ok(format!(
        "component\t{}\tTilemap\t{}",
        rest[0],
        map.to_component_str()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 迁移的核心不变量：**旧编码读进来的地图，重存成新编码后再读回来，逐字段相等**。
    /// 这条等价性就是"像素不变"的成因——渲染只读这些字段。
    #[test]
    fn legacy_payload_round_trips_through_resave() {
        let legacy =
            "tile_size=32x32;width=3;height=2;texture=assets/tileset.png;tiles=120345;solid=001000";
        let line = format!("component\t1\tTilemap\t{legacy}");
        let (bytes, changes) = resave_bytes(line.as_bytes()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(changes, 1, "旧编码必须被重存");
        assert!(text.contains("tiles_format=1"), "新编码要带版本号：{text}");
        assert!(text.contains("tiles=1,2,0,3,4,5"), "逐格十进制：{text}");
        // 幂等：再跑一次不再改动。
        let (_, second) = resave_bytes(text.as_bytes()).unwrap();
        assert_eq!(second, 0, "重存必须幂等");
        // 数据等价：新旧两份解析结果逐字段相同——唯一该变的就是版本号本身。
        let payload = text.split('\t').nth(3).unwrap();
        let mut expected = Tilemap::from_component_str(legacy).unwrap();
        assert_eq!(expected.tiles_format, 0, "旧串读出来必须标成旧编码");
        expected.tiles_format = TILES_FORMAT_ATLAS;
        assert_eq!(Tilemap::from_component_str(payload).unwrap(), expected);
    }

    #[test]
    fn non_tilemap_lines_and_line_endings_are_untouched() {
        let input = "scene_version=2\r\nname=manor\r\nentity\t1\tGround\r\n\
                     component\t1\tTransform\tx=0.000;y=0.000;rotation=0.000;scale_x=1.000;scale_y=1.000\r\n\
                     component\t1\tTilemap\ttile_size=32x32;width=1;height=1;texture=t.png;tiles=1;solid=0\r\n";
        let (bytes, changes) = resave_bytes(input.as_bytes()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(changes, 1, "只有 Tilemap 行该动");
        assert_eq!(
            text.matches("\r\n").count(),
            input.matches("\r\n").count(),
            "CRLF 必须原样保留"
        );
        assert!(text.contains("component\t1\tTransform\tx=0.000;y=0.000"));
        assert!(text.contains("scene_version=2"));
    }
}
