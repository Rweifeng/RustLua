//! M74-01 的 headless 门：空间音频的**模型**与**平台契约**。
//!
//! ## 为什么这条门可以不碰声卡
//!
//! 空间化的决策是"**模型权威、平台尽力**"：听感只由 `rustlua_audio::spatial::evaluate` 决定
//! （纯函数），平台只是把 `gain`/`pan` 应用出去。所以本门断言的是数字，不是波形：
//! `gain`/`pan` 在哪一档该是多少、非法参数会不会被拒、折回位置会不会让 rodio 二次衰减。
//! 真机上"左边那个声音确实偏左了"仍要人耳验（见任务书 §验收方式），但**数值口径**在这里钉死。
//!
//! ## 五段账
//!
//! ① 模型四档：同点 / 正前方 / 正右 / 超距——`gain` 与 `pan` 的唯一定义处。
//! ② `min_distance` 内不衰减（2D 玩法里"脚下那口锅"不该比"三米外的鸡"更响）。
//! ③ 非法输入一律**拒绝**（NaN / 模型反了 / 朝向退化）：静默夹紧会让"我调了参数但声音没变"
//!    变成无法定位的问题。
//! ④ 折回位置（`fold_to_ears`）必须满足 rodio 的模型前提：到两耳距离 < 1 m（否则 rodio 的
//!    `1/dist²` 会二次衰减，桌面与 web 听感就分叉了）、两耳不重合（rodio 里有
//!    `debug_assert!(left_ear != right_ear)`）、声像方向与 `pan` 同号。
//! ⑤ 宿主契约：没接线时 `play_at` 拒绝；接线后缺文件仍然是 `Io`（不是静默成功）；
//!    非法模型当场拒绝且不覆盖旧值。
//!
//! Usage:
//! ```text
//! cargo run -p game --example jhyt_m93_spatial_audio
//! ```

use rustlua_audio::audio::{AudioError, AudioSystem};
use rustlua_audio::spatial::{
    Listener, SpatialError, SpatialModel, distance, evaluate, fold_to_ears,
};

fn banner(title: &str) {
    println!("\n=== {title} ===");
}

/// 断言 + 打印：读数是"证据"，失败是"门"。
fn check(what: &str, ok: bool) {
    println!("  [{}] {what}", if ok { "ok" } else { "FAIL" });
    assert!(ok, "{what}");
}

