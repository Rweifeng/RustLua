//! 两层共用的**几何**：把设计分辨率 contain 进实际画布（等比 + 居中留黑边）。
//!
//! 放在不受 `target_arch`/`feature` 门控的模块里，是为了它能被**原生单测**覆盖：
//! 算法本身是纯数学，而它一旦错，症状（GPU 层被非等比拉伸）在浏览器读数里完全看不出来——
//! 那正是需要一条能在 CI 里红的小测试的地方（见本模块 `tests`）。
//!
//! 不是"随便挑一个比例"：这是与 Canvas2D 层 `plugins/webgl/web-runtime.js`
//! 每帧做的 `fitS = min(w/1280, h/720)` + 居中平移**同一套算法**。两层各自算一遍没问题，
//! 但不能各算各的口径——一边 contain、一边铺满，就是"两个不同的盒子"。

/// 等比适配盒：`DESIGN_WIDTH×DESIGN_HEIGHT` contain 进 `box`，返回**像素尺寸**（宽, 高）。
///
/// 结果与设计分辨率同比（±1 个像素），且不会超出 `box`；退化输入（0 或极端小）也给正数，
/// 因为 0×0 的画布会让 surface 配置直接失败（`wgpu` 不接受 0 尺寸）。
///
/// 偏移在渲染器那一侧算（`engine::host::fit::contain_rect`：它是唯一同时看得见"声明盒"和
/// "实际附件"的地方），这里只把矩形降成尺寸——**两处共用一份换算**，不允许各写一套口径。
pub fn contain_box(box_w: u32, box_h: u32, design_w: u32, design_h: u32) -> (u32, u32) {
    let (_, _, width, height) = engine::host::fit::contain_rect(
        (design_w.max(1) as f32, design_h.max(1) as f32),
        (box_w, box_h),
    );
    (width, height)
}

