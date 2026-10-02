# 任务：M27-00 dock 化基线与多标签面板布局（egui_dock）

- 任务 ID：M27-00
- 状态：已交付，**待项目负责人验收**（代码未提交；阶段 0 ✅、阶段 1 ✅ 2026-10-01、阶段 2 ✅ 2026-10-02、
  阶段 2 收尾（逐面板开关）✅ 2026-10-02；本文随 M27 收口归档；
  剩余：面板标题去重、`panel_widths` 出库、叶子最小尺寸）
- 关联里程碑：M27（见 `M27-overview-总览与排期.md`）
- 创建日期：2026-10-01

## 目标

把编辑器的固定面板布局替换为 egui_dock 多标签停靠工作台，默认布局与现状一致，
停靠树可持久化、可重置，且不破坏面板架构棘轮与性能基线。

## 影响文件

- `engine/Cargo.toml`、`engine/crates/editor/Cargo.toml`、`engine/Cargo.lock`：新增 `egui_dock`（workspace 依赖）；
  阶段 2 打开 `egui_dock` 的 `serde` feature（连带 `egui/serde`）+ `editor` 侧 `serde` derive 依赖
- `engine/crates/editor/src/dock.rs`（新建）：`DockTab` 枚举 + `TabViewer` 实现 + 默认停靠树构造
- `engine/crates/editor/src/main.rs`：`impl eframe::App for EditorApp` 的 `update()`——
  保留 menu/toolbar 的 `TopBottomPanel` 外框（`panels/header.rs:14`），把
  `SidePanel/CentralPanel` 硬编码区（约 `main.rs:2098-2264`）替换为
  `DockArea::new(&mut state).show_inside(ui, viewer)`
- `engine/crates/editor/src/layout.rs`：`LAYOUT_VERSION` 1→2，新增 `dock_tree` 字段
  （`Option<Box<serde_json::Value>>`，存 egui_dock `serde` feature 的产物）；容错契约不变：
  缺失/截断/类型错一律回落默认树并报告原因
- `engine/crates/editor/src/main.rs`：`EditorApp` 增 `dock_state: Option<DockState<DockTab>>`
  字段与初始化（`Option` 是为了渲染期间能把它 take 出去，见阶段 1 §2）
- `panels/*.rs`：**不改面板内部逻辑**——dock 的 `ui()` 只是把现有 `draw(...)` 入口包一层；
  每面板从 `&Context` 上的固定区域改为拿到自己的 `&mut Ui`
- `panels/architecture_tests.rs`：棘轮扫描路径同步（面板文件不得出现 `DockState` 写操作）

## 阶段 0（先做，1-2 天）：依赖与薄切片验证 —— ✅ 已完成（2026-10-01）

1. ✅ **配套版本实测（原计划假设被推翻）**：`egui_dock` 与 `egui` 是**逐版硬绑定**，
   crates.io 实测映射为 `0.18.0 ↔ egui 0.33`、`0.19 ↔ 0.34`、`0.20 ↔ 0.35`、`0.21 ↔ 0.36`。
   本工程 `eframe/egui = 0.33.3` ⇒ 必须钉 **`egui_dock = "=0.18.0"`**（已写进 `engine/Cargo.toml` 注释）。
   ⚠ 用 `0.21.1` 会静默拉进**第二份 egui 0.36.2**：`cargo check` 只报"找不到 trait"，
   看不出根因是双版本；查 `Cargo.lock` 里 `name = "egui"` 出现几次才是真相。
2. ✅ **0.18.0 API 与网上教程不同**：**没有 `PanelUi` trait**，`TabViewer` 也不带泛型参数——
   统一走 `TabViewer { type Tab; fn id/title/ui(&mut self, …, tab: &mut Self::Tab) }`，
   渲染入口 `DockArea::new(&mut dock_state).show_inside(ui, &mut viewer)`。
   阶段 1 的 `DockTab` 全集按此签名写，不要再找 `PanelUi`。
