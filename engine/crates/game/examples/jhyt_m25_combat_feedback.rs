//! Headless acceptance drive for M25 / 计划 G2：最小战斗闭环的**受击反馈**
//! （硬直 hitstun / 击退 knockback / 无敌帧 i-frames）。
//!
//! M18-01 已经落地了 hitbox、命中去重、冷却、击杀与战败重生；缺少的是"打中之后
//! 双方各自发生什么"：目标不被打断（可以贴脸连砍/无脑换血）、不被推开（打击感为零）、
//! 玩家挨打后没有无敌时间（多只敌人贴身即死亡螺旋）。本样例把这三件事锁成断言：
//!
//! 1. **文档 ↔ 数据**：直接解析 `docs/33-江湖有田-数值设计案.md` §6.3 受击反馈表，
//!    与 `data/combat.lua`（玩家无敌帧）+ `data/skills.lua`（每招击退/硬直）逐格比对。
//! 2. **玩家无敌帧**：无敌期内命中的伤害被整次吸收（记 dodge），且连续挨打时掉血速度
//!    被无敌帧窗口封顶——这就是"防死亡螺旋"的可验证形式。
//! 3. **玩家硬直**：被打中后 `player_can_act()` 为 false、移动输入不产生位移、
//!    出手被拒（reason = stunned）、HUD 出中文文案；硬直耗尽后恢复。
//! 4. **玩家击退**：位移方向 = 远离攻击者、距离 = 招式数据；撞墙不穿墙（位移 0 + blocked）。
//! 5. **敌人硬直与击退**：千风掌把灰狼推开 2 格并打断追击，硬直期间不移动；
//!    训练木人抵抗为 1 → 完全不动（练功房不该被自己的普攻推着满场追木人）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m25_combat_feedback [-- <game_jhyt-root>]

