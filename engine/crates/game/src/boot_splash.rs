//! 启动图的第一帧：在 GPU 渲染器就绪之前，用 GDI 把它画进窗口客户区。
//!
//! 为什么需要这一层：`Renderer::new` 在 Windows 上实测 ≈2.5 s（临时打点读数，已撤：
//! `Instance::new` 1.88 s、adapter 0.28 s、surface configure 0.29 s、管线 10 ms）。窗口在
//! 这 2.5 s 之前就已经创建，但那时没有任何东西可画 —— 用户看到的是一片纯白的"等待
//! 空白屏"。启动图不该等 GPU：这里在窗口创建后立刻用 GDI 画同一张图，GPU 侧的
//! `splash.rs` 在首帧 `Presented` 之后接管。两层用同一张图、同一套 contain 几何、同一个
//! 清屏底色 ⇒ 交接时画面不跳。
//!
//! 为什么是"子窗口"而不是直接往主窗口 DC 上画：主窗口的客户区归 winit/wgpu 所有，
//! GDI 直接画上去会在下一次 `WM_PAINT`/`WM_ERASEBKGND`（winit 的背景刷子）被抹掉。
//! 一个覆盖客户区的子窗口有自己的 `WM_PAINT`，它不依赖消息泵就能被 DWM 合成出去 ——
//! 而这正是关键：`Renderer::new` 阻塞主线程的这 2.5 s 里没有消息泵。
//!
//! 平台：Windows 之外是 no-op（[`BootSplash::show`] 返回 `None`），行为与没有这一层
//! 逐字节一致。

use image::RgbaImage;

/// 窗口客户区里图片之外的底色：渲染器的默认清屏色是线性的
/// `rustlua_camera::camera::DEFAULT_3D_CLEAR` = (0.04, 0.07, 0.12)，画到 sRGB 附件上就是
/// 这三个字节。两层必须是同一个值，否则 contain 留边在交接时会闪一下。
pub(crate) const LETTERBOX_RGB: [u8; 3] = [56, 75, 97];

/// 由启动图与目标客户区算出 contain 矩形 `(x, y, w, h)`（像素，居中）。
///
/// 与 GPU 侧 `splash::quad`（`scale = min(w/iw, h/ih)`、居中、留边）是同一套几何：
/// 交接时图片在屏幕上的位置与大小必须一致，否则用户会看到一次"缩放跳变"。
pub(crate) fn contain(image: (u32, u32), target: (u32, u32)) -> (i32, i32, i32, i32) {
    let (image_width, image_height) = (image.0.max(1), image.1.max(1));
    let (target_width, target_height) = (target.0.max(1), target.1.max(1));
    let scale = (f64::from(target_width) / f64::from(image_width))
        .min(f64::from(target_height) / f64::from(image_height));
    let width =
        ((f64::from(image_width) * scale).round() as i64).clamp(1, i64::from(target_width)) as i32;
    let height = ((f64::from(image_height) * scale).round() as i64)
        .clamp(1, i64::from(target_height)) as i32;
    (
        (target_width as i32 - width) / 2,
        (target_height as i32 - height) / 2,
        width,
        height,
    )
}

/// RGBA → 32 位 DIB 需要的 BGRA，并把半透明像素按 `background` 合成掉。
///
/// GDI 的 `StretchBlt` 不看 alpha 通道，直接把 32 位值当 RGB 画；启动图若有透明像素，
/// 不合成就会露出 PNG 里那些通常是黑色/垃圾的 RGB。GPU 侧是 alpha 混合到清屏色上，
/// 这里在 sRGB 空间里做同一件事（差 1 LSB，交接时看不出来）。
pub(crate) fn bgra_over(image: &RgbaImage, background: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.as_raw().len());
    for pixel in image.as_raw().as_chunks::<4>().0 {
        let alpha = u32::from(pixel[3]);
        let blend = |channel: u8, background: u8| -> u8 {
            ((u32::from(channel) * alpha + u32::from(background) * (255 - alpha) + 127) / 255) as u8
        };
        out.extend_from_slice(&[
            blend(pixel[2], background[2]),
            blend(pixel[1], background[1]),
            blend(pixel[0], background[0]),
            255,
        ]);
    }
    out
}

/// 早期启动图。`Drop` 即撤下（窗口销毁、GDI 对象释放）。
pub struct BootSplash {
    #[cfg(windows)]
    hwnd: windows_sys::Win32::Foundation::HWND,
    #[cfg(not(windows))]
    _private: (),
}

impl BootSplash {
    /// 把 `image` 画到 `window` 客户区的最上层（同步完成，返回时已经在屏幕上）。
    ///
    /// 返回 `None` 表示这一层不可用（非 Windows、拿不到 `HWND`、GDI 资源分配失败）：
    /// 调用方按"没有早期启动图"继续，GPU 侧启动图照旧。
    pub fn show(window: &winit::window::Window, image: &RgbaImage) -> Option<Self> {
        #[cfg(windows)]
        {
            imp::show(window, image)
        }
        #[cfg(not(windows))]
        {
            let _ = (window, image);
            None
        }
    }
}

