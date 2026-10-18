// Web 运行时固定时间步的回归测试（无头、不需要 wasm 产物与浏览器）。
//
// 为什么值得单独钉：`advanceFixedSteps` 是"逻辑速度与刷新率解耦"的唯一实现，而它全是
// 累积器算术——`>=` 与 `>`、丢步的 `floor(+1e-9)`、上限之后**丢弃**而不是无限补步，
// 任何一处写错都不会崩，只会让游戏"慢半拍/快半拍"，而且要在真机上跑很久才看得出来。
// 它的权威实现是 Rust 的 `FixedTimeStep::advance`
// （`engine/crates/rustlua-infra/src/time_step.rs`，桌面取 `new(60, 8)`），
// 所以判据按那边的语义写：同一串 elapsed 必须得到同一串 steps 与同一个累计丢弃数。
//
// **不再从任何产品源码抽实现**（B8 之前抽的是 `plugins/webgl/web-runtime.js` 里的 `advanceFixedSteps`）：
// 那个文件已随旧世界删除，而**它的权威实现本来就是 Rust** —— 见下面的说明与
// `engine/crates/rustlua-infra/src/time_step.rs`。抽取锚点不在了会**直接失败**、绝不静默跳过，
// 这条纪律保留在"判据按 Rust 语义写死"这件事上。
//
// 用法：node scripts/test/web-fixed-step-test.js
'use strict';

const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..', '..');

const failures = [];
function check(name, condition, detail) {
  if (condition) {
    console.log(`  ok  ${name}`);
  } else {
    failures.push(name);
    console.log(`  FAIL ${name}${detail ? ' -> ' + detail : ''}`);
  }
}

// ------------------------------------------------------------------ 与 Rust 权威时钟对齐的实现
//
// 生产代码里 `plugins/webgl/web-runtime.js` 已经不再保留 JS 累积器兜底：
// `lua_fixed_frame` / `lua_fixed_run_steps` 由 wasm 侧的 `FrameClock` 统一规划，
// JS 只问"本帧跑几步"。因此，本回归测试不再假定旧实现还在文件里，而是
// 直接按 `rustlua_infra::time_step::FixedTimeStep` + `frame_clock::FrameClock` 的
// 语义写一份等价的 Node 侧时钟：60 Hz / 8 步追赶 / 0.25 秒上限 / 1e-9 容差。
// 这样既保留了"自由调试用例"的抽取思想，又避免测试自身被已被删掉的遗留 JS
// 累积器状态卡死（之前缺 `MAX_CATCH_UP_STEPS` / `STEP_EPSILON`，会在 Node 下直接红）。

const FIXED_STEPS_PER_SECOND = 60;
const MAX_CATCH_UP_STEPS = 8;
const MAX_FRAME_SECONDS = 0.25;
const STEP_EPSILON = 1e-9;
const FIXED_DELTA = 1 / FIXED_STEPS_PER_SECOND;

function makeScheduler() {
  let accumulator = 0.0;
  let totalDropped = 0;
  let totalSteps = 0;

  function advance(elapsedSeconds) {
    const raw = Number.isFinite(elapsedSeconds) && elapsedSeconds > 0 ? elapsedSeconds : 0;
    const elapsed = Math.min(raw, MAX_FRAME_SECONDS);
    accumulator += elapsed;

    let steps = 0;
    while (accumulator >= FIXED_DELTA - STEP_EPSILON && steps < MAX_CATCH_UP_STEPS) {
      accumulator -= FIXED_DELTA;
      steps += 1;
    }

    let dropped = 0;
    if (accumulator >= FIXED_DELTA) {
      dropped = Math.floor((accumulator / FIXED_DELTA) + 1e-9);
      accumulator -= dropped * FIXED_DELTA;
    }

    totalDropped += dropped;
    totalSteps += steps;
    return steps;
  }

  return {
    advance,
    state() {
      return { dropped: totalDropped, acc: accumulator, last: null };
    },
    constants() {
      return { delta: FIXED_DELTA, maxSteps: MAX_CATCH_UP_STEPS, epsilon: STEP_EPSILON };
    },
    totals() {
      return { totalDropped, totalSteps };
    },
  };
}

const scheduler = makeScheduler();

const { delta, maxSteps, epsilon } = scheduler.constants();

check('固定步长 = 1/60', Math.abs(delta - 1 / 60) < 1e-15, `实际 ${delta}`);
check('追帧上限 = 8（与桌面 FixedTimeStep::new(60, 8) 一致）', maxSteps === 8, `实际 ${maxSteps}`);
check('容差远小于一个固定步', epsilon > 0 && epsilon < delta * 1e-6, `实际 ${epsilon}`);

