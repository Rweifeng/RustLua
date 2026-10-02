//! M27-01：拖放的**协议层**——载荷类型、外部文件归类、"落在哪块面板 ⇒ 做什么"的判定。
//!
//! 分工：判定部分（载荷、归类、落点矩阵、文案）是**纯函数**；`impl EditorApp` 那一半只持有
//! 帧状态（本帧到达的系统拖放、拖拽途中的悬停列表）并把动作分派给提交层。写文档与写盘仍然
//! 只发生在 `scene_ops`（建实体）和 `import`（复制入库）里，面板负责采集事件与画高亮。
//!
//! 为什么要把内部拖放收成一个枚举：迁移前三条拖放链各自用不同的载荷类型
//! （资源浏览器/工具栏用 `String`，层级重排用 `SceneEntityId`），而 egui 的
//! `dnd_drop_zone::<T, _>` 按 `T` 过滤——层级的放置区看不见资源的 `String` 载荷，
//! 反过来也一样。类型不统一，"资产拖到层级"这类组合在协议上就不可达。

use crate::EditorApp;
use crate::util::strip_verbatim_prefix;
use eframe::egui;
use engine::core::{resource_kind::ResourceKind, scene::SceneEntityId};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// 工具栏「控件」调色板：数组顺序就是按钮顺序，元素既是显示文案也是控件类型
/// （`create_widget_at` 与 `DropPayload::Widget` 共用同一份字符串）。
pub(crate) const WIDGET_PALETTE: &[&str] = &["Text", "Image", "Button", "Panel", "Container"];

/// 面板之间内部拖放的唯一载荷类型。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DropPayload {
    /// 工程内资产：项目相对路径，`/` 分隔（与资源浏览器行、组件字符串里的写法一致）。
    Asset(String),
    /// 工具栏的控件类型（`Text`/`Image`/`Button`/`Panel`/`Container`）。
    Widget(&'static str),
    /// 层级里的实体（重排/挂父）。
    Entity(SceneEntityId),
}

/// 一次拖放的来源。外部文件不经过 egui 的 DnD 存储（系统直接把路径塞进 `RawInput`），
/// 但落点判定与内部拖放共用同一张表，所以在这里合成一个类型。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DragSource {
    Internal(DropPayload),
    External(Vec<PathBuf>),
}

/// 放置目标区（四个，对应 M27-01 的落点矩阵）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DropZone {
    /// Scene 画布（2D 与 Widget 模式都算，具体建什么由面板按模式决定）。
    #[default]
    SceneCanvas,
    /// 层级面板的一行。
    Hierarchy,
    /// 资源浏览器。
    ResourceBrowser,
    /// Inspector 的资源引用字段行。
    InspectorField,
}

/// 松手之后要执行的动作。**这一个值就是提交层的输入**：面板不再自己判断"能拖什么"，
/// 非法目标统一是 [`DropAction::Reject`]（画红框 + 中文提示 + 无副作用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DropAction {
    /// 为这些工程相对路径建实体（纹理→精灵、模型→MeshRenderer、预制体→实例化）。
    SpawnAssets(Vec<String>),
    /// 同上，但挂到某个父节点下（层级行的语义）。
    SpawnAssetsAsChild(Vec<String>),
    /// Widget 画布上按落点创建控件。
    CreateWidget(&'static str),
    /// 复制进 `assets/` 并刷新资源库。
    Import(Vec<PathBuf>),
    /// 先复制入库，再为可生成的那批建实体。
    ImportAndSpawn(Vec<PathBuf>),
    /// 先入库，再作为子节点建实体。
    ImportAndSpawnAsChild(Vec<PathBuf>),
    /// 把资产引用写进 Inspector 字段。
    SetAssetRef(String),
    /// 先入库，再把引用写进 Inspector 字段（单文件）。
    ImportAndSetRef(PathBuf),
    /// 层级内的实体重排（提交层已有，这里只是把矩阵补全）。
    Reparent,
    /// 非法组合：不做任何事，提示文案在值里。
    Reject(String),
}

impl DropAction {
    #[must_use]
    pub(crate) fn is_reject(&self) -> bool {
        matches!(self, Self::Reject(_))
    }