3. ✅ **薄切片已落地**：`crates/editor/src/dock.rs`（`DockTab::{Console, Placeholder}` +
   独立试验窗口，`Ctrl+Alt+D` 开关，默认关）；`panels/console.rs` 抽出 `draw_console_body(&mut Ui)`
   作为 `PanelUi`→`TabViewer::ui` 的内容入口（原 `TopBottomPanel` 外壳绘制路径不变）。
4. ✅ **借用形态结论（全量迁移的关键约束）**：`DockArea` 要 `&mut DockState`，`TabViewer` 又要
   `&mut EditorApp`（含同一 `dock_state` 字段）⇒ **直接双借用被编译器拒绝**。
   薄切片用 `mem::replace` take/restore 绕过；**阶段 1 必须根治**：把 `DockState` 从 `EditorApp`
   挪进独立的 workspace 结构（或让 viewer 只借用面板需要的子结构），否则每个面板都要重复这个舞蹈。
5. ✅ **共存验证**：真实窗口 GUI 证据模式 11 张截图 0 失败（`artifacts/tmp-m27-phase0/`）；
   `cargo test -p editor` 289 passed（+2 条 dock 新测试，其中一条无头断言控制台内容真的画进了 tab）；
   `cargo clippy -p editor --all-targets -- -D warnings` 干净；改动文件 `rustfmt --check` 干净。

### 阶段 0 遗留（阶段 1 处理）

- `cargo fmt --all -- --check` 在 `crates/engine/examples/zz_scratch_circle.rs` 有 diff，
  **非本次改动**（工作区既有在途内容），按部分提交纪律不代为整理。
- ✅ 试验窗口已删除（`show_dock_trial` 字段与 `Ctrl+Alt+D` 开关一并移除），
  `dock.rs` 接管 `CentralPanel`。

## 阶段 1（2026-10-01）：五块固定面板区全量迁入 dock 树 —— ✅ 已完成

1. ✅ **落地形态**：`DockTab::{Resource, Hierarchy, Scene, Game, Inspector, Console}`；
   `main.rs::draw_frame` 里原先硬编码的"底部控制台 + 左资源 + 左层级 + 右属性 + 中央
   Scene/Game"五块合并成一次 `crate::dock::show_workspace(context, self)`。
   面板侧只做"外壳/主体"切分：`draw_console_body`、`draw_resource_browser_body`、
   `draw_hierarchy_body`（`draw_hierarchy_panel`/`draw_lua_console_panel` 的 `Context` 外壳
   保留，只服务 `perf_baseline` 的合成帧路径）。
2. ✅ **借用形态的根治方式（与原计划不同）**：没有把 `DockState` 挪出 `EditorApp`，
   而是把字段改成 `Option<DockState<DockTab>>`，渲染时 `take()` → 本地渲染 → 帧末放回。
   舞蹈只留在 [`dock::show_workspace`] 一处，面板侧拿到的就是普通 `&mut Ui`。
   独立 workspace 结构等阶段 2 真需要"布局 + 树 + 菜单"一起说话时再拆。
3. ✅ **`workspace_tab` 降级为派生态**：Scene/Game 标签在绘制自己时写回 `workspace_tab`；
   所有程序化切换必须走 `EditorApp::focus_workspace_tab`（工具栏 ◫/▣、窗口菜单、
   `apply_layout`、`restore_editor_context`、Play 自动切 Game 共 5 个入口）。
   直赋值会**静默失效**——实测 `game_view.rs` 的"Play 后自动切 Game 视图"就是这么坏的。
4. ✅ **可见性映射**：`sync_visibility` 每帧把 `show_resources_panel` / `show_lua_console`
   落到树的成员关系上（隐藏=移除、重新显示=落回原侧）。View 菜单的两个开关无需改动即继续生效。
5. ✅ **帧计时桶保留**：`record_panel` 从各面板外壳搬进 `DockTabViewer::ui`，键名不变
   （`resource_browser`/`hierarchy`/`workspace`/`inspector`/`console`），`perf_baseline` 口径不破。

### 阶段 1 实测到的 egui_dock 0.18 坑（写码前无法从教程得知，必须记住）

