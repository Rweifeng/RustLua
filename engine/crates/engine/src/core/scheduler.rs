//! Deterministic stage-based system scheduling for the ECS world.

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet},
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    time::{Duration, Instant},
};

use crate::core::world::World;

/// The fixed execution stages of the runtime frame.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ScheduleStage {
    Input,
    Script,
    Physics,
    Animation,
    Transform,
    Render,
}

impl ScheduleStage {
    const fn rank(self) -> usize {
        match self {
            Self::Input => 0,
            Self::Script => 1,
            Self::Physics => 2,
            Self::Animation => 3,
            Self::Transform => 4,
            Self::Render => 5,
        }
    }
}

impl fmt::Display for ScheduleStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Input => "Input",
            Self::Script => "Script",
            Self::Physics => "Physics",
            Self::Animation => "Animation",
            Self::Transform => "Transform",
            Self::Render => "Render",
        };
        formatter.write_str(name)
    }
}

/// Error returned by a system callback.
#[derive(Clone, Debug, Eq, PartialEq)]
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

impl fmt::Display for SystemError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SystemError {}

/// Errors produced while validating or executing a schedule.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchedulerError {
    DuplicateSystem {
        name: String,
    },
    MissingDependency {
        system: String,
        dependency: String,
    },
    InvalidStageDependency {
        system: String,
        dependency: String,
        system_stage: ScheduleStage,
        dependency_stage: ScheduleStage,
    },
    DependencyCycle {
        systems: Vec<String>,
    },
    InvalidDeltaTime {
        delta_seconds: String,
    },
    SystemFailed {
        system: String,
        stage: ScheduleStage,
        message: String,
    },
    SystemPanicked {
        system: String,
        stage: ScheduleStage,
    },
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSystem { name } => write!(formatter, "duplicate system: {name}"),
            Self::MissingDependency { system, dependency } => {
                write!(
                    formatter,
                    "system {system} depends on missing system {dependency}"
                )
            }
            Self::InvalidStageDependency {
                system,
                dependency,
                system_stage,
                dependency_stage,
            } => write!(
                formatter,
                "system {system} ({system_stage}) cannot depend on later system {dependency} ({dependency_stage})"
            ),
            Self::DependencyCycle { systems } => {
                write!(
                    formatter,
                    "system dependency cycle: {}",
                    systems.join(" -> ")
                )
            }
            Self::InvalidDeltaTime { delta_seconds } => {
                write!(formatter, "invalid scheduler delta time: {delta_seconds}")
            }
            Self::SystemFailed {
                system,
                stage,
                message,
            } => write!(formatter, "system {system} in {stage} failed: {message}"),
            Self::SystemPanicked { system, stage } => {
                write!(formatter, "system {system} in {stage} panicked")
            }
        }
    }
}

impl std::error::Error for SchedulerError {}

type SystemCallback = Box<dyn FnMut(&mut World, f32) -> Result<(), SystemError> + Send>;

/// A named system with a stage, dependencies and executable callback.
pub struct System {
    name: String,
    stage: ScheduleStage,
    dependencies: Vec<String>,
    callback: SystemCallback,
    registration_order: usize,
}

impl System {
    /// Creates a system. Dependencies are names of systems registered in the same scheduler.
    pub fn new<F>(
        name: impl Into<String>,
        stage: ScheduleStage,
        dependencies: impl IntoIterator<Item = impl Into<String>>,
        callback: F,
    ) -> Self
    where
        F: FnMut(&mut World, f32) -> Result<(), SystemError> + Send + 'static,
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
#[derive(Default)]
pub struct Scheduler {
    systems: Vec<System>,
}

impl Scheduler {
    /// Creates an empty scheduler.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a system and rejects duplicate names.
    pub fn add_system(&mut self, mut system: System) -> Result<(), SchedulerError> {
        if self
            .systems
            .iter()
            .any(|registered| registered.name == system.name)
        {
            return Err(SchedulerError::DuplicateSystem { name: system.name });
        }
        system.registration_order = self.systems.len();
        self.systems.push(system);
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

    /// Executes every registered system once for one simulation step.
    pub fn run(
        &mut self,
        world: &mut World,
        delta_seconds: f32,
    ) -> Result<ScheduleReport, SchedulerError> {
        if !delta_seconds.is_finite() || delta_seconds < 0.0 {
            return Err(SchedulerError::InvalidDeltaTime {
                delta_seconds: delta_seconds.to_string(),
            });
        }
        let order = self.topological_order()?;
        let started = Instant::now();
        let mut report = ScheduleReport::default();
        for index in order {
            let system = &mut self.systems[index];
            let system_started = Instant::now();
            let name = system.name.clone();
            let stage = system.stage;
            let result = catch_unwind(AssertUnwindSafe(|| (system.callback)(world, delta_seconds)));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    return Err(SchedulerError::SystemFailed {
                        system: name,
                        stage,
                        message: error.to_string(),
                    });
                }
                Err(_) => {
                    return Err(SchedulerError::SystemPanicked {
                        system: name,
                        stage,
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
    use crate::core::world::World;

    fn recording_system(
        name: &'static str,
        stage: ScheduleStage,
        log: Arc<Mutex<Vec<&'static str>>>,
        dependencies: &'static [&'static str],
    ) -> System {
        System::new(name, stage, dependencies.iter().copied(), move |_, _| {
            log.lock().unwrap().push(name);
            Ok(())
        })
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
                |_, _| Ok(()),
            ))
            .unwrap();
        assert_eq!(
            scheduler.add_system(System::new(
                "a",
                ScheduleStage::Input,
                std::iter::empty::<&'static str>(),
                |_, _| Ok(())
            )),
            Err(SchedulerError::DuplicateSystem { name: "a".into() })
        );
        scheduler
            .add_system(System::new(
                "b",
                ScheduleStage::Script,
                ["missing"],
                |_, _| Ok(()),
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
            |_, _| Ok(()),
        ))
        .unwrap();
        late.add_system(System::new(
            "input",
            ScheduleStage::Input,
            ["render"],
            |_, _| Ok(()),
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
            .add_system(System::new("a", ScheduleStage::Physics, ["b"], |_, _| {
                Ok(())
            }))
            .unwrap();
        scheduler
            .add_system(System::new("b", ScheduleStage::Physics, ["a"], |_, _| {
                Ok(())
            }))
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
                |_, _| Err(SystemError::new("invalid transform")),
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
                |_, _| panic!("boom"),
            ))
            .unwrap();
        assert_eq!(
            panic_scheduler.run(&mut world, 0.1),
            Err(SchedulerError::SystemPanicked {
                system: "broken".into(),
                stage: ScheduleStage::Script,
            })
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
                |_, _| Ok(()),
            ))
            .unwrap();
        let mut world = World::new();
        assert!(matches!(
            scheduler.run(&mut world, f32::NAN),
            Err(SchedulerError::InvalidDeltaTime { .. })
        ));
    }
}
