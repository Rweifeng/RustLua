//! 设备模拟（M??-04）：把 Game 视图预览补成"像真机"的那一半。
//!
//! 证据（缺失项）：分辨率预设已存在（`main.rs` 的 `preview_resolutions`），安全区只有开/关
//! （`menu.rs` 的「安全区」）。缺的是：**按机型内缩的安全区（刘海/底部条）、触摸与虚拟摇杆
//! 模拟、DPI 与 UI 缩放档、竖屏方向锁定**。
//!
//! 本模块只放**数据与纯逻辑**（可单测的 golden 部分）：
//! * [`DevicePreset`] / [`DEVICE_PRESETS`]：机型预设 —— 设计分辨率 + 显示缩放档(DPI) +
//!   安全区四边内缩 + 竖屏方向。全部以**设计像素**口径给出（与引擎 Canvas 一致；
//!   显示缩放档是读数语义：该机型上每设计像素对应多少设备像素，不改变渲染目标）。
//! * [`DeviceSimState`]：运行态（机型下标 + 三个开关），随布局持久化（见
//!   `rustlua-editor-core::layout::DeviceSimLayout`）。
//! * [`StickState`] / [`stick_key`]：编辑器侧虚拟摇杆的跨帧状态机。摇杆输出的是
//!   **虚拟物理键**（`KeyW/KeyS/KeyA/KeyD`），与键盘、真机 `virtual_pad.lua`
//!   （`DIR_KEY = {up=keyw,…}`）走同一个动作层（`InputSnapshot::virtual_keys` 并入
//!   `action_is_down`/`action_axis`），因此「摇杆动作映射 == 键盘动作映射」是构造保证，
//!   并由 [`stick_output_matches_keyboard_actions`] 用 golden 锁死。
//!
//! 方向判定镜像真机 `virtual_pad.lua`：逐轴阈值（死区 × 全行程）、对角 = 两轴同真、
//! 抓取用按下位置、跟随用实时光标。切边沿 = `set_virtual_pressed`（`action_pressed` 系
//! 能吃到）+ `set_virtual_key(true)`（按住态跨帧沿用），抬手 = `set_virtual_key(false)`。

use rustlua_input::input::{InputSnapshot, KeyCode};

/// 单台设备的模拟参数（设计像素口径）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DevicePreset {
    /// 显示名（工具栏下拉 / 窗口菜单 / Game 视图 HUD 读数）。
    pub(crate) name: &'static str,
    /// 设计分辨率（宽×高）：Game 视图渲染目标 / UI 参考分辨率。
    pub(crate) resolution: (u32, u32),
    /// 显示缩放档：设备物理像素 / 设计像素（1x/2x/3x…）。只读读数，不改渲染目标。
    pub(crate) dpi: f32,
    /// 安全区四边内缩 [left, top, right, bottom]（设计像素）。
    pub(crate) safe_area: [f32; 4],
    /// 竖屏方向锁定：true = 预览分辨率保持竖屏（宽≤高），切换横屏比例也会被翻回竖屏。
    pub(crate) portrait: bool,
}

impl DevicePreset {
    /// 该机型的显示分辨率（设计分辨率 × 显示缩放档）。只读读数，供 HUD/文档参考；
    /// 仅测试与演示使用（HUD 常驻行读 DPI 百分比，不占宽度）。
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn display_resolution(self) -> (u32, u32) {
        (
            ((self.resolution.0 as f32) * self.dpi).round() as u32,
            ((self.resolution.1 as f32) * self.dpi).round() as u32,
        )
    }
}

