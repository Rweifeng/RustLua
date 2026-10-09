//! `LuaRuntime` 的 VM 生命周期：建状态、读源码、热重载、每实体脚本、逐步调用 `update`。
//!
//! 三段式热重载是这里的核心不变量：先在**新状态**上 `load_candidate` 并跑 `on_create`，
//! 成功后才把 `self.lua` 换掉——状态在 Rust 侧（`ScriptState`），换 VM 不丢世界数据。

// WP11-USE:BEGIN
use crate::script::EntityScriptState;
use crate::script::LuaRuntime;
use crate::script::ReloadStatus;
use crate::script::ScriptState;
use crate::script::api;
use crate::script::call_entity_hook;
use crate::script::call_optional;
use crate::script::error::{LuaBridgeError, ScriptDebugger, bridge_error_parts};
use crate::script::guard::{
    BudgetSlot, ContainOutcome, EntityHook, ScriptErrorPolicy, ScriptGuard,
};
use crate::script::progress;
use crate::script::require::{
    file_fingerprint, install_encoded_require_searcher, read_project_script,
};
use mlua::{Function, Lua, Table};
use rustlua_ecs::entity::EntityId;
use rustlua_input::input::ActionMap;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
// WP11-USE:END

impl LuaRuntime {
    pub fn new() -> Result<Self, LuaBridgeError> {
        let lua = Lua::new();
        let state = Rc::new(RefCell::new(ScriptState::default()));
        state.borrow_mut().ui_state.reference = rustlua_ui::ui::UI_REFERENCE_SIZE;
        // M20-07：空映射 + 默认表 = 完整默认绑定（见 `ScriptState::action_map`）。
        state
            .borrow_mut()
            .action_map
            .merge_missing(&ActionMap::with_defaults());
        api::register_all(&lua, Rc::clone(&state))
            .map_err(|error| LuaBridgeError::Init(error.to_string()))?;
        // 心跳 + 指令预算：**每个 Lua 状态都装**（工程脚本、每实体脚本各自一份状态）。
        // 默认给足余量（见 `DEFAULT_INSTRUCTION_BUDGET`）：正常脚本撞不到它，
        // 而 `while true do end` 会被换成一条带 `源:行` 的报错——同步宿主上这是"页面别死"的唯一护栏。
        progress::install(&lua, progress::DEFAULT_INSTRUCTION_BUDGET)
            .map_err(|error| LuaBridgeError::Init(error.to_string()))?;
        Ok(Self {
            lua,
            state,
            current_path: None,
            fingerprint: None,
            debugger: RefCell::new(ScriptDebugger::new()),
            guard: RefCell::new(ScriptGuard::default()),
            entity_scripts: HashMap::new(),
            entity_scripts_spawned: HashSet::new(),
            entity_script_requests: Vec::new(),
            entity_script_requests_revision: u64::MAX,
        })
    }

    pub fn execute_project_script(&mut self, path: &Path) -> Result<(), LuaBridgeError> {
        let source = read_project_script(path)?;
        let candidate = match self.load_candidate(path, &source) {
            Ok(candidate) => candidate,
            Err(error) => {
                self.record_bridge_error(&error);
                return Err(error);
            }
        };
        if let Err(error) = call_optional(&candidate, "on_create") {
            let error = LuaBridgeError::Runtime {
                path: path.to_path_buf(),
                reason: error.to_string(),
            };
            self.record_bridge_error(&error);
            return Err(error);
        }
        self.lua = candidate;
        self.current_path = Some(path.to_path_buf());
        self.fingerprint = Some(file_fingerprint(path, &source)?);
        // 新 VM = 新一局：上一份脚本攒下的错误预算与"已停用"结论必须作废，否则
        // "改好脚本再重载"仍会被旧结论拦在门外（停用是一次性判定，不会自己解除）。
        self.guard.borrow_mut().reset();
        Ok(())
    }

