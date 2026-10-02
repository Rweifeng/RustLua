//! Deterministic stage-based system scheduling for the ECS world.

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet},
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    time::{Duration, Instant},
};

use crate::world::World;

/// The fixed execution stages of the runtime frame.
///
/// Stage order (lower rank runs earlier): Input → Script → Physics →
/// Simulation → Animation → Transform → Render. `Simulation` (M18-03) hosts
/// world-advancing steps that are neither collision physics nor animation:
/// particles, the game clock and pending scene transitions.
///
/// WP7 收敛后的统一帧序（负责人裁定）：脚本 → 控制器 → 静态/运动学同步 → 物理 →
/// 动态回写 → GlobalTransform → RenderSnapshot。对应到阶段就是
/// [`ScheduleStage::Script`] → [`ScheduleStage::Sync`] → [`ScheduleStage::Physics`] →
/// [`ScheduleStage::Writeback`] → [`ScheduleStage::Spatial`] → 提取（宿主在
/// `Transform`/`Render` 阶段之后自己做）。中间那三段是本轮新增的**类型化术语**：
/// 此前"同步/回写/世界变换传播"分别散在 `step_physics`、宿主末尾和提取入口里，
/// 顺序只存在于注释与调用顺序中。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ScheduleStage {
    Input,
    Script,
    /// 物理步之前的静态/运动学同步（把 ECS 侧权威位置推给模拟世界）。
    Sync,
    Physics,
    /// 物理步之后的动态回写（把模拟结果写回 ECS）。
    Writeback,
    Simulation,
    Animation,
    /// `GlobalTransform` 传播（父链组合世界变换）。
    Spatial,
    Transform,
    Render,
}

impl ScheduleStage {
    /// Execution rank: lower runs earlier within a frame.
    #[must_use]
    pub const fn rank(self) -> usize {
        match self {
            Self::Input => 0,
            Self::Script => 1,
            Self::Sync => 2,
            Self::Physics => 3,
            Self::Writeback => 4,
            Self::Simulation => 5,
            Self::Animation => 6,
            Self::Spatial => 7,
            Self::Transform => 8,
            Self::Render => 9,
        }
    }
}

impl fmt::Display for ScheduleStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Input => "Input",
            Self::Script => "Script",
            Self::Sync => "Sync",
            Self::Physics => "Physics",
            Self::Writeback => "Writeback",
            Self::Simulation => "Simulation",
            Self::Animation => "Animation",
            Self::Spatial => "Spatial",
            Self::Transform => "Transform",
            Self::Render => "Render",
        };
        formatter.write_str(name)
    }
}

/// Error returned by a system callback.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct SystemError {
    message: String,
}

impl SystemError {
    /// Creates a system error with a human-readable message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Errors produced while validating or executing a schedule.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SchedulerError {
    #[error("duplicate system: {name}")]
    DuplicateSystem { name: String },
    #[error("system {system} depends on missing system {dependency}")]
    MissingDependency { system: String, dependency: String },
    #[error(
        "system {system} ({system_stage}) cannot depend on later system {dependency} ({dependency_stage})"
    )]
    InvalidStageDependency {
        system: String,
        dependency: String,
        system_stage: ScheduleStage,
        dependency_stage: ScheduleStage,
    },
    #[error("system dependency cycle: {}", .systems.join(" -> "))]
    DependencyCycle { systems: Vec<String> },
    #[error("invalid scheduler delta time: {delta_seconds}")]
    InvalidDeltaTime { delta_seconds: String },
    #[error("system {system} in {stage} failed: {message}")]
    SystemFailed {
        system: String,
        stage: ScheduleStage,
        message: String,
    },
    #[error(
        "system {system} in {stage} panicked{}",
        if .message.is_empty() { String::new() } else { format!(": {}", .message) }
    )]
    SystemPanicked {
        system: String,
        stage: ScheduleStage,
        /// Panic payload when it is a `&str`/`String`; empty otherwise.
        message: String,
    },
}

/// A system callback receives the scheduling context (`&mut World` for the
/// default [`Scheduler`]; other runtime pipelines may schedule over their own
/// context type) plus the delta seconds of the current step.
///
/// The runtime schedules on a single thread, so callbacks are not required to
/// be `Send` (M18-03): script-backed systems capture non-`Send` Lua state.
type SystemCallback<C> = Box<dyn FnMut(&mut C, f32) -> Result<(), SystemError> + 'static>;

