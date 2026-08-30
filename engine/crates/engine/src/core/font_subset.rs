//! TrueType 字体子集化：打包时把项目字体裁剪到用到的字符，大幅缩小发布体积
//! （替代原 `scripts/subset-font.py` 的 fontTools 步骤）。
//!
//! 仅支持 TrueType（glyf）非 CFF 字体；复合字形保留结构并补丁组件 ID；可变字体
//! 的 gvar/fvar 被丢弃（取默认实例轮廓，渲染为静态字重）。

use std::collections::{HashMap, HashSet};

/// 把 TrueType（glyf）字体子集化到 `chars` 用到的字符，返回子集 TTF 字节。
/// 非 TrueType / 解析失败返回 `Err`（调用方应保留原字体）。
pub fn subset_ttf(source: &[u8], chars: &str) -> Result<Vec<u8>, String> {
    let face = ttf_parser::Face::parse(source, 0).map_err(|e| format!("字体解析失败: {e}"))?;
    let glyf_raw = raw_table(source, b"glyf").ok_or("非 TrueType（glyf）字体，无法子集化")?;
    let loca_raw = raw_table(source, b"loca").ok_or("缺少 loca 表")?;
    let head_raw = raw_table(source, b"head").ok_or("缺少 head 表")?;
    let hhea_raw = raw_table(source, b"hhea").ok_or("缺少 hhea 表")?;
    let maxp_raw = raw_table(source, b"maxp").ok_or("缺少 maxp 表")?;
    let name_raw = raw_table(source, b"name").ok_or("缺少 name 表")?;
    let os2_raw = raw_table(source, b"OS/2");
    let post_raw = raw_table(source, b"post");

    let num_glyphs = face.number_of_glyphs().max(1);
    let loca_short = read_u16(head_raw, 50) == 0;
    let loca = Loca { short: loca_short, data: loca_raw, count: num_glyphs as usize + 1 };

    // 收集用到的字形 ID（复合字形组件递归入栈）。
    let mut used: HashSet<u16> = HashSet::new();
    let mut stack: Vec<u16> = Vec::new();
    for ch in chars.chars() {
        if let Some(gid) = face.glyph_index(ch) {
            stack.push(gid.0);
        }
    }
    while let Some(gid) = stack.pop() {
        if !used.insert(gid) {
            continue;
        }
        if let Some(range) = loca.glyph_range(gid) {
            let data = &glyf_raw[range];
            if is_composite(data) {
                for cid in composite_glyph_ids(data) {
                    stack.push(cid);
                }
            }
        }
    }
    used.insert(0); // .notdef

    // 重映射旧 ID → 新 ID（0 恒为 0，其余按升序）。
    let mut ids: Vec<u16> = used.iter().copied().collect();
    ids.sort_unstable();
    let mut remap: HashMap<u16, u16> = HashMap::new();
    let mut next = 0u16;
    for old in &ids {
        if *old == 0 {
            remap.insert(0, 0);
        } else {
            next += 1;
            remap.insert(*old, next);
        }
    }
    let new_num_glyphs = next + 1;

    // 按新 ID 顺序拷贝字形（简单直接拷，复合补丁组件 ID），每字形补 0 使 offset 偶数。
    let mut new_glyf: Vec<u8> = Vec::new();
    let mut new_loca: Vec<u32> = Vec::new();
    for new_id in 0..new_num_glyphs {
        new_loca.push(new_glyf.len() as u32);
        let old = ids.iter().find(|&&o| remap[&o] == new_id).copied();
        if let Some(old) = old
            && let Some(range) = loca.glyph_range(old)
        {
            let data = &glyf_raw[range];
            if is_composite(data) {
                new_glyf.extend(patch_composite(data, &remap));
            } else {
                new_glyf.extend_from_slice(data);
            }
            if new_glyf.len() % 2 != 0 {
                new_glyf.push(0);
            }
        }
    }
    new_loca.push(new_glyf.len() as u32);

    // loca 格式：所有 offset 偶数且 offset/2 可容纳 → short。
    let use_short = new_loca.iter().all(|o| o % 2 == 0 && *o / 2 <= 0xFFFF);
    let mut loca_bytes = Vec::new();
    for o in &new_loca {
        if use_short {
            push_u16(&mut loca_bytes, (*o / 2) as u16);
        } else {
            push_u32(&mut loca_bytes, *o);
        }
    }

    // cmap 格式 4（BMP；附加 0xFFFF 哨兵段映射到 .notdef）。
    let mut cmap_chars: Vec<(char, u16)> = chars
        .chars()
        .filter(|c| (*c as u32) <= 0xFFFF)
        .filter_map(|c| face.glyph_index(c).map(|gid| (c, remap[&gid.0])))
        .collect();
    cmap_chars.sort_by_key(|(c, _)| *c as u32);
    let cmap_bytes = build_cmap_format4(&cmap_chars);

    // hmtx：每字形一个条目。
    let mut hmtx_bytes = Vec::new();
    for new_id in 0..new_num_glyphs {
        let old = ids.iter().find(|&&o| remap[&o] == new_id).copied();
        let advance = old
            .and_then(|o| face.glyph_hor_advance(ttf_parser::GlyphId(o)))
            .unwrap_or(0);
        let lsb = old
            .and_then(|o| face.glyph_hor_side_bearing(ttf_parser::GlyphId(o)))
            .unwrap_or(0);
        push_u16(&mut hmtx_bytes, advance);
        push_i16(&mut hmtx_bytes, lsb);
    }

    // head/hhea/maxp 更新关键字段。
    let mut head = head_raw.to_vec();
    patch_u16(&mut head, 50, if use_short { 0 } else { 1 }); // indexToLocFormat
    let mut hhea = hhea_raw.to_vec();
    patch_u16(&mut hhea, 34, new_num_glyphs); // numberOfHMetrics
    let mut maxp = maxp_raw.to_vec();
    patch_u16(&mut maxp, 4, new_num_glyphs); // numGlyphs

    // 写出 TTF。
    write_ttf(&[
        (b"OS/2".to_vec(), os2_raw.map(|b| b.to_vec()).unwrap_or_default()),
        (b"cmap".to_vec(), cmap_bytes),
        (b"glyf".to_vec(), new_glyf),
        (b"head".to_vec(), head),
        (b"hhea".to_vec(), hhea),
        (b"hmtx".to_vec(), hmtx_bytes),
        (b"loca".to_vec(), loca_bytes),
        (b"maxp".to_vec(), maxp),
        (b"name".to_vec(), name_raw.to_vec()),
        (b"post".to_vec(), post_raw.map(|b| b.to_vec()).unwrap_or_default()),
    ])
}

