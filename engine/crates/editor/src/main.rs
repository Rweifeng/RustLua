use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
};

use editor::{ProjectError, ProjectInfo, check_project, open_project};
use eframe::egui;

#[derive(Debug, Default)]
struct Arguments {
    project: Option<PathBuf>,
    check: bool,
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut arguments = Arguments::default();
    let mut values = env::args().skip(1);
    while let Some(argument) = values.next() {
        match argument.as_str() {
            "--project" => {
                let path = values
                    .next()
                    .ok_or_else(|| "--project 需要一个路径参数".to_owned())?;
                arguments.project = Some(PathBuf::from(path));
            }
            "--check" => arguments.check = true,
            "--help" | "-h" => {
                println!("用法：editor [--project <项目路径>] [--check]");
                std::process::exit(0);
            }
            unknown => return Err(format!("未知参数：{unknown}")),
        }
    }
    Ok(arguments)
}

fn default_project_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .and_then(|path| path.parent())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = parse_arguments().map_err(|message| format!("参数错误：{message}"))?;
    let project_path = arguments.project.unwrap_or_else(default_project_path);
    if arguments.check {
        print!(
            "{}",
            check_project(&project_path)
                .map_err(|error| format_project_error(&project_path, error))?
        );
        return Ok(());
    }

    let project =
        open_project(&project_path).map_err(|error| format_project_error(&project_path, error))?;
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "P2 RustLua 编辑器",
        options,
        Box::new(move |_creation_context| Ok(Box::new(EditorApp::new(project)))),
    )?;
    Ok(())
}

fn format_project_error(path: &Path, error: ProjectError) -> String {
    format!("无法打开项目 {}：{}", path.display(), error)
}

struct EditorApp {
    project: ProjectInfo,
}

impl EditorApp {
    fn new(project: ProjectInfo) -> Self {
        Self { project }
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("header").show(context, |ui| {
            ui.heading("P2 RustLua 编辑器");
            ui.label(format!("项目：{}", self.project.root.display()));
        });
        egui::SidePanel::left("project_status").show(context, |ui| {
            ui.heading("项目状态");
            ui.label(format!("基线：{}", self.project.version));
            ui.separator();
            for directory in &self.project.directories {
                ui.label(format!("✓ {}/", directory.name));
            }
        });
        egui::CentralPanel::default().show(context, |ui| {
            ui.heading("编辑器 MVP");
            ui.label("M5-01 已打开项目。场景树、Inspector 和资源浏览器将在后续任务实现。");
            ui.separator();
            ui.label("当前只读项目状态，不会修改现有资源或场景文件。");
        });
    }
}