    /// hover 期间显示在目标区角落的动作文案（"松手会发生什么"）。
    #[must_use]
    pub(crate) fn caption(&self) -> String {
        match self {
            Self::SpawnAssets(paths) => spawn_caption(paths.len(), false),
            Self::SpawnAssetsAsChild(paths) => spawn_caption(paths.len(), true),
            Self::CreateWidget(kind) => format!("创建 {kind} 控件"),
            Self::Import(files) => format!("导入 {} 个文件", files.len()),
            Self::ImportAndSpawn(files) => import_spawn_caption(files, false),
            Self::ImportAndSpawnAsChild(files) => import_spawn_caption(files, true),
            Self::SetAssetRef(path) => format!("引用 {}", file_name(path)),
            Self::ImportAndSetRef(path) => format!("导入并引用 {}", displayed_name(path)),
            Self::Reparent => "移动到此处".to_owned(),
            Self::Reject(reason) => reason.clone(),
        }
    }
}

fn spawn_caption(count: usize, as_child: bool) -> String {
    let tail = if as_child {
        "（作为子节点）"
    } else {
        ""
    };
    format!("创建 {count} 个实体{tail}")
}

fn import_spawn_caption(files: &[PathBuf], as_child: bool) -> String {
    let (spawns, imports) = partition(files);
    let tail = if spawns > 0 && as_child {
        "（作为子节点）"
    } else {
        ""
    };
    match (spawns, imports) {
        (0, imports) => format!("导入 {imports} 个文件"),
        (spawns, 0) => format!("导入并创建 {spawns} 个实体{tail}"),
        (spawns, imports) => format!("导入 {imports} 个文件，创建 {spawns} 个实体{tail}"),
    }
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

fn displayed_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// 能变成场景实体/资产引用的资源家族。其余只能进资源库：音频/字体/Lua/场景本就不是
/// 场景引用目标，图集要靠 `图集路径#区域名` 指到某个区域，整张 `.atlas` 直接当引用会缺
/// 区域信息（装配期报错），所以也不给拖成实体或填进引用字段。
#[must_use]
pub(crate) fn referenceable(kind: ResourceKind) -> bool {
    matches!(
        kind,
        ResourceKind::Texture | ResourceKind::Model | ResourceKind::Prefab
    )
}

/// 外部文件批次里"能建实体的"与"只能入库的"各多少个（文案与提交都用这个划分）。
#[must_use]
pub(crate) fn partition(files: &[PathBuf]) -> (usize, usize) {
    let spawns = files
        .iter()
        .filter(|path| referenceable(ResourceKind::from_path(path)))
        .count();
    (spawns, files.len() - spawns)
}

/// 非法落点：不做任何事，只带一句原因文案。
fn reject(reason: impl Into<String>) -> DropAction {
    DropAction::Reject(reason.into())
}

/// 落点矩阵：这个来源拖到这块面板上会执行什么。纯函数，逐格有单测。
#[must_use]
pub(crate) fn plan(source: &DragSource, zone: DropZone) -> DropAction {
    match (source, zone) {
        (DragSource::Internal(DropPayload::Asset(path)), DropZone::SceneCanvas) => {
            DropAction::SpawnAssets(vec![path.clone()])
        }
        (DragSource::Internal(DropPayload::Asset(path)), DropZone::Hierarchy) => {
            DropAction::SpawnAssetsAsChild(vec![path.clone()])
        }
        (DragSource::Internal(DropPayload::Asset(path)), DropZone::InspectorField) => {
            DropAction::SetAssetRef(path.clone())
        }
        // 资源浏览器是资产的**家**：把自家资产再拖回家没有意义（也没有安全的语义——
        // 是移动？是重命名？两者都要重写引用）。按"无副作用 + 说明"处理。
        (DragSource::Internal(DropPayload::Asset(_)), DropZone::ResourceBrowser) => {
            reject("资产已在资源库中，拖到场景或属性面板才有动作")
        }

        (DragSource::Internal(DropPayload::Widget(kind)), DropZone::SceneCanvas) => {
            DropAction::CreateWidget(kind)
        }
        (DragSource::Internal(DropPayload::Widget(_)), DropZone::Hierarchy) => {
            reject("控件只能拖到 Widget 模式的画布上")
        }
        (DragSource::Internal(DropPayload::Widget(_)), DropZone::ResourceBrowser) => {
            reject("控件不是资产，无法导入资源库")
        }
        (DragSource::Internal(DropPayload::Widget(_)), DropZone::InspectorField) => {
            reject("控件不能填进资源引用字段")
        }

        (DragSource::Internal(DropPayload::Entity(_)), DropZone::Hierarchy) => DropAction::Reparent,
        (DragSource::Internal(DropPayload::Entity(_)), DropZone::SceneCanvas) => {
            reject("改变父子关系请在层级面板里放")
        }
        (DragSource::Internal(DropPayload::Entity(_)), DropZone::ResourceBrowser) => {
            reject("场景实体不是资产，无法导入资源库")
        }
        (DragSource::Internal(DropPayload::Entity(_)), DropZone::InspectorField) => {
            reject("把实体填进资源引用字段没有意义，请选择资产")
        }

        (DragSource::External(files), DropZone::ResourceBrowser) => {
            if files.is_empty() {
                reject("没有可导入的文件")
            } else {
                DropAction::Import(files.clone())
            }
        }
        (DragSource::External(files), DropZone::SceneCanvas) => external_spawn(files, false),
        (DragSource::External(files), DropZone::Hierarchy) => external_spawn(files, true),
        (DragSource::External(files), DropZone::InspectorField) => match files.as_slice() {
            [] => reject("没有可导入的文件"),
            // 引用字段一次只收一个资产：多文件批次语义不清（引用哪个？）。
            [one] if referenceable(ResourceKind::from_path(one)) => {
                DropAction::ImportAndSetRef(one.clone())
            }
            [_] => reject("这个字段要的是纹理/模型/预制体，拖进来的文件不是"),
            many => reject(format!(
                "资源引用字段一次只收一个文件，本次拖入 {} 个",
                many.len()
            )),
        },
    }
}

fn external_spawn(files: &[PathBuf], as_child: bool) -> DropAction {
    if files.is_empty() {
        return reject("没有可导入的文件");
    }
    let (spawns, _) = partition(files);
    if spawns == 0 {
        // 全是音频/字体这类"只能入库"的：仍然导入，但不建实体。
        return DropAction::Import(files.to_vec());
    }
    if as_child {
        DropAction::ImportAndSpawnAsChild(files.to_vec())
    } else {
        DropAction::ImportAndSpawn(files.to_vec())
    }
}

/// 一次系统拖放（一帧内到达的文件批次 + 展开结果）。
#[derive(Clone, Debug, Default)]
pub(crate) struct ExternalDrop {
    /// 已去过 verbatim 前缀、目录已展开的绝对路径。
    pub(crate) files: Vec<PathBuf>,
    /// 落点。系统事件本身不带坐标（`DroppedFile` 在 0.33 里没有 `pos` 字段），
    /// 取拖放结束那一帧的指针位置。
    pub(crate) position: Option<egui::Pos2>,
    /// 只有字节没有路径的条目（浏览器来源）个数——第一版直接拒绝并说明。
    pub(crate) bytes_only: usize,
    /// 目录展开被深度/总量上限截断，或目录读不动。
    pub(crate) truncated: bool,
    /// 跳过的符号链接/junction/reparse point 数量。
    pub(crate) skipped_links: usize,
}

impl ExternalDrop {
    /// 三类损耗的补充说明（合法动作的文案后面追加这一段；无损耗时为空串）。
    #[must_use]
    pub(crate) fn notes(&self) -> String {
        let mut notes: Vec<String> = Vec::new();
        if self.truncated {
            notes.push(format!("目录按 {MAX_FILES} 文件/{MAX_DEPTH} 层截断"));
        }
        if self.skipped_links > 0 {
            notes.push(format!("跳过 {} 个链接", self.skipped_links));
        }
        if self.bytes_only > 0 {
            notes.push(format!(
                "{} 个条目无文件路径（浏览器拖入）已忽略",
                self.bytes_only
            ));
        }
        if notes.is_empty() {
            String::new()
        } else {
            format!("（{}）", notes.join("，"))
        }
    }
}

/// 目录展开的上限：深度 ≤4、总文件数 ≤256，防止把 `C:\` 或整个游戏库拖进来卡死编辑器。
const MAX_DEPTH: usize = 4;
const MAX_FILES: usize = 256;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Expansion {
    pub(crate) files: Vec<PathBuf>,
    /// 跳过的符号链接/junction/reparse point 数量。
    pub(crate) skipped_links: usize,
    /// 因为深度、总量上限或读不动的目录被截断。
    pub(crate) truncated: bool,
}

/// 展开拖进来的目录，跳过链接（防环），并按上限截断。
#[must_use]
pub(crate) fn expand(paths: &[PathBuf]) -> Expansion {
    let mut out = Expansion::default();
    for path in paths {
        if path.is_dir() {
            walk(path, 1, &mut out);
        } else {
            push(&mut out, path.clone());
        }
    }
    out
}

fn push(out: &mut Expansion, path: PathBuf) {
    if out.files.len() >= MAX_FILES {
        out.truncated = true;
        return;
    }
    out.files.push(path);
}

fn walk(dir: &Path, depth: usize, out: &mut Expansion) {
    if depth > MAX_DEPTH {
        out.truncated = true;
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        // 读不动的目录（权限、网络盘断开）计入截断，而不是让整批失败。
        out.truncated = true;
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_link(&path) {
            out.skipped_links += 1;
            continue;
        }
        if path.is_dir() {
            walk(&path, depth + 1, out);
        } else {
            push(out, path);
        }
    }
}

/// 链接判定：symlink 各平台都算，Windows 还要看 reparse point（junction 不是 symlink）。
#[must_use]
fn is_link(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() {
        return true;
    }
    is_reparse_point(&meta)
}

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_meta: &std::fs::Metadata) -> bool {
    false
}

