//! 启动状态属性（`data-rustlua-boot`）的**归属判据**。
//!
//! 这个属性以前只有 JS 世界写（`web-runtime.js` 的 `markBoot`）：JS 世界删掉之后它就没有写者了，
//! 而**诊断与回归门读的正是它**。于是宿主（浏览器 `gpu_host`）与微信适配层
//! （`wechat-adapter.js` 的 `publishBootOk`，同形孪生）各自补上成功态。
//!
//! 判据只有一条：**只补不盖**。`failed` 绝不能变成 `ok` —— 否则"启动真失败"会被读成成功，而
//! 这个属性正是失败诊断的入口。**这就是本模块唯一存在的理由**，所以它的单测就是那条负例。
//!
//! 门是 `any(target_arch = "wasm32", test)`（同 `host_config` / `fit`）：纯判据、不碰 `web_sys`，
//! 桌面单测据此守住负例；原生非测试构建里它没有消费者，不门就成了死代码。

/// 宿主该不该把 `data-rustlua-boot` 写成 `ok`；`None` = 不写。
///
/// * `Some("failed")` → 不写：**这条是全部意义所在**；
/// * `Some("ok")` → 不写：已是终态，省一次 DOM 写；
/// * 其它（`None` / `Some("running")` / …）→ 写 `ok`。
#[must_use]
pub fn boot_state_to_publish(current: Option<&str>) -> Option<&'static str> {
    match current {
        Some("failed") | Some("ok") => None,
        _ => Some("ok"),
    }
}

#[cfg(test)]
mod tests {
    use super::boot_state_to_publish;

    #[test]
    fn boot_state_never_clobbers_failed() {
        assert_eq!(
            boot_state_to_publish(Some("failed")),
            None,
            "failed 必须原样留着"
        );
        assert_eq!(boot_state_to_publish(Some("ok")), None, "已是 ok 就不重写");
        assert_eq!(
            boot_state_to_publish(Some("running")),
            Some("ok"),
            "进行中才由宿主补 ok"
        );
        assert_eq!(
            boot_state_to_publish(None),
            Some("ok"),
            "没人写过时宿主就是写者"
        );
    }
}
