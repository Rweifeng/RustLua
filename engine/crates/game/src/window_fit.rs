//! PC 初始窗口的尺寸与落点：设计分辨率装不进屏幕可用区时按**同一比例**缩小，并把
//! 窗口摆到可用区中央。
//!
//! **为什么需要**：竖版工程（ClashRoyale 720×1280）按设计分辨率开窗时，窗口外框在
//! 1080p 桌面上是 736×1319——比整块屏幕还高，底部按钮行与提示文案直接落在屏幕外，
//! 看上去"只显示了半屏"。运行期的逻辑屏幕空间固定为设计分辨率（`GameApp::design_size`），
//! 渲染器把设计盒 `contain_rect` 到窗口，所以窗口只要**保持设计宽高比**缩小，整幅画面
//! 等比变小，Lua 布局一个像素都不用改。落点也必须自己给：winit 不指定位置时由系统级联，
//! 缩小后的窗口仍可能有一截挂在屏幕外（实测外框 557×1000 被摆到 y=205）。
//!
//! 可用区优先取 Windows 工作区（`SPI_GETWORKAREA`，已扣任务栏），拿不到时退回显示器
//! 物理尺寸；再扣窗口外框（标题栏 + 边框）与一圈边距，剩下的才是客户区上限。
//!
//! 只缩不放：显示器比设计分辨率大时窗口仍是设计尺寸（像素级 1:1），位置取可用区居中。

/// 窗口外框相对客户区的额外像素（96 DPI 基准，宽 +16 / 高 +39）。
///
/// 实测来源：720×1280 客户区的窗口在 100% 缩放桌面上外框为 736×1319。
/// `with_inner_size` 收的是**客户区**尺寸，所以适配时必须先扣外框，否则窗口仍会顶到
/// 任务栏甚至超出屏幕。
const FRAME_INSETS_96: (u32, u32) = (16, 39);

/// 客户区与屏幕可用区之间留的呼吸边距（96 DPI 基准，四边各一份）。
///
/// 不留边距时窗口会与任务栏/屏幕边缘严丝合缝；外框尺寸在不同 Windows 版本与主题下
/// 还有几像素出入，边距就是那份容差。
const SCREEN_MARGIN_96: u32 = 16;

/// 屏幕可用区（物理像素）：左上角 + 尺寸。Windows 上就是工作区（已扣任务栏）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// 初始窗口：客户区尺寸（`with_inner_size`）+ 落点（`with_position`，左上角 = 外框角）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitialWindow {
    pub inner_size: (u32, u32),
    /// `None` = 拿不到屏幕可用区，交给系统摆位。
    pub position: Option<(i32, i32)>,
}

/// 把 `desired` 等比缩小到 `available` 以内；装得下时原样返回。
///
/// 两条不变量：**宽高比不变**（整数取整误差 ≤1 px）、结果至少 `1×1`
/// （`with_inner_size(PhysicalSize::new(0, _))` 在 winit 上会被夹成 1 或在后端报错）。
#[must_use]
pub fn fit_to_area(desired: (u32, u32), available: (u32, u32)) -> (u32, u32) {
    let desired = (desired.0.max(1), desired.1.max(1));
    let available = (available.0.max(1), available.1.max(1));
    if desired.0 <= available.0 && desired.1 <= available.1 {
        return desired;
    }
    let scale = (f64::from(available.0) / f64::from(desired.0))
        .min(f64::from(available.1) / f64::from(desired.1));
    // 取整用 round 而不是 floor：比例由 `available / desired` 得来，乘法回程在 f64 下
    // 可能落在 N−ε（floor 会白白丢 1 px）。乘法结果数学上 ≤ available，round 也不会溢出。
    (
        ((f64::from(desired.0) * scale).round() as u32)
            .max(1)
            .min(available.0),
        ((f64::from(desired.1) * scale).round() as u32)
            .max(1)
            .min(available.1),
    )
}