#[cfg(not(windows))]
impl Drop for BootSplash {
    fn drop(&mut self) {}
}

#[cfg(windows)]
impl Drop for BootSplash {
    fn drop(&mut self) {
        imp::destroy(self.hwnd);
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::Once;
    use std::sync::atomic::{AtomicBool, Ordering};

    use image::RgbaImage;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, CreateCompatibleDC, CreateDIBSection,
        CreateSolidBrush, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint, FillRect, HALFTONE,
        HDC, HGDIOBJ, InvalidateRect, PAINTSTRUCT, SRCCOPY, SelectObject, SetBrushOrgEx,
        SetStretchBltMode, StretchBlt, UpdateWindow,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA,
        GetClientRect, GetWindowLongPtrW, RegisterClassW, SetWindowLongPtrW, WM_ERASEBKGND,
        WM_PAINT, WNDCLASSW, WS_CHILD, WS_VISIBLE,
    };

    use super::{LETTERBOX_RGB, bgra_over, contain};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    const CLASS_NAME: &str = "RustLuaBootSplash";

    static REGISTER: Once = Once::new();
    static REGISTERED: AtomicBool = AtomicBool::new(false);

    /// 一次性 GDI 资源：位图 + 与之配对的 memory DC。窗口过程只读它。
    struct Paint {
        image: HDC,
        bitmap: HGDIOBJ,
        previous: HGDIOBJ,
        width: i32,
        height: i32,
    }

    impl Paint {
        fn paint(&self, hwnd: HWND, dc: HDC) {
            unsafe {
                let mut client: RECT = std::mem::zeroed();
                if GetClientRect(hwnd, &mut client) == 0 {
                    return;
                }
                let brush = CreateSolidBrush(colorref(LETTERBOX_RGB));
                if !brush.is_null() {
                    FillRect(dc, &client, brush);
                    DeleteObject(brush);
                }
                let (x, y, width, height) = contain(
                    (self.width as u32, self.height as u32),
                    (
                        (client.right - client.left).max(0) as u32,
                        (client.bottom - client.top).max(0) as u32,
                    ),
                );
                if width <= 0 || height <= 0 {
                    return;
                }
                // HALFTONE：客户区被拉大（用户拖窗口）时别用粗糙的 stretching 模式。
                SetStretchBltMode(dc, HALFTONE);
                SetBrushOrgEx(dc, 0, 0, std::ptr::null_mut());
                StretchBlt(
                    dc,
                    x,
                    y,
                    width,
                    height,
                    self.image,
                    0,
                    0,
                    self.width,
                    self.height,
                    SRCCOPY,
                );
            }
        }
    }

    impl Drop for Paint {
        fn drop(&mut self) {
            unsafe {
                if !self.previous.is_null() {
                    SelectObject(self.image, self.previous);
                }
                if !self.bitmap.is_null() {
                    DeleteObject(self.bitmap);
                }
                if !self.image.is_null() {
                    DeleteDC(self.image);
                }
            }
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_PAINT => {
                let mut paint_struct: PAINTSTRUCT = unsafe { std::mem::zeroed() };
                let dc = unsafe { BeginPaint(hwnd, &mut paint_struct) };
                let paint = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Paint };
                if !paint.is_null() && !dc.is_null() {
                    unsafe { (*paint).paint(hwnd, dc) };
                }
                unsafe { EndPaint(hwnd, &paint_struct) };
                0
            }
            // 整块客户区都由 WM_PAINT 负责：不要让系统再刷一次背景。
            WM_ERASEBKGND => 1,
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn colorref(rgb: [u8; 3]) -> u32 {
        u32::from(rgb[0]) | (u32::from(rgb[1]) << 8) | (u32::from(rgb[2]) << 16)
    }

    fn register_class() -> bool {
        REGISTER.call_once(|| {
            let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
            let class_name = wide(CLASS_NAME);
            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: std::ptr::null_mut(),
                // 空光标：DefWindowProc 会把它交给父窗口（winit 负责设置箭头）。
                hCursor: std::ptr::null_mut(),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class_name.as_ptr(),
            };
            let ok = unsafe { RegisterClassW(&class) } != 0;
            REGISTERED.store(ok, Ordering::Relaxed);
        });
        REGISTERED.load(Ordering::Relaxed)
    }

