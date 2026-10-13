//! 声明视口 → 目标矩形：一张**声明的逻辑盒**怎样摆进一块实际附件（等比 contain + 居中）。
//!
//! 为什么在引擎侧而不是宿主侧：渲染器是唯一同时知道"投影按哪张盒算"（`RenderView::viewport`）
//! 和"像素最终落在哪块附件"（`config.width/height` / 离屏纹理尺寸）的地方。这两者不等时
//! 必须有一个**居中且等比**的矩形；此前这里只会把盒夹到 `(0,0,w,h)`，于是宿主只要
//! "声明的盒 ≠ 附件"就得到左上角一块内容 + 右下一大片底色（微信接管形态实测：内容块
//! `1380×720` / 表面 `2532×1170`，非底色像素只占 `31.1%`）。
//!
//! 口径与 Canvas2D 层 `plugins/webgl/web-runtime.js` 的 `fitS = min(w/1280, h/720)` +
//! 居中平移**同一套算法**（`game_web::fit::contain_box` 现在就是把本函数丢掉偏移），
//! 两层各自算一遍可以，各算各的口径不行。

/// 把声明的 `declared`（浮点逻辑盒）等比 contain 进 `target` 附件，返回居中的整数矩形
/// `(x, y, width, height)`。
///
/// 三条不变量，都有测试钉着：
/// - **不溢出**：结果永远落在 `target` 内，且至少 `1×1`（越界的 `set_viewport` 会被 wgpu
///   校验直接判错，而宿主给的是逻辑像素，与实际附件尺寸可能有取整差）。
/// - **等比**：结果宽高比 = `declared` 宽高比（±1 px），因此不会出现"圆画成椭圆"。
/// - **贴合即整目标**：某一轴贴满、另一轴也在 1 px 容差内贴满时，返回整个 `target`——
///   渲染器据此**不调用** `set_viewport`，于是"声明盒 ≡ 附件"的既有宿主（桌面、编辑器
///   离屏、浏览器 fit 后的 GL 画布）命令流逐字节不变，本次收敛不引入像素差异。
#[must_use]
pub fn contain_rect(declared: (f32, f32), target: (u32, u32)) -> (u32, u32, u32, u32) {
    let (target_width, target_height) = (target.0.max(1), target.1.max(1));
    let (declared_width, declared_height) =
        (declared.0.max(1.0) as f64, declared.1.max(1.0) as f64);
    let scale =
        (f64::from(target_width) / declared_width).min(f64::from(target_height) / declared_height);
    // 退化（NaN / inf / 非正比例）时不缩放也不偏移：整目标就是最不错误的答案，
    // 让"宿主给的盒坏了"停在读数里可见，而不是变成 1×1 的黑点。
    if !scale.is_finite() || scale <= 0.0 {
        return (0, 0, target_width, target_height);
    }
    let width = (declared_width * scale).round().max(1.0) as u32;
    let height = (declared_height * scale).round().max(1.0) as u32;
    if width.saturating_add(1) >= target_width && height.saturating_add(1) >= target_height {
        return (0, 0, target_width, target_height);
    }
    let (width, height) = (width.min(target_width), height.min(target_height));
    (
        (target_width - width) / 2,
        (target_height - height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::contain_rect;

    /// 等比 + 居中：细长表面里声明盒按**短边**贴合，两轴留边相等。
    #[test]
    fn letterboxes_and_centers() {
        // 微信接管形态的实测表面（设计 1280×720 进 2532×1170）。
        assert_eq!(
            contain_rect((1280.0, 720.0), (2532, 1170)),
            (226, 0, 2080, 1170)
        );
        // 竖着的表面：按宽贴合，上下留边。
        assert_eq!(
            contain_rect((1280.0, 720.0), (800, 1000)),
            (0, 275, 800, 450)
        );
    }

    /// 同比例 = 贴满，不产生任何偏移，也不产生 1 px 缝。
    #[test]
    fn fills_a_matching_target() {
        assert_eq!(
            contain_rect((1280.0, 720.0), (1280, 720)),
            (0, 0, 1280, 720)
        );
        assert_eq!(
            contain_rect((1280.0, 720.0), (2560, 1440)),
            (0, 0, 2560, 1440)
        );
    }

    /// **1 px 容差**是既有宿主的护栏：浏览器 fit 后的 GL 画布与设计盒同比例但按整数取整，
    /// 若这里差 1 px 就返回小一号的矩形，画面会多出一条底色缝，命令流也不再是"不调用"。
    #[test]
    fn snaps_within_one_pixel_of_the_target() {
        assert_eq!(contain_rect((1280.0, 720.0), (756, 426)), (0, 0, 756, 426));
        assert_eq!(contain_rect((1280.0, 720.0), (755, 425)), (0, 0, 755, 425));
    }

    /// 结果不许溢出附件，也不许给出 0（wgpu 校验与 surface 配置都拒绝这些）。
    #[test]
    fn stays_inside_the_target() {
        for (declared, target) in [
            ((1280.0, 720.0), (1, 1)),
            ((0.0, 0.0), (100, 100)),
            ((1.0e30, 1.0e30), (640, 480)),
            ((640.0, 480.0), (0, 0)),
        ] {
            let (x, y, width, height) = contain_rect(declared, target);
            let (tw, th) = (target.0.max(1), target.1.max(1));
            assert!(
                width >= 1 && height >= 1,
                "{declared:?}→{target:?} 给出 {width}x{height}"
            );
            assert!(
                x + width <= tw && y + height <= th,
                "{declared:?}→{target:?} 溢出"
            );
        }
    }

    /// 非有限/负的声明盒不许传染成偏移溢出：每根轴先被 `max(1.0)` 收正，结果仍然落在附件内。
    /// 这里刻意只断言"不溢出、不为 0"，不断言具体矩形——退化输入的语义就是"宿主给了坏数"，
    /// 要钉的是它不许把渲染命令流带崩。
    #[test]
    fn survives_non_finite_declarations() {
        for declared in [(f32::NAN, 720.0), (1280.0, f32::INFINITY), (-4.0, -8.0)] {
            let (x, y, width, height) = contain_rect(declared, (640, 480));
            assert!(
                width >= 1 && height >= 1,
                "{declared:?} 给出 {width}x{height}"
            );
            assert!(x + width <= 640 && y + height <= 480, "{declared:?} 溢出");
        }
    }
}