/// `windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT`。
/// 取字面值：editor 的 `windows-sys` feature 集里没开 `Storage` 模块，为一个常量扩 feature 不值。
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// 从本帧的原始输入里取走系统拖放事件（文件批次 + 松手时的指针位置）。
///
/// `RawInput::dropped_files` 只反映**自上一帧以来**到达的事件（后端每帧
/// `take()` 搬走一次），因此松手那一帧必须收下，否则这件事就永久丢了。
#[must_use]
pub(crate) fn take_external_drop(context: &egui::Context) -> Option<ExternalDrop> {
    let (dropped, position) =
        context.input(|input| (input.raw.dropped_files.clone(), input.pointer.hover_pos()));
    if dropped.is_empty() {
        return None;
    }
    let mut bytes_only = 0;
    let mut paths = Vec::with_capacity(dropped.len());
    for file in dropped {
        match file.path.as_deref().map(strip_verbatim_prefix) {
            Some(path) => paths.push(path),
            // 只有字节没有路径（从浏览器拖出来的）：第一版不落盘，计数后由状态栏说明。
            None => bytes_only += 1,
        }
    }
    let expansion = expand(&paths);
    if expansion.files.is_empty() && bytes_only == 0 {
        return None;
    }
    Some(ExternalDrop {
        files: expansion.files,
        position,
        bytes_only,
        truncated: expansion.truncated,
        skipped_links: expansion.skipped_links,
    })
}