/// 字形数据是否为复合字形（numberOfContours == -1）。
fn is_composite(data: &[u8]) -> bool {
    data.len() >= 2 && read_i16(data, 0) == -1
}

/// 提取复合字形里所有组件字形 ID。
fn composite_glyph_ids(data: &[u8]) -> Vec<u16> {
    let mut ids = Vec::new();
    let mut pos = 10; // numContours(2) + bbox(8)
    while pos + 4 <= data.len() {
        let flags = read_u16(data, pos);
        ids.push(read_u16(data, pos + 2));
        pos += 4;
        pos += if flags & 0x0001 != 0 { 4 } else { 2 }; // args
        if flags & 0x0008 != 0 {
            pos += 2;
        } else if flags & 0x0040 != 0 {
            pos += 4;
        } else if flags & 0x0080 != 0 {
            pos += 8;
        }
        if flags & 0x0020 == 0 {
            break;
        }
    }
    ids
}

/// 重建复合字形字节：头部与组件原样拷贝，组件 glyphIndex 按 `remap` 补丁。
fn patch_composite(data: &[u8], remap: &HashMap<u16, u16>) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&data[..10]);
    let mut pos = 10;
    while pos + 4 <= data.len() {
        let flags = read_u16(data, pos);
        let gid = read_u16(data, pos + 2);
        out.extend_from_slice(&data[pos..pos + 2]); // flags
        push_u16(&mut out, remap.get(&gid).copied().unwrap_or(gid)); // glyphIndex 补丁
        pos += 4;
        let args_len = if flags & 0x0001 != 0 { 4 } else { 2 };
        out.extend_from_slice(&data[pos..pos + args_len]);
        pos += args_len;
        if flags & 0x0008 != 0 {
            out.extend_from_slice(&data[pos..pos + 2]);
            pos += 2;
        } else if flags & 0x0040 != 0 {
            out.extend_from_slice(&data[pos..pos + 4]);
            pos += 4;
        } else if flags & 0x0080 != 0 {
            out.extend_from_slice(&data[pos..pos + 8]);
            pos += 8;
        }
        if flags & 0x0020 == 0 {
            break;
        }
    }
    if pos < data.len() {
        out.extend_from_slice(&data[pos..]); // WE_HAVE_INSTRUCTIONS 的指令区
    }
    out
}

