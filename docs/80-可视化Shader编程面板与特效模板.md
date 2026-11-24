# 可视化 Shader 编程面板与特效模板

> 状态：M84。目标是让不熟悉 WGSL/Shader 的开发者通过中文模板和参数面板制作常见游戏特效；
> 当前只生成 fragment 函数片段，不改变现有 mesh 顶点 ABI。

## 实战：炎刃斩击（多模板合成一张图，M84）

`examples/game_api/assets/fx/flame_blade_slash.sgraph` 是一份**可直接运行的成品**，演示怎么用模板节点
合成一个文档里没有现成模板的效果。它把三个模板的图拼进同一张图、同一个 `Output`：

| 子效果 | 用到的节点 | 接到哪里 |
|---|---|---|
| 刀光弧 | `sector(arc_width/radius/thickness/spin)` | `Output.alpha` 遮罩；`arc_width=0.24`（约 86°）才是"一道斩击"，太小会退化成月牙 |
| 白热刃口 | `sector → ×自身 → ×自身 …` 的幂次链 | 高次幂把软边压暗、只留刃口最亮处，再乘火焰色 → 亮白核心 + 暖橙过渡 |
| 火焰流动 | `texture(rotate_uv(uv)) × constant × sine(闪烁)` | 与幂次链相加进 `Output.tint`，沿刀弧铺一层会跳动的火 |
| 冲击波 | `ring(speed)` | 与幂次链相乘后并入 tint，环带扫过时提亮 |
| 深度描边 | `depth_edge(edge_width)` | 与火焰色相乘后加到 tint，场景交界处发亮 |

**关键构图经验**：`sector` 输出 Scalar，`Multiply`/`Add` 输出 Vec4；校验只允许 Scalar→Vec4
隐式提升，反过来不行，所以 alpha 只能直接接 `sector`，白热核心必须在 Vec4 一侧做。
`radius` 决定弧的向心位置——取 0.36 左右弧才落在画面内，0.5 会贴边被裁掉一半。
只用一层 `smoothstep` 的 sector 当 alpha 会得到"一团暗雾"，必须靠幂次链压出锐边。

它不是"加载三个模板再叠三层"——场景只能声明一个 `meta\teffect_overlay`（M88 时是 `project.toml`
的一个 `effect_overlay`），M90 的多层 API 只服务
编辑器手动叠加。正确做法是在一张图里用 `Multiply`/`Add` 把几种效果合成，输出一个 tint 与一个 alpha
（见该文件节点：`blade2/blade4/blade8` 幂次链 → `hot → white`、`fire → fire_body`、`shock → halo`
汇入 `tintmix → final → out.tint`，`blade → out.alpha`）。

命名参数是各模板参数的并集：`edge_width`、`color_r/g/b`、`frequency`、`speed`、`spin`、
`angle0/spin_speed`、`arc_width/radius/thickness`（10 个槽，远低于 16 上限）。
注意**同名参数出现多次会被自动加节点名前缀**（两个 `sector` 会得到 `blade_spin`/`core_spin`），
那会打断演示脚本里 `set_effect_param('spin')` 的调用——所以本图只用**一个** sector，
白热核心靠 `Multiply` 自乘而不是加第二个 sector。`demo_3d.lua` 的 `effect_overlay` 步
逐帧改 `spin`/`speed`/`color_r/g/b` 与 `time`，看到的就是运行时真在用的那套 uniform。

槽位顺序由拓扑遍历决定：本图 slot0=`edge_width`、slot1=`color`，其余按节点顺序排。
改图前先看 `compile_wgsl_uniform` 的槽位表，别在脚本里写死槽号。

> 三处 uniform 修正（本效果必需）：`ring` 读自己的 `speed` 槽、`rotate_uv` 读 `spin_speed` 槽、
> `constant` 的默认色取节点 `r/g/b` 而不是恒为白色。三者此前都错误地回退到 slot0，
> 逐帧调参会静默失效；回归门在 `rustlua-visual-script/tests/shader_graph.rs`。

## 使用方式

1. 在编辑器打开 **Shader Graph** 面板，下拉框按分类选择模板并点击「载入模板」。
2. 右栏顶部就是**实时预览**：默认自动播放，改模板/改参数后下一帧就能看到画面变化；
   可点「⏸ 暂停」定格，控制条会显示当前时间与单帧渲染耗时。工具栏的**效果强度**
   （0~2）是预览的总强度：同时缩放颜色与不透明度，拖到 0 即完全关掉效果；暂停时
   拖动也会重绘一帧，不必重新播放。
3. 画布里的节点与连线就是 `.sgraph` 里的节点与连线：拖动节点摆位置，选中节点后在右侧
   检查器调参数（正弦频率、边缘硬度、常量红绿蓝）。每次拖动只压一步撤销。
4. 检查器同时给出「分类 / 性能等级 / 先调什么」三项，非程序员不必翻文档就能开始调。
5. 「保存 .sgraph」按工程内相对路径原子保存图，并**顺带导出同名 `assets/fx/<名字>.wgsl`**
   作为可审阅产物；「打开 .sgraph」按同一路径回读。
6. 资源面板**双击 `.sgraph`** 直接打开本面板并载入那张图（简单操作入口）。

Shader Graph 资产使用 `.sgraph` 扩展名。资源导入器校验 UTF-8、BOM、图版本和连线结构；
`rustlua-editor-core::shader_graph_ops` 提供工程内路径限制、加载、原子保存与 WGSL 导出。
图是唯一源，生成的 WGSL 不应手工编辑。图模型位于 `rustlua-visual-script::shader_graph`，
不依赖 GPU，方便 CI 和无显卡编辑器运行。

## 实时预览是怎么工作的

- 面板持有一台 `ShaderPreview`（`panels/shader_graph_preview.rs`）：采样器、棋盘输入纹理、
  256×160 离屏目标、回读缓冲**只建一次**；换模板/改参数只重编管线。