/// 初始窗口的尺寸与落点。
///
/// `screen` = 屏幕可用区（[`screen_area`]；拿不到时传 `None`），`scale_factor` = 该显示器
/// 的 DPI 缩放（外框与边距都按它放大）。
pub fn initial_window(
    desired: (u32, u32),
    screen: Option<ScreenArea>,
    scale_factor: f64,
) -> InitialWindow {
    let desired = (desired.0.max(1), desired.1.max(1));
    let (Some(screen), Some(limit)) = (
        screen,
        screen.and_then(|screen| client_area_limit(screen, scale_factor)),
    ) else {
        return InitialWindow {
            inner_size: desired,
            position: None,
        };
    };
    let inner_size = fit_to_area(desired, limit);
    let (frame_width, frame_height) = frame_insets(scale_factor);
    let outer = (
        inner_size.0.saturating_add(frame_width),
        inner_size.1.saturating_add(frame_height),
    );
    InitialWindow {
        inner_size,
        position: Some((
            screen.x + (screen.width.saturating_sub(outer.0) / 2) as i32,
            screen.y + (screen.height.saturating_sub(outer.1) / 2) as i32,
        )),
    }
}

/// 客户区能占用的最大物理像素：屏幕可用区 − 外框 − 边距。
fn client_area_limit(screen: ScreenArea, scale_factor: f64) -> Option<(u32, u32)> {
    if screen.width == 0 || screen.height == 0 {
        return None;
    }
    let (frame_width, frame_height) = frame_insets(scale_factor);
    let margin = scale_px(SCREEN_MARGIN_96, scale_factor) * 2;
    Some((
        screen
            .width
            .saturating_sub(frame_width)
            .saturating_sub(margin)
            .max(1),
        screen
            .height
            .saturating_sub(frame_height)
            .saturating_sub(margin)
            .max(1),
    ))
}

fn scale_px(base: u32, scale_factor: f64) -> u32 {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    (f64::from(base) * scale).round() as u32
}

fn frame_insets(scale_factor: f64) -> (u32, u32) {
    (
        scale_px(FRAME_INSETS_96.0, scale_factor),
        scale_px(FRAME_INSETS_96.1, scale_factor),
    )
}

/// 屏幕可用区：Windows 走工作区（扣任务栏），其余平台退回显示器尺寸的 90%。
pub fn screen_area(
    monitor_position: Option<(i32, i32)>,
    monitor_size: Option<(u32, u32)>,
) -> Option<ScreenArea> {
    platform_screen_area(monitor_position, monitor_size)
}

#[cfg(windows)]
fn platform_screen_area(
    monitor_position: Option<(i32, i32)>,
    monitor_size: Option<(u32, u32)>,
) -> Option<ScreenArea> {
    work_area().or_else(|| {
        let (x, y) = monitor_position?;
        let (width, height) = monitor_size?;
        Some(ScreenArea {
            x,
            y,
            width,
            height,
        })
    })
}

#[cfg(windows)]
fn work_area() -> Option<ScreenArea> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETWORKAREA, SystemParametersInfoW};

    let mut rect: RECT = unsafe { std::mem::zeroed() };
    // SAFETY: `rect` 是可写输出参数；`fwinini = 0` 表示只读不写注册表。
    let ok =
        unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut rect as *mut RECT).cast(), 0) };
    if ok == 0 {
        return None;
    }
    let width = (rect.right - rect.left).max(0) as u32;
    let height = (rect.bottom - rect.top).max(0) as u32;
    (width > 0 && height > 0).then_some(ScreenArea {
        x: rect.left,
        y: rect.top,
        width,
        height,
    })
}

#[cfg(not(windows))]
fn platform_screen_area(
    monitor_position: Option<(i32, i32)>,
    monitor_size: Option<(u32, u32)>,
) -> Option<ScreenArea> {
    let (x, y) = monitor_position?;
    let (width, height) = monitor_size?;
    // 其他平台拿不到工作区：四周各让 5%，把菜单栏/任务栏近似扣掉。
    Some(ScreenArea {
        x: x + (width / 20) as i32,
        y: y + (height / 20) as i32,
        width: width.saturating_mul(9) / 10,
        height: height.saturating_mul(9) / 10,
    })
}

#[cfg(test)]
mod tests {
    use super::{InitialWindow, ScreenArea, fit_to_area, initial_window};