/// 构建 cmap 表（头 + 一条 Windows/Unicode BMP 编码记录 + 格式 4 子表）。
/// 每字符一段 + 0xFFFF 哨兵段（未映射码点落到 .notdef）。
fn build_cmap_format4(chars: &[(char, u16)]) -> Vec<u8> {
    // 哨兵段保证未映射码点落到 .notdef。
    let mut segs: Vec<(u16, u16)> = chars.iter().map(|(c, id)| (*c as u16, *id)).collect();
    segs.push((0xFFFF, 0));
    let seg_count = segs.len() as u16;
    let seg_count_x2 = seg_count * 2;
    let mut pow2 = 1u16;
    while (pow2 << 1) <= seg_count {
        pow2 <<= 1;
    }
    let search_range = pow2 * 2;
    let entry_selector = (pow2 as f32).log2() as u16;
    let range_shift = seg_count_x2 - search_range;
    let length = 16 + seg_count_x2 * 4;

    // 格式 4 子表。
    let mut subtable = Vec::new();
    push_u16(&mut subtable, 4); // format
    push_u16(&mut subtable, length);
    push_u16(&mut subtable, 0); // language
    push_u16(&mut subtable, seg_count_x2);
    push_u16(&mut subtable, search_range);
    push_u16(&mut subtable, entry_selector);
    push_u16(&mut subtable, range_shift);
    for (end, _) in &segs {
        push_u16(&mut subtable, *end); // endCode
    }
    push_u16(&mut subtable, 0); // reservedPad
    for (start, _) in &segs {
        push_u16(&mut subtable, *start); // startCode
    }
    for (start, id) in &segs {
        push_u16(&mut subtable, id.wrapping_sub(*start)); // idDelta
    }
    for _ in 0..seg_count {
        push_u16(&mut subtable, 0); // idRangeOffset
    }

    // cmap 表头 + 编码记录。
    let mut out = Vec::new();
    push_u16(&mut out, 0); // version
    push_u16(&mut out, 1); // numTables
    push_u16(&mut out, 3); // platformID: Windows
    push_u16(&mut out, 1); // encodingID: Unicode BMP
    push_u32(&mut out, 12); // 子表偏移（表头 12 字节）
    out.extend_from_slice(&subtable);
    out
}

/// 写出 sfnt/TTF：offset 表 + 目录 + 表数据（4 字节对齐）。
fn write_ttf(tables: &[(Vec<u8>, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let n = tables.len() as u16;
    let mut pow2 = 1u16;
    while (pow2 << 1) <= n {
        pow2 <<= 1;
    }
    let search_range = pow2 * 16;
    let entry_selector = (pow2 as f32).log2() as u16;
    let range_shift = n * 16 - search_range;

    struct Rec {
        tag: Vec<u8>,
        checksum: u32,
        offset: u32,
        len: u32,
        data: Vec<u8>,
    }
    let mut offset = 12u32 + n as u32 * 16;
    let mut records = Vec::new();
    for (tag, data) in tables {
        let len = data.len() as u32;
        let padded = pad4(data);
        records.push(Rec {
            tag: tag.clone(),
            checksum: checksum(&padded),
            offset,
            len,
            data: padded,
        });
        offset += records.last().unwrap().data.len() as u32;
    }

    let mut out = Vec::new();
    push_u32(&mut out, 0x00010000); // sfnt version
    push_u16(&mut out, n);
    push_u16(&mut out, search_range);
    push_u16(&mut out, entry_selector);
    push_u16(&mut out, range_shift);
    for r in &records {
        out.extend_from_slice(&r.tag);
        push_u32(&mut out, r.checksum);
        push_u32(&mut out, r.offset);
        push_u32(&mut out, r.len);
    }
    for r in &records {
        out.extend_from_slice(&r.data);
    }
    Ok(out)
}

fn pad4(data: &[u8]) -> Vec<u8> {
    let mut v = data.to_vec();
    while v.len() % 4 != 0 {
        v.push(0);
    }
    v
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |acc, chunk| {
        let mut buf = [0u8; 4];
        buf[..chunk.len()].copy_from_slice(chunk);
        acc.wrapping_add(u32::from_be_bytes(buf))
    })
}

/// 简易 loca 表：按 head.indexToLocFormat 解析字形在 glyf 中的字节范围。
struct Loca<'a> {
    short: bool,
    data: &'a [u8],
    count: usize,
}

impl<'a> Loca<'a> {
    fn offset(&self, index: usize) -> u32 {
        let index = index.min(self.count.saturating_sub(1));
        if self.short {
            read_u16(self.data, index * 2) as u32 * 2
        } else {
            read_u32(self.data, index * 4)
        }
    }