use rustlua_input::input::{InputSnapshot, KeyCode};
use rustlua_scripting::script::LuaRuntime;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 演武场：训练木人 home (14,5)，玩家站在 (13,5) 面向右即可命中。
const BEFORE_DUMMY: (f32, f32) = (432.0, 176.0);
/// 灰狼 home (2,9)；玩家站 (6,8) 会进入它的索敌半径（aggro 240）。
const WOLF_LURE: (f32, f32) = (208.0, 272.0);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        rustlua_infra::games_dir()
            .join("examples")
            .join("game_jhyt")
    });
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let design_doc = project.join("docs/33-江湖有田-数值设计案.md");
    let design = fs::read_to_string(&design_doc)
        .map_err(|error| format!("cannot read {}: {error}", design_doc.display()))?;
    let main_script = project.join("scripts/main.lua");
    let save_path = fresh_save_path(&project, "jhyt_m25_combat_feedback.sav");

    let (mut runtime, host) = new_runtime(&project, &save_path)?;
    runtime.execute_project_script(&main_script)?;
    // 与 jhyt_m18_combat 同一条启动路径：先门面，再直接进演武场。
    open_scene(&mut runtime, "main")?;

    banner("① 设计案 §6.3 受击反馈表 == data/combat.lua + data/skills.lua");
    let rows = markdown_table(&design, "项")?;
    let mut checked = 0;
    for row in &rows {
        if row.len() < 2 {
            return Err(format!("受击反馈表行格式不符: {row:?}").into());
        }
        let doc_value = number_from_cell(&row[1])?;
        let expr = match row[0].as_str() {
            "玩家无敌帧" => "require('data.combat').player.iframes".to_owned(),
            // MVP 里"玩家挨打硬直多久"由敌人所用招式决定（敌人统一用 strike），
            // 数据侧没有独立的玩家硬直字段 ⇒ 该行必须与 strike.hitstun 一致。
            "玩家受击硬直" => "require('data.skills').skills.strike.hitstun".to_owned(),
            name => {
                let skill = if let Some(rest) = name.strip_suffix("击退") {
                    Some((rest, "knockback"))
                } else {
                    name.strip_suffix("硬直").map(|rest| (rest, "hitstun"))
                };
                match skill {
                    Some((skill_name, field)) => {
                        let id = match skill_name {
                            "普攻" => "strike",
                            "千风掌" => "palm_gust",
                            other => {
                                return Err(format!(
                                    "设计案受击反馈表出现未映射的招式「{other}」：新增招式必须同步本示例"
                                )
                                .into());
                            }
                        };
                        format!("require('data.skills').skills.{id}.{field}")
                    }
                    None => {
                        return Err(format!(
                            "设计案受击反馈表新增了「{name}」但本示例没有映射：请补映射或移出表格"
                        )
                        .into());
                    }
                }
            }
        };
        let data_value = query_num(&mut runtime, &expr)?;
        if (doc_value - data_value).abs() > f64::EPSILON {
            return Err(format!(
                "设计案受击反馈表「{}」= {doc_value}，{expr} = {data_value}",
                row[0]
            )
            .into());
        }
        checked += 1;
        println!("  {} = {doc_value}", row[0]);
    }
    if checked < 4 {
        return Err(format!("受击反馈表只比对了 {checked} 行，表格被删改？").into());
    }
    let iframes = query_num(&mut runtime, "require('data.combat').player_iframes()")?;
    let strike_kb = query_num(
        &mut runtime,
        "require('data.skills').skills.strike.knockback",
    )?;
    let strike_stun = query_num(&mut runtime, "require('data.skills').skills.strike.hitstun")?;
    let palm_kb = query_num(
        &mut runtime,
        "require('data.skills').skills.palm_gust.knockback",
    )?;

    banner("② 进入演武场：新档满气血，受击反馈计时从零开始");
    open_scene(&mut runtime, "combat_arena")?;
    assert_num(&mut runtime, "require('systems.combat').player_hp()", 100.0)?;
    assert_num(
        &mut runtime,
        "require('systems.combat').player_hurt_frames()",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.combat').player_iframes()",
        0.0,
    )?;
    assert_str(
        &mut runtime,
        "require('systems.combat').player_can_act()",
        "true",
    )?;

    banner("③ 训练木人：抵抗为 1 → 挨打不后退、不进硬直（练功房站桩不动）");
    teleport(&mut runtime, BEFORE_DUMMY)?;
    set_facing(&mut runtime, "right")?;
    frames(&mut runtime, &host, 2)?;
    let dummy_cell_before = enemy_cell(&mut runtime, "enemy_dummy")?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('j').expect("QWERTY 字符必须有对应物理键"),
    )?;
    // 逐帧推进到命中：命中当步就要读到"木人不动、不硬直"。
    let mut landed = false;
    for _ in 0..30 {
        frames(&mut runtime, &host, 1)?;
        let hp = query_num(
            &mut runtime,
            "require('systems.enemy').query('enemy_dummy').hp",
        )?;
        if hp < 30.0 {
            landed = true;
            break;
        }
    }
    if !landed {
        return Err("普攻没有命中训练木人（站位/朝向需要调整）".into());
    }
    assert_num(
        &mut runtime,
        "require('systems.enemy').query('enemy_dummy').last_knockback",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.enemy').query('enemy_dummy').stun_frames",
        0.0,
    )?;
    frames(&mut runtime, &host, 30)?;
    assert_num(
        &mut runtime,
        "require('systems.enemy').query('enemy_dummy').cell_x",
        dummy_cell_before.0,
    )?;
    // 对照：同一招打在灰狼身上就会推动它（见第 ⑤ 节），所以这里的 0 来自抵抗而不是没结算。
    assert_num(
        &mut runtime,
        "require('data.enemies').get('enemy_dummy').knockback_resist",
        1.0,
    )?;

    banner("④ 灰狼命中玩家：硬直锁操作 + 无敌帧吸收 + 击退真实位移");
    teleport(&mut runtime, WOLF_LURE)?;
    let mut hurt_at_hit = None;
    for _ in 0..900 {
        frames(&mut runtime, &host, 1)?;
        let hurt = query_num(
            &mut runtime,
            "require('systems.combat').player_hurt_frames()",
        )?;
        if hurt > 0.0 {
            hurt_at_hit = Some(hurt);
            break;
        }
    }
    let hurt_at_hit = hurt_at_hit.ok_or("wolf never landed a hit within 900 frames")?;
    if (hurt_at_hit - strike_stun).abs() > f64::EPSILON {
        return Err(format!(
            "命中当步的硬直 {hurt_at_hit} != data/skills.lua:strike.hitstun {strike_stun}"
        )
        .into());
    }
    let hp_after_first = query_num(&mut runtime, "require('systems.combat').player_hp()")?;
    if hp_after_first >= 100.0 {
        return Err(format!("first hit did not damage the player: hp={hp_after_first}").into());
    }
    let iframes_at_hit = query_num(&mut runtime, "require('systems.combat').player_iframes()")?;
    if iframes_at_hit <= 0.0 || iframes_at_hit > iframes {
        return Err(format!("命中当步的无敌帧 {iframes_at_hit} 不在 (0, {iframes}] 内").into());
    }
    assert_str(
        &mut runtime,
        "require('systems.combat').player_can_act()",
        "false",
    )?;
    assert_str(
        &mut runtime,
        "select(2, require('systems.combat').player_attack('strike'))",
        "stunned",
    )?;
    let pos_before = player_pos(&mut runtime)?;
    move_input(
        &mut runtime,
        &host,
        KeyCode::from_char('d').expect("QWERTY 字符必须有对应物理键"),
        2,
    )?;
    let pos_after = player_pos(&mut runtime)?;
    if (pos_after.0 - pos_before.0).abs() > 0.001 || (pos_after.1 - pos_before.1).abs() > 0.001 {
        return Err(format!("硬直期间移动输入产生了位移: {pos_before:?} → {pos_after:?}").into());
    }
    assert_str(&mut runtime, "require('core.state').last_action", "hurt")?;
    assert_str(
        &mut runtime,
        "require('ui.hud').feedback(require('core.state').last_action)",
        "受击僵直",
    )?;
    // 击退：方向背离狼、距离等于 strike.knockback；被墙挡住时位移 0 且 blocked。
    let pushed = query_num(
        &mut runtime,
        "require('systems.combat').last_knockback().distance",
    )?;
    let dx = query_num(
        &mut runtime,
        "require('systems.combat').last_knockback().dx",
    )?;
    let dy = query_num(
        &mut runtime,
        "require('systems.combat').last_knockback().dy",
    )?;
    let blocked = query_str(
        &mut runtime,
        "require('systems.combat').last_knockback().blocked",
    )?;
    let moved = (dx * dx + dy * dy).sqrt();
    if pushed > 0.0 {
        if blocked != "false" {
            return Err(format!("有位移却标记 blocked: {blocked}").into());
        }
        if (moved - strike_kb).abs() > 0.001 {
            return Err(format!(
                "玩家被击退 {moved} 单位，data/skills.lua:strike.knockback = {strike_kb}"
            )
            .into());
        }
    } else if blocked != "true" {
        return Err("位移为 0 却没有标记 blocked（既没推动也没撞墙？）".into());
    }
    println!(
        "  首次被咬：硬直={hurt_at_hit} 无敌帧={iframes_at_hit} 击退={moved}(blocked={blocked})"
    );

    // 无敌帧的防死亡螺旋效果：把狼的攻击间隔压到远小于无敌帧（12 < 48），
    // 连续 200 步内真实掉血次数必须被"无敌帧窗口"封顶，且至少有一次被吸收。
    lua(
        &mut runtime,
        "require('data.enemies').get('enemy_wolf').attack_cooldown = 12",
    )?;
    let ignored_before = query_num(&mut runtime, "require('systems.combat').stats().ignored")?;
    let hits_before = query_num(&mut runtime, "require('systems.combat').stats().hits_taken")?;
    let hp_before_burst = query_num(&mut runtime, "require('systems.combat').player_hp()")?;
    // 贴到狼身边，保证它一直在攻击距离内（被打退后它会自己追回来）。
    frames(&mut runtime, &host, 200)?;
    let ignored_after = query_num(&mut runtime, "require('systems.combat').stats().ignored")?;
    let hits_after = query_num(&mut runtime, "require('systems.combat').stats().hits_taken")?;
    let hp_after_burst = query_num(&mut runtime, "require('systems.combat').player_hp()")?;
    if ignored_after <= ignored_before {
        return Err(format!(
            "狼以 12 步间隔连续咬 200 步，却没有任何一次被无敌帧吸收（ignored {ignored_before} → {ignored_after}）"
        )
        .into());
    }
    let real_hits = hits_after - hits_before;
    let capped = (200.0 / iframes).floor() + 2.0;
    if real_hits > capped {
        return Err(format!(
            "无敌帧没有封顶掉血速度：200 步内实际受击 {real_hits} 次，上限 {capped}（无敌帧 {iframes} 步）"
        )
        .into());
    }
    if hp_before_burst - hp_after_burst <= 0.0 {
        return Err("连续挨打期间血量没有变化（无敌帧把伤害全吃了？）".into());
    }
    println!(
        "  连击测试：ignored {ignored_before} → {ignored_after}，真实受击 {real_hits} 次（上限 {capped}），hp {hp_before_burst} → {hp_after_burst}"
    );
    assert_str(
        &mut runtime,
        "(function() for _, e in ipairs(require('systems.combat').log()) do \
         if e.event == 'dodge' and e.target == 'player' then return 'true' end end return 'false' end)()",
        "true",
    )?;
    // 先恢复狼的正常攻击间隔（否则它会继续每 12 步咬一口，无敌帧永远刷新不完），
    // 再等无敌帧与硬直自然归零。注意：恢复数据不会撤销"已经在判定窗里的那一击"
    // （registration 时的 cooldown 已经放行），所以这里轮询等待而不是固定帧数。
    lua(
        &mut runtime,
        "require('data.enemies').get('enemy_wolf').attack_cooldown = 72",
    )?;
    let mut recovered = false;
    for _ in 0..300 {
        frames(&mut runtime, &host, 1)?;
        let iframes_now = query_num(&mut runtime, "require('systems.combat').player_iframes()")?;
        if iframes_now == 0.0 {
            recovered = true;
            break;
        }
    }
    if !recovered {
        return Err(format!(
            "无敌帧在 300 步内没有归零（last = {}）",
            query_num(&mut runtime, "require('systems.combat').player_iframes()")?
        )
        .into());
    }
    assert_str(
        &mut runtime,
        "require('systems.combat').player_can_act()",
        "true",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.combat').player_iframes()",
        0.0,
    )?;

    banner("⑤ 击退/硬直真的生效：拿掉木人的抵抗后，千风掌把它推开 2 格");
    // 木人是"完全抵抗"的对照个体（第 ③ 节），这里临时把抵抗改成 0，
    // 用同一套命中链路验证"击退距离 = 招式数据、硬直步数 > 0、硬直期间不动"，
    // 最后恢复抵抗并验证它又变回不动 —— 抵抗字段不是装饰。
    lua(
        &mut runtime,
        "local dummy = require('data.enemies').get('enemy_dummy')\n\
         dummy.knockback_resist = 0\n\
         dummy.hitstun_resist = 0",
    )?;
    teleport(&mut runtime, BEFORE_DUMMY)?;
    set_facing(&mut runtime, "right")?;
    frames(&mut runtime, &host, 2)?;
    let dummy_cell_0 = enemy_cell(&mut runtime, "enemy_dummy")?;
    let dummy_hp_0 = query_num(
        &mut runtime,
        "require('systems.enemy').query('enemy_dummy').hp",
    )?;
    lua(&mut runtime, "require('core.state').inner_power = 50")?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('k').expect("QWERTY 字符必须有对应物理键"),
    )?;
    // 逐帧推进到命中，命中当步就要读到硬直（与 ④ 的玩家侧同一口径）。
    let mut dummy_stun_at_hit = None;
    for _ in 0..40 {
        frames(&mut runtime, &host, 1)?;
        let hp = query_num(
            &mut runtime,
            "require('systems.enemy').query('enemy_dummy').hp",
        )?;
        if hp < dummy_hp_0 {
            dummy_stun_at_hit = Some(hp);
            break;
        }
    }
    let dummy_hp_1 = dummy_stun_at_hit.ok_or("千风掌没有命中训练木人（站位/朝向需要调整）")?;
    let push = query_num(
        &mut runtime,
        "require('systems.enemy').query('enemy_dummy').last_knockback",
    )?;
    let stun_at_hit = query_num(
        &mut runtime,
        "require('systems.enemy').query('enemy_dummy').stun_frames",
    )?;
    let blocked = query_str(
        &mut runtime,
        "require('systems.enemy').query('enemy_dummy').knockback_blocked",
    )?;
    if blocked == "true" {
        return Err("训练木人被墙挡住，无法验证击退距离（演武场布局变化？）".into());
    }
    if (push - palm_kb).abs() > 0.001 {
        return Err(
            format!("木人被击退 {push}，data/skills.lua:palm_gust.knockback = {palm_kb}").into(),
        );
    }
    let dummy_cell_1 = enemy_cell(&mut runtime, "enemy_dummy")?;
    if (dummy_cell_1.0 - dummy_cell_0.0 - palm_kb / 32.0).abs() > 0.001 {
        return Err(format!(
            "击退 {palm_kb} 单位应等于 {} 格，实际 cell {dummy_cell_0:?} → {dummy_cell_1:?}",
            palm_kb / 32.0
        )
        .into());
    }
    if stun_at_hit <= 0.0 {
        return Err("千风掌命中后木人没有硬直".into());
    }
    println!(
        "  木人：hp {dummy_hp_0} → {dummy_hp_1}，击退 {push}（{} 格），cell {dummy_cell_0:?} → {dummy_cell_1:?}，硬直 {stun_at_hit}",
        palm_kb / 32.0
    );
    // 硬直期间：状态 stunned 且不再移动。
    assert_str(
        &mut runtime,
        "require('systems.enemy').query('enemy_dummy').state",
        "stunned",
    )?;
    frames(&mut runtime, &host, 4)?;
    let during = enemy_cell(&mut runtime, "enemy_dummy")?;
    frames(&mut runtime, &host, 4)?;
    let during2 = enemy_cell(&mut runtime, "enemy_dummy")?;
    if during != during2 {
        return Err(format!("硬直期间木人仍在移动: {during:?} → {during2:?}").into());
    }
    // 恢复抵抗：木人又变回"打不动"。
    lua(
        &mut runtime,
        "local dummy = require('data.enemies').get('enemy_dummy')\n\
         dummy.knockback_resist = 1\n\
         dummy.hitstun_resist = 1",
    )?;
    frames(&mut runtime, &host, 60)?;
    let cell_before_again = enemy_cell(&mut runtime, "enemy_dummy")?;
    press(
        &mut runtime,
        &host,
        KeyCode::from_char('j').expect("QWERTY 字符必须有对应物理键"),
    )?;
    frames(&mut runtime, &host, 30)?;
    let cell_after_again = enemy_cell(&mut runtime, "enemy_dummy")?;
    if cell_before_again != cell_after_again {
        return Err(format!(
            "恢复抵抗后木人仍被推动: {cell_before_again:?} → {cell_after_again:?}"
        )
        .into());
    }

    banner("⑥ 实战：打断灰狼的追击（命中当步 path 被撤销 + 进入硬直）");
    // 灰狼是"会追"的敌人：让玩家贴到它旁边、能出手时打一剑，命中当步就要看到
    // 寻路被撤销、状态变 stunned，硬直结束后恢复追击/攻击。
    let mut wolf_hit = None;
    for _ in 0..24 {
        // 站到狼**世界 y 更小**的一侧并朝上：combat 的 FACING_VECTORS.up = +y，
        // 即"朝上"指向世界 y 更大的方向（与格坐标 y 朝上相反，这里以世界坐标为准）。
        lua(
            &mut runtime,
            "local q = require('systems.enemy').query('enemy_wolf')\n\
             if q.live then\n\
               require('player.player').teleport(q.cell_x * 32 + 16, (q.cell_y - 1) * 32 + 16)\n\
               require('core.state').facing = 'up'\n\
             end",
        )?;
        frames(&mut runtime, &host, 2)?;
        if !query_str(&mut runtime, "require('systems.combat').player_can_act()")?.eq("true") {
            frames(&mut runtime, &host, 20)?;
            continue;
        }
        let hp_before = query_num(
            &mut runtime,
            "require('systems.enemy').query('enemy_wolf').hp",
        )?;
        if hp_before <= 0.0 {
            break;
        }
        press(
            &mut runtime,
            &host,
            KeyCode::from_char('j').expect("QWERTY 字符必须有对应物理键"),
        )?;
        for _ in 0..30 {
            frames(&mut runtime, &host, 1)?;
            let hp_now = query_num(
                &mut runtime,
                "require('systems.enemy').query('enemy_wolf').hp",
            )?;
            if hp_now < hp_before {
                wolf_hit = Some((hp_before, hp_now));
                break;
            }
        }
        if wolf_hit.is_some() {
            break;
        }
        frames(&mut runtime, &host, 30)?;
    }
    let (wolf_hp_before, wolf_hp_after) =
        wolf_hit.ok_or("24 次尝试都没能打中灰狼（站位/朝向需要调整）")?;
    assert_num(
        &mut runtime,
        "require('systems.enemy').query('enemy_wolf').last_knockback",
        strike_kb,
    )?;
    assert_str(
        &mut runtime,
        "require('systems.enemy').query('enemy_wolf').path_active",
        "false",
    )?;
    assert_str(
        &mut runtime,
        "require('systems.enemy').query('enemy_wolf').state",
        "stunned",
    )?;
    let wolf_push = query_num(
        &mut runtime,
        "require('systems.enemy').query('enemy_wolf').last_knockback",
    )?;
    let stun_frames = query_num(
        &mut runtime,
        "require('systems.enemy').query('enemy_wolf').stun_frames",
    )?;
    println!(
        "  灰狼：hp {wolf_hp_before} → {wolf_hp_after}，击退 {wolf_push}，硬直 {stun_frames}，追击已打断"
    );
    frames(&mut runtime, &host, 40)?;
    let resumed = query_str(
        &mut runtime,
        "require('systems.enemy').query('enemy_wolf').state",
    )?;
    if !matches!(resumed.as_str(), "chase" | "attack" | "idle") {
        return Err(format!("硬直结束后灰狼状态异常: {resumed:?}").into());
    }

    banner("⑦ 校验期兜底：缺 hitstun/knockback、非法抵抗、非法无敌帧都必须报出");
    assert_str(
        &mut runtime,
        "(function() local s = require('data.skills').skills.strike\n\
         local old = s.hitstun\n\
         s.hitstun = nil\n\
         local ok, issues = require('data.skills').validate_all()\n\
         s.hitstun = old\n\
         return not ok and table.concat(issues, '|'):find('strike.hitstun', 1, true) ~= nil end)()",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "(function() local dummy = require('data.enemies').get('enemy_dummy')\n\
         local old = dummy.knockback_resist\n\
         dummy.knockback_resist = 2\n\
         local ok, issues = require('data.enemies').validate_all()\n\
         dummy.knockback_resist = old\n\
         return not ok and table.concat(issues, '|'):find('knockback_resist', 1, true) ~= nil end)()",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "(function() local c = require('data.combat')\n\
         local old = c.player.iframes\n\
         c.player.iframes = -1\n\
         local ok, issues = c.validate_all()\n\
         c.player.iframes = old\n\
         return not ok and table.concat(issues, '|'):find('iframes', 1, true) ~= nil end)()",
        "true",
    )?;
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("还原后数据校验未通过:\n{report}").into());
    }

    println!(
        "jhyt_m25_combat_feedback_ok project={} iframes={} strike_knockback={} strike_hitstun={} \
         palm_knockback={} player_push={} dummy_push={} wolf_push={} ignored={} capped={}",
        project.display(),
        iframes,
        strike_kb,
        strike_stun,
        palm_kb,
        moved,
        push,
        wolf_push,
        query_num(&mut runtime, "require('systems.combat').stats().ignored")?,
        capped
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers ----------

fn new_runtime(
    project: &Path,
    save_path: &Path,
) -> Result<(LuaRuntime, rustlua_input::host::HostInput), Box<dyn std::error::Error>> {
    let (installers, handles) = rustlua_gameplay::input_lua::registry_with_handles();
    let host = handles.input;
    let mut runtime = LuaRuntime::with_installers(installers)?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok((runtime, host))
}

fn open_scene(runtime: &mut LuaRuntime, scene: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("scene.load('{scene}')"))?;
    runtime.process_scene_transition()?;
    let current = query_str(runtime, "scene.reload()")?;
    if current != scene {
        return Err(format!("scene = {current:?}, want {scene:?}").into());
    }
    Ok(())
}