/// 内置机型表（值为公开机型数据的**参考值**，取整到设计像素口径）。
///
/// 覆盖验收要的几种形态：iPhone 刘海 + 底部条、Android 顶部挖孔 + 手势条、
/// 微信小游戏竖/横屏（dpi=1：设计=渲染，安全区取 `getSafeArea` 常见值）、
/// iPad 横屏无内缩。安全区内缩默认 0 的机型就是"没有刘海"的诚实表达。
pub(crate) const DEVICE_PRESETS: &[DevicePreset] = &[
    DevicePreset {
        name: "iPhone 15",
        resolution: (393, 852),
        dpi: 3.0,
        safe_area: [0.0, 59.0, 0.0, 34.0],
        portrait: true,
    },
    DevicePreset {
        name: "iPhone 13 mini",
        resolution: (375, 812),
        dpi: 3.0,
        safe_area: [0.0, 47.0, 0.0, 34.0],
        portrait: true,
    },
    DevicePreset {
        name: "Pixel 8",
        resolution: (412, 915),
        dpi: 2.6,
        safe_area: [0.0, 24.0, 0.0, 24.0],
        portrait: true,
    },
    DevicePreset {
        name: "微信小游戏 竖屏",
        resolution: (720, 1280),
        dpi: 1.0,
        safe_area: [0.0, 80.0, 0.0, 40.0],
        portrait: true,
    },
    DevicePreset {
        name: "微信小游戏 横屏",
        resolution: (1280, 720),
        dpi: 1.0,
        safe_area: [0.0, 0.0, 0.0, 0.0],
        portrait: false,
    },
    DevicePreset {
        name: "iPad mini 横屏",
        resolution: (1133, 744),
        dpi: 2.0,
        safe_area: [0.0, 0.0, 0.0, 0.0],
        portrait: false,
    },
];

/// 设备模拟运行态（编辑器本地；随布局持久化）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DeviceSimState {
    /// 选中机型在 [`DEVICE_PRESETS`] 的下标；`None` = 自定义（不模拟）。
    pub(crate) preset: Option<usize>,
    /// 触摸模拟：鼠标左键按下 → press cursor / pointer_count（多指语义）。
    pub(crate) touch_sim: bool,
    /// 虚拟摇杆：Game 视图左下角可拖摇杆，输出与键盘同一动作层的虚拟键。
    pub(crate) joystick: bool,
    /// 竖屏方向锁定：切换分辨率时保持竖屏（宽≤高）。随竖屏预设自动开启。
    pub(crate) lock_portrait: bool,
}

impl Default for DeviceSimState {
    fn default() -> Self {
        Self {
            preset: None,
            touch_sim: true,
            joystick: true,
            lock_portrait: false,
        }
    }
}

impl DeviceSimState {
    /// 当前生效的机型预设。
    #[must_use]
    pub(crate) fn active(&self) -> Option<&'static DevicePreset> {
        self.preset.and_then(|index| DEVICE_PRESETS.get(index))
    }

    /// 显示缩放档：未模拟时 = 1.0（读数为"每设计像素 = 1 设备像素"）。
    #[must_use]
    pub(crate) fn dpi(&self) -> f32 {
        self.active().map_or(1.0, |preset| preset.dpi)
    }

    /// 生效安全区：设备模拟开启时用机型内缩（按设计分辨率钳制，负值归零），
    /// 否则保留调用方给的场景配置值。
    #[must_use]
    pub(crate) fn effective_safe_area(&self, scene_safe: [f32; 4], design: (u32, u32)) -> [f32; 4] {
        let Some(preset) = self.active() else {
            return scene_safe;
        };
        let (w, h) = (design.0 as f32, design.1 as f32);
        [
            preset.safe_area[0].clamp(0.0, w),
            preset.safe_area[1].clamp(0.0, h),
            preset.safe_area[2].clamp(0.0, w),
            preset.safe_area[3].clamp(0.0, h),
        ]
    }

    /// 应用竖屏方向锁定：锁定中且目标是横屏（宽>高）时翻转为竖屏（保持尺寸对）。
    #[must_use]
    pub(crate) fn clamp_resolution(&self, resolution: (u32, u32)) -> (u32, u32) {
        if self.lock_portrait && resolution.0 > resolution.1 {
            (resolution.1, resolution.0)
        } else {
            resolution
        }
    }

    /// 从布局还原：钳制机型下标（预设表增删后旧布局仍可加载），`None` = 未启用。
    pub(crate) fn apply_layout(
        &mut self,
        layout: Option<&rustlua_editor_core::layout::DeviceSimLayout>,
    ) {
        match layout {
            None => *self = Self::default(),
            Some(layout) => {
                self.preset = layout
                    .preset
                    .map(|index| index as usize)
                    .filter(|index| *index < DEVICE_PRESETS.len());
                self.touch_sim = layout.touch_sim;
                self.joystick = layout.joystick;
                self.lock_portrait = layout.lock_portrait;
            }
        }
    }
}

/// 虚拟摇杆的方向键（引擎 KeyCode）：与键盘 WASD、真机 `virtual_pad.lua` 的
/// `DIR_KEY`（up=keyw / down=keys / left=keya / right=keyd）完全一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StickDir {
    Up,
    Down,
    Left,
    Right,
}