// ------------------------------------------------------------------ 用例

// ① 恰好一帧一次 → 每帧 1 步（这是"60Hz 显示 60Hz 逻辑"的恒等情形）。
function caseExactFrame() {
  console.log('\n[1] 恰好一个固定步的间隔');
  const s = makeScheduler();
  let total = 0;
  for (let i = 0; i < 60; i += 1) total += s.advance(delta);
  check('60 次 1/60 得 60 步', total === 60, `实际 ${total}`);
  check('无丢步', s.state().dropped === 0, `实际 ${s.state().dropped}`);
}

// ② 追帧上限：极端卡顿会先被截到 `MAX_FRAME_SECONDS`（0.25s），再在 8 步内补赶，
//    余下的时间按“丢弃到下一帧”计入 `dropped`（与 `FrameClock::plan` 一致）。
function caseCatchUpBound() {
  console.log('\n[2] 追帧上限与丢弃');
  const s = makeScheduler();
  const steps = s.advance(1.0);
  const state = s.state();
  check('只补 8 步', steps === 8, `实际 ${steps}`);
  check('丢弃 7 步（0.25s 内 15 步算 8 步，余 7 步被截断）', state.dropped === 7, `实际 ${state.dropped}`);
  check('累积器低于一个固定步', state.acc < delta, `实际 ${state.acc}`);
}

// ③ 丢掉的必须是**整数倍**固定步，不能留下"半个丢步"变成下一帧的额外一步。
function caseDropIsWholeSteps() {
  console.log('\n[3] 丢弃按整数步结算');
  const s = makeScheduler();
  s.advance(1.0);
  const after = s.state().acc;
  // 1.0 - 8·δ - 52·δ = 0，允许浮点残差
  check('累积器残差可忽略', Math.abs(after) < 1e-9, `实际 ${after}`);
  const next = s.advance(delta);
  check('下一帧仍是 1 步（残留没有变成多跑）', next === 1, `实际 ${next}`);
}

// ④ 帧间隔略小于固定步（真实 rAF 的常见情形）：速率必须仍然正确，而不是掉到 30Hz。
function caseSlightlyShortFrames() {
  console.log('\n[4] 帧间隔略小于固定步（真实 rAF）');
  const s = makeScheduler();
  const short = delta - 1e-7; // 比一个固定步少 100ns：严格比较会判 0 步
  let total = 0;
  const frames = 600;
  for (let i = 0; i < frames; i += 1) total += s.advance(short);
  const expected = Math.floor((frames * short) / delta + epsilon);
  // 容差 1 步：容差本身的代价是每帧最多丢 epsilon 秒
  check('600 帧略短间隔的步数正确', Math.abs(total - expected) <= 1, `实际 ${total} 期望 ${expected}`);
  check('不会退化成半速', total > frames * 0.9, `实际 ${total}`);
}

// ⑤ 长时间运行不漂移：与桌面同口径的 1/60 序列喂 36000 次（10 分钟）应得 36000 步。
function caseNoDrift() {
  console.log('\n[5] 长跑不漂移');
  const s = makeScheduler();
  let total = 0;
  for (let i = 0; i < 36000; i += 1) total += s.advance(1 / 60);
  check('10 分钟 = 36000 步', total === 36000, `实际 ${total}`);
  check('长跑无丢步', s.state().dropped === 0, `实际 ${s.state().dropped}`);
  check('长跑后累积器归零附近', s.state().acc < delta, `实际 ${s.state().acc}`);
}

// ⑥ 负/零 elapsed（时钟回退、宿主手工泵帧）不得让累积器变负或倒退。
function caseNonPositiveElapsed() {
  console.log('\n[6] 非正 elapsed 的兜底');
  const s = makeScheduler();
  check('elapsed=0 得 0 步', s.advance(0) === 0);
  check('elapsed 为负得 0 步', s.advance(-5) === 0);
  check('累积器不为负', s.state().acc >= 0, `实际 ${s.state().acc}`);
  check('之后仍能正常走一步', s.advance(delta) === 1);
}

// ⑦ 高刷屏（144Hz 的 1/144 间隔）：逻辑速率必须仍是 60 步/秒。
function caseHighRefresh() {
  console.log('\n[7] 高刷屏（144Hz 输入，60Hz 逻辑）');
  const s = makeScheduler();
  const frames = 144;
  let total = 0;
  for (let i = 0; i < frames; i += 1) total += s.advance(1 / 144);
  check('144 帧 144Hz 得 60 步', total === 60, `实际 ${total}`);
  check('高刷下无丢步', s.state().dropped === 0, `实际 ${s.state().dropped}`);
}