- 每帧：`ShaderGraph → WGSL → 与上一份比对（变了才 `set_shader`）→ 推进 time → 渲染 →
  回读像素 → 上传成 egui 纹理`。播放时按 30fps 约帧（`request_repaint_after(33ms)`），
  不播放时不重绘，编辑器不会空转。
- 参数是 **uniform**（每个槽一个 `vec4<f32>`），拖 Inspector 参数只改这块 buffer，
  **不重编管线**；预览里看到的就是将来运行时同一段代码、同一套参数槽。
- 「效果强度」走 binding 2 的 `PreviewParams.intensity`（与时间同一个 buffer，32 字节），
  在包装层同时缩放 `rgb` 与 `alpha`；它只作用于编辑器预览，不写运行时叠加层的
  `EffectParams`（那条路径由 `render.set_effect_param(s)` 控制，避免动到 GPU 契约）。
- 预览器记住上一帧的槽位值与强度：暂停时只要其中任一变化就重写 uniform 并重渲一帧，
  否则"暂停调参"只改了内存、画面却不动。`effect_intensity_slider_repaints_while_paused`
  守住这个行为。
- 面板铺绘制指令的顺序是「检查器底色 → 实时预览图 → 模板说明」。底色必须在预览之前画：
  两者共用同一个 painter，若让检查器在预览之后再铺满底色，预览就会被整块盖住，
  表现为"模板说明上方一片空白"。`inspector_background_is_painted_before_live_preview`
  用无头帧守住这个顺序（GPU/无 GPU 两条路径各有断言）。
- 没有 GPU 设备（无窗口/远程）时面板显示「实时预览需要 GPU 设备」，不假装通过。

## 内置模板（47 个）

模板按**玩家感知的效果**分类，而不是按噪声、Fresnel 等实现算法分类。每行都对应
`ShaderTemplate` 里的一个枚举值，默认图可编译、可预览。

### 角色与状态

| 模板 | 中文提示 | 先调什么 | 性能 |
|---|---|---|---|
| 溶解消散 | 用噪声阈值控制透明度，适合角色消失、传送和物体生成 | 溶解阈值、边缘宽度、边缘颜色 | L2 |
| 边缘描边 | 根据边缘亮度增强轮廓，适合选中描边和稀有物品高亮 | 描边颜色、强度、宽度 | L1 |
| 受击闪白 | 短时间把颜色推向白色，适合受击反馈和无敌帧 | 闪白强度、持续时间 | L1 |
| 状态染色 | 状态色叠加在角色上，适合中毒、灼烧和冰冻等持续状态 | 状态颜色、混合强度 | L1 |
| 残影隐身 | 波形控制透明度并偏冷色，适合残影、隐身和闪避 | 残影透明度、时效 | L1 |
| 拾取闪烁 | 周期性亮度闪烁突出可拾取物，适合金币、掉落和宝箱 | 闪烁频率、高亮颜色 | L1 |
| 低血量暗角 | 边缘暗角提示危险，适合低血量、载具损坏和 Boss 狂暴 | 暗角强度、危险颜色 | L1 |
| 冰霜结晶 | 冷色边缘光表现冰晶质感，适合冰冻、护甲和寒冰技能 | 边缘强度、冰蓝色 | L1 |
| 暗影侵蚀 | 暗色透明波纹制造侵蚀感，适合暗影、诅咒和黑雾 | 侵蚀速度、暗色 | L2 |
| 辉光脉动 | 柔和脉动强调高亮区域，适合宝物、拾取物和魔法核心 | 脉动柔和度、高亮颜色 | L1 |

### 技能与魔法

| 模板 | 中文提示 | 先调什么 | 性能 |
|---|---|---|---|
| 能量护盾 | 边缘光叠加扫描线，适合护盾、结界和安全区域 | 边缘强度、扫描速度 | L1 |
| 全息扫描 | 时间和 UV 生成横向扫描线，适合全息投影、科幻 UI 和幽灵 | 扫描线密度、主色 | L2 |
| 魔法阵 | 环带脉冲配合纹理，适合技能预警、传送门和法阵 | 环带宽度、旋转速度 | L1 |
| 传送门 | 旋转波纹制造传送门能量，适合入口、出口和快速移动 | 旋转速度、能量颜色 | L1 |
| 闪电脉冲 | 高频脉冲制造电光闪烁，适合闪电、雷击和电弧武器 | 闪烁频率、电弧亮度 | L1 |
| 彩虹光谱 | 时间驱动色彩循环，适合彩虹、棱镜和稀有品质反馈 | 色相循环速度 | L1 |
| 治疗光环 | 绿色边缘光和脉动表现回复，适合治疗技能和回复区域 | 光环范围、回复色 | L1 |
| 技能预警圈 | 红色边缘脉动标出技能范围，适合地面预警和范围伤害 | 预警脉动速度、警示色 | L1 |
| 冲击波 | 扩张圆环配合脉冲，适合爆炸、落地冲击和范围技能 | 扩散速度、环宽 | L1 |
| 激光束 | 窄亮光束横贯画面，适合激光武器、瞄准线和扫描束 | 光束宽度、亮度 | L1 |

### 环境与元素

| 模板 | 中文提示 | 先调什么 | 性能 |
|---|---|---|---|
| 水波扭曲 | 时间推动 UV 波纹，适合水面、热浪和空间扭曲 | 波纹密度、流速 | L2 |
| 火焰流动 | 滚动纹理混合暖色，适合火焰、魔法火花和熔岩 | 流动速度、颜色梯度 | L2 |
| 熔岩流动 | 暖色滚动噪声表现岩浆流动，适合熔岩、熔炉和火山 | 滚动速度、熔岩亮色 | L2 |
| 毒雾 | 绿色脉动和透明度表现毒雾，适合中毒区域和药剂效果 | 脉动速度、毒雾颜色 | L2 |
| 烟雾翻涌 | 低频波纹表现烟雾厚度，适合尘埃、爆炸和遮蔽 | 翻涌速度、烟尘颜色 | L2 |
| 水下色偏 | 蓝绿色波动模拟水下光照，适合水底、潜水和潮湿区域 | 波动速度、水色 | L2 |
| 灼烧边缘 | 橙色边缘光和快速闪烁表现灼烧，适合点燃和持续伤害 | 灼烧频率、火焰颜色 | L1 |