    fn area(width: u32, height: u32) -> ScreenArea {
        ScreenArea {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    fn inner(desired: (u32, u32), screen: ScreenArea, scale_factor: f64) -> (u32, u32) {
        initial_window(desired, Some(screen), scale_factor).inner_size
    }

    /// 1080p 桌面（Win11 工作区 1920×1032，任务栏 48 px）：720×1280 必须按高缩小，
    /// 且宽高比不变。
    #[test]
    fn portrait_design_shrinks_to_the_work_area() {
        let (width, height) = inner((720, 1280), area(1920, 1032), 1.0);
        // 1032 − 39（外框）− 32（边距）= 961。
        assert_eq!(height, 961);
        assert_eq!(width, 541);
        let ratio = f64::from(height) / f64::from(width);
        let design_ratio = 1280.0 / 720.0;
        assert!(
            (ratio - design_ratio).abs() < 0.01,
            "宽高比被改坏：{width}x{height}"
        );
    }

    /// 缩小后的窗口必须整块落在可用区里，并且居中。
    #[test]
    fn centers_the_fitted_window_inside_the_area() {
        let window = initial_window((720, 1280), Some(area(1920, 1032)), 1.0);
        let (x, y) = window.position.expect("有可用区就必须给落点");
        let outer = (window.inner_size.0 + 16, window.inner_size.1 + 39);
        assert!(x >= 0 && y >= 0, "落点跑到屏幕左上角外：{x},{y}");
        assert!(
            x as u32 + outer.0 <= 1920 && y as u32 + outer.1 <= 1032,
            "外框 {outer:?} 从 {x},{y} 越出可用区"
        );
        assert_eq!(
            (x, y),
            ((1920 - outer.0) as i32 / 2, (1032 - outer.1) as i32 / 2)
        );
    }

    /// 显示器足够大 = 原样返回，不放大（像素级 1:1），仍然居中。
    #[test]
    fn keeps_the_design_size_when_it_fits() {
        let window = initial_window((1280, 720), Some(area(1920, 1032)), 1.0);
        assert_eq!(window.inner_size, (1280, 720));
        assert_eq!(window.position, Some((312, 136)));
        assert_eq!(inner((720, 1280), area(3840, 2112), 1.0), (720, 1280));
    }

    /// 横版设计在小屏上同样只按比例缩：结果不许溢出可用区。
    #[test]
    fn never_overflows_the_area() {
        for available in [(800, 600), (1024, 768), (300, 2000), (2000, 300)] {
            let fitted = fit_to_area((720, 1280), available);
            assert!(
                fitted.0 <= available.0 && fitted.1 <= available.1,
                "{available:?} 溢出成 {fitted:?}"
            );
            assert!(fitted.0 >= 1 && fitted.1 >= 1, "{available:?} 给出 0");
        }
    }

    /// 退化输入（0 屏幕、拿不到显示器）不缩放也不摆位：0 可用区当"信息缺失"处理。
    #[test]
    fn survives_degenerate_inputs() {
        assert_eq!(
            initial_window((720, 1280), None, 1.0).inner_size,
            (720, 1280)
        );
        assert_eq!(initial_window((720, 1280), None, 1.0).position, None);
        assert_eq!(
            initial_window((720, 1280), Some(area(0, 0)), 1.0),
            InitialWindow {
                inner_size: (720, 1280),
                position: None,
            }
        );
        assert_eq!(fit_to_area((0, 0), (0, 0)), (1, 1));
        assert_eq!(fit_to_area((720, 1280), (0, 0)), (1, 1));
    }

    /// 高 DPI：外框与边距按缩放放大，客户区上限跟着变小。
    #[test]
    fn scales_the_frame_with_dpi() {
        // 200% DPI 的 1440p：1440 − 78（外框×2）− 64（边距×2）= 1298 ⇒ 装得下原尺寸。
        assert_eq!(inner((720, 1280), area(2560, 1440), 2.0), (720, 1280));
        // 工作区矮到 1378：1378 − 78 − 64 = 1236 ⇒ 缩到 1236 高、宽按比例 695。
        let (width, height) = inner((720, 1280), area(2560, 1378), 2.0);
        assert_eq!(height, 1236);
        assert_eq!(width, 695);
    }
}