1. **`split` 的 `fraction` 属于"第一个子节点（左/上）"，不是新节点**。`Split::Right`/`Below`
   里第一个子节点是**旧**节点 ⇒ 直接把"新标签想要多大"传进去会得到互补的比例。
   症状极其隐蔽：控制台吃掉 79% 高度、资源列只剩 2.9% 宽度，编译与逻辑全"对"。
   `dock.rs::split_at` 按方向换算一次，调用点按字面意思读。
2. **`DockArea` 默认给"关闭全部/折叠"按钮预留 48px**（`show_leaf_close_all_buttons`/
   `show_leaf_collapse_buttons` 默认 `true`）。窄标签页的 tab bar 可用宽度会被正好压成
   `0.0`，命中 `widgets/dock_area/show/leaf.rs` 里 `tab_bar_scroll` 的 `assert_ne!` **panic**
   （framegate 的 resize 用例实测打崩）。这六个面板是常驻视图、可见性只有 View 菜单一个来源
   ⇒ `show_add_buttons/show_close_buttons/show_leaf_close_all_buttons/show_leaf_collapse_buttons`
   全关；浮动窗口的关闭按钮保留（0.18 的替代 setter 还是 deprecated）。
3. **`Tree::find_tab_from` 只搜主表面**：标签被拖成浮动窗口后会被判为"不存在"，
   于是每帧往主表面再补一份重复标签（同一个 `DockTab` 画两处）。跨表面判存在要用
   `DockState::find_tab`。
4. **`set_active_tab` 不改焦点**：`Tree::find_active_focused` 读的是*聚焦窗口*的活动标签，
   只调 `set_active_tab` 时"当前视图"仍停在旧焦点上 ⇒ 程序化切换要
   `set_focused_node(node)` + `set_active_tab(node, index)` 成对调用。

## 阶段 2（2026-10-02）：`dock_tree` 持久化、布局 v2 与重置 —— ✅ 已完成

1. ✅ **用 egui_dock 自己的 serde，不自定义格式**：0.18 **没有** `DockState::serialize()/deserialize()`
   方法（网上教程里那对 API 不存在），实际形态是 `DockState/Surface/Tree/Node/LeafNode/SplitNode/…`
   上的 `#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]` ⇒ 打开 `serde` feature
   就能整棵编码（它连带打开 `egui/serde`，已在 `engine/Cargo.toml` 注明）。
   自研"更小"的格式要手写递归 `Tree::split` 编排才能还原任意树，收益只有体积，不做。
2. ✅ **落盘形态与容错契约**：`EditorLayout.dock_tree: Option<Box<serde_json::Value>>`，
   `layout.rs` 只按"是不是对象"接受、**原样存取、绝不解读** ⇒ v1 的三条契约（未知键保留、
   save→load→save 字节相同、损坏回落默认且不覆盖源文件）继续成立；100 次往返幂等测试已含 `dock_tree`。
3. ✅ **`LAYOUT_VERSION` 1→2 不需要迁移步骤**：v1 文件只是缺 `dock_tree`，读回来就是"没存过"，
   `clamp()` 把旧版本号盖章成 2，下一次保存即为完整 v2 形状
   （`a_v1_file_loads_without_a_dock_tree_and_upgrades_its_version`）。
4. ✅ **"重置布局"零额外代码**：默认布局的 `dock_tree` 是 `None`，`apply_layout` → `restore_dock_tree`
   据此重建默认树；切工程、重启、重置三条路径共用同一个入口（`the_dock_tree_round_trips_through_the_editor_shell`
   末段守住"重置必须打回默认树"）。
5. ✅ **加载/重置不产生多余写盘**：`dock_tree_change` 首次观察只登记基线指纹，不算变化。
6. ✅ **解析失败的现场可观测**：`dock_tree` 存在但读不回来时推一条控制台提示
   （"布局文件里的面板树无法解析，已改用默认面板布局"），不静默回落。

### 阶段 2 实测到的 egui_dock 0.18 事实（同上，必须记住）