### UI 与后处理

| 模板 | 中文提示 | 先调什么 | 性能 |
|---|---|---|---|
| 复古扫描线 | 周期性横向亮暗条纹，适合 CRT、监控画面和复古界面 | 条纹疏密、亮度 | L2 |
| 速度线 | 快速时间波纹制造拉伸感，适合冲刺、飞行和高速移动 | 拉伸速度、线条亮度 | L1 |
| 屏幕染色 | 采样已经画好的画面再染色，适合受伤滤镜、夜色和毒雾屏效 | 染色与强度 | L2 |
| 热浪扭曲 | 画面按时间抖动，适合热浪、火焰上方空气扭曲和传送门 | 扭曲强度与速度 | L2 |
| 屏幕折射 | 画面按偏移采样并偏冷，适合玻璃、水面与能量场折射 | 偏移量与折射颜色 | L2 |
| 深度描边 | 按深度差勾出物体轮廓，适合选中描边、X 光和剖面提示 | 描边宽度与强度 | L2 |
| 深度雾 | 按到相机的距离叠雾，适合远景雾、深水与恐怖氛围 | 雾浓度与雾颜色 | L2 |
| 环形刀光 | 扩张圆环配合快速脉冲，适合斩击、挥砍与技能轨迹 | 扩散速度与闪光频率 | L1 |
| 极光 | 缓慢流动的绿色光带，适合夜晚天空与高纬度环境 | 光带流动速度与颜色 | L1 |
| 星尘 | 高频闪烁的点状光，适合夜空、魔法粒子与氛围点缀 | 闪烁频率与星点亮度 | L1 |
| 螺旋传送 | 画面按极坐标卷成螺旋，适合传送门、隧道与吸入效果 | 旋转速度与极坐标缩放 | L2 |
| 漩涡扭曲 | 绕中心旋转采样画面，适合漩涡、时空扭曲与眩晕 | 旋转速度与扭曲强度 | L2 |
| 刀光弧 | 旋转扇形环带，适合挥砍轨迹、斩击与刀光 | 弧宽、转速与半径 | L2 |
| 扇形预警 | 宽扇区脉冲，适合技能范围预警与扫描雷达 | 扇区角度、转速与警示色 | L2 |
| 雨幕 | 竖排光束配高频闪烁，适合下雨、雨丝与坠落特效 | 雨丝宽度、密度与亮度 | L2 |
| 水柱扭曲 | 竖向抖动采样画面，适合瀑布、水柱与水下折射 | 抖幅与流速 | L2 |
| 线性扫光 | 方向渐变的脉冲光带，适合 UI 扫光与技能充能 | 方向与扫光速度 | L1 |
| 散景光斑 | 柔和径向光晕，适合 bokeh、灯光光晕与氛围点光 | 光晕强度与颜色 | L1 |
| 能量脉冲 | 扩张环的快速脉冲，适合能量爆发与冲击波 | 扩散速度与脉冲频率 | L1 |
| 胶片颗粒 | 高频噪点，适合复古滤镜、紧张与回忆氛围 | 颗粒频率与强度 | L1 |

## 性能等级与使用规则

| 等级 | 概述 | 模板示例 | 使用规则 |
|---|---|---|---|
| L0 极低 | 仅颜色/UV/时间与基础数学，无纹理采样 | （当前模板最低为 L1） | 可批量常驻；移动端默认档 |
| L1 低 | 边缘光或单次正弦；无纹理采样 | 护盾、描边、魔法阵、闪电、闪白、彩虹、速度线、冰霜、传送门、辉光、状态染色、残影、拾取闪烁、低血量暗角、治疗光环、技能预警、灼烧边缘 | 常规实时对象；同屏实例建议 ≤100 |
| L2 中 | 1 次纹理采样 + 时间波形 | 溶解、全息、水波、火焰、熔岩、毒雾、烟雾、水下、扫描线、暗影 | 绑定质量档与实例上限；避免多个全屏叠加 |
| L3 高 | 全屏后处理 / 多次采样 / 循环叠加 | 需要新增节点，当前模板集未包含 | 必须提供开关与降级路径；移动端默认关闭 |

> 等级是模板设计阶段的相对预算，不替代目标平台实测。面板在编译读数旁显示节点数、连线数
> 与生成耗时；超过等级预算时给警告，不阻止编译。

## 参数面板统一字段

参数元数据固定为：`显示名称`、`内部名称`、`类型`、`最小值`、`最大值`、`默认值`、
`单位`、`中文提示`、`性能影响`、`适用质量档位`。例如：

```text
显示名称：边缘发光
内部名称：edge_intensity
类型：Scalar
范围：0-5，默认 1.8
单位：倍
提示：提高边缘亮度；过高会过曝
性能影响：无额外采样；L1
适用质量档位：移动/中/高
```

对逻辑驱动的进度、生命周期和受击时间，面板显示默认预览值并标注「由游戏逻辑传入」；
恢复默认值不改变节点连线。

## 覆盖情况与待扩展

已覆盖的常见需求：角色消失/选中/受击/状态染色/残影/拾取/低血量/冰冻/暗影/拾取高亮、
护盾/全息/法阵/传送门/闪电/彩虹/治疗/技能预警、水面/火焰/熔岩/毒雾/烟雾/水下/灼烧、
CRT 扫描线与冲刺速度线。

需要**新增节点类型**才能做的常见效果（当前节点集做不到，暂不列进模板集，避免假模板）：

- 刀光弧/扇形技能：需要旋转 UV 与角度遮罩节点；
- 弹道辉光/命中火花/粒子拖尾：需要参数节点、随机数与多采样；
- 全屏后处理（泛光、色调、暗角、像素化、故障、CRT 曲面）：需要屏幕 UV 与场景颜色输入；
- 卡通阴影/法线贴图/顶点风摆：需要法线、世界坐标和顶点偏移节点。