fn teleport(runtime: &mut LuaRuntime, pos: (f32, f32)) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "require('player.player').teleport({}, {})",
        pos.0, pos.1
    ))?;
    Ok(())
}

fn set_facing(runtime: &mut LuaRuntime, facing: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("require('core.state').facing = '{facing}'"))?;
    Ok(())
}

fn lua(runtime: &mut LuaRuntime, code: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(code)?;
    Ok(())
}

/// 单次按键：按下 1 帧 + 空 1 帧。
fn press(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    key: KeyCode,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = InputSnapshot::default();
    input.key_event(key, true);
    host.set_input(input);
    runtime.call_update()?;
    host.set_input(InputSnapshot::default());
    runtime.call_update()?;
    Ok(())
}

/// 按住方向键若干帧（验证"硬直期间移动无效"）。
fn move_input(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    key: KeyCode,
    n: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..n {
        let mut input = InputSnapshot::default();
        input.key_event(key, true);
        host.set_input(input);
        runtime.call_update()?;
    }
    host.set_input(InputSnapshot::default());
    runtime.call_update()?;
    Ok(())
}

fn frames(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    n: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    host.set_input(InputSnapshot::default());
    for _ in 0..n {
        runtime.call_update()?;
    }
    Ok(())
}

fn player_pos(runtime: &mut LuaRuntime) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let x = query_num(runtime, "require('player.player').position().x")?;
    let y = query_num(runtime, "require('player.player').position().y")?;
    Ok((x, y))
}