1. **`Rect::NOTHING` 的 ±∞ 经 `serde_json` 会变成 `null`，写进文件就再也读不回来**
   （egui_dock 自己在 `WindowState::rect` 注释里踩过同一个坑）。
   ⇒ `dock::serialize` 序列化后**立刻试解析一次**，失败就返回 `None`：宁可这一次不存布局，
   也不写一个打不开的文件。连带后果：**没画过一帧的树不可序列化**，`Tree::split` 新建的叶子
   在下一帧之前矩形仍是 ±∞（阶段 2 真实窗口证据的种子生成第一次就是这么失败的）
   ⇒ 任何"改完立刻存"的路径都要先画一帧。
2. **`show_separator` 每帧把 `split.fraction` 重新夹紧到"两侧各留 `separator.extra`（默认 175px）"**
   ⇒ 拖动窗口边缘本身就是**连续的真实结构变化**（1280×720 下控制台已经贴在 175px 地板上：
   1−175/704 = 0.75142；同一棵树缩到 640×360 后变成 0.50872，
   `shrinking_the_window_rewrites_split_fractions` 守住这条）。
   ⇒ 变化检测必须用**不含矩形**的结构指纹（`dock::signature`），且落盘要等稳定
   （`EditorApp::DOCK_LAYOUT_SETTLE = 300ms`），否则一次拖窗 = 每帧一次写盘。
3. **payload 里除了绝对矩形还带 egui_dock 的 `translations` 文案表**（整份文件约 4 KB）。
   矩形无害：`DockArea` 每帧从根矩形按 `fraction` 重算所有节点矩形，读回来的旧坐标首帧即被覆盖
   ⇒ 恢复出的布局与窗口大小无关。文案表是 `DockState` 的字段，剥掉会让反序列化失败，只能原样带走。
4. **同一标签出现两份 ⇒ 整棵树按损坏拒绝**（`deserialize`）：`sync_visibility` 按"树里有没有这个标签"
   决定补还是删，重复标签会让它判错（藏掉一处、另一处仍在）。

### 阶段 2 收尾（2026-10-02）：窗口菜单逐面板开关

1. ✅ **六个 `DockTab` 各自可切**：`EditorApp` 补 `show_hierarchy_panel / show_inspector_panel /
   show_scene_view / show_game_view`（原来只有资源/控制台两个开关），`dock::sync_visibility`
   六个标签逐个按开关增删，`add_tab` 给每个标签写死"原侧"锚点：层级回资源右侧、属性回中央右侧、
   **Scene/Game 回到同一个标签组**（`insert_tab` 进对方所在叶子，不新开一块区域）。
2. ✅ **一条守卫**：`set_central_view_visible` 是中央视图的唯一写入口，
   **最后一个亮着的中央视图拒绝关闭**（只写状态栏，不改状态）——两个都关掉工作区就没地方画了。
   关掉当前聚焦的那个时聚焦搬到还亮着的（`workspace_tab` 由标签绘制时回写，不搬就会停在
   一个不再绘制的标签上）。
3. ✅ **持久化**：`panel_visible` 六个键一起写（新增 `scene_view` / `game_view`，
   复用已有的 `hierarchy` / `inspector`）；`LAYOUT_VERSION` 保持 2——缺键默认 `true`，
   不需要迁移。文件里两个中央视图**同时**为 `false`（手改）时收敛成"只留 Scene"，
   并把聚焦搬到 Scene；加载路径先纠正聚焦再落树，避免把用户关掉的视图又打开。
4. ✅ **菜单形态**：窗口菜单的两条 `Item` 换成一行 `MenuRow::PanelToggles`（六个复选框）。
   勾选状态只读自 `show_*`，写入一律回 `run_menu_action`；点完**不关菜单**，一次打开可连勾多个。
   浮层三件（领域数据/发布/性能）仍是 `Item`——它们不是 dock 标签。
5. 验证：`cargo test -p editor` 179（lib）+ 306（bin）/ 0 failed，新增 4 条
   （dock 落点 1、菜单守卫/聚焦搬迁 2、布局往返+收敛 1），
   `cargo clippy -p editor --all-targets -- -D warnings` 与 `rustfmt` 干净。

## 实现要点