这些应当在节点系统扩展后按同一格式加入，保持「每个模板都有可编译默认图 + 中文提示 + 性能等级」。

## 设计边界

- 节点支持 `UV`、`Time`、顶点色、常量、加减乘除、正弦、Fresnel、纹理采样和输出；端口做
  Scalar/Vec2/Vec3/Vec4 类型检查，不允许节点注入任意 WGSL。
- 图校验拒绝重复/非法节点 id、悬空连线、多个输出节点、类型不匹配和数据环。
- `.sgraph` 导入已注册；面板「保存 .sgraph」走工程内路径约束 + 临时文件原子替换，并导出
  同名 `.wgsl`；资源变化已接入文件观察去抖。
- 画布与磁盘是**同一份图**：节点标题/端口/连线都从 `ShaderGraph` 读，保存就是把这份图序列化，
  不存在"画布一套、资产另一套"。
- 面板检查器可改正弦频率、Fresnel 硬度、常量 RGB；一次拖动压一步撤销（快照在拖动开始时记录）。

## 验证门

`cargo test -p editor shader_graph`（20 项）在真实 wgpu fallback adapter 上跑离屏渲染并回读像素：

| 门 | 判据 |
|---|---|
| `every_template_renders_deterministic_non_empty_pixels` | 27 个模板各渲染一帧：无校验错误、像素数量正确、不是全透明、同输入两次渲染逐字节一致 |
| `preview_pixels_show_varied_colors` | 火焰/扫描线/全息至少两个模板能画出 ≥8 种颜色（证明不只是清屏） |
| `animated_templates_change_with_time` | 扫描线/火焰/速度线在 `time=0` 与 `time=0.5` 输出不同（时间 uniform 真接进 shader） |
| `texture_sampling_reads_the_bound_texture` | 棋盘纹理输入下输出至少两种颜色（纹理绑定真的被采样） |
| `invalid_shader_is_reported_instead_of_panicking` | 坏 WGSL 返回中文错误、渲染请求被拒，编辑器不 panic |
| `live_preview_uploads_a_texture_and_advances_time` | 面板画一帧就建出预览器、上传纹理、推进时间，且带纹理的预览图画在检查器底色之上（实时预览真的在跑） |
| `preview_intensity_scales_pixels_without_recompiling` | 同一份 WGSL、同一组图参数，只改强度 uniform：满强度非空、半强度画面不同、0 时全透明 |
| `effect_intensity_slider_repaints_while_paused` | 暂停时改「效果强度」必须重渲并上传新画面；参数没变时不反复重渲（防止滑杆空接线） |
| `inspector_background_is_painted_before_live_preview` | 无 GPU 时检查器底色之后还能看到预览占位块（防止整块底色盖掉预览，截图回归） |
| `panel_draws_headless_without_gpu_and_keeps_the_graph` | 无 GPU 时面板仍能画一帧且不改动图，也不会建预览器 |
| `open_relative_loads_a_saved_graph` | 双击入口按相对路径载入磁盘上的图，且打开即"干净" |
| `generated_overlay_source_changes_real_renderer_pixels` | 面板生成的叠加层交给真实 `Renderer`：能改变像素、关档回到基线（端到端契约） |
| `auto_layout_spreads_template_graph_by_topology` | 模板图自动布局后输出节点排在输入节点右侧 |
| `saved_graph_round_trips_through_panel_state` | 刚载入的图显示"未保存"，与保存快照一致后转干净 |

模板目录与数据层另有 9 项测试（`cargo test -p rustlua-visual-script --test shader_graph`），
其中 `templates_compile_to_pairwise_distinct_wgsl` 钉住"任意两个模板生成的 WGSL 不同"。

## 模板画廊（M89，已落地）

面板工具栏「模板画廊」把整块画布换成缩略图墙：**27 个模板各一张真实渲染的小图**，点一下就切过去。

- 缩略图不是占位图：每格都走 `compile_wgsl_uniform → ShaderPreview 渲染 → 回读 → egui 纹理`，
  与右侧预览是同一条代码路径。
- 逐帧补图（每帧最多 3 张），打开画廊不会卡住 UI；已渲染的缩略图常驻，第二次打开不再重渲。
- 没 GPU 时显示「模板画廊需要 GPU 设备」，不伪造。

验证门：`gallery_fills_every_template_thumbnail`（editor）在 fallback adapter 上连画 14 帧，
断言 27 个模板全部拿到缩略图且无渲染错误。

## 设计边界与下一阶段

### 运行时接入（M86，第一步已落地）

渲染器新增了**自定义特效叠加 pass**（`rustlua-gpu/src/renderer/effect_overlay.rs`），固定契约：

```wgsl
struct EffectParams { time: f32, width: f32, height: f32, _pad0: f32, tint: vec4<f32> };
@group(0) @binding(0) var<uniform> effect: EffectParams;
@group(0) @binding(1) var shader_graph_texture: texture_2d<f32>;
@group(0) @binding(2) var shader_graph_sampler: sampler;
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> ...
@fragment fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32>
```

- `Renderer::set_effect_overlay(Some(source) / None)` 设置或关闭；`set_effect_time(秒)` 推进动画。
- 默认 `None` ⇒ 渲染路径与旧版**逐字节相同**（像素门不受影响）；坏 shader 返回错误并保留旧管线。
- 纹理/采样器固定绑到 `binding 1/2`（`compile_wgsl_with_binding_base(1)`），给 uniform 让出 0；
  渲染器侧提供 1×1 白贴图，没有纹理输入的模板也能走同一条管线。
- 编辑器面板「**应用到场景叠加**」把当前图编译成这份契约并交给场景渲染器，场景视图里立刻能看到
  效果叠在画面上；「关闭场景叠加」撤掉。

验证门（`cargo test -p rustlua-gpu --lib effect_overlay`，4 项）：