    pub fn reload_if_changed(&mut self) -> Result<ReloadStatus, LuaBridgeError> {
        let path = self
            .current_path
            .clone()
            .ok_or_else(|| LuaBridgeError::Api("no project script is loaded".to_string()))?;
        let source = read_project_script(&path)?;
        let fingerprint = file_fingerprint(&path, &source)?;
        if self.fingerprint.as_ref() == Some(&fingerprint) {
            return Ok(ReloadStatus::Unchanged);
        }
        let candidate = self.load_candidate(&path, &source)?;
        // P1-②：原子热重载——先在**新状态**上跑 `on_create`；失败则旧 VM 原封不动
        // （顶层状态与模块缓存都还在），只有新状态建立成功才销毁旧状态并交换。
        if let Err(error) = call_optional(&candidate, "on_create") {
            let error = LuaBridgeError::Runtime {
                path: path.clone(),
                reason: error.to_string(),
            };
            self.record_bridge_error(&error);
            return Err(error);
        }
        if let Err(error) = call_optional(&self.lua, "on_destroy") {
            // 新状态已建立：旧状态的清理失败只记诊断，交换照常完成——绝不能把
            // "旧已毁、新未立"的半撕裂状态留给下一帧。
            self.record_bridge_error(&LuaBridgeError::Runtime {
                path: path.clone(),
                reason: format!("reload: old on_destroy failed: {error}"),
            });
        }
        self.lua = candidate;
        self.fingerprint = Some(fingerprint);
        // 热重载的语义就是"这是修好的新版本"：预算清零，被停用的 `update()` 重新有机会。
        self.guard.borrow_mut().reset();
        Ok(ReloadStatus::Reloaded)
    }

    pub fn reload(&mut self) -> Result<(), LuaBridgeError> {
        let path = self
            .current_path
            .clone()
            .ok_or_else(|| LuaBridgeError::Api("no project script is loaded".to_string()))?;
        let source = read_project_script(&path)?;
        let candidate = self.load_candidate(&path, &source)?;
        // P1-②：与 `reload_if_changed` 同样的原子顺序（先立新、后破旧）。
        if let Err(error) = call_optional(&candidate, "on_create") {
            let error = LuaBridgeError::Runtime {
                path: path.clone(),
                reason: error.to_string(),
            };
            self.record_bridge_error(&error);
            return Err(error);
        }
        if let Err(error) = call_optional(&self.lua, "on_destroy") {
            self.record_bridge_error(&LuaBridgeError::Runtime {
                path: path.clone(),
                reason: format!("reload: old on_destroy failed: {error}"),
            });
        }
        self.lua = candidate;
        self.fingerprint = Some(file_fingerprint(&path, &source)?);
        // 与 `reload_if_changed` 同因：新 VM 就是新版本，旧的停用结论不再适用。
        self.guard.borrow_mut().reset();
        Ok(())
    }

    /// docs/54 §4.1：把世界里的 `Script` 组件同步成"已加载的每实体脚本"。
    ///
    /// 三件事，顺序固定：
    /// ① 收集**启用**的 `Script` 组件（按实体 id 排序，因此加载顺序确定）；
    /// ② 尚未加载的文件读进来、编译成各自独立的 Lua 状态（同文件只加载一次）；
    /// ③ 对每个**还没生成过**的实体调用一次 `on_spawn(id)`。
    ///
    /// 返回本次新生成（调用过 `on_spawn`）的实体数。**没有脚本实体时直接返回 0**，不碰项目根，
    /// 因此既有工程与无工程根的诊断模式一字未改。有脚本却没有工程根时报错——那说明场景声明了
    /// 一个无法解析的路径，静默跳过只会让"脚本没跑"变成谜题。
    pub fn sync_entity_scripts(&mut self) -> Result<usize, LuaBridgeError> {
        self.refresh_entity_script_requests();
        if self.entity_script_requests.is_empty() {
            return Ok(0);
        }
        // P0-②：稳态快速路径——所有请求的文件都已加载、实体都已 on_spawn，
        // 直接返回 0，不 clone 请求表、也不碰工程根。
        let settled = self.entity_script_requests.iter().all(|(id, file)| {
            self.entity_scripts.contains_key(file) && self.entity_scripts_spawned.contains(id)
        });
        if settled {
            return Ok(0);
        }
        let requests = self.entity_script_requests.clone();
        let root = self.state.borrow().project_root.clone().ok_or_else(|| {
            LuaBridgeError::Api(
                "entity scripts require a project root (set_project_root)".to_owned(),
            )
        })?;
        for (_, file) in &requests {
            if self.entity_scripts.contains_key(file) {
                continue;
            }
            let path = root.join(file);
            let source = read_project_script(&path)?;
            let lua = self.load_candidate(&path, &source)?;
            let fingerprint = file_fingerprint(&path, &source)?;
            self.entity_scripts
                .insert(file.clone(), EntityScriptState { lua, fingerprint });
        }
        let mut spawned = 0;
        for (id, file) in requests {
            if !self.entity_scripts_spawned.insert(id) {
                continue;
            }
            let state = self
                .entity_scripts
                .get(&file)
                .expect("脚本刚刚已加载（同文件只加载一次）");
            // 生成钩子失败也**照记 spawned**：不记就会每帧重试同一个必炸的钩子，
            // 而实体本身已经进世界了（重试并不会让它更完整）。
            match call_entity_hook(&state.lua, "on_spawn", id, None, Path::new(&file)) {
                Ok(()) => {
                    self.guard
                        .borrow_mut()
                        .record_success(BudgetSlot::Entity(id, EntityHook::Spawn));
                }
                Err(error) => self.contain_entity_hook(id, EntityHook::Spawn, error)?,
            }
            spawned += 1;
        }
        Ok(spawned)
    }