1. **DockTab 枚举**收口全部可停靠面板
：`Hierarchy / ResourceBrowser / Inspector / Scene / Game /
   Console / Performance / RuntimeDebug / DomainData / InputBindings / ProjectSettings`
   （对照 `panels/mod.rs` 清单，逐个登记来源面板文件）。
   → **阶段 1 只做前六个**（迁移前它们是固定面板区）。后五个本来就是独立 `egui::Window`
   （性能/领域数据等生产工具窗），不是"面板区"，是否收进 dock 留给阶段 2 决策。
2. **默认树 = 现状布局**：左列（Resource+Hierarchy）、右列（Inspector）、
   中央（Scene/Game 同 tab 组）、底部 Console。✅ 已落地，并且用**几何测试**钉住
   （`dock::tests::default_layout_matches_the_previous_fixed_panels`：渲染一帧后读
   `Node::rect()`，断言资源 200 / 层级 240 / 属性 360 / 控制台 150 的比例），
   比"肉眼看截图"更早抓住上面那条 fraction 反转坑。
3. **持久化**（阶段 2）：✅ 已落地，形态与计划不同——egui_dock 0.18 **没有** `serialize()/deserialize()`
   方法，改用它的 `serde` feature 整棵编码，`EditorLayout.dock_tree` 原样存取（详见"阶段 2"一节）。
   启动时读回、解析失败回落默认树并给控制台提示；"重置布局"= 默认布局（`dock_tree: None`）→ 重建默认树。
   100 次 save/load 幂等测试已扩到含 `dock_tree` 的整份 layout.json。
4. **panel_visible / panel_widths 语义迁移**：可见性 ✅ 已映射为 tab 的存在/隐藏；
   `panel_widths` 目前**只影响 `perf_baseline` 合成帧的外壳**，生产路径的宽度由 egui_dock
   split 自管 ⇒ 阶段 2 要么把它降级成"默认树初始比例"并在 UI 上说明，要么彻底出库。
5. ~~**拖出独立窗口（floats）第一期不持久化**（egui_dock 对 floats 的序列化支持有限）~~
   → **阶段 2 实测推翻**：浮动窗就是 `DockState` 的额外表面，serde 一并带走，
   `a_detached_float_window_survives_the_round_trip` 守住这条（含"不得被 `sync_visibility`
   重复补回主表面"）。剩下的真实限制是**窗口矩形为绝对坐标**，见"未完成项"。

## 风险

- ✅ egui_dock 小版本强绑定 egui——已实测并把映射写进 `engine/Cargo.toml` 注释（阶段 0 §1）。
- ✅ `CentralPanel` 消失后的"中央工作区矩形"改锚：`workspace_rect` 现在取 Scene/Game 标签的
  `ui.max_rect()`，唯一消费者 `panels/performance.rs`（性能面板默认落点）行为不变，
  其矮窗口退化断言测试继续通过。
- ✅ 帧预算：`perf_baseline` 32 场景全 ok，且阶段 2 补上了**同机 A/B**（基准 2026-09-27，
  dock 路径场景无劣化；详见验收清单最后一条）。
- ✅ 持久化的写盘频率：egui_dock 每帧按 175px 地板重夹 `fraction` ⇒ 拖窗口就是连续结构变化。
  已用"不含矩形的结构指纹 + 300ms 稳定窗口"收敛成一次拖窗一次写盘
  （`the_dock_tree_autosaves_only_after_the_layout_settles`）。

## 验收方式

- [x] 阶段 0 薄切片有独立提交或可复现 diff
- [x] 默认启动布局与 M27 前一致（截图 + 几何断言；截图在 `artifacts/tmp-m27-phase1/`，
      与 `tmp-m27-phase0/` 的旧布局逐块对齐，唯一新增 chrome 是 tab 条）
- [x] View 菜单可开关资源面板/控制台（迁移前也只有这两个开关）
- [x] **View 菜单可逐一开关全部 DockTab；"重置布局"回默认树**（重置 ✅ 阶段 2；
      逐项可切 ✅ 阶段 2 收尾：六个面板各一个复选框 + "最后一个中央视图不许关"的守卫，
      见 `dock::tests::every_panel_toggle_lands_on_its_own_side`、
      `menu::tests::{the_last_central_view_cannot_be_hidden, hiding_the_focused_central_view_moves_focus}`、
      `layout_tests::every_panel_visibility_round_trips_and_the_central_pair_converges`）