/// A named system with a stage, dependencies and executable callback.
pub struct System<C = World> {
    name: String,
    stage: ScheduleStage,
    dependencies: Vec<String>,
    callback: SystemCallback<C>,
    registration_order: usize,
}

impl<C: 'static> System<C> {
    /// Creates a system. Dependencies are names of systems registered in the same scheduler.
    pub fn new<F>(
        name: impl Into<String>,
        stage: ScheduleStage,
        dependencies: impl IntoIterator<Item = impl Into<String>>,
        callback: F,
    ) -> Self
    where
        F: FnMut(&mut C, f32) -> Result<(), SystemError> + 'static,
    {
        Self {
            name: name.into(),
            stage,
            dependencies: dependencies.into_iter().map(Into::into).collect(),
            callback: Box::new(callback),
            registration_order: 0,
        }
    }

    /// Returns the system name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the system stage.
    #[must_use]
    pub const fn stage(&self) -> ScheduleStage {
        self.stage
    }

    /// Returns the declared dependency names.
    #[must_use]
    pub fn dependencies(&self) -> &[String] {
        &self.dependencies
    }
}

/// Timing information collected for one executed system.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemTiming {
    pub name: String,
    pub stage: ScheduleStage,
    pub elapsed: Duration,
}

/// Result of one scheduler frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScheduleReport {
    pub timings: Vec<SystemTiming>,
    pub total_elapsed: Duration,
}

impl ScheduleReport {
    /// Returns the number of systems that ran.
    #[must_use]
    pub fn executed_systems(&self) -> usize {
        self.timings.len()
    }
}

/// Owns systems and executes them in validated deterministic order.
///
/// The context type parameter defaults to [`World`]. The runtime frame
/// pipeline (M18-03) instantiates `Scheduler<RuntimeScene>` so the fixed
/// script/physics/clock/animation steps share one scheduling entry with
/// dependency validation and per-system timings.
pub struct Scheduler<C = World> {
    systems: Vec<System<C>>,
    /// 缓存的拓扑执行序（索引到 `systems`）。注册变化时失效。
    ///
    /// 拓扑排序每次调用都要建名字表 + 算入度，在固定步循环里每步一次是纯浪费：
    /// 运行期（如 `RuntimeScene::run_script_steps` 一帧多步）会连续调用 `run`
    /// 很多次，而系统集合只在注册时变化。
    cached_order: Option<Vec<usize>>,
}

impl<C> Default for Scheduler<C> {
    fn default() -> Self {
        Self {
            systems: Vec::new(),
            cached_order: None,
        }
    }
}

