//! 分包面板：`bundles.json` 的配置编辑 + 按发布同款编码器做**落盘字节预估**。
//!
//! 约束与图集面板相同：
//! 1. **只读工程、只写 `bundles.json`**——不产出容器、不改任何资源；
//! 2. 归约规则走 `rustlua_infra::bundle_plan`，与 `assetc` 同一份（`docs/67` §8 第 4 项）；
//! 3. 预估用的是 `BundleBuilder`（与发布同一个 zstd 编码器、同一个块大小），所以
//!    "4 MB 会不会超"这个判断是**真的**，不是按原始字节拍的。

use std::path::{Path, PathBuf};

use eframe::egui;
use rustlua_infra::bundle::BundleBuilder;
use rustlua_infra::bundle::plan::BundlePlan;
use rustlua_infra::bundle_plan::{
    self, BundleConfig, GroupSpec, MAX_BUNDLE_BYTES, MAX_TOTAL_BYTES, ProjectKeys, UnitKind,
    parse_unit,
};

use crate::EditorApp;

/// 预览用的块大小：与 `assetc` 默认一致（256 KB）。
const PREVIEW_BLOCK_SIZE: u32 = 256 * 1024;

/// 分包面板状态（挂在 `EditorApp` 上，切换工程时重播）。
pub(crate) struct BundlePanelState {
    pub show_panel: bool,
    pub config: BundleConfig,
    pub dirty: bool,
    pub message: String,
    /// 最近一次预估（配置改动后失效）。
    pub preview: Option<BundlePreview>,
    pub selected: usize,
    pub show_paths: bool,
    /// 新增 assign 行的输入。
    pub assign_path: String,
    pub assign_group: String,
}

impl BundlePanelState {
    pub fn new() -> Self {
        Self {
            show_panel: false,
            config: BundleConfig::default_domains(),
            dirty: false,
            message: String::new(),
            preview: None,
            selected: 0,
            show_paths: false,
            assign_path: String::new(),
            assign_group: String::new(),
        }
    }

    /// 切换/重新打开工程：读 `<root>/bundles.json`，没有就用内置内容域规则。
    pub fn reseed(&mut self, root: &Path) {
        self.config = BundleConfig::load_or_default(&root.join(BundleConfig::FILE))
            .unwrap_or_else(|_| BundleConfig::default_domains());
        self.dirty = false;
        self.message.clear();
        self.preview = None;
        self.selected = 0;
        self.assign_path.clear();
        self.assign_group.clear();
    }

    /// 扫描工程 + 逐组按发布编码器估算落盘字节（不写盘）。
    pub fn scan(&mut self, root: &Path) -> anyhow::Result<()> {
        let entries = bundle_plan::scan_project_entries(root)?;
        let mut plan = bundle_plan::build_plan(&self.config, &entries, PREVIEW_BLOCK_SIZE)?;
        let mut issues = Vec::new();
        for group in &mut plan.groups {
            let mut builder = BundleBuilder::new(PREVIEW_BLOCK_SIZE);
            let mut read_failed = None;
            for path in &group.paths {
                match std::fs::read(root.join(path)) {
                    Ok(bytes) => builder
                        .add(path, bytes)
                        .map_err(|error| anyhow::anyhow!("{error}"))?,
                    Err(error) => {
                        read_failed = Some(format!("{path}: {error}"));
                        break;
                    }
                }
            }
            if let Some(error) = read_failed {
                issues.push(format!("组 {} 有文件读不到（{error}）", group.name));
                continue;
            }
            let built = builder
                .build()
                .map_err(|error| anyhow::anyhow!("构建组 {} 失败：{error}", group.name))?;
            group.bundle_bytes = built.bytes.len() as u64;
            group.hash = format!("{:016x}", built.checksum);
        }

        let keys = ProjectKeys::from_project(root);
        issues.extend(bundle_plan::check_plan(
            &plan,
            entries.len() as u64,
            &keys,
            &self.config,
            MAX_BUNDLE_BYTES,
        ));

        // 按单元汇总 + 配额门（主包 4 MB / 总量 20 MB）。
        let totals = unit_totals(&plan);
        for total in &totals {
            if total.unit == "main" && total.bytes > MAX_BUNDLE_BYTES {
                issues.push(format!(
                    "主包（main）预估 {} KB 超过 4,096 KB",
                    total.bytes / 1024
                ));
            }
            if let Ok(UnitKind::Sub(Some(name))) = parse_unit(&total.unit)
                && total.bytes > MAX_BUNDLE_BYTES
            {
                issues.push(format!(
                    "分包 {name} 预估 {} KB 超过 4,096 KB（微信单个普通分包不限制大小，但建议 ≤4 MB）",
                    total.bytes / 1024
                ));
            }
        }
        let counted: u64 = totals
            .iter()
            .filter(|total| total.unit != "cdn")
            .map(|total| total.bytes)
            .sum();
        if counted > MAX_TOTAL_BYTES {
            issues.push(format!(
                "主包 + 全部分包预估 {} KB 超过总量上限 20,480 KB",
                counted / 1024
            ));
        }

        self.selected = self.selected.min(plan.groups.len().saturating_sub(1));
        self.preview = Some(BundlePreview {
            plan,
            totals,
            issues,
        });
        Ok(())
    }