| 门 | 判据 |
|---|---|
| `effect_overlay_is_off_by_default_and_removable` | 默认关档像素与基线相同；开档改变像素；关档回到基线 |
| `effect_overlay_blends_the_requested_color` | 红色叠加后红通道升高、绿/蓝不升高（真的混合了指定颜色） |
| `effect_overlay_reads_the_time_uniform` | `time=0` 与 `time=1` 输出不同（时间 uniform 真接进片元） |
| `invalid_overlay_keeps_the_previous_pipeline` | 坏 WGSL 报错、旧管线保留、画面不空白 |

编辑器侧另有端到端门 `generated_overlay_source_changes_real_renderer_pixels`：面板生成的叠加层
WGSL 交给真实 `Renderer` 后能改变渲染像素、关档能回到基线。

### 参数 uniform 化（M87，已落地）

参数不再烘焙进 WGSL，而是走 uniform 槽位：

- 生成器提供 `compile_wgsl_uniform(texture_base, params_binding)` → `CompiledShader`：
  - `source`：WGSL 片段，参数引用 `graph_params.p[槽].分量`
  - `params`：槽位表（节点 id + 参数名 → 槽号 + 分量），例如正弦频率占一个标量、常量颜色三通道共享一个 `vec4`
  - `slots`：16 个 `vec4` 槽的默认值；`param_values(&graph)` 按当前图重算
- 参数表固定绑在 `binding 3`（预览与运行时叠加同一编号）；纹理/采样器在预览是 `0/1`、在叠加层是 `1/2`。
- 改参数 ⇒ **只重写 256 字节 uniform，不重编管线**：面板拖动滑杆立即见效，运行时也可逐帧调。
- 槽位超过 16 个 → `TooManyParams` 明确报错，不静默截断。
- 字面量版 `compile_wgsl` 保留给 `.wgsl` 导出（导出文件应自包含、不依赖宿主参数 buffer）。

验证门：

| 门 | 判据 |
|---|---|
| `uniform_params_emit_slots_instead_of_baked_literals`（visual-script） | uniform 版不含烘焙频率字面量、含 `graph_params.p[`、参数表绑在 3；字面量版反之；颜色三通道共享一个槽；改图参数槽位值跟着变 |
| `too_many_params_are_rejected`（visual-script） | 17 个常量节点触发 `TooManyParams` |
| `effect_overlay_reads_the_params_uniform_without_recompiling`（rustlua-gpu） | 只改 `set_effect_params`、不重编管线，像素随之变化 |
| `parameter_edits_change_pixels_without_recompiling`（editor） | 改 Inspector 参数：像素变、生成的 WGSL 逐字节不变、槽位值跟着变 |
| `generated_overlay_source_changes_real_renderer_pixels`（editor） | 面板生成的叠加层交给真实 `Renderer` 能改像素、换参数再变、关档回基线 |

### 项目配置接入（M88，已落地）

> **2026-11-24 修订（`c75b7fa8`）**：本节记录的是 M88 的**工程级**接入，已经不再是现行口径——
> 叠加层已从 `project.toml [render] effect_overlay` **搬到场景级**：由 `.scene` 的
> `meta\teffect_overlay\t<工程内 .sgraph 路径>` 声明（工程级会让切到任何场景都共用同一个特效）。
> 代码侧入口是 `rustlua_scene::Scene::effect_overlay` / `set_effect_overlay`；现行示例见
> `examples/game_api/scenes/api_fx.scene` 与 `docs/81`。下文读作"M88 当时的设计"。

`project.toml` 的 `[render]` 段新增一个键：

```toml
[render]
effect_overlay = "assets/fx/fire.sgraph"
```

- **game 运行时**：启动读 `RenderConfig`，`GameApp::with_effect_overlay` 在渲染器建好后调用
  `ShaderGraph::load_runtime_overlay`，编译成叠加层并 `set_effect_overlay`；主循环每帧
  `set_effect_time` + `set_effect_params`（参数来自 `.sgraph` 槽位默认值）。坏资产只记日志、
  不阻止启动。
- **编辑器**：渲染器就绪（`attach_scene_shader_overrides`）与「渲染设置」保存后自动按配置挂载；
  每帧推进时间与参数。面板里手动「应用到场景叠加」优先于配置（手动状态存在时不再用配置覆盖）。
- 共用加载路径在 `rustlua-visual-script`：`ShaderGraph::load_runtime_overlay(root, relative)`，
  编辑器与运行时只有一份契约字符串，不会两边漂移。
- 路径约束与 `.sgraph` 保存同一口径：必须是工程内相对路径，`../` 逃逸被拒绝。

验证门：

| 门 | 判据 |
|---|---|
| `effect_overlay_path_round_trips_and_defaults_off`（rustlua-render） | `[render]` 段写出/解析 `.sgraph` 路径；空白值等于关闭；默认关 |
| `upsert_is_idempotent_and_appends_missing_section`（rustlua-render） | 新键写回后整段仍幂等 |
| `load_runtime_overlay_reads_project_sgraph`（visual-script） | 从工程 `.sgraph` 读出 `binding 0` 效果 uniform、`binding 3` 参数表与 `vs_main/fs_main`；`../` 逃逸被拒 |

### 场景颜色输入（M91，已落地）

叠加层现在**能采样当前画面**：有叠加层时整帧先渲到离屏场景色，再用恒等后处理回拷到最终目标，叠加层通过 `binding 4/5` 采样那份画面。默认无叠加层时渲染路径与旧版逐字节相同。

- 生成器新增 `SceneColor` 节点（采样画面），固定绑 `binding 4/5`（预览用棋盘占位，运行时用真实画面）。
- 新模板「屏幕染色」：画面 × 主色 + 时间脉动，适合受伤/夜色/毒雾滤镜。
- GPU 门 `overlay_can_sample_the_scene_color_binding`：叠加层把采样到的画面原样输出，必须与无叠加层时同一像素**逐字节一致**——回拷、绑定、坐标任一环错都会红。

### 深度输入（M92，已落地）