/// 摇杆方向 → 注入的虚拟物理键。
#[must_use]
pub(crate) const fn stick_key(dir: StickDir) -> KeyCode {
    match dir {
        StickDir::Up => KeyCode::KeyW,
        StickDir::Down => KeyCode::KeyS,
        StickDir::Left => KeyCode::KeyA,
        StickDir::Right => KeyCode::KeyD,
    }
}

/// 本帧需要对摇杆输出的一个写入动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StickStep {
    /// 方向边沿 + 按住态（镜像真机 `button_press`：`virtual_pressed` + `virtual_key(true)`）。
    Press(KeyCode),
    /// 释放（`virtual_key(false)`）。
    Release(KeyCode),
}

/// 摇杆跨帧状态机：只在**方向变化**时输出写入（按住态由引擎在帧边界沿用，
/// 见真机 `virtual_pad.lua` 对 `virtual_key` 的注释；`set_virtual_key` 不产生边沿，
/// 边沿只靠 [`StickStep::Press`] 里的 `virtual_pressed` 补一次）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StickState {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
}

impl StickState {
    /// 按真机算法推进一步。
    ///
    /// * `active`：摇杆是否被抓着（按下且在圆内）；
    /// * `(nx, ny)`：杆头位移**归一化**到全行程 [-1, 1]（y 向下为正）；
    /// * `deadzone`：方向阈值 = 死区 × 全行程（真机 `STICK_DEADZONE` 同口径）。
    ///
    /// 返回本帧要写入的虚拟键动作（空 = 无变化，无需重发）。
    pub(crate) fn update(
        &mut self,
        active: bool,
        nx: f32,
        ny: f32,
        deadzone: f32,
    ) -> Vec<StickStep> {
        // 释放 / 失焦 / 回中：全部松开（回中时按阈值归零方向，与真机 set_dir 同语义）。
        let mut dirs = [false; 4];
        if active {
            let threshold = deadzone.max(0.0);
            dirs[0] = ny < -threshold; // up：y 负
            dirs[1] = ny > threshold; // down
            dirs[2] = nx < -threshold; // left
            dirs[3] = nx > threshold; // right
        }
        let mut steps = Vec::with_capacity(4);
        for (index, dir) in [
            StickDir::Up,
            StickDir::Down,
            StickDir::Left,
            StickDir::Right,
        ]
        .into_iter()
        .enumerate()
        {
            let now = dirs[index];
            let before = match dir {
                StickDir::Up => self.up,
                StickDir::Down => self.down,
                StickDir::Left => self.left,
                StickDir::Right => self.right,
            };
            if now == before {
                continue;
            }
            match dir {
                StickDir::Up => self.up = now,
                StickDir::Down => self.down = now,
                StickDir::Left => self.left = now,
                StickDir::Right => self.right = now,
            }
            steps.push(if now {
                StickStep::Press(stick_key(dir))
            } else {
                StickStep::Release(stick_key(dir))
            });
        }
        steps
    }

    /// 强制释放全部方向（失焦 / Stop / 切到非 Play）。返回需要注入的释放动作。
    pub(crate) fn release_all(&mut self) -> Vec<StickStep> {
        self.update(false, 0.0, 0.0, 0.0)
    }
}

/// 把 [`StickStep`] 写进输入快照（与真机 `button_press`/`button_release` 同注入口径）。
pub(crate) fn apply_stick_steps(input: &mut InputSnapshot, steps: &[StickStep]) {
    for step in steps {
        match *step {
            StickStep::Press(key) => {
                input.set_virtual_pressed(key);
                input.set_virtual_key(key, true);
            }
            StickStep::Release(key) => input.set_virtual_key(key, false),
        }
    }
}