fn near(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 听者：原点、朝 -Z、上 +Y ⇒ 右轴 = +X。
    let listener = Listener::at_origin();
    let model = SpatialModel::DEFAULT;
    println!("listener = {:?}", listener);
    println!("model = {:?}", model);

    banner("① 模型四档（增益与声像的唯一定义处）");
    let same = evaluate([0.0, 0.0, 0.0], &listener, model)?;
    println!("  同点      gain={:.4} pan={:+.4}", same.gain, same.pan);
    check("同点：满增益", near(same.gain, 1.0, 1e-6));
    check(
        "同点：声像居中（方向无定义，不产生 NaN）",
        same.pan == 0.0 && same.gain.is_finite(),
    );

    let ahead = evaluate([0.0, 0.0, -10.0], &listener, model)?;
    println!("  正前 10m  gain={:.4} pan={:+.4}", ahead.gain, ahead.pan);
    check("正前方：声像居中", near(ahead.pan, 0.0, 1e-6));
    check(
        "正前方 10m：线性衰减 = (30-10)/(30-1)",
        near(ahead.gain, 20.0 / 29.0, 1e-6),
    );

    let right = evaluate([10.0, 0.0, 0.0], &listener, model)?;
    let left = evaluate([-10.0, 0.0, 0.0], &listener, model)?;
    println!("  正右 10m  pan={:+.4}", right.pan);
    println!("  正左 10m  pan={:+.4}", left.pan);
    check("正右：pan 饱和到 +1", near(right.pan, 1.0, 1e-6));
    check("正左：pan 饱和到 -1", near(left.pan, -1.0, 1e-6));
    check("左右对称：增益相同", near(left.gain, right.gain, 1e-6));

    let far = evaluate([100.0, 0.0, 0.0], &listener, model)?;
    println!("  100m      gain={:.4} pan={:+.4}", far.gain, far.pan);
    check("超 max_distance：静音", far.gain == 0.0);
    check("静音仍保留声像（便于调试读数）", near(far.pan, 1.0, 1e-6));

    banner("② min_distance 内不衰减");
    let close_model = SpatialModel {
        min_distance: 5.0,
        max_distance: 30.0,
    };
    let close = evaluate([0.0, 0.0, -4.9], &listener, close_model)?;
    let edge = evaluate([0.0, 0.0, -5.0], &listener, close_model)?;
    println!("  4.9m gain={:.4} / 5.0m gain={:.4}", close.gain, edge.gain);
    check("min 之内满增益", close.gain == 1.0);
    check("min 边界满增益", edge.gain == 1.0);

    banner("③ 非法输入一律拒绝");
    check(
        "NaN 位置 → NonFinite",
        matches!(
            evaluate([f32::NAN, 0.0, 0.0], &listener, model),
            Err(SpatialError::NonFinite("emitter.x"))
        ),
    );
    check(
        "模型反了 → InvalidModel",
        matches!(
            evaluate(
                [0.0, 0.0, 0.0],
                &listener,
                SpatialModel {
                    min_distance: 30.0,
                    max_distance: 10.0
                }
            ),
            Err(SpatialError::InvalidModel(_))
        ),
    );
    check(
        "零朝向 → DegenerateBasis",
        matches!(
            evaluate(
                [0.0, 0.0, 0.0],
                &Listener::new([0.0; 3], [0.0; 3], [0.0, 1.0, 0.0]),
                model
            ),
            Err(SpatialError::DegenerateBasis(_))
        ),
    );
    check(
        "朝向与 up 平行 → DegenerateBasis",
        matches!(
            evaluate(
                [0.0, 0.0, 0.0],
                &Listener::new([0.0; 3], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
                model
            ),
            Err(SpatialError::DegenerateBasis(_))
        ),
    );

    banner("④ 折回位置（rodio 的模型前提）");
    for pan in [-1.0_f32, -0.5, 0.0, 0.5, 1.0] {
        let (left_ear, right_ear, emitter) = fold_to_ears(&listener, pan);
        let folded = distance(emitter, listener.position);
        let to_left = distance(emitter, left_ear);
        let to_right = distance(emitter, right_ear);
        println!(
            "  pan={:+.1}  折回距离={:.4}  到左耳={:.4} 到右耳={:.4}",
            pan, folded, to_left, to_right
        );
        check(
            "折回位置到听者 < 1m（rodio 内不衰减，避免二次衰减）",
            folded < 1.0,
        );
        check("两耳不重合（rodio 的 debug_assert）", left_ear != right_ear);
        if pan > 0.0 {
            check("pan>0：声源离右耳更近", to_right < to_left);
        } else if pan < 0.0 {
            check("pan<0：声源离左耳更近", to_left < to_right);
        } else {
            check("pan=0：两耳等距", near(to_left, to_right, 1e-6));
        }
    }

    banner("⑤ 宿主契约（AudioSystem）");
    let mut audio = AudioSystem::new();
    audio.set_asset_dir(std::env::temp_dir());
    let missing = "rustlua-audio-definitely-missing";
    let no_listener = audio.play_at(missing, [1.0, 0.0, 0.0]);
    println!("  未接线 play_at ⇒ {no_listener:?}");
    check(
        "没接线时拒绝（不按原点瞎衰减）",
        matches!(
            no_listener,
            Err(AudioError::Spatial(SpatialError::NoListener))
        ),
    );

    audio.set_listener(Listener::facing([0.0, 0.0, 0.0], [0.0, 0.0, -1.0]));
    check(
        "`facing` 与默认朝向等价（pan 仍为 +1）",
        near(
            evaluate([10.0, 0.0, 0.0], &audio.listener().expect("已接线"), model)?.pan,
            1.0,
            1e-6,
        ),
    );
    let missing_clip = audio.play_at(missing, [0.0, 0.0, -5.0]);
    println!("  接线后缺文件 ⇒ {missing_clip:?}");
    check(
        "接好线后缺文件仍是 Io（不是静默成功）",
        matches!(missing_clip, Err(AudioError::Io(_))),
    );

    let bad_model = audio.set_spatial_model(SpatialModel {
        min_distance: 30.0,
        max_distance: 10.0,
    });
    check(
        "非法模型被拒",
        matches!(bad_model, Err(AudioError::Spatial(_))),
    );
    check(
        "被拒之后模型保持原值",
        audio.spatial_model() == SpatialModel::DEFAULT,
    );
    audio.set_spatial_model(SpatialModel {
        min_distance: 2.0,
        max_distance: 50.0,
    })?;
    check("合法模型可写入", audio.spatial_model().max_distance == 50.0);

    println!("\n[ok] M74-01 空间音频：模型/拒绝/折回/宿主契约 五段全过");
    Ok(())
}