/// 拖进来的文件在工程里的落点：`assets/<家族目录>/<文件名>`，重名自动加 `-1`、`-2`。
///
/// 目录按家族分（`assets/textures`、`assets/models`、`assets/prefabs`…）与资源浏览器
/// 现有样例工程的排法一致；家族没有目录概念的统一放 `assets/imports`。
#[must_use]
pub(crate) fn target_path(root: &Path, source: &Path) -> PathBuf {
    let kind = ResourceKind::from_path(source);
    let dir = root.join("assets").join(kind_directory(kind));
    let Some(file_name) = source.file_name() else {
        return dir.join("dropped-file");
    };
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = Path::new(file_name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = Path::new(file_name)
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    for index in 1.. {
        let candidate = dir.join(format!("{stem}-{index}{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("文件名重试循环不会收敛失败");
}

fn kind_directory(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Texture => "textures",
        ResourceKind::Audio => "audio",
        ResourceKind::Model => "models",
        ResourceKind::Prefab => "prefabs",
        ResourceKind::Font => "fonts",
        ResourceKind::LuaScript => "scripts",
        ResourceKind::Scene => "scenes",
        ResourceKind::Atlas => "atlases",
        ResourceKind::Animation | ResourceKind::Other => "imports",
    }
}

/// 拖拽**途中**悬停在窗口上的文件路径（去重）。
///
/// `hovered_files` 与 `dropped_files` 不同：它跨帧保留（`RawInput::take()` 只 clone 不搬走），
/// 直到系统发出"取消/已释放"才清空——所以画高亮和提示文案要用它，而不是等松手那一帧。
#[must_use]
pub(crate) fn hover_paths(context: &egui::Context) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    context.input(|input| {
        for file in &input.raw.hovered_files {
            if let Some(path) = file.path.as_deref().map(strip_verbatim_prefix)
                && !out.contains(&path)
            {
                out.push(path);
            }
        }
    });
    out
}

/// 指针是否落在放置区内（矩形或指针缺失都算"不在"）。
#[must_use]
fn inside(rect: egui::Rect, pointer: Option<egui::Pos2>) -> bool {
    pointer.is_some_and(|position| rect.contains(position))
}

/// 当前挂在指针上的**内部**拖放载荷（拖拽途中可读，egui 在本帧结束时清掉）。
#[must_use]
pub(crate) fn internal_drag(context: &egui::Context) -> Option<DragSource> {
    egui::DragAndDrop::payload::<DropPayload>(context)
        .map(|payload| DragSource::Internal((*payload).clone()))
}

/// 拖拽悬停时画在放置区上的反馈：一圈描边 + 右上角一句"松手会发生什么"。
///
/// 合法（绿）与非法（红）用同一套绘制，非法也要有文案——用户得知道**为什么**不行，
/// 而不是看到一个没反应的框。
pub(crate) fn paint_drop_feedback(ui: &egui::Ui, rect: egui::Rect, id: u8, action: &DropAction) {
    let color = if action.is_reject() {
        egui::Color32::from_rgb(220, 100, 100)
    } else {
        egui::Color32::from_rgb(110, 210, 130)
    };
    ui.painter().rect_stroke(
        rect,
        2.0_f32,
        egui::Stroke::new(2.0_f32, color),
        egui::StrokeKind::Inside,
    );
    let text = action.caption();
    egui::Area::new(egui::Id::new(("drop_feedback", id)))
        .fixed_pos(rect.right_top())
        .order(egui::Order::Foreground)
        .movable(false)
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::default()
                .fill(egui::Color32::from_black_alpha(180))
                .inner_margin(egui::Margin::symmetric(6, 3))
                .show(ui, |ui| {
                    ui.colored_label(color, text);
                });
        });
}