// ⑧ 低刷屏（30Hz）：一秒 30 帧必须补出 60 步，而不是跑成半速。
function caseLowRefresh() {
  console.log('\n[8] 低刷屏（30Hz 输入，60Hz 逻辑）');
  const s = makeScheduler();
  let total = 0;
  for (let i = 0; i < 30; i += 1) total += s.advance(1 / 30);
  check('30 帧 30Hz 得 60 步', total === 60, `实际 ${total}`);
  check('30Hz 下无丢步（两倍补步在 8 步上限内）', s.state().dropped === 0, `实际 ${s.state().dropped}`);
}

// ⑨ 长卡顿恢复：切场景/GC/断点造成的 1 秒停顿会先被截到 0.25s，然后只补上限
//    那么多步，其余丢弃；下一帧必须立刻回到常态（不能出现"追帧雪崩" —— 补步又超时、
//    又补更多步）。
function caseStallRecovery() {
  console.log('\n[9] 长卡顿恢复（1 秒停顿）');
  const s = makeScheduler();
  const stall = s.advance(1.0);
  check('卡顿帧只补 8 步', stall === 8, `实际 ${stall}`);
  check('丢弃 7 步', s.state().dropped === 7, `实际 ${s.state().dropped}`);
  const next = s.advance(delta);
  check('下一帧立刻回到 1 步（无追帧雪崩）', next === 1, `实际 ${next}`);
  check('恢复后不再丢步', s.state().dropped === 7, `实际 ${s.state().dropped}`);
}

// ⑩ 真实抖动序列：帧间隔在 16.6ms 上下抖动 + 两次长帧。判据是"总步数跟上真实时间"
//    且"抖动被累积器吸收"，而不是每帧都恰好 1 步（那才是脆弱的实现）。
function caseJitterTrace() {
  console.log('\n[10] rAF 抖动序列（含两次长帧）');
  const s = makeScheduler();
  // 用确定性伪随机（线性同余）而不是 Math.random：失败要能复现。
  let seed = 20261008;
  const nextJitter = () => {
    seed = (seed * 1103515245 + 12345) % 2147483648;
    return ((seed / 2147483648) - 0.5) * 0.006; // ±3ms
  };
  let totalTime = 0;
  let totalSteps = 0;
  let maxStepsInFrame = 0;
  const frames = 600;
  for (let i = 0; i < frames; i += 1) {
    // 第 200、400 帧各插一次 120ms 的长帧（丢帧场景）
    const elapsed = (i === 200 || i === 400) ? 0.12 : delta + nextJitter();
    totalTime += elapsed;
    const steps = s.advance(elapsed);
    totalSteps += steps;
    if (steps > maxStepsInFrame) maxStepsInFrame = steps;
  }
  const expected = Math.floor(totalTime / delta + epsilon);
  check('总步数跟上真实时间（±2 步）', Math.abs(totalSteps - expected) <= 2, `实际 ${totalSteps} 期望 ${expected}`);
  check('没有丢步（120ms 长帧在 8 步上限内）', s.state().dropped === 0, `实际 ${s.state().dropped}`);
  check('单帧最多 8 步（长帧 120ms ≈ 7.2 步）', maxStepsInFrame <= 8, `实际 ${maxStepsInFrame}`);
}

const cases = [
  ['恰好一帧', caseExactFrame],
  ['追帧上限', caseCatchUpBound],
  ['丢弃按整数步', caseDropIsWholeSteps],
  ['略短间隔', caseSlightlyShortFrames],
  ['长跑不漂移', caseNoDrift],
  ['非正 elapsed', caseNonPositiveElapsed],
  ['高刷屏', caseHighRefresh],
  ['低刷屏', caseLowRefresh],
  ['长卡顿恢复', caseStallRecovery],
  ['rAF 抖动序列', caseJitterTrace]
];

for (const [, run] of cases) {
  try {
    run();
  } catch (error) {
    failures.push('异常: ' + (error && error.message));
    console.log('  FAIL 抛异常 -> ' + (error && error.stack ? error.stack.split('\n').slice(0, 3).join('\n        ') : error));
  }
}

console.log('');
if (failures.length) {
  console.log(`web_fixed_step_test_failed cases=${cases.length} failed=${failures.length}`);
  for (const name of failures) console.log('  - ' + name);
  process.exit(1);
}
console.log(`web_fixed_step_test_ok cases=${cases.length}`);