/// 触摸模拟：把鼠标按下位置写成按下点（`press_position` / `pointer_count`）。
/// `touching=false` 时清空多指槽（`pointer_count = 0`），不放任何命中。
pub(crate) fn bridge_touch(input: &mut InputSnapshot, touching: bool, x: f64, y: f64) {
    if touching {
        input.press_cursor_moved(x, y);
        input.set_press_cursors(&[(x, y)]);
    } else {
        input.set_press_cursors(&[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_input::input::{ActionMap, Binding};

    fn map() -> ActionMap {
        ActionMap::with_defaults()
    }

    #[test]
    fn presets_are_well_formed_and_unique() {
        assert!(!DEVICE_PRESETS.is_empty());
        let mut names = std::collections::BTreeSet::new();
        for preset in DEVICE_PRESETS {
            assert!(!preset.name.trim().is_empty(), "机型名不能为空");
            assert!(names.insert(preset.name), "机型名重复：{}", preset.name);
            assert!(
                preset.resolution.0 > 0 && preset.resolution.1 > 0,
                "{} 分辨率非法",
                preset.name
            );
            assert!(preset.dpi > 0.0, "{} 缩放档非法", preset.name);
            assert!(
                preset.safe_area.iter().all(|v| *v >= 0.0),
                "{} 安全区不能为负",
                preset.name
            );
        }
    }

    /// 验收门 ①：切换预设后，设计分辨率 / 显示缩放档 / 安全区四边内缩三项读数一起变。
    #[test]
    fn switching_preset_changes_all_three_readings() {
        let mut state = DeviceSimState::default();
        assert_eq!(state.dpi(), 1.0);
        assert_eq!(state.active(), None);

        state.preset = Some(0); // iPhone 15
        let preset = state.active().expect("预设 0 存在");
        assert_eq!(preset.resolution, (393, 852));
        assert_eq!(state.dpi(), 3.0);
        assert_eq!(preset.safe_area, [0.0, 59.0, 0.0, 34.0]);
        assert!(preset.portrait);

        // 安全区按设计分辨率钳制（机型内缩不会越出设计框）。
        let safe = state.effective_safe_area([0.0; 4], (393, 852));
        assert_eq!(safe, [0.0, 59.0, 0.0, 34.0]);
        let clipped = state.effective_safe_area([0.0; 4], (300, 400));
        let clamped: [f32; 4] = [0.0_f32, 59.0, 0.0, 34.0].map(|v| v.min(300.0).min(400.0));
        assert_eq!(clipped, clamped);

        // 未模拟：回落到场景配置（不改显示口径）。
        state.preset = None;
        assert_eq!(
            state.effective_safe_area([1.0, 2.0, 3.0, 4.0], (100, 100)),
            [1.0, 2.0, 3.0, 4.0]
        );
        assert_eq!(state.dpi(), 1.0);
    }

    /// 竖屏方向锁定：锁竖屏时横屏目标被翻回竖屏，竖屏目标原样通过。
    #[test]
    fn portrait_lock_keeps_resolution_portrait() {
        let state = DeviceSimState {
            preset: Some(0),
            touch_sim: true,
            joystick: true,
            lock_portrait: true,
        };
        assert_eq!(state.clamp_resolution((852, 480)), (480, 852));
        assert_eq!(state.clamp_resolution((480, 852)), (480, 852));
        assert_eq!(state.clamp_resolution((1280, 720)), (720, 1280));

        let unlocked = DeviceSimState {
            lock_portrait: false,
            ..Default::default()
        };
        assert_eq!(unlocked.clamp_resolution((1280, 720)), (1280, 720));
    }

    /// 布局还原：预设下标越界（表增删后旧布局）安全回退为不模拟。
    #[test]
    fn apply_layout_clamps_stale_preset_index() {
        let mut state = DeviceSimState::default();
        state.apply_layout(Some(&rustlua_editor_core::layout::DeviceSimLayout {
            preset: Some(usize::MAX as u32),
            touch_sim: false,
            joystick: false,
            lock_portrait: true,
        }));
        assert_eq!(state.preset, None);
        assert!(!state.touch_sim && !state.joystick && state.lock_portrait);

        state.apply_layout(None);
        assert_eq!(state, DeviceSimState::default());
    }

    /// 摇杆状态机：只在方向变化时输出写入；回中/松开统一释放。
    #[test]
    fn stick_state_only_emits_on_direction_change() {
        let mut stick = StickState::default();
        // 推向正上：一次 Press(KeyW)，随后无变化（按住态由引擎沿用）。
        assert_eq!(
            stick.update(true, 0.0, -1.0, 0.25),
            vec![StickStep::Press(KeyCode::KeyW)]
        );
        assert!(stick.update(true, 0.0, -1.0, 0.25).is_empty());
        // 从"只按上"切到"上+右"对角：只有 Right 是新边沿，Up 按住不重发。
        let steps = stick.update(true, 1.0, -1.0, 0.25);
        assert_eq!(steps, vec![StickStep::Press(KeyCode::KeyD)]);
        // 死区内回中：释放全部（此时按住的是 W 与 D 两颗键）。
        let steps = stick.update(true, 0.1, 0.1, 0.25);
        assert_eq!(steps.len(), 2, "W/D 释放；已松的 S/A 不重复");
        assert!(steps.iter().all(|s| matches!(s, StickStep::Release(_))));
        // 松开摇杆：无新输出（方向已全松）。
        assert!(stick.update(false, 0.0, 0.0, 0.25).is_empty());
    }

    /// 失焦/Stop：强制释放，且 idempotent（第二次无输出）。
    #[test]
    fn stick_release_all_is_idempotent() {
        let mut stick = StickState::default();
        stick.update(true, 1.0, 1.0, 0.25);
        let steps = stick.release_all();
        assert_eq!(steps.len(), 2);
        assert!(stick.release_all().is_empty());
    }

    /// 验收门 ②（golden）：虚拟摇杆输出的动作映射与键盘一致。
    ///
    /// 键盘 W/A/S/D → `move_up/move_left/move_down/move_right`；
    /// 摇杆上/左/下/右 → 同一组动作、同一批 KeyCode（默认 ActionMap 绑定），
    /// 且 `action_is_down` 结果逐对相等。对角 = 键盘两键同按 = 摇杆两轴同真。
    #[test]
    fn stick_output_matches_keyboard_actions() {
        let map = map();
        let pairs = [
            (StickDir::Up, KeyCode::KeyW, "move_up"),
            (StickDir::Down, KeyCode::KeyS, "move_down"),
            (StickDir::Left, KeyCode::KeyA, "move_left"),
            (StickDir::Right, KeyCode::KeyD, "move_right"),
        ];
        for (dir, key, action) in pairs {
            // 默认绑定里 move_* 就是这颗键（engine 侧已有断言，这里锁编辑器侧映射）。
            assert_eq!(map.bindings(action), [Binding::Key(key)]);
            assert_eq!(stick_key(dir), key, "{dir:?} 应注入与键盘相同的键");

            let mut from_keyboard = InputSnapshot::default();
            from_keyboard.key_event(key, true);
            let mut from_stick = InputSnapshot::default();
            apply_stick_steps(&mut from_stick, &[StickStep::Press(stick_key(dir))]);

            assert!(from_stick.action_is_down(&map, action));
            assert_eq!(
                from_stick.action_is_down(&map, action),
                from_keyboard.action_is_down(&map, action)
            );
        }

        // 对角：摇杆（上+右）与键盘（W+D）逐动作相等，互不干扰对方轴。
        let mut stick = StickState::default();
        let steps = stick.update(true, 1.0, -1.0, 0.25);
        assert_eq!(steps.len(), 2);
        let mut from_stick = InputSnapshot::default();
        apply_stick_steps(&mut from_stick, &steps);
        let mut from_keyboard = InputSnapshot::default();
        from_keyboard.key_event(KeyCode::KeyW, true);
        from_keyboard.key_event(KeyCode::KeyD, true);
        for action in ["move_up", "move_right", "move_left", "move_down"] {
            assert_eq!(
                from_stick.action_is_down(&map, action),
                from_keyboard.action_is_down(&map, action)
            );
        }
    }

    /// 触摸桥：按下写入按下点与 pointer_count；抬起清空多指槽。
    #[test]
    fn touch_bridge_writes_and_clears_press_cursors() {
        let mut input = InputSnapshot::default();
        assert_eq!(input.pointer_count(), 0_u8);
        bridge_touch(&mut input, true, 120.0, 340.0);
        assert_eq!(input.pointer_count(), 1_u8);
        assert_eq!(input.press_position(None), Some((120.0, 340.0)));
        assert_eq!(input.press_position(Some(0)), Some((120.0, 340.0)));
        assert_eq!(input.press_position(Some(1)), None, "越界不得冒充主按下点");

        bridge_touch(&mut input, false, 0.0, 0.0);
        assert_eq!(input.pointer_count(), 0_u8);
        assert_eq!(input.press_position(Some(0)), None);
    }

    /// 显示分辨率读数（设计 × 缩放档）只读，不参与任何渲染决策。
    #[test]
    fn display_resolution_is_a_reading() {
        let preset = &DEVICE_PRESETS[0];
        assert_eq!(preset.display_resolution(), (1179, 2556));
    }
}