impl EditorApp {
    /// 帧首一次：收下本帧到达的系统拖放，并刷新"正在拖拽"的文件列表。
    pub(crate) fn begin_dnd_frame(&mut self, context: &egui::Context) {
        self.pending_external_drop = take_external_drop(context);
        self.external_hover = hover_paths(context);
    }

    /// 外部拖拽正悬停在 `rect` 上时的提示文案（用于画高亮角标）。
    #[must_use]
    pub(crate) fn external_hover_action(
        &self,
        zone: DropZone,
        rect: egui::Rect,
        pointer: Option<egui::Pos2>,
    ) -> Option<DropAction> {
        if self.external_hover.is_empty() || !inside(rect, pointer) {
            return None;
        }
        Some(plan(
            &DragSource::External(self.external_hover.clone()),
            zone,
        ))
    }

    /// 本帧有一次系统拖放、指针落在 `rect` 内且还没被别的面板认领 → 认领并算出动作。
    ///
    /// 四个区域各自的矩形互不重叠，"谁先认领"只取决于绘制顺序里第一个包住指针的区域；
    /// 没人认领时由 [`EditorApp::sweep_dnd_frame`] 收尾。
    pub(crate) fn claim_external_drop(
        &mut self,
        zone: DropZone,
        rect: egui::Rect,
    ) -> Option<(DropAction, ExternalDrop)> {
        let drop = self.pending_external_drop.take()?;
        if !inside(rect, drop.position) {
            self.pending_external_drop = Some(drop);
            return None;
        }
        let action = plan(&DragSource::External(drop.files.clone()), zone);
        Some((action, drop))
    }

    /// 帧末一次：没被任何区域认领的系统拖放要显式丢掉并说明，否则它会一直挂到下一次拖放。
    pub(crate) fn sweep_dnd_frame(&mut self) -> Option<String> {
        let drop = self.pending_external_drop.take()?;
        if drop.bytes_only > 0 {
            return Some(drop.notes());
        }
        Some("释放位置不在可放置的面板内，未做任何改动".to_owned())
    }