叠加层还能采样**深度**：`binding 6` 是 `texture_depth_2d`，`binding 7` 是采样器；渲染器每帧把当前深度视图交给 `EffectOverlay::prepare`（与 3D pass 写入的是同一份深度）。生成器新增 `DepthEdge`（邻居深度差 → 边缘强度，接 alpha 就是屏幕描边）与 `DepthFog`（距离做幂次 → 雾浓度）。新模板「深度描边」「深度雾」。

- 预览侧用 1×1 远平面占位深度（深度格式不能用 `write_texture` 直接拷，改用一次清屏 pass 写成 1.0）。
- **边界**：叠加层（屏幕后处理）本身不做软粒子——它拿不到粒子深度；软粒子在**粒子绘制**路径里做（GPU 粒子片元着色器比较自身片元深度与场景深度，见下节「软粒子」）。
- GPU 门 `overlay_can_sample_the_scene_depth_binding`：空场景深度是均匀远平面：叠加层把深度当灰度输出后整屏同色、alpha=1，证明绑定真的采到了深度值。

### 叠加层性能快路径与 2D 帧深度（M93，已落地）

- **按需离屏**：生成器只在图里真的用到 `SceneColor` 时才声明 `binding 4/5`、用到 `DepthEdge/DepthFog` 时才声明 `binding 6/7`；渲染器据"着色器源码是否出现 `@binding(4)` / `@binding(6)`"决定要不要走"离屏渲染 + 恒等回拷"。纯程序化模板（火焰/闪电/速度线等）直接叠在最终目标上，省掉一次全屏 pass 与一张全屏纹理读写。
- **2D 帧深度清理**：快路径下若该帧没有任何 3D 写入（`scene3d` 为空且没有网格 draw），叠加层绘制前先把深度清成远平面，避免深度类模板读到上一帧残留。
- 门：`scene_route_is_only_taken_when_the_overlay_samples_scene_or_depth`（纯颜色叠加层不触发离屏回拷，采样场景色的才触发）；原有场景色/深度像素门全部保持。
- **未取得**：开窗目视证据（只有无头 GPU 像素证据）与"有/无叠加层"的帧耗时读数；这两项需要在真实窗口里跑一次。

### 软粒子（M98，已落地并开放宽度）

引擎的 GPU 粒子**本来就有软边**（P3-R107）：绘制着色器按屏幕坐标 `textureLoad` 场景深度，与粒子自身片元深度比较，
`soft = smoothstep(0, softness, scene - frag.z)`，并且 `scene - frag.z < 0` 时手动深度剔除。M98 把原来写死的 `0.02`
变成可配置宽度：

```toml
[render]
particle_softness = 0.08   # 深度单位；0 = 硬边（关），默认 0.02 = 旧行为
```

- 编辑器：渲染设置面板新增「特效与粒子 → 粒子软边宽度（0=硬边）」拖拽条，保存即写入 `[render] particle_softness` 并套用到场景渲染器。
- 宿主 API：`Renderer::set_particle_softness(f32)` / `particle_softness()`；`apply_render_config` 会套用工程值。
- 改宽度会重建粒子绘制管线（缓存键 = 软边宽度 + 目标格式），只付一次编译。
- 门：`gpu_particle_draw_shader(w)` 的源级断言（默认档=0.0200、可调档=0.0800、占位符必须被替换）
  + 配置读写/夹紧门（越界夹到 0.5，`0` 表示关闭）。
- 像素门：`gpu_particles_soft_edge_fades_when_touching_scene_depth`（地面平面 + GPU 粒子）——粒子贴住场景深度时中心像素明显变暗（相对平面前方），且扫描多个间距能找到窄/宽软边像素不同的档，证明宽度真的进了管线。
- **未覆盖**：CPU/世界 Sprite 粒子路径没有深度比较（它们按画家序画、不写深度），目前仍是硬边。

### 叠加层帧耗时 A/B（M97，已落地）

`pwsh scripts/verify/bench_overlay_ab.ps1`（可用 `-Frames 600` 拉长采样以压噪声），或总门 `pwsh scripts/verify/verify.ps1 -OverlayBench`（显式开关，默认关）会临时给工程配两种 `.sgraph`（纯程化快路径 / 采样场景色路径），用 `game --exit-after-frames N --frame-profile` 在真实窗口跑三档，输出 `frame_p50/p95` 对比表，无论成功失败都还原 `project.toml` 并删除临时资产。

本机参考读数（release、真实窗口、fallback 软件适配器、240 帧、fixed-step、`examples/game_api`）：

| 档位 | frame p50 | 相对基准 |
|---|---|---|
| 无叠加层 | 78 660.5 µs | — |
| 纯程化（快路径） | 78 749.8 µs | +0.09 ms（+0.11%） |
| 采样场景色（离屏+回拷） | 78 954.2 µs | +0.29 ms（+0.37%） |

注意：软件适配器上单次差值会被噪声淹没（重复测量出现过 ±1%~3% 的反号），只把 p50 差值当量级参考；脚本自带“跑满帧数 / p50、p95 为正 / p95>=p50”的宽松断言，不做跨硬件阈值。

### 多叠加层（M90，已落地）

同一帧可以叠加多层特效：`Renderer::push_effect_overlay(source)` 追加一层（编译失败不动现有层），`clear_effect_overlays()` 清空，`set_effect_overlay(...)` 保持"整层替换"语义；多层按 push 顺序绘制，时间与参数一次写给所有层。GPU 门 `multiple_overlays_stack_in_push_order` 断言：第二层追加后像素变化、单层替换后退回一层、清空后像素回到基线。

### 特效预设：把特效做成预制体在逻辑里加载（M96，已落地）

#### 在属性面板里直接看预览（M99）

资源面板选中一份 `.sgraph`，右侧属性面板在资源详情下面直接跑一次真实 GPU 渲染：
预览图 + 「⏸/▶」+ 渲染耗时 + 模板来源 + 「在 Shader Graph 面板中打开」。

- **只在选中 `.sgraph` 时出现**：属性面板是"看当前选中项"的地方，没选中图不该常驻空预览。
- **与 Shader Graph 面板共用一台离屏渲染器与同一份包装着色器**（
  `shader_graph_preview::AssetShaderPreview`）：两边看到的观感不会漂移。