    /// 写 `<root>/bundles.json`（临时文件 + rename）。
    pub fn save(&mut self, root: &Path) -> anyhow::Result<PathBuf> {
        self.config.validate()?;
        let bytes = self.config.to_json()?;
        let path = root.join(BundleConfig::FILE);
        write_atomic(&path, &bytes)?;
        self.dirty = false;
        Ok(path)
    }
}

impl Default for BundlePanelState {
    fn default() -> Self {
        Self::new()
    }
}

/// 一次预估的结果。
pub(crate) struct BundlePreview {
    pub plan: BundlePlan,
    pub totals: Vec<UnitTotal>,
    pub issues: Vec<String>,
}

/// 一个分发单元的合计。
pub(crate) struct UnitTotal {
    pub unit: String,
    pub groups: usize,
    pub entries: u64,
    pub bytes: u64,
}

impl UnitTotal {
    /// 主包按 4 MB 上限，其它单元按"分包建议上限"给进度条一个分母。
    pub fn fraction(&self) -> f32 {
        let limit = if self.unit == "main" {
            MAX_BUNDLE_BYTES
        } else {
            MAX_TOTAL_BYTES
        };
        (self.bytes as f32 / limit as f32).clamp(0.0, 1.0)
    }
}

fn unit_totals(plan: &BundlePlan) -> Vec<UnitTotal> {
    let mut totals: Vec<UnitTotal> = Vec::new();
    for group in &plan.groups {
        match totals.iter_mut().find(|total| total.unit == group.unit) {
            Some(total) => {
                total.groups += 1;
                total.entries += group.entries;
                total.bytes += group.bundle_bytes;
            }
            None => totals.push(UnitTotal {
                unit: group.unit.clone(),
                groups: 1,
                entries: group.entries,
                bytes: group.bundle_bytes,
            }),
        }
    }
    // 主包排最前，其余按名字稳定排序。
    totals.sort_by(|a, b| {
        let rank = |unit: &str| if unit == "main" { 0 } else { 1 };
        rank(&a.unit)
            .cmp(&rank(&b.unit))
            .then_with(|| a.unit.cmp(&b.unit))
    });
    totals
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            Err(error)
        }
    }
}

fn unit_badge(unit: &str) -> (egui::Color32, &'static str) {
    match parse_unit(unit) {
        Ok(UnitKind::Main) => (egui::Color32::from_rgb(90, 170, 110), "主包"),
        Ok(UnitKind::Sub(None)) => (egui::Color32::from_rgb(90, 130, 200), "分包"),
        Ok(UnitKind::Sub(Some(_))) => (egui::Color32::from_rgb(120, 110, 200), "具名分包"),
        Ok(UnitKind::Cdn) => (egui::Color32::from_rgb(170, 140, 80), "CDN"),
        Err(_) => (egui::Color32::from_rgb(200, 80, 80), "非法"),
    }
}

impl EditorApp {
    /// 菜单入口：打开分包面板并按需从磁盘载入配置。
    pub(crate) fn open_bundle_panel(&mut self) {
        if !self.project_loaded {
            return;
        }
        if !self.bundles.show_panel {
            let root = crate::util::strip_verbatim_prefix(&self.project.root);
            self.bundles.reseed(&root);
        }
        self.bundles.show_panel = true;
    }

    pub(crate) fn draw_bundle_panel(&mut self, context: &egui::Context) {
        if !self.bundles.show_panel {
            return;
        }
        let mut open = self.bundles.show_panel;
        let root = crate::util::strip_verbatim_prefix(&self.project.root);
        let project_loaded = self.project_loaded;
        egui::Window::new("资源分包（Bundles）")
            .open(&mut open)
            .default_width(960.0)
            .default_height(700.0)
            .resizable(true)
            .show(context, |ui| {
                if !project_loaded {
                    ui.label("先打开一个工程，才能编辑该工程的 bundles.json。");
                    return;
                }
                self.draw_bundle_body(ui, &root);
            });
        self.bundles.show_panel = open && self.bundles.show_panel;
    }