    /// "建实体 / 入库"这一族动作的共用提交路径（画布与层级只差一个父节点）。
    ///
    /// `parent` 只被 `*AsChild` 变体使用。`CreateWidget`、`Reparent`、`SetAssetRef*`
    /// 需要各自区域才有的上下文（落点坐标、被拖实体、目标字段），由面板自己处理；
    /// 它们落到这里说明调用点配错了动作，直接报出来而不是静默跳过。
    pub(crate) fn commit_spawn_drop(
        &mut self,
        action: &DropAction,
        parent: Option<SceneEntityId>,
    ) -> String {
        match action {
            DropAction::Reject(reason) => reason.clone(),
            DropAction::SpawnAssets(paths) => self.spawn_dropped_batch(paths, None),
            DropAction::SpawnAssetsAsChild(paths) => self.spawn_dropped_batch(paths, parent),
            DropAction::Import(files) => self.ingest_external_files(files),
            DropAction::ImportAndSpawn(files) => self.ingest_and_spawn(files, None),
            DropAction::ImportAndSpawnAsChild(files) => self.ingest_and_spawn(files, parent),
            other => format!("落点用错了提交入口：{}", other.caption()),
        }
    }
}

/// 工程根 → 该路径的项目相对写法（`/` 分隔）。不在工程根里面时返回 `None`
/// （外部落文件永远落在 `assets/` 下，所以提交层拿到的一定是 `Some`）。
#[must_use]
pub(crate) fn relative_asset(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    Some(
        relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> PathBuf {
        PathBuf::from(text)
    }

    fn asset(path: &str) -> DragSource {
        DragSource::Internal(DropPayload::Asset(path.to_owned()))
    }

    /// 内部资产 × 四个落点。
    #[test]
    fn an_asset_plan_per_zone() {
        let source = asset("assets/textures/slime.png");
        assert_eq!(
            plan(&source, DropZone::SceneCanvas),
            DropAction::SpawnAssets(vec!["assets/textures/slime.png".to_owned()])
        );
        assert_eq!(
            plan(&source, DropZone::Hierarchy),
            DropAction::SpawnAssetsAsChild(vec!["assets/textures/slime.png".to_owned()])
        );
        assert_eq!(
            plan(&source, DropZone::InspectorField),
            DropAction::SetAssetRef("assets/textures/slime.png".to_owned())
        );
        assert!(plan(&source, DropZone::ResourceBrowser).is_reject());
    }

    /// 控件 × 四个落点：只有画布可收。
    #[test]
    fn a_widget_only_lands_on_the_canvas() {
        let source = DragSource::Internal(DropPayload::Widget("Text"));
        assert_eq!(
            plan(&source, DropZone::SceneCanvas),
            DropAction::CreateWidget("Text")
        );
        for zone in [
            DropZone::Hierarchy,
            DropZone::ResourceBrowser,
            DropZone::InspectorField,
        ] {
            assert!(plan(&source, zone).is_reject(), "{zone:?} 应该拒绝控件");
        }
    }

    /// 实体 × 四个落点：只有层级可收（重排）。
    #[test]
    fn an_entity_only_reparents_in_the_hierarchy() {
        let source = DragSource::Internal(DropPayload::Entity(SceneEntityId(7)));
        assert_eq!(plan(&source, DropZone::Hierarchy), DropAction::Reparent);
        for zone in [
            DropZone::SceneCanvas,
            DropZone::ResourceBrowser,
            DropZone::InspectorField,
        ] {
            assert!(plan(&source, zone).is_reject(), "{zone:?} 应该拒绝实体");
        }
    }

    /// M27-03 验收 4 的 prefab 一行：预制体是"能建实体的资产"，四个落点语义各不同。
    ///
    /// 单独拎出来测是因为 prefab 与纹理的差别全在**落地之后**（实例化整棵子树 vs 建一个精灵），
    /// 而 `plan` 只看得到 `ResourceKind`；这一格的文案一旦与 `partition` 的计数分叉，
    /// 用户看到"创建 1 个实体"、实际生出两层子树，是不会有人报上来的那种错。
    #[test]
    fn a_prefab_plan_per_zone() {
        const PREFAB: &str = "assets/prefabs/enemy.prefab";
        let prefab = asset(PREFAB);
        for (zone, expected) in [
            (
                DropZone::SceneCanvas,
                DropAction::SpawnAssets(vec![PREFAB.to_owned()]),
            ),
            (
                DropZone::Hierarchy,
                DropAction::SpawnAssetsAsChild(vec![PREFAB.to_owned()]),
            ),
            (
                DropZone::InspectorField,
                DropAction::SetAssetRef(PREFAB.to_owned()),
            ),
        ] {
            assert_eq!(plan(&prefab, zone), expected, "{zone:?}");
        }
        assert_eq!(
            plan(&prefab, DropZone::SceneCanvas).caption(),
            "创建 1 个实体"
        );
        assert_eq!(
            plan(&prefab, DropZone::Hierarchy).caption(),
            "创建 1 个实体（作为子节点）"
        );
        assert_eq!(
            plan(&prefab, DropZone::InspectorField).caption(),
            "引用 enemy.prefab"
        );
        assert!(
            plan(&prefab, DropZone::ResourceBrowser).is_reject(),
            "自家预制体拖回资源库仍然是无意义格"
        );

        // 外部预制体：入库 + 实例化，文案必须把两件事都说清（只说"创建实体"会漏掉复制）。
        let external = DragSource::External(vec![path("C:/tmp/boss.prefab")]);
        assert_eq!(
            plan(&external, DropZone::SceneCanvas),
            DropAction::ImportAndSpawn(vec![path("C:/tmp/boss.prefab")])
        );
        assert_eq!(
            plan(&external, DropZone::SceneCanvas).caption(),
            "导入并创建 1 个实体"
        );
        assert_eq!(
            plan(&external, DropZone::Hierarchy).caption(),
            "导入并创建 1 个实体（作为子节点）"
        );
        assert_eq!(
            plan(&external, DropZone::ResourceBrowser).caption(),
            "导入 1 个文件"
        );
        assert_eq!(
            plan(&external, DropZone::InspectorField),
            DropAction::ImportAndSetRef(path("C:/tmp/boss.prefab"))
        );
        assert_eq!(
            plan(&external, DropZone::InspectorField).caption(),
            "导入并引用 boss.prefab"
        );
        // 与纹理同族：混合批次里两者都计入"能建实体"那一半。
        assert_eq!(
            partition(&[path("a.png"), path("b.prefab"), path("c.mp3")]),
            (2, 1)
        );
    }

    /// 外部文件：能建实体的后缀决定 Scene/层级是"导入+建实体"还是"只导入"。
    #[test]
    fn external_files_split_into_import_and_spawn() {
        let mixed = DragSource::External(vec![
            path("C:/tmp/a.png"),
            path("C:/tmp/b.glb"),
            path("C:/tmp/c.mp3"),
        ]);
        assert_eq!(
            plan(&mixed, DropZone::SceneCanvas),
            DropAction::ImportAndSpawn(vec![
                path("C:/tmp/a.png"),
                path("C:/tmp/b.glb"),
                path("C:/tmp/c.mp3")
            ])
        );
        assert_eq!(
            import_spawn_caption(&[path("a.png"), path("c.mp3")], false),
            "导入 1 个文件，创建 1 个实体"
        );
        // 全是不可生成的家族：只入库。
        let audio = DragSource::External(vec![path("C:/tmp/b.mp3"), path("C:/tmp/c.ttf")]);
        assert_eq!(
            plan(&audio, DropZone::SceneCanvas),
            DropAction::Import(vec![path("C:/tmp/b.mp3"), path("C:/tmp/c.ttf")])
        );
        assert_eq!(
            plan(&audio, DropZone::ResourceBrowser),
            DropAction::Import(vec![path("C:/tmp/b.mp3"), path("C:/tmp/c.ttf")])
        );
        assert_eq!(
            partition(&[path("a.png"), path("b.prefab"), path("c.lua")]),
            (2, 1)
        );
    }

    /// 外部文件 × Inspector：单文件才收，多文件与不可引用的后缀都是拒绝。
    #[test]
    fn the_inspector_field_takes_one_external_file() {
        let one = DragSource::External(vec![path("C:/tmp/a.png")]);
        assert_eq!(
            plan(&one, DropZone::InspectorField),
            DropAction::ImportAndSetRef(path("C:/tmp/a.png"))
        );
        let two = DragSource::External(vec![path("C:/tmp/a.png"), path("C:/tmp/b.png")]);
        assert!(plan(&two, DropZone::InspectorField).is_reject());
        let wav = DragSource::External(vec![path("C:/tmp/a.wav")]);
        assert!(plan(&wav, DropZone::InspectorField).is_reject());
        let empty = DragSource::External(vec![]);
        for zone in [
            DropZone::SceneCanvas,
            DropZone::Hierarchy,
            DropZone::ResourceBrowser,
            DropZone::InspectorField,
        ] {
            assert!(plan(&empty, zone).is_reject(), "{zone:?} 空批次必须拒绝");
        }
    }

    /// 文案：合法动作说"做什么"，拒绝说"为什么不行"，且都非空（高亮角标不能是空的）。
    #[test]
    fn every_action_has_a_caption() {
        let cases = [
            DropAction::SpawnAssets(vec!["a.png".to_owned()]),
            DropAction::SpawnAssetsAsChild(vec!["a.png".to_owned(), "b.png".to_owned()]),
            DropAction::CreateWidget("Button"),
            DropAction::Import(vec![path("a.png")]),
            DropAction::ImportAndSpawn(vec![path("a.png"), path("b.mp3")]),
            DropAction::ImportAndSpawnAsChild(vec![path("a.png")]),
            DropAction::SetAssetRef("assets/a.png".to_owned()),
            DropAction::ImportAndSetRef(path("a.png")),
            DropAction::Reparent,
            reject("不行"),
        ];
        for action in &cases {
            let caption = action.caption();
            assert!(!caption.is_empty(), "{action:?} 没有文案");
            if action.is_reject() {
                assert_eq!(caption, "不行");
            }
        }
        assert_eq!(
            DropAction::SpawnAssetsAsChild(vec!["a.png".to_owned()]).caption(),
            "创建 1 个实体（作为子节点）"
        );
    }

    /// 矩阵完整性：4 个来源 × 4 个落点不能有"漏格"（这里靠逐条测试覆盖，加一条
    /// 汇总断言防未来新增变体时静默漏判）。
    #[test]
    fn the_matrix_covers_every_source_and_zone() {
        let sources = [
            asset("assets/a.png"),
            DragSource::Internal(DropPayload::Widget("Text")),
            DragSource::Internal(DropPayload::Entity(SceneEntityId(1))),
            DragSource::External(vec![path("C:/tmp/a.png")]),
        ];
        let zones = [
            DropZone::SceneCanvas,
            DropZone::Hierarchy,
            DropZone::ResourceBrowser,
            DropZone::InspectorField,
        ];
        let mut legal = 0;
        for source in &sources {
            for zone in zones {
                let action = plan(source, zone);
                // 每条都有可显示的文案；合法格计数用于确认矩阵不是全拒绝。
                assert!(!action.caption().is_empty());
                if !action.is_reject() {
                    legal += 1;
                }
            }
        }
        assert_eq!(legal, 9, "落点矩阵的合法格数量变了，请同步本文与 docs");
    }

    /// 目录展开：深度上限、跳过链接、总量截断都要能在临时目录里真实复现。
    #[test]
    fn folder_expansion_bounds_depth_and_count() {
        let root = std::env::temp_dir().join(format!(
            "rustlua-dnd-expand-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let deep = (0..=MAX_DEPTH).fold(root.clone(), |acc, _| acc.join("level"));
        std::fs::create_dir_all(&deep).expect("create deep tree");
        std::fs::write(deep.join("deep.png"), b"png").expect("write deep");
        let top = root.join("top.png");
        std::fs::write(&top, b"png").expect("write top");

        // 第 5 层（depth > MAX_DEPTH）不该被带进来。
        let expansion = expand(std::slice::from_ref(&root));
        assert!(
            expansion.files.contains(&top),
            "浅层文件必须收到：{:?}",
            expansion.files
        );
        assert!(
            !expansion.files.iter().any(|f| f.ends_with("deep.png")),
            "超过深度上限的文件必须被截断：{:?}",
            expansion.files
        );
        assert!(expansion.truncated, "截断必须留痕");

        // 重名落点：同名文件第二次要拿到 `-1` 后缀，绝不能覆盖已有资产。
        let first = target_path(&root, &top);
        std::fs::create_dir_all(first.parent().expect("parent")).expect("create assets dir");
        std::fs::write(&first, b"old").expect("occupy first");
        let second = target_path(&root, &top);
        assert_ne!(
            first, second,
            "重名必须换名，不能覆盖：{first:?} / {second:?}"
        );
        assert_eq!(
            relative_asset(&root, &second),
            Some("assets/textures/top-1.png".to_owned()),
            "相对路径要用 `/` 分隔并落在家族目录里：{second:?}"
        );
        assert_eq!(
            relative_asset(
                &root,
                &target_path(&root, &PathBuf::from("C:/tmp/hero.glb"))
            )
            .map(|relative| relative.starts_with("assets/models/hero").to_owned()),
            Some(true)
        );
        assert_eq!(
            relative_asset(&root, &PathBuf::from("/somewhere/else.png")),
            None,
            "工程外的路径没有相对写法"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