    /// 每步调用所有启用脚本的 `on_update(id, dt)`，返回实际调用的实体数。
    ///
    /// 先做一次 [`Self::sync_entity_scripts`]，因此**运行时新生成的实体**（Lua `entity.spawn`、
    /// 预制体实例化）下一帧就会拿到自己的 `on_spawn`/`on_update`，不需要宿主额外接线。
    ///
    /// 容错档下某个实体的 `on_update` 报错只跳过**它自己**（不是整个循环，更不是整个固定步）；
    /// 连续报错达上限的实体会被停用，此后不再进 Lua。
    pub fn call_entity_scripts(&mut self, delta_seconds: f32) -> Result<usize, LuaBridgeError> {
        self.sync_entity_scripts()?;
        // `sync_entity_scripts` 内部已刷新过缓存；这里再确认一次（修订号没变则零成本）。
        self.refresh_entity_script_requests();
        let mut called = 0;
        for (id, file) in &self.entity_script_requests {
            let Some(state) = self.entity_scripts.get(file) else {
                continue;
            };
            let slot = BudgetSlot::Entity(*id, EntityHook::Update);
            if self.guard.borrow().is_disabled(slot) {
                continue;
            }
            match call_entity_hook(
                &state.lua,
                "on_update",
                *id,
                Some(delta_seconds),
                Path::new(file),
            ) {
                Ok(()) => self.guard.borrow_mut().record_success(slot),
                Err(error) => self.contain_entity_hook(*id, EntityHook::Update, error)?,
            }
            called += 1;
        }
        Ok(called)
    }

    /// 热重载：文件指纹变了的脚本重新读入并替换 Lua 状态（返回重载的文件数）。
    ///
    /// 重载会**清掉该文件 Lua 状态里的一切**（顶层局部变量、模块缓存），因此文档写明：每实体
    /// 状态要么放在实体组件里，要么放在脚本内以 `id` 为键的表里，并接受重载时重建。
    pub fn reload_entity_scripts(&mut self) -> Result<usize, LuaBridgeError> {
        let root = match self.state.borrow().project_root.clone() {
            Some(root) => root,
            None => return Ok(0),
        };
        let files: Vec<String> = self.entity_scripts.keys().cloned().collect();
        let mut reloaded = 0;
        for file in files {
            let path = root.join(&file);
            let source = read_project_script(&path)?;
            let fingerprint = file_fingerprint(&path, &source)?;
            let unchanged = self
                .entity_scripts
                .get(&file)
                .is_some_and(|state| state.fingerprint == fingerprint);
            if unchanged {
                continue;
            }
            let lua = self.load_candidate(&path, &source)?;
            self.entity_scripts
                .insert(file, EntityScriptState { lua, fingerprint });
            reloaded += 1;
        }
        Ok(reloaded)
    }

    /// 当前已加载的每实体脚本文件（诊断/验收用，按路径排序）。
    #[must_use]
    pub fn entity_script_files(&self) -> Vec<String> {
        let mut files: Vec<String> = self.entity_scripts.keys().cloned().collect();
        files.sort();
        files
    }