fn enemy_cell(
    runtime: &mut LuaRuntime,
    persist_id: &str,
) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let x = query_num(
        runtime,
        &format!("require('systems.enemy').query('{persist_id}').cell_x"),
    )?;
    let y = query_num(
        runtime,
        &format!("require('systems.enemy').query('{persist_id}').cell_y"),
    )?;
    Ok((x, y))
}

fn query_str(runtime: &mut LuaRuntime, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(runtime.execute_console(expr)?.trim().to_owned())
}

fn query_num(runtime: &mut LuaRuntime, expr: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let value = query_str(runtime, &format!("return tostring({expr})"))?;
    value
        .parse::<f64>()
        .map_err(|error| format!("{value:?} is not a number: {error}").into())
}

fn assert_str(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(runtime, &format!("return tostring({expr})"))?;
    if got != expected {
        return Err(format!("{expr} = {got:?}, want {expected:?}").into());
    }
    Ok(())
}

fn assert_num(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_num(runtime, &format!("({expr})"))?;
    if (got - expected).abs() > 1e-6 {
        return Err(format!("{expr} = {got}, want {expected}").into());
    }
    Ok(())
}

/// 从设计案 markdown 里取出以 `first_cell` 为表头的表格行（去掉表头与分隔行）。
fn markdown_table(
    markdown: &str,
    first_cell: &str,
) -> Result<Vec<Vec<String>>, Box<dyn std::error::Error>> {
    let mut rows = Vec::new();
    let mut inside = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            if inside {
                break;
            }
            continue;
        }
        let cells: Vec<String> = trimmed
            .trim_matches('|')
            .split('|')
            .map(|cell| cell.trim().to_owned())
            .collect();
        if !inside {
            if cells.first().map(String::as_str) == Some(first_cell) {
                inside = true;
            }
            continue;
        }
        if cells
            .iter()
            .all(|cell| !cell.is_empty() && cell.chars().all(|c| c == '-' || c == ':'))
        {
            continue;
        }
        rows.push(cells);
    }
    if rows.is_empty() {
        return Err(format!("设计案里找不到以「{first_cell}」为表头的表格").into());
    }
    Ok(rows)
}

fn number_from_cell(cell: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let digits: String = cell
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    digits
        .parse::<f64>()
        .map_err(|error| format!("cell {cell:?} has no leading number: {error}").into())
}

fn banner(title: &str) {
    println!("\n== {title} ==");
}

fn fresh_save_path(project: &Path, name: &str) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(name);
    let _ = fs::remove_file(&path);
    path
}