- **按路径复用**：选中另一个文件才重编管线，反复点同一个文件不重建。
- **坏图给红字而不是崩**：文件缺失 / JSON 半截 / 图非法都只写 `error`——
  点一下资源就崩编辑器，等于让人再也不敢点资源面板。

真窗口证据：`artifacts/shader-asset-preview/16-dpi1.5-1280x720-shader-asset.png`。

> 一个措辞细节：手工合成的图（如示例的炎刃斩击）按名字匹配不到内置模板，此时显示
> 「自定义图：<名字>（手工合成的节点图，不对应单个内置模板）」——匹配不到不是错误，
> 措辞不能让人以为模板识别坏了。

验证门：

| 门 | 判据 |
|---|---|
| `asset_preview_reports_bad_files_without_panicking` | 缺文件 / 半截 JSON 都只返回错误字符串 |
| `asset_preview_retargets_to_a_different_file` | 换文件必须重编（否则画面停在上一份图） |
| `asset_preview_advances_time_only_while_playing` | 播放时时间推进、暂停后不动 |

#### 在面板里生成预设

预设**不用手写**。Shader Graph 面板工具栏点「生成预设」，画布下沿开一条抽屉：

- 抽屉列出**全部**命名参数（不只是选中节点的），一行一个（宽屏一行两个）滑块，
  初值取当前图上的值；
- 拖滑块时右侧实时预览跟着变——预览读取的就是抽屉里的值，所以是"边调边看"而不是盲调；
- 填预设名与路径（默认 `assets/fx/<图名>.prefab`），点「生成 .prefab」。

生成时**先存 `.sgraph` 再写 `.prefab`**：预设引用的图必须和画面一致，否则会出现"参数对、
画面对不上"。写盘前会先把文本解析一遍，解析不过就不落盘——半截预设留到运行时才报错，
那时已经很难联想到是生成那一步写坏了。

参数是**全量写出**的（每个命名参数都写一行），不是只写动过的：只写动过的会让预设默默依赖
图的当前默认值，明天改了模板默认，这份预设就不再是"那一刻的样子"。

真窗口证据：`artifacts/shader-preset-evidence/15-dpi1.5-1280x720-shader-preset.png`。
这一版布局最初写成"一行排开 + 自适应宽度"，在 430px 宽的画布里「生成」按钮被挤出抽屉
右边界、点不到；`preset_drawer_keeps_every_widget_inside_a_narrow_drawer` 用"每个形状都
必须落在抽屉内"钉住了这件事，改成三行固定结构后通过。

#### 3D 舞台演示：怪物循环出刀（api_fx）

示例工程新增独立场景 `api_fx`（入口菜单卡片「Effect Presets」），三步看完：

| 步骤 | 看什么 |
|---|---|
| `blade_loop` | 怪物每 1.95 秒出一刀：出刀瞬间按预设挂层 → 逐帧改 `spin`/`speed` → 收势 `stop_effect`；**暖色与冷蓝两份预设交替** |
| `tune_params` | 锁定暖色预设，只让参数随时间走，能盯住"同一条弧在张缩" |
| `preset_swap` | 换成冷蓝预设：不改代码、不改 `project.toml`、不重启 |

实现要点（`scripts/demo_fx.lua`）：

- 三步共用一套时序 `stance → swing → follow`，只有 `advance(dt, mode)` 的 `mode` 不同；
  这样"循环释放"的逻辑只写一遍，三个步骤不会各走各的。
- 出刀时**每帧都可以 `play_effect`**：同名重播是替换而不是叠加，所以天然幂等，重播还会把
  刀光弧的相位重置成从头扫一遍。
- `on_destroy` 里必须 `stop_effect()`：特效层是宿主侧全局状态，不带干净会让**下一个场景**
  莫名其妙带着刀光。

**一个必须知道的坐标事实**：叠加层是全屏坐标，而演示框架（`lab`）把屏幕分成了左右分栏，
3D 视口只占中间/左侧一块。所以"怪物站在画面正中"并不等于"刀光弧圆心在怪物身上"——
`sector` 节点原本把圆心写死 `(0.5, 0.5)`，现在多了 `center_x` / `center_y` 两个命名参数
（默认仍是 0.5），预设里按视口位置调。示例预设用 `center_x=0.46` 把弧推到怪物身前。

> 同理：`radius` 从 0.36 调到 0.16 是为了让弧贴着角色扫过；半径 0.95 那种大环会跑到
> 屏幕边缘之外，看起来像"刀光消失了"。

证据图：`artifacts/fx-demo/{00-stage,01-blade-warm,02-blade-frost}.png`。

#### 预设文件格式与运行

> **与场景预制体共用扩展名**：`.prefab` 现在有两种用途——场景预制体（有 `entity`/`component` 行）
> 与特效预设（`kind=effect` + `graph=` + `param` 行）。凡是按 `assets/**/*.prefab` 枚举的工具，
> 都要先用 `EffectPrefab::from_text` / `kind=effect` 判一下，否则会把特效预设当场景预制体去解析，
> 报"unknown record"。（编辑器的 UI 布局报告测试就是这么被撞红的。）

前面几节说的都是"常驻特效"——同一时刻只能挂一张图，改特效要改场景声明（M88 时是工程配置）。真做游戏时
需要的是"这一下打出去，播一刀光"：**主动、可重复、有时机**。M96 把它做成预制体：

```
assets/fx/
├── flame_blade_slash.sgraph      # 图：怎么画（唯一源）
├── flame_blade_slash.prefab      # 预设：哪张图 + 一整套参数（暖色）
└── frost_blade_slash.prefab      # 预设：同一张图 + 另一套参数（冷色、更细）
```

预设文件沿用普通预制体的文本格式（`key=value` 头 + 制表符分行），用 `kind=effect` 区分，
因此资源扫描、编辑器预制体面板不需要为它加新分支：

```text
prefab_version=1
kind=effect
name=炎刃斩击
graph=assets/fx/flame_blade_slash.sgraph
overlay=true
param	spin	2.4
param	color_g	0.42
```