    /// P0-②：刷新"启用的每实体脚本请求（按实体 id 排序）"缓存。
    ///
    /// 失效判据是 `World::revision`：只有组件结构（insert/remove/despawn）变化才会重建。
    /// 稳态（脚本实体不增不减）下本函数是 O(1)，此前是每帧 O(n log n) 的全量遍历 + 排序。
    fn refresh_entity_script_requests(&mut self) {
        let revision = self.state.borrow().world.revision();
        if revision == self.entity_script_requests_revision {
            return;
        }
        let mut requests: Vec<(EntityId, String)> = Vec::new();
        {
            let state = self.state.borrow();
            state
                .world
                .for_each::<rustlua_ecs::entity_script::EntityScript>(|id, script| {
                    if script.enabled {
                        requests.push((id, script.file.clone()));
                    }
                });
        }
        requests.sort_by_key(|(id, _)| *id);
        // 消亡实体的容错预算在这里剪掉：`EntityId` 的槽位会被复用、generation 每次都不同，
        // 不剪就是"每生成一次实体泄漏一条预算"。
        self.guard
            .borrow_mut()
            .prune_entities(requests.iter().map(|(id, _)| id));
        self.entity_script_requests = requests;
        self.entity_script_requests_revision = revision;
    }

    fn load_candidate(&self, path: &Path, source: &str) -> Result<Lua, LuaBridgeError> {
        let candidate = Lua::new();
        api::register_all(&candidate, Rc::clone(&self.state))
            .map_err(|error| LuaBridgeError::Init(error.to_string()))?;
        // 候选状态是**新造的 Lua 状态**，不带 `LuaRuntime::new()` 装过的那套钩子——而
        // `execute_project_script` 恰恰是在它上面跑工程脚本的（wasm 上卡死的那一段就是这里）。
        // 所以钩子必须在**这里**装：预算沿用当前值，周期从这一刻清零。
        progress::install(&candidate, progress::budget())
            .map_err(|error| LuaBridgeError::Init(error.to_string()))?;
        progress::arm();
        // Let Lua `require` resolve sibling modules relative to the project
        // script directory (e.g. `require("player")` -> `scripts/player.lua`).
        if let Ok(package) = candidate.globals().get::<Table>("package")
            && let Ok(existing) = package.get::<String>("path")
            && let Some(script_dir) = path.parent()
        {
            let module_path = format!("{};{}", script_dir.join("?.lua").display(), existing);
            let _ = package.set("path", module_path);
        }
        // 拦截 require：支持压缩/加密的模块文件（明文模块同样可用）。
        if let Some(script_dir) = path.parent() {
            let _ = install_encoded_require_searcher(&candidate, script_dir.to_path_buf());
        }
        candidate
            .load(source)
            // P2-⑤：chunk 名用**实际源路径**（实体脚本 / 重载文件的报错才会指对地方）。
            .set_name(path.display().to_string())
            .exec()
            .map_err(|error| LuaBridgeError::Load {
                path: path.to_path_buf(),
                reason: error.to_string(),
            })?;
        Ok(candidate)
    }
    /// 调用工程级 `update()`。
    ///
    /// **错误处置**：`Strict`（默认）把 Lua 错误原样上报给宿主；`Contain` 把错误记进
    /// 结构化诊断与预算后返回 `Ok("")`，让宿主继续跑（见 [`ScriptErrorPolicy`]）。
    /// 连续报错达到上限的回调会被**停用**——此后不再进 Lua，日志也不会被同一个错误淹没。
    pub fn call_update(&self) -> Result<String, LuaBridgeError> {
        // 每个 `update` 回调一个周期：预算是"这一段不许无限跑"，不是"这一局不许跑够 N 条"。
        progress::arm();
        let path = self
            .current_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("<project script>"));
        // 已停用的回调直接短路：既省下每帧的报错开销，也避免"停用"变成一句空话。
        if self.guard.borrow().is_disabled(BudgetSlot::Update) {
            self.state.borrow_mut().last_update_micros = 0;
            return Ok(String::new());
        }
        let update: Option<Function> = self
            .lua
            .globals()
            .get("update")
            .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        // No script loaded (engine-only diagnostic mode) or the script has no
        // `update` function: the game simply idles instead of erroring out.
        let Some(update) = update else {
            self.state.borrow_mut().last_update_micros = 0;
            return Ok(String::new());
        };
        // 计时用 `web_time`：wasm32-unknown-unknown 的 std 不实现 `Instant`（unsupported.rs
        // 直接 panic），`web_time` 在原生上就是它的再导出，所以两边**同一段代码**即可。
        //
        // 以前这里是 `#[cfg(not(target_arch = "wasm32"))]` + wasm 侧记 0，那是"std 时钟会炸"
        // 时代的权宜：后果是 Web 上性能读数里 Lua `update` 的成本**永远是 0**，
        // 于是"每帧逻辑贵在哪"没法回答（本轮的前提就是修掉它）。
        let started = web_time::Instant::now();
        let result = update.call(()).map_err(|error| LuaBridgeError::Runtime {
            path,
            reason: error.to_string(),
        });
        // 失败也要记账：否则"Lua 抛错的那一帧"在性能面板上看起来是零成本。
        self.state.borrow_mut().last_update_micros =
            u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        match result {
            Ok(value) => {
                self.guard.borrow_mut().record_success(BudgetSlot::Update);
                Ok(value)
            }
            Err(error) => self.contain_or_propagate(BudgetSlot::Update, error),
        }
    }

    /// 按策略处置一次**工程级回调**错误。
    ///
    /// 严格档**不**记诊断：与容器化之前的语义逐字一致（由 `call_update_debug` 负责记录），
    /// 否则 `debugger_extracts_location_deduplicates_and_filters` 里的 `occurrences`
    /// 计数会被记成两倍。
    fn contain_or_propagate(
        &self,
        slot: BudgetSlot,
        error: LuaBridgeError,
    ) -> Result<String, LuaBridgeError> {
        if self.guard.borrow().policy() == ScriptErrorPolicy::Strict {
            return Err(error);
        }
        self.contain_script_error(slot, &error);
        Ok(String::new())
    }

    /// 记一次被容错的回调错误：计数 + 结构化诊断 + 去重日志。
    ///
    /// `pub(super)`：UI 点击分发的容错入口在 `scene_spawn` 里，与这里同属 `script` 模块。
    /// 借用刻意**不跨越 Lua 调用**（这里的 Lua 调用已经返回了）：`RefCell` 的两次
    /// `borrow_mut` 各占一条语句的生存期。
    pub(super) fn contain_script_error(&self, slot: BudgetSlot, error: &LuaBridgeError) {
        let outcome = self.guard.borrow_mut().record_error(slot);
        let (path, phase, reason) = bridge_error_parts(error);
        let is_new = self
            .debugger
            .borrow_mut()
            .record(path.clone(), phase, reason.clone());
        match outcome {
            // 停用要单独吼一次：此前每帧一条的报错到这里结束，玩家/开发者必须知道
            // "这个回调已经不跑了"，否则表现为"某个系统莫名其妙不再更新"。
            ContainOutcome::Disabled => tracing::error!(
                target: "script",
                callback = %slot.key(),
                path = %path.display(),
                "[lua-guard] 回调连续报错已停用（不再调用）：{reason}"
            ),
            // 每个**不同**的错误只打一条：脚本每帧炸同一行时，日志里应留得下别的信息。
            ContainOutcome::Continue if is_new => tracing::error!(
                target: "script",
                callback = %slot.key(),
                path = %path.display(),
                "[lua-guard] 脚本错误已容错（本帧继续）：{reason}"
            ),
            ContainOutcome::Continue => {}
        }
    }

    /// 每实体钩子的容错入口：`Ok(())` = 已容错（调用方继续下一个实体），
    /// `Err` = 严格档要求上报。
    fn contain_entity_hook(
        &self,
        id: EntityId,
        hook: EntityHook,
        error: LuaBridgeError,
    ) -> Result<(), LuaBridgeError> {
        if self.guard.borrow().policy() == ScriptErrorPolicy::Strict {
            return Err(error);
        }
        self.contain_script_error(BudgetSlot::Entity(id, hook), &error);
        Ok(())
    }

    /// 上一次 `call_update` 内部 Lua 调用的耗时（微秒）。宿主用它填 `lua_ms`：
    /// 只算 Lua 体，不含 ECS/物理/渲染，指标名与含义因此一致。
    #[must_use]
    pub fn last_update_micros(&self) -> u64 {
        self.state.borrow().last_update_micros
    }

    pub fn call_update_debug(&mut self) -> Result<String, LuaBridgeError> {
        match self.call_update() {
            Ok(value) => Ok(value),
            Err(error) => {
                self.record_bridge_error(&error);
                Err(error)
            }
        }
    }

    /// 访问底层 mlua 状态（wasm Lua 桥用：注入宿主回调 / 调用自定义全局 /
    /// 执行补丁代码串）。只读借用；mlua 状态是单线程语义（wasm 单线程、
    /// 桌面线程内），调用方不得跨线程持有。
    #[must_use]
    pub fn lua(&self) -> &Lua {
        &self.lua
    }
}