    pub fn show(window: &winit::window::Window, image: &RgbaImage) -> Option<super::BootSplash> {
        if !register_class() {
            tracing::warn!(target: "game", "[boot] 启动图窗口类注册失败；本层跳过");
            return None;
        }
        let Ok(handle) = window.window_handle() else {
            tracing::warn!(target: "game", "[boot] 窗口句柄不可用；本层跳过");
            return None;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return None;
        };
        let parent = handle.hwnd.get() as HWND;
        let (image_width, image_height) = image.dimensions();
        if image_width == 0 || image_height == 0 {
            return None;
        }
        let mut client: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetClientRect(parent, &mut client) } == 0 {
            return None;
        }
        let (width, height) = (client.right - client.left, client.bottom - client.top);
        if width <= 0 || height <= 0 {
            return None;
        }
        let pixels = bgra_over(image, LETTERBOX_RGB);
        let started = std::time::Instant::now();
        unsafe {
            let memory_dc = CreateCompatibleDC(std::ptr::null_mut());
            if memory_dc.is_null() {
                return None;
            }
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: image_width as i32,
                // 负高度 = 自上而下的行序，正好是 PNG 解码出来的顺序。
                biHeight: -(image_height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(
                memory_dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if bitmap.is_null() || bits.is_null() {
                DeleteDC(memory_dc);
                return None;
            }
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
            let previous = SelectObject(memory_dc, bitmap);
            let paint = Box::into_raw(Box::new(Paint {
                image: memory_dc,
                bitmap,
                previous,
                width: image_width as i32,
                height: image_height as i32,
            }));
            let instance = GetModuleHandleW(std::ptr::null());
            let hwnd = CreateWindowExW(
                0,
                wide(CLASS_NAME).as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                width,
                height,
                parent,
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                drop(Box::from_raw(paint));
                tracing::warn!(target: "game", "[boot] 启动图子窗口创建失败；本层跳过");
                return None;
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, paint as isize);
            // 同步画一次：这里之后主线程就要阻塞在 `Renderer::new` 里了，
            // 消息泵不会转，千万别指望 WM_PAINT 自己来。
            InvalidateRect(hwnd, std::ptr::null(), 0);
            UpdateWindow(hwnd);
            tracing::info!(
                target: "game",
                "[boot] splash painted before GPU init: window={}x{} image={}x{} elapsed_ms={:.1}",
                width,
                height,
                image_width,
                image_height,
                started.elapsed().as_secs_f64() * 1000.0
            );
            Some(super::BootSplash { hwnd })
        }
    }

    pub fn destroy(hwnd: HWND) {
        unsafe {
            let paint = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Paint;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DestroyWindow(hwnd);
            if !paint.is_null() {
                drop(Box::from_raw(paint));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_matches_gpu_geometry_and_never_overflows() {
        // 与 GPU 侧 quad 同一个口径：等比、居中、留边在两轴之和恰为 1。
        let (x, y, width, height) = contain((1280, 720), (1280, 720));
        assert_eq!((x, y, width, height), (0, 0, 1280, 720));

        // 客户区更大：图片按宽/高的较小缩放比放大并居中。
        let (x, y, width, height) = contain((1280, 720), (1920, 1080));
        assert_eq!((width, height), (1920, 1080));
        assert_eq!((x, y), (0, 0));

        // 竖向留边：1280x720 的图放进 1000x1000 的客户区，宽撑满、上下各留 218~219 px。
        let (x, y, width, height) = contain((1280, 720), (1000, 1000));
        assert_eq!((x, width), (0, 1000));
        assert_eq!(height, 563);
        assert_eq!(y, 218);
        assert!(y * 2 + height <= 1000);

        // 横向留边：竖图放进横窗口。
        let (x, y, width, height) = contain((720, 1280), (1280, 720));
        assert_eq!((y, height), (0, 720));
        assert_eq!(width, 405);
        assert_eq!(x, 437);

        // 退化输入不产生 0 面积/越界矩形。
        for (image, target) in [
            ((0, 0), (0, 0)),
            ((1, 1), (1, 1)),
            ((4096, 1), (3, 3)),
            ((1, 4096), (3, 3)),
        ] {
            let (x, y, width, height) = contain(image, target);
            assert!(width >= 1 && height >= 1, "{image:?} -> {width}x{height}");
            assert!(x >= 0 && y >= 0, "{image:?} -> {x},{y}");
            assert!(
                x + width <= target.0.max(1) as i32 && y + height <= target.1.max(1) as i32,
                "{image:?} in {target:?} -> {x},{y} {width}x{height}"
            );
        }
    }

    #[test]
    fn bgra_over_swaps_channels_and_composites_alpha() {
        let mut image = RgbaImage::new(3, 1);
        image.put_pixel(0, 0, image::Rgba([1, 2, 3, 255]));
        image.put_pixel(1, 0, image::Rgba([255, 0, 0, 128]));
        image.put_pixel(2, 0, image::Rgba([9, 9, 9, 0]));
        let bytes = bgra_over(&image, [56, 75, 97]);
        assert_eq!(&bytes[0..4], &[3, 2, 1, 255], "不透明像素：RGBA → BGRA");
        // 半透明红合成到 (56,75,97) 上：R≈156、G≈37、B≈48。
        assert_eq!(&bytes[4..8], &[48, 37, 156, 255]);
        // 全透明像素退化成留边底色。
        assert_eq!(&bytes[8..12], &[97, 75, 56, 255]);
    }
}