- [x] **任意拖拽重排 → 关闭重启 → 布局恢复；损坏 layout.json → 回落默认且不 panic**（阶段 2）
      —— 重排这一步是**程序化**的（`dock::tests::a_rearranged_tree_round_trips_through_json` +
      `main.rs::layout_tests` 两条接线测试），真实鼠标拖拽分隔条/标签页没有自动化覆盖；
      "关闭重启"用真实窗口复核过：把控制台搬到属性上方存盘 → 真实窗口重启 →
      11 张截图 0 失败（`artifacts/tmp-m27-phase2/`，`01-*.png` 里控制台确实在右列上方）。
      损坏回落：`layout.rs` 的四类坏文件测试 + `dock::deserialize` 的重复标签拒绝。
- [x] **`cargo test -p editor` 全绿 + 100 次布局往返幂等测试含 `dock_tree`**（阶段 2：
      179 passed（lib）+ 301 passed / 2 ignored（bin），`dock_tree` 已进 100 次往返测试）
- [x] `cargo test -p editor` 全绿（294 passed / 0 failed / 2 ignored）
- [x] perf_baseline **对比**报告（同机、同 debug profile，基准 = 2026-09-27 dock 化之前）：
      32 场景 `failed=0 result=ok`，报告 `artifacts/tmp-m27-phase2/perf-after-stage2.txt`。
      p50 逐场景对比：**dock 路径相关场景无一劣化**（`hierarchy_panel_frame_10k` −3.3%、
      `console_panel_frame_100k` −8.0%、`inspector_schema_path` −3.9%）；全表最差是
      `schema_field_edit_1k` +8.4%（2.120→2.298 ms，与 dock 无关的字段编辑路径），
      而同机同 profile 的run-to-run 摆动幅度本身就有 ±20%（`schema_field_rules_audit_400` −22.0%），
      故 ≤3% 的门槛判为**通过**（对 dock 热路径），其余差异记为噪声。

## 实现记录

- 阶段 0（2026-10-01）：见上节。
- 阶段 1（2026-10-01）：`dock.rs` 重写为 6 标签全集 + 默认树 + `show_workspace`；
  `main.rs` 删五块硬编码面板与试验窗口、增 `dock_state: Option<…>` 字段与
  `focus_workspace_tab`；`panels/{console,hierarchy,resource_browser}` 切"外壳/主体"；
  `panels/toolbar.rs`、`menu.rs`、`panels/game_view.rs` 的视图切换改走 `focus_workspace_tab`。
  验证：`cargo test -p editor` 294 passed（含 7 条 dock 测试：渲染后几何断言、
  240×180 极端窗口不 panic 断言各 1 条）、
  `cargo clippy -p editor --all-targets -- -D warnings` 干净、改动文件 `rustfmt` 干净、
  真实窗口 GUI 证据 11 张 0 失败（含 Play 切到 Game 标签的截图）、
  `--perf-baseline` 32 场景 `failed=0 result=ok`。
- 阶段 2（2026-10-02）：`layout.rs` 加 `dock_tree`（`Option<Box<Value>>`，原样存取）+
  `LAYOUT_VERSION` 2；`dock.rs` 加 `serialize`（带 ±∞ 试解析守卫）/`deserialize`（带重复标签拒绝）/
  `signature`（不含矩形的结构指纹）；`main.rs` 加 `dock_signature`+`dock_dirty_since` 两个字段、
  `restore_dock_tree`（加载/切工程/重置共用）、`dock_tree_change`（300ms 稳定窗口后才序列化），
  并把树接进 `capture_layout`/`maybe_persist_layout_on_change`。
  新增测试 8 条（dock 5 + layout 2 + main 接线 1，另加浮动窗 1 条）。
  验证：`cargo test -p editor` 179（lib）+ 301（bin）passed / 0 failed、
  `cargo clippy -p editor --all-targets -- -D warnings` 干净、改动文件 `rustfmt --check` 干净、
  `cargo check --workspace --all-targets` 通过、`--gui-smoke` 29 例 0 失败、
  真实窗口证据 11 张 0 失败（**这次带一棵非默认树**：控制台在右列属性上方，重启后仍在）、
  `--perf-baseline` 32 场景全 ok 且与 2026-09-27 基准同机 A/B（见验收清单）。
  ⚠ 踩坑记录：`gui_evidence` 运行器**会清空输出目录**，把临时备份放在证据目录里会被一起删掉。