三个设计决定：

1. **预设只存参数，不复制图**。同一张 `.sgraph` 可以有多份预设（`two_presets_on_one_graph_differ_only_in_values`
   断言两份预设的槽位不同、命名参数表完全一致）。改画面 = 改参数，不重编管线。
2. **参数覆盖在加载期就合进槽位**。脚本 `play_effect` 之后不用补一遍预设参数，只写它真正想改的那几个；
   预设里写错名字（`spin_speeed`）在**加载期**就报错并列出可用参数，而不是运行时静默丢参数。
3. **命名层与常驻层分开存**。`render.stop_effect` 撤不掉场景声明的那一层——那是配置的
   意图（M88 的 `project.toml` 工程级，现为 `.scene` 的 `meta\teffect_overlay`）；
   反过来常驻层也不会被 `play_effect` 替换掉。绘制顺序是先常驻后脚本，招式盖在氛围之上。

Lua 面只有两个函数：

```lua
render.play_effect("assets/fx/flame_blade_slash.prefab")   -- 触发
render.play_effect("assets/fx/frost_blade_slash.prefab", "hit")  -- 带实例名
render.stop_effect("hit")   -- 撤这一层
render.stop_effect()        -- 撤全部脚本层
```

`play_effect` 的幂等性是关键：**同名再播是替换**，所以可以每帧调它（像状态机那样"声明我想要
这层在")，不会叠出两层越来越浓的残影。每层各自从 0 计时，重播一次刀光弧会从头扫，而不是接着
上一次的相位。

验证门：

| 门 | 判据 |
|---|---|
| `effect_prefab_round_trips_and_rejects_bad_paths`（editor-core） | 面板写盘：文本能解析回去、只收 `.prefab`、逃逸路径被拒、坏文本写盘前就挡下 |
| `generating_a_preset_saves_both_files_and_round_trips`（editor） | 生成预设先落 `.sgraph`；`.prefab` 含全部命名参数且能读回 |
| `preset_drawer_keeps_every_widget_inside_a_narrow_drawer`（editor） | 430px 宽抽屉里每个控件都在边界内（抓"生成按钮被挤出"） |
| `preset_drawer_lists_every_named_param_at_the_graphs_values`（editor） | 抽屉列全参数且初值等于图上的值；重开会重置 |
| `loads_the_sample_preset_and_merges_overrides_into_slots`（visual-script） | 示例预设 → 编译真实图 → `spin`/`color_g` 落进槽位，没写的 `radius` 保持模板默认 |
| `two_presets_on_one_graph_differ_only_in_values`（visual-script） | 两份预设指向同一张图、参数不同、结构一致 |
| `unknown_param_is_rejected_at_load_time_with_the_known_list` | 拼错的参数名加载期报错，并列出可用参数 |
| `scene_prefabs_are_rejected` | 普通场景预制体（`entity`/`component` 行）被判为"不是特效预设"，而不是报"第 2 行格式错误" |
| `named_effect_overlay_draws_replaces_and_removes`（rustlua-gpu） | 挂上变像素、同名重播逐字节相同（没叠两层）、撤掉回基线 |
| `named_layers_stack_on_top_of_the_project_overlay`（rustlua-gpu） | 两层并存；撤脚本层不影响工程级层 |
| `effect_play_and_stop_record_intent_and_reject_bad_paths`（rustlua-scripting） | 只接受 `.prefab` 路径；意图取走即清空 |

其中 `named_effect_overlay_draws_replaces_and_removes` 抓到过一个真 bug：快路径的判定写的是
`!self.effect_overlays.is_empty()`（只看工程级层），只有脚本层时整段叠加 pass 被跳过，
**挂上去一帧都不画**。改成 `effect_overlay_active()` 后修复，临时回退可稳定复现红灯。

`named_effect_overlay_applies_params_at_mount_time` 抓到第二个：`EffectOverlay` 的槽位初值是
**全 0**，而模板里 0 往往意味着"半径 0 / 厚度 0"。第一版实现是"先挂层、下一帧再推参数"，
结果起手那一帧（最显眼的那一帧）整层透明——离屏验收图里数出的差异是 0 字节。修法是把参数
并进挂载调用（`set_named_effect_overlay_with_params`），并在重挂时**显式清零**，否则换预设
会粘住上一套的颜色与半径。

离屏验收（`artifacts/effect-preset/`）：同一场景 `00-no-preset.png` vs `01-with-preset.png`，
冷蓝预设挂上后 136587/2073600 字节（6.6%）变化，弧形刀光可见。

### 仍然存在的边界

- **边界**：运行时宿主从配置挂载已通，参数也可由 Lua 逐帧调（M94/M95），预设加载也已通（M96）；
  剩下的缺口是**没有"播完自动停"的时长概念**——`.sgraph` 是无限循环的时间函数，收招要逻辑自己
  `stop_effect()`。要做"播 0.6 秒"得在预设里加 `duration=`，由宿主到点撤层（下一阶段）。
- 每层共用同一个 `set_effect_time`：引擎现状是单时钟，所以多层同时播时它们的相位是同步的
  （现在取"最年轻那层"的年龄）。要各层独立节奏，得让 `set_effect_time` 变成按层写入。
- 叠加层是**全屏层**，不遮挡也不跟随物体：特效固定在屏幕上，不做世界空间定位。
- 参数仍是编译期常量之外的部分已 uniform 化；节点类型/连线变化仍要重编管线（加载预设时一次性完成）。

### 下一阶段（按价值排序）

1. **预设带时长**：预设里加 `duration=` 与 `loop=`，宿主到点自动撤层，逻辑里就不必自己计时收招。
2. **按层时钟**：`set_effect_time` 变成 `set_effect_time(layer, seconds)`，多层各走各的节奏。
3. **编辑器内试播预设**：Shader 面板加"以这份预设预览"，所见即运行时所得。
4. **世界空间特效**：把叠加层的位置/尺寸从投影里算出来，让刀光跟着角色而不是贴屏幕。