    fn glyph_range(&self, glyph_id: u16) -> Option<std::ops::Range<usize>> {
        let start = self.offset(glyph_id as usize);
        let end = self.offset(glyph_id as usize + 1);
        if end < start {
            return None;
        }
        Some(start as usize..end as usize)
    }
}

/// 从 sfnt 表目录按 tag 取原始表字节（兼容 TTC 集合，取 face 0）。
fn raw_table<'a>(source: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    let dir_offset = if source.starts_with(b"ttcf") {
        // TTC 头：`ttcf` + version(4) + numFonts(4) + offsetTable[0](4)。
        if source.len() < 16 {
            return None;
        }
        let num_fonts = read_u32(source, 8) as usize;
        if num_fonts == 0 {
            return None;
        }
        read_u32(source, 12) as usize
    } else {
        0
    };
    parse_table_dir(source, dir_offset, tag)
}

fn parse_table_dir<'a>(source: &'a [u8], base: usize, tag: &[u8; 4]) -> Option<&'a [u8]> {
    if base + 12 > source.len() {
        return None;
    }
    let num_tables = read_u16(source, base + 4) as usize;
    if base + 12 + num_tables * 16 > source.len() {
        return None;
    }
    for i in 0..num_tables {
        let rec = base + 12 + i * 16;
        if source[rec..rec + 4] == *tag {
            let offset = read_u32(source, rec + 8) as usize;
            let len = read_u32(source, rec + 12) as usize;
            return source.get(offset..offset + len);
        }
    }
    None
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    if offset + 4 > data.len() {
        0
    } else {
        u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
    }
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    if offset + 2 > data.len() {
        0
    } else {
        u16::from_be_bytes([data[offset], data[offset + 1]])
    }
}

fn read_i16(data: &[u8], offset: usize) -> i16 {
    if offset + 2 > data.len() {
        0
    } else {
        i16::from_be_bytes([data[offset], data[offset + 1]])
    }
}

fn push_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn push_i16(out: &mut Vec<u8>, v: i16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn patch_u16(out: &mut [u8], offset: usize, v: u16) {
    let b = v.to_be_bytes();
    if offset + 2 <= out.len() {
        out[offset] = b[0];
        out[offset + 1] = b[1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_font() -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/editor/fonts/NotoSansSC-VF.ttf"),
        )
        .unwrap()
    }

    /// 子集化后：能用 ab_glyph 加载、能取到字符字形与 advance，且体积显著缩小。
    #[test]
    fn subset_reduces_size_and_keeps_glyphs() {
        let source = test_font();
        let chars = "开始游戏暂停ABCD0123，。";
        let subset = subset_ttf(&source, chars).expect("subset");
        assert!(subset.len() < source.len() / 2, "子集应明显更小");
        let face = ttf_parser::Face::parse(&subset, 0).expect("subset 可解析");
        for c in chars.chars() {
            assert!(
                face.glyph_index(c).is_some(),
                "子集应保留字符 {c:?}"
            );
        }
        let not_included = '翼';
        assert!(
            face.glyph_index(not_included).is_none(),
            "未收集的字符不应在子集 cmap 中"
        );
    }

    /// msyh（TTC 集合，face 0）也能子集化，体积显著缩小且字形保留。
    #[test]
    fn subset_works_on_msyh_ttc() {
        let source = std::fs::read(
            crate::core::games_dir().join("ui_demo/assets/fonts/msyh.ttc"),
        )
        .expect("msyh.ttc");
        let chars = "开始游戏暂停";
        let subset = subset_ttf(&source, chars).expect("msyh 子集化");
        assert!(subset.len() < source.len() / 10, "msyh 子集应远小于 19MB");
        let face = ttf_parser::Face::parse(&subset, 0).expect("子集可解析");
        for c in chars.chars() {
            assert!(face.glyph_index(c).is_some(), "子集应保留 {c:?}");
        }
    }

    /// 子集字体能被引擎 FontAtlas 加载并生成字形（端到端）。
    #[test]
    fn subset_font_loads_in_font_atlas() {
        let source = test_font();
        let chars = "引擎子集化测试";
        let subset = subset_ttf(&source, chars).expect("subset");
        let mut atlas = crate::core::font::FontAtlas::load(&subset).expect("FontAtlas 加载子集");
        for c in chars.chars() {
            let metrics = atlas.glyph_metrics(c);
            assert!(metrics.advance > 0.0, "{c:?} 应有 advance");
            assert!(
                metrics.bounds[2] > metrics.bounds[0] && metrics.bounds[3] > metrics.bounds[1],
                "{c:?} 应有非退化 bounds"
            );
        }
    }
}