## 未完成项 / 已知限制

1. ~~**dock 树不持久化**~~ ✅ 阶段 2 已解决：`layout.json` 的 `dock_tree` 带走整棵树
   （含浮动窗、活动标签、折叠态、拆分比例），重启/切工程/重置三条路径共用 `restore_dock_tree`。
   残留一条时序限制：**结构刚变完的那一帧不可序列化**（新叶子还没有矩形），要等下一帧——
   生产的 300ms 稳定窗口天然覆盖，但任何"改完立刻同步存盘"的新调用点都必须先画一帧。
2. **面板内标题与 dock tab 标题重复**（"资源面板/层级/属性"各出现两次）：截图上最明显的观感
   差异。要去重就得动 `panels/inspector.rs`——该文件当前有他人在途的未格式化改动，
   本轮按部分提交纪律不碰。登记为阶段 2 一并处理。
3. **Scene/Game 被拖到两个不同叶子时**，两者同帧都绘制 ⇒ `workspace_tab` 由树里后画的那个决定
   （默认树不会触发；用户自定义重排才可能）。工具栏 ◫/▣ 高亮会跟着后画者，属可接受抖动。
4. **隐藏后重新显示只保证"回到原侧"**，不还原本次会话里移除之前的精确位置（移除标签会丢掉它在
   树里的位置）。**跨重启已经解决**（树整体落盘），会话内这一条仍成立——要还原得另存移除历史。
5. **`panel_widths` 在生产路径失效**（见实现要点 §4），旧 layout.json 里的宽度值现在是死数据。
   阶段 2 未处理：它只影响 `perf_baseline` 合成帧外壳，出库要连带改那条路径。
6. **dock 叶子没有最小尺寸约束**：0.18 的 splitter 只把 fraction 夹在"分隔条宽度/可用空间"附近，
   用户可以把某个叶子拖到几像素宽。属性面板内部还有 `set_min_width(340.0)`，窄叶里内容会被裁掉
   （不会 panic，240×180 极端窗口有测试守着）。登记待决：给 `DockTab` 补最小尺寸，
   还是把面板内的硬 `min_width` 改成可横向滚动。
   ⚠ 这条与持久化有耦合：拖到极窄后 `fraction` 会被 175px 夹紧并**真的写进文件**，
   下次启动就是那个窄布局（用户视角是"面板突然变得很窄"）。
7. **Performance/RuntimeDebug/DomainData/InputBindings/ProjectSettings 五个工具窗未 dock 化**
   （见实现要点 §1）。
8. ~~**View 菜单只能逐一开关资源面板与控制台**~~ ✅ 阶段 2 收尾已解决：六个 DockTab 各有复选框，
   中央两个配了"至少保留一个"的守卫（守卫在**菜单这一侧**；手改文件把两个都写成 `false`
   由加载收敛处理，不报错）。残留一条与 §4 同源的限制：重新显示只保证回到"原侧"，
   不还原移除前的精确宽度。
10. **工具栏 ◫/▣ 与窗口菜单 ▸ 面板是两套语义**：菜单负责"在不在树里"，工具栏负责"聚焦哪个标签"，
    而聚焦一个被关掉的视图会**顺手把它打开**（`focus_workspace_tab`）。这是刻意的（点了就要看到），
    但用户视角是"我关了 Game，点了一下又回来了"。
9. **浮动窗的矩形是绝对坐标**（`WindowState::rect` 随树一起存）：换显示器/分辨率后窗口可能落在
   工作区外面。0.18 没有"恢复时夹到视口"的钩子，只能在读回后自己按 `screen_rect` 夹一次——
   本轮未做（真实拖窗场景需要多显示器复现）。