impl<C: 'static> Scheduler<C> {
    /// Creates an empty scheduler.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of registered systems.
    #[must_use]
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    /// Returns `true` when no system is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    /// Returns the registered system names in registration order.
    #[must_use]
    pub fn system_names(&self) -> Vec<String> {
        self.systems
            .iter()
            .map(|system| system.name.clone())
            .collect()
    }

    /// Moves every registered system back out in registration order.
    #[must_use]
    pub fn into_systems(self) -> Vec<System<C>> {
        self.systems
    }

    /// Registers a system and rejects duplicate names.
    pub fn add_system(&mut self, mut system: System<C>) -> Result<(), SchedulerError> {
        if self
            .systems
            .iter()
            .any(|registered| registered.name == system.name)
        {
            return Err(SchedulerError::DuplicateSystem { name: system.name });
        }
        system.registration_order = self.systems.len();
        self.systems.push(system);
        // 注册变化 → 缓存的执行序失效。
        self.cached_order = None;
        Ok(())
    }

    /// Validates dependencies and returns the deterministic execution order.
    pub fn execution_order(&self) -> Result<Vec<String>, SchedulerError> {
        let indices = self.topological_order()?;
        Ok(indices
            .into_iter()
            .map(|index| self.systems[index].name.clone())
            .collect())
    }

    /// 缓存的执行序：与 `topological_order` 结果相同，但只在注册变化后重算一次。
    fn plan(&mut self) -> Result<Vec<usize>, SchedulerError> {
        if let Some(order) = &self.cached_order {
            return Ok(order.clone());
        }
        let order = self.topological_order()?;
        self.cached_order = Some(order.clone());
        Ok(order)
    }

    /// Executes every registered system once for one simulation step.
    pub fn run(
        &mut self,
        context: &mut C,
        delta_seconds: f32,
    ) -> Result<ScheduleReport, SchedulerError> {
        if !delta_seconds.is_finite() || delta_seconds < 0.0 {
            return Err(SchedulerError::InvalidDeltaTime {
                delta_seconds: delta_seconds.to_string(),
            });
        }
        let order = self.plan()?;
        let started = Instant::now();
        let mut report = ScheduleReport::default();
        for index in order {
            let system = &mut self.systems[index];
            let system_started = Instant::now();
            let name = system.name.clone();
            let stage = system.stage;
            let result = catch_unwind(AssertUnwindSafe(|| {
                (system.callback)(context, delta_seconds)
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    return Err(SchedulerError::SystemFailed {
                        system: name,
                        stage,
                        message: error.to_string(),
                    });
                }
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| {
                            payload
                                .downcast_ref::<&'static str>()
                                .map(|message| (*message).to_string())
                        })
                        .unwrap_or_default();
                    return Err(SchedulerError::SystemPanicked {
                        system: name,
                        stage,
                        message,
                    });
                }
            }
            report.timings.push(SystemTiming {
                name,
                stage,
                elapsed: system_started.elapsed(),
            });
        }
        report.total_elapsed = started.elapsed();
        Ok(report)
    }

    fn topological_order(&self) -> Result<Vec<usize>, SchedulerError> {
        let names: HashMap<&str, usize> = self
            .systems
            .iter()
            .enumerate()
            .map(|(index, system)| (system.name.as_str(), index))
            .collect();
        let mut indegree = vec![0usize; self.systems.len()];
        let mut outgoing = vec![Vec::new(); self.systems.len()];
        for (index, system) in self.systems.iter().enumerate() {
            let mut seen = HashSet::new();
            for dependency in &system.dependencies {
                if !seen.insert(dependency.as_str()) {
                    continue;
                }
                let Some(&dependency_index) = names.get(dependency.as_str()) else {
                    return Err(SchedulerError::MissingDependency {
                        system: system.name.clone(),
                        dependency: dependency.clone(),
                    });
                };
                let dependency_system = &self.systems[dependency_index];
                if dependency_system.stage > system.stage {
                    return Err(SchedulerError::InvalidStageDependency {
                        system: system.name.clone(),
                        dependency: dependency.clone(),
                        system_stage: system.stage,
                        dependency_stage: dependency_system.stage,
                    });
                }
                indegree[index] += 1;
                outgoing[dependency_index].push(index);
            }
        }

        let mut ready = BinaryHeap::new();
        for (index, &degree) in indegree.iter().enumerate() {
            if degree == 0 {
                ready.push(Reverse((
                    self.systems[index].stage.rank(),
                    self.systems[index].registration_order,
                    index,
                )));
            }
        }
        let mut order = Vec::with_capacity(self.systems.len());
        while let Some(Reverse((_, _, index))) = ready.pop() {
            order.push(index);
            for dependent in &outgoing[index] {
                indegree[*dependent] -= 1;
                if indegree[*dependent] == 0 {
                    let system = &self.systems[*dependent];
                    ready.push(Reverse((
                        system.stage.rank(),
                        system.registration_order,
                        *dependent,
                    )));
                }
            }
        }
        if order.len() != self.systems.len() {
            let systems = self
                .systems
                .iter()
                .enumerate()
                .filter_map(|(index, system)| (indegree[index] > 0).then_some(system.name.clone()))
                .collect();
            return Err(SchedulerError::DependencyCycle { systems });
        }
        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{ScheduleStage, Scheduler, SchedulerError, System, SystemError};
    use crate::world::World;

    fn recording_system(
        name: &'static str,
        stage: ScheduleStage,
        log: Arc<Mutex<Vec<&'static str>>>,
        dependencies: &'static [&'static str],
    ) -> System {
        System::new(
            name,
            stage,
            dependencies.iter().copied(),
            move |_: &mut World, _| {
                log.lock().unwrap().push(name);
                Ok(())
            },
        )
    }

    #[test]
    fn runs_systems_in_stage_order_and_records_timings() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut scheduler = Scheduler::new();
        scheduler
            .add_system(recording_system(
                "render",
                ScheduleStage::Render,
                log.clone(),
                &[],
            ))
            .unwrap();
        scheduler
            .add_system(recording_system(
                "input",
                ScheduleStage::Input,
                log.clone(),
                &[],
            ))
            .unwrap();
        scheduler
            .add_system(recording_system(
                "transform",
                ScheduleStage::Transform,
                log.clone(),
                &[],
            ))
            .unwrap();

        let mut world = World::new();
        let report = scheduler.run(&mut world, 1.0 / 60.0).unwrap();
        assert_eq!(*log.lock().unwrap(), vec!["input", "transform", "render"]);
        assert_eq!(report.executed_systems(), 3);
        assert_eq!(report.timings[0].stage, ScheduleStage::Input);
        assert!(report.total_elapsed >= report.timings[0].elapsed);
    }

    /// 缓存的执行序必须在注册变化后失效：否则新系统的依赖错误会被旧计划掩盖，
    /// 调度器会"照旧跑"一个已经不成立的顺序。
    #[test]
    fn cached_execution_order_is_invalidated_by_registration() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut scheduler = Scheduler::new();
        scheduler
            .add_system(recording_system(
                "a",
                ScheduleStage::Input,
                log.clone(),
                &[],
            ))
            .unwrap();
        let mut world = World::new();
        // 第一次 run 建立缓存计划。
        let report = scheduler.run(&mut world, 1.0 / 60.0).unwrap();
        assert_eq!(report.executed_systems(), 1);

        // 新增一个依赖不存在名字的系统：注册本身合法，问题在计划校验。
        scheduler
            .add_system(recording_system(
                "b",
                ScheduleStage::Transform,
                log.clone(),
                &["does_not_exist"],
            ))
            .unwrap();
        let error = scheduler
            .run(&mut world, 1.0 / 60.0)
            .expect_err("新增系统后必须重新校验计划");
        assert_eq!(
            error,
            SchedulerError::MissingDependency {
                system: "b".to_owned(),
                dependency: "does_not_exist".to_owned(),
            }
        );

        // 错误持续存在（没有用旧计划"蒙混过关"），两个系统都还在注册表里。
        assert_eq!(scheduler.len(), 2);
        assert!(matches!(
            scheduler.run(&mut world, 1.0 / 60.0),
            Err(SchedulerError::MissingDependency { .. })
        ));

        // 另一条路径：**合法**注册之后，缓存计划必须反映新的执行序。
        let repaired = Arc::new(Mutex::new(Vec::new()));
        let mut scheduler = Scheduler::new();
        scheduler
            .add_system(recording_system(
                "a",
                ScheduleStage::Input,
                repaired.clone(),
                &[],
            ))
            .unwrap();
        scheduler.run(&mut world, 1.0 / 60.0).unwrap();
        scheduler
            .add_system(recording_system(
                "c",
                ScheduleStage::Render,
                repaired.clone(),
                &["a"],
            ))
            .unwrap();
        let report = scheduler.run(&mut world, 1.0 / 60.0).unwrap();
        assert_eq!(report.executed_systems(), 2);
        assert_eq!(*repaired.lock().unwrap(), vec!["a", "a", "c"]);
    }

    /// 被拒绝的注册（重名）不得破坏已有计划，也不得让系统集合"少一个还在跑"。
    #[test]
    fn rejected_registration_leaves_the_schedule_intact() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut scheduler = Scheduler::new();
        scheduler
            .add_system(recording_system(
                "a",
                ScheduleStage::Input,
                log.clone(),
                &[],
            ))
            .unwrap();
        scheduler
            .add_system(recording_system(
                "b",
                ScheduleStage::Physics,
                log.clone(),
                &["a"],
            ))
            .unwrap();
        let mut world = World::new();
        scheduler.run(&mut world, 1.0 / 60.0).unwrap();
        assert_eq!(*log.lock().unwrap(), vec!["a", "b"]);

        let error = scheduler
            .add_system(recording_system(
                "a",
                ScheduleStage::Render,
                log.clone(),
                &[],
            ))
            .expect_err("重名必须被拒绝");
        assert_eq!(
            error,
            SchedulerError::DuplicateSystem {
                name: "a".to_owned()
            }
        );

        // 拒绝之后：系统数不变、执行序不变、两个系统都还在跑。
        assert_eq!(scheduler.len(), 2);
        assert_eq!(scheduler.execution_order().unwrap(), vec!["a", "b"]);
        log.lock().unwrap().clear();
        scheduler.run(&mut world, 1.0 / 60.0).unwrap();
        assert_eq!(*log.lock().unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn dependencies_define_order_within_and_across_stages() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut scheduler = Scheduler::new();
        scheduler
            .add_system(recording_system(
                "transform",
                ScheduleStage::Transform,
                log.clone(),
                &["physics"],
            ))
            .unwrap();
        scheduler
            .add_system(recording_system(
                "physics",
                ScheduleStage::Physics,
                log.clone(),
                &[],
            ))
            .unwrap();
        assert_eq!(
            scheduler.execution_order().unwrap(),
            vec!["physics", "transform"]
        );
    }

    #[test]
    fn rejects_missing_duplicate_and_late_dependencies() {
        let mut scheduler = Scheduler::new();
        scheduler
            .add_system(System::new(
                "a",
                ScheduleStage::Input,
                std::iter::empty::<&'static str>(),
                |_: &mut World, _| Ok(()),
            ))
            .unwrap();
        assert_eq!(
            scheduler.add_system(System::new(
                "a",
                ScheduleStage::Input,
                std::iter::empty::<&'static str>(),
                |_: &mut World, _| Ok(())
            )),
            Err(SchedulerError::DuplicateSystem { name: "a".into() })
        );
        scheduler
            .add_system(System::new(
                "b",
                ScheduleStage::Script,
                ["missing"],
                |_: &mut World, _| Ok(()),
            ))
            .unwrap();
        assert!(matches!(
            scheduler.execution_order(),
            Err(SchedulerError::MissingDependency { .. })
        ));
        let mut late = Scheduler::new();
        late.add_system(System::new(
            "render",
            ScheduleStage::Render,
            std::iter::empty::<&'static str>(),
            |_: &mut World, _| Ok(()),
        ))
        .unwrap();
        late.add_system(System::new(
            "input",
            ScheduleStage::Input,
            ["render"],
            |_: &mut World, _| Ok(()),
        ))
        .unwrap();
        assert!(matches!(
            late.execution_order(),
            Err(SchedulerError::InvalidStageDependency { .. })
        ));
    }

    #[test]
    fn detects_dependency_cycles() {
        let mut scheduler = Scheduler::new();
        scheduler
            .add_system(System::new(
                "a",
                ScheduleStage::Physics,
                ["b"],
                |_: &mut World, _| Ok(()),
            ))
            .unwrap();
        scheduler
            .add_system(System::new(
                "b",
                ScheduleStage::Physics,
                ["a"],
                |_: &mut World, _| Ok(()),
            ))
            .unwrap();
        assert!(matches!(
            scheduler.execution_order(),
            Err(SchedulerError::DependencyCycle { .. })
        ));
    }

    #[test]
    fn reports_system_errors_and_panics_with_context() {
        let mut error_scheduler = Scheduler::new();
        error_scheduler
            .add_system(System::new(
                "movement",
                ScheduleStage::Transform,
                std::iter::empty::<&'static str>(),
                |_: &mut World, _| Err(SystemError::new("invalid transform")),
            ))
            .unwrap();
        let mut world = World::new();
        assert_eq!(
            error_scheduler.run(&mut world, 0.1),
            Err(SchedulerError::SystemFailed {
                system: "movement".into(),
                stage: ScheduleStage::Transform,
                message: "invalid transform".into(),
            })
        );

        let mut panic_scheduler = Scheduler::new();
        panic_scheduler
            .add_system(System::new(
                "broken",
                ScheduleStage::Script,
                std::iter::empty::<&'static str>(),
                |_: &mut World, _| panic!("boom"),
            ))
            .unwrap();
        assert_eq!(
            panic_scheduler.run(&mut world, 0.1),
            Err(SchedulerError::SystemPanicked {
                system: "broken".into(),
                stage: ScheduleStage::Script,
                message: "boom".into(),
            })
        );
    }

    #[test]
    fn system_panicked_display_includes_payload_message() {
        let error = SchedulerError::SystemPanicked {
            system: "broken".into(),
            stage: ScheduleStage::Simulation,
            message: String::new(),
        };
        assert_eq!(error.to_string(), "system broken in Simulation panicked");
        let error = SchedulerError::SystemPanicked {
            system: "broken".into(),
            stage: ScheduleStage::Simulation,
            message: "index out of bounds".into(),
        };
        assert_eq!(
            error.to_string(),
            "system broken in Simulation panicked: index out of bounds"
        );
    }

    #[test]
    fn rejects_invalid_delta_time_before_running_systems() {
        let mut scheduler = Scheduler::new();
        scheduler
            .add_system(System::new(
                "input",
                ScheduleStage::Input,
                std::iter::empty::<&'static str>(),
                |_: &mut World, _| Ok(()),
            ))
            .unwrap();
        let mut world = World::new();
        assert!(matches!(
            scheduler.run(&mut world, f32::NAN),
            Err(SchedulerError::InvalidDeltaTime { .. })
        ));
    }
}