    fn draw_bundle_body(&mut self, ui: &mut egui::Ui, root: &Path) {
        let before = self.bundles.config.clone();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("配置：{}", root.join(BundleConfig::FILE).display()))
                    .small()
                    .weak(),
            );
            if self.bundles.dirty {
                ui.label(
                    egui::RichText::new("● 未保存")
                        .small()
                        .color(egui::Color32::YELLOW),
                );
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("从磁盘重载").clicked() {
                self.bundles.reseed(root);
                self.bundles.message = "已从磁盘重载".to_owned();
            }
            if ui.button("保存 bundles.json").clicked() {
                match self.bundles.save(root) {
                    Ok(path) => self.bundles.message = format!("已保存 {}", path.display()),
                    Err(error) => self.bundles.message = format!("保存失败：{error}"),
                }
            }
            if ui.button("扫描并预估").clicked() {
                match self.bundles.scan(root) {
                    Ok(()) => {
                        let groups = self
                            .bundles
                            .preview
                            .as_ref()
                            .map_or(0, |preview| preview.plan.groups.len());
                        self.bundles.message = format!("预估完成：{groups} 个组");
                    }
                    Err(error) => self.bundles.message = format!("预估失败：{error}"),
                }
            }
            if !self.bundles.message.is_empty() {
                ui.label(
                    egui::RichText::new(self.bundles.message.as_str())
                        .small()
                        .weak(),
                );
            }
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .id_salt("bundle_config_scroll")
            .max_height(330.0)
            .show(ui, |ui| {
                self.draw_bundle_config(ui);
            });

        if before != self.bundles.config {
            self.bundles.dirty = true;
            self.bundles.preview = None;
        }

        ui.separator();
        self.draw_bundle_preview(ui);
    }

    fn draw_bundle_config(&mut self, ui: &mut egui::Ui) {
        ui.heading("归组规则");
        ui.horizontal(|ui| {
            ui.label("默认组");
            ui.add(
                egui::TextEdit::singleline(&mut self.bundles.config.default_group)
                    .desired_width(140.0),
            )
            .on_hover_text("没被任何 include/assign 命中的文件落这里；发布期默认拒绝非空。");
            ui.label("默认单元");
            ui.add(
                egui::TextEdit::singleline(&mut self.bundles.config.default_unit)
                    .desired_width(140.0),
            )
            .on_hover_text("main | sub | sub:<name> | cdn");
            ui.checkbox(&mut self.bundles.config.allow_unowned, "允许未归组")
                .on_hover_text("默认关闭：未归组的文件在首屏读到就是白图，发布期应直接失败。");
        });
        ui.label(
            egui::RichText::new(
                "单元语义：main = 启动即挂（微信主包）；sub = 分包；sub:<name> = 具名分包（计划表保留，\
                 当前发布器尚未按名拆根）；cdn = 不占包内配额。",
            )
            .small()
            .weak(),
        );

        ui.add_space(6.0);
        ui.heading("组（声明顺序即优先级）");
        let mut remove: Option<usize> = None;
        for (index, group) in self.bundles.config.groups.iter_mut().enumerate() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("名称");
                    ui.add(egui::TextEdit::singleline(&mut group.name).desired_width(150.0));
                    ui.label("单元");
                    let mut unit = group.unit.clone().unwrap_or_default();
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut unit)
                            .hint_text("main | sub | sub:<name> | cdn")
                            .desired_width(140.0),
                    );
                    if response.changed() {
                        group.unit = if unit.trim().is_empty() {
                            None
                        } else {
                            Some(unit.trim().to_owned())
                        };
                    }
                    let unit_text = group.unit.clone().unwrap_or_else(|| "（默认）".to_owned());
                    let (color, label) = unit_badge(group.unit.as_deref().unwrap_or("sub"));
                    ui.label(egui::RichText::new(label).small().color(color))
                        .on_hover_text(unit_text);
                    if ui.small_button("删除").clicked() {
                        remove = Some(index);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("include");
                    let mut text = group.include.join("\n");
                    let response = ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .hint_text("assets/art/ui/\nassets/fonts/")
                            .desired_rows(2)
                            .desired_width(520.0),
                    );
                    if response.changed() {
                        group.include = text
                            .lines()
                            .map(str::trim)
                            .filter(|line| !line.is_empty())
                            .map(str::to_owned)
                            .collect();
                    }
                });
            });
        }
        if let Some(index) = remove {
            self.bundles.config.groups.remove(index);
        }
        ui.horizontal(|ui| {
            if ui.button("新增组").clicked() {
                self.bundles.config.groups.push(GroupSpec {
                    name: "new_group".to_owned(),
                    unit: Some("sub".to_owned()),
                    include: vec!["assets/new/".to_owned()],
                });
            }
            ui.label(
                egui::RichText::new("路径以 `/` 结尾按前缀匹配，否则按精确路径。")
                    .small()
                    .weak(),
            );
        });

        ui.add_space(6.0);
        ui.heading("逐路径指派（assign，优先于组规则）");
        let mut remove_assign: Option<String> = None;
        let rows: Vec<(String, String)> = self
            .bundles
            .config
            .assign
            .iter()
            .map(|(path, group)| (path.clone(), group.clone()))
            .collect();
        for (path, group) in rows {
            ui.horizontal(|ui| {
                ui.monospace(&path);
                ui.label("→");
                ui.monospace(&group);
                if ui.small_button("移除").clicked() {
                    remove_assign = Some(path.clone());
                }
            });
        }
        if let Some(path) = remove_assign {
            self.bundles.config.assign.remove(&path);
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.bundles.assign_path)
                    .hint_text("assets/art/ui/ (前缀) 或 project.toml")
                    .desired_width(300.0),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.bundles.assign_group)
                    .hint_text("目标组名")
                    .desired_width(140.0),
            );
            if ui.button("加入").clicked() {
                let path = self.bundles.assign_path.trim().to_owned();
                let group = self.bundles.assign_group.trim().to_owned();
                if !path.is_empty() && !group.is_empty() {
                    self.bundles.config.assign.insert(path, group);
                    self.bundles.assign_path.clear();
                    self.bundles.assign_group.clear();
                }
            }
        });
    }

    fn draw_bundle_preview(&mut self, ui: &mut egui::Ui) {
        let Some(preview) = self.bundles.preview.as_ref() else {
            ui.label(
                egui::RichText::new(
                    "点「扫描并预估」：按当前 bundles.json 归组，并用发布同款 zstd 编码器估算每组的落盘字节。",
                )
                .weak(),
            );
            return;
        };
        let issues = preview.issues.clone();
        let totals: Vec<(String, usize, u64, u64, f32)> = preview
            .totals
            .iter()
            .map(|total| {
                (
                    total.unit.clone(),
                    total.groups,
                    total.entries,
                    total.bytes,
                    total.fraction(),
                )
            })
            .collect();
        let groups: Vec<(String, String, u64, u64, u64)> = preview
            .plan
            .groups
            .iter()
            .map(|group| {
                (
                    group.name.clone(),
                    group.unit.clone(),
                    group.entries,
                    group.raw_bytes,
                    group.bundle_bytes,
                )
            })
            .collect();
        let selected = self.bundles.selected;
        let total_bytes = preview.plan.groups.len();

        if !issues.is_empty() {
            egui::CollapsingHeader::new(format!("问题（{}）", issues.len()))
                .default_open(true)
                .show(ui, |ui| {
                    for issue in &issues {
                        ui.label(
                            egui::RichText::new(issue)
                                .small()
                                .color(egui::Color32::from_rgb(220, 120, 100)),
                        );
                    }
                });
        }

        ui.label(egui::RichText::new("按分发单元").strong());
        for (unit, groups, entries, bytes, fraction) in &totals {
            let limit = if unit == "main" {
                MAX_BUNDLE_BYTES
            } else {
                MAX_TOTAL_BYTES
            };
            let (color, label) = unit_badge(unit);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(label).small().color(color));
                ui.monospace(format!("{unit:<14}"));
                ui.add(
                    egui::ProgressBar::new(*fraction)
                        .desired_width(240.0)
                        .text(format!("{} / {} KB", bytes / 1024, limit / 1024)),
                );
                ui.label(
                    egui::RichText::new(format!("{groups} 组 · {entries} 条目"))
                        .small()
                        .weak(),
                );
            });
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.label(format!("共 {} 个组", total_bytes));
            ui.checkbox(&mut self.bundles.show_paths, "显示条目清单");
        });
        egui::ScrollArea::vertical()
            .id_salt("bundle_group_list")
            .max_height(220.0)
            .show(ui, |ui| {
                egui::Grid::new("bundle_groups")
                    .num_columns(5)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label("组");
                        ui.label("单元");
                        ui.label("条目");
                        ui.label("原始");
                        ui.label("落盘");
                        ui.end_row();
                        for (index, (name, unit, entries, raw, bundle)) in groups.iter().enumerate()
                        {
                            let response = ui.selectable_label(selected == index, name);
                            if response.clicked() {
                                self.bundles.selected = index;
                            }
                            let (color, label) = unit_badge(unit);
                            ui.label(egui::RichText::new(label).small().color(color));
                            ui.monospace(entries.to_string());
                            ui.monospace(format!("{} KB", raw / 1024));
                            ui.monospace(format!("{} KB", bundle / 1024));
                            ui.end_row();
                        }
                    });
            });

        if self.bundles.show_paths
            && let Some(group) = self
                .bundles
                .preview
                .as_ref()
                .and_then(|preview| preview.plan.groups.get(self.bundles.selected))
        {
            let name = group.name.clone();
            let unit = group.unit.clone();
            let paths = group.paths.clone();
            ui.separator();
            ui.label(format!("{name}（{unit}）· {} 条", paths.len()));
            egui::ScrollArea::vertical()
                .id_salt("bundle_path_list")
                .max_height(180.0)
                .show(ui, |ui| {
                    for path in paths.iter().take(500) {
                        ui.monospace(path);
                    }
                    if paths.len() > 500 {
                        ui.label(format!("……其余 {} 条省略", paths.len() - 500));
                    }
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rustlua-editor-bundles-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::create_dir_all(root.join("assets/art/ui")).unwrap();
        std::fs::create_dir_all(root.join("assets/audio")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "name = \"t\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(root.join("scripts/main.lua"), "return {}\n").unwrap();
        std::fs::write(root.join("scenes/main.scene"), "scene_version=2\n").unwrap();
        std::fs::write(root.join("assets/art/ui/a.png"), b"png-a").unwrap();
        std::fs::write(root.join("assets/audio/bgm.mp3"), b"mp3").unwrap();
        root
    }

    #[test]
    fn save_and_reseed_roundtrip() {
        let root = temp_root("roundtrip");
        let mut state = BundlePanelState::new();
        state.config.default_unit = "sub".to_owned();
        state
            .config
            .assign
            .insert("assets/art/ui/a.png".to_owned(), "art:ui".to_owned());
        state.save(&root).expect("保存 bundles.json");
        assert!(root.join(BundleConfig::FILE).is_file());
        assert!(!state.dirty);

        let mut reloaded = BundlePanelState::new();
        reloaded.reseed(&root);
        assert_eq!(reloaded.config, state.config);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_builds_plan_and_reports_unit_totals() {
        let root = temp_root("scan");
        let mut state = BundlePanelState::new();
        state.scan(&root).expect("扫描");
        let preview = state.preview.as_ref().expect("有预览");
        assert!(!preview.plan.groups.is_empty());
        assert!(preview.totals.iter().any(|total| total.unit == "main"));
        assert!(preview.totals.iter().any(|total| total.unit == "sub"));
        let main = preview
            .totals
            .iter()
            .find(|total| total.unit == "main")
            .unwrap();
        assert!(main.bytes > 0, "主包必须有落盘字节");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_flags_unowned_files() {
        let root = temp_root("unowned");
        let mut state = BundlePanelState::new();
        // 放一张只被 `assets/` 兜底组接住的图，再把兜底组删掉：它就成了"没人认领"的文件。
        std::fs::write(root.join("assets/misc.txt"), b"x").unwrap();
        // 去掉 `assets/` 的兜底组（对应"新目录忘了写规则"）。
        state
            .config
            .groups
            .retain(|group| group.name != "art:world");
        state.scan(&root).expect("扫描");
        let issues = &state.preview.as_ref().unwrap().issues;
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("没被任何规则命中")),
            "{issues:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn invalid_unit_is_reported_not_panicking() {
        let root = temp_root("bad-unit");
        let mut state = BundlePanelState::new();
        state.config.default_unit = "bundle".to_owned();
        let error = state.scan(&root).expect_err("非法 unit 必须报错");
        assert!(error.to_string().contains("unit"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_atomic_replaces_existing_file() {
        let root = temp_root("atomic");
        let path = root.join("bundles.json");
        std::fs::write(&path, b"old").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