/// CSS 像素坐标 → 设计坐标（1280×720）。
///
/// `fit_box` = 等比适配盒在 CSS 像素里的 `[left, top, width, height]`（`fit_now` 每帧算出来、
/// 同时也写进画布 style 的那一组数）。宿主拿到的是 `MouseEvent::client_x/client_y`，
/// 而脚本要的是设计坐标：**这两者之间差着"窗口不是 16:9"的那一段**。
///
/// 为什么和 `contain_box` 放在一起：它们必须互逆——画面按 contain 摆、光标按同一组数反算，
/// 分别各写一份就会出现"画的是一处、点的是另一处"。这种错在读数里完全看不见
/// （`frames=`/`draws=` 都正常），只有对着画面点一下才发作。
///
/// 参数名不叫 `box`：那是 edition 2024 的保留字（编译期直接红）。
#[must_use]
pub fn css_to_design(fit_box: [f64; 4], css: [f64; 2]) -> [f64; 2] {
    let [left, top, width, height] = fit_box;
    if width <= 0.0 || height <= 0.0 {
        // 退化盒（还没布局 / 尺寸为 0）：退回"CSS 像素即设计像素"，别产生 inf/NaN。
        return [css[0] - left, css[1] - top];
    }
    [
        (css[0] - left) * super::DESIGN_WIDTH as f64 / width,
        (css[1] - top) * super::DESIGN_HEIGHT as f64 / height,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const DW: u32 = 1280;
    const DH: u32 = 720;

    /// 适配盒必须是**设计分辨率同比**的（否则两层不同框 ⇒ 画面被拉伸）。
    ///
    /// 反例就是本轮修掉的行为：固定 1280×720 铺满 756×441 的窗口（纵向拉伸约 7%）。
    #[test]
    fn keeps_design_aspect() {
        let want = DW as f64 / DH as f64;
        for (box_w, box_h) in [
            (DW, DH),
            (756, 441),
            (800, 600),
            (1920, 1080),
            (3840, 2160),
            (100, 100),
            (1440, 900),
        ] {
            let (w, h) = contain_box(box_w, box_h, DW, DH);
            assert!(w > 0 && h > 0, "box={box_w}x{box_h} 给出 {w}x{h}");
            assert!(
                w <= box_w && h <= box_h,
                "box={box_w}x{box_h} 溢出成 {w}x{h}"
            );
            let ratio = w as f64 / h as f64;
            assert!(
                (ratio - want).abs() < want * 0.01,
                "box={box_w}x{box_h} 比例 {ratio:.4} 不是设计比例 {want:.4}"
            );
        }
    }

    /// contain 的黑边总是出现在**不贴合的那个方向**——两个方向都要能留边，而不是只保一个方向。
    ///
    /// 这里刻意把三种情形都钉住：更宽的盒子（左右留边）、更高的盒子（上下留边）、
    /// 以及**同比例**（两边都贴满、没有黑边）。最后一种最容易写错：`1280×1000` 的高度比例
    /// 更大，于是按宽度贴合，得到 1280×720——它是"贴满宽度"而不是"上下留边"。
    #[test]
    fn letterboxes_the_unbound_axis() {
        // 更宽（2000/720 > 16:9）：按高度贴合，左右留黑边。
        let (w, h) = contain_box(2000, 720, DW, DH);
        assert_eq!((w, h), (1280, 720), "更宽的盒子该按高度贴合");
        assert!(w < 2000, "更宽的盒子该在左右留黑边");

        // 更高（800/1000 < 16:9）：按宽度贴合，上下留黑边。
        let (w, h) = contain_box(800, 1000, DW, DH);
        assert_eq!((w, h), (800, 450), "更高的盒子该按宽度贴合");
        assert!(h < 1000, "更高的盒子该在上下留黑边");

        // 同比例：两边都贴满。
        let (w, h) = contain_box(1280, 1000, DW, DH);
        assert_eq!((w, h), (1280, 720), "宽度刚好同比例时就该贴满宽度");

        // 实测窗口（756×441 不是 16:9）：这是本轮修掉的那个盒子。
        let (w, h) = contain_box(756, 441, DW, DH);
        assert_eq!((w, h), (756, 425), "756×441 的实测窗口应给出 756×425");
    }

    /// 退化输入不许 panic，也不许给出 0（0×0 的画布会让 surface 配置直接失败）。
    #[test]
    fn survives_degenerate_boxes() {
        for (box_w, box_h) in [(0, 0), (0, 600), (800, 0), (1, 1)] {
            let (w, h) = contain_box(box_w, box_h, DW, DH);
            assert!(w > 0 && h > 0, "box={box_w}x{box_h} 给出 {w}x{h}");
        }
    }

    /// 光标换算必须与适配盒**互逆**：盒内四角映射到设计四角，盒外按同一比例外推。
    ///
    /// 这条是"点哪儿都不对"的守卫。实测窗口 756×441 ⇒ 内容盒 756×425 居中（上下各 8 px）：
    /// 内容盒左上角在 CSS 上是 (0,8)，映射到设计 (0,0)；右下角 (756,433) 映射到 (1280,720)。
    /// 此前不是这么算的（把 CSS 像素当设计像素），于是虚拟摇杆/按钮与"点哪块地"整条命中链一起错位。
    #[test]
    fn css_to_design_is_inverse_of_the_fit_box() {
        let fit_box = [0.0, 8.0, 756.0, 425.0];
        assert_eq!(css_to_design(fit_box, [0.0, 8.0]), [0.0, 0.0]);
        let bottom_right = css_to_design(fit_box, [756.0, 433.0]);
        assert!(
            (bottom_right[0] - 1280.0).abs() < 0.5 && (bottom_right[1] - 720.0).abs() < 0.5,
            "内容盒右下角该映射到设计右下角，实际 {bottom_right:?}"
        );
        // 中心：盒中心 ↔ 设计中心。
        let center = css_to_design(fit_box, [378.0, 8.0 + 212.5]);
        assert!(
            (center[0] - 640.0).abs() < 1.0 && (center[1] - 360.0).abs() < 1.0,
            "内容盒中心该映射到设计中心，实际 {center:?}"
        );
        // 盒外（左边留黑边处）：按同一比例外推成负值，而不是被夹到 0——
        // 「点在黑边上」与「点在设计原点」是两件事，夹住会让脚本以为是后一种。
        let outside = css_to_design([100.0, 8.0, 756.0, 425.0], [50.0, 8.0]);
        assert!(outside[0] < 0.0, "盒外该外推出负坐标，实际 {outside:?}");
    }

    /// 退化盒不许产生 inf/NaN（那会让"点哪块地"的判定彻底失效，且很难回溯到几何）。
    #[test]
    fn css_to_design_survives_a_degenerate_box() {
        for fit_box in [
            [0.0, 0.0, 0.0, 0.0],
            [10.0, 10.0, 0.0, 100.0],
            [10.0, 10.0, 100.0, 0.0],
        ] {
            let point = css_to_design(fit_box, [30.0, 40.0]);
            assert!(
                point[0].is_finite() && point[1].is_finite(),
                "fit_box={fit_box:?} 给出 {point:?}"
            );
        }
    }
}
