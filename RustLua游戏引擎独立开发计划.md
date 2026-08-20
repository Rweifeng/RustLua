# Rust + Lua 独立游戏引擎开发计划

> 版本：2026-08-20
>
> 目标：以 Rust 为引擎核心、Lua 为游戏脚本，借鉴 Unreal Engine 5、Unity、Godot 的优点，为独立开发者设计一套可持续交付的 2D/轻量 3D 游戏引擎路线。
>
> 重要判断：这不是“从零复刻 UE5”，而是先做一个能支撑自己完成一款游戏的**垂直切片引擎**，再从真实需求中抽象通用能力。

---

## 1. 目标与非目标

### 1.1 第一阶段目标

在 12–18 个月、每周 15–25 小时投入下，完成：

- Windows 桌面平台优先；
- 2D 完整能力；
- 轻量 3D 能力；
- 可视化场景编辑器；
- Lua 游戏脚本、热重载和调试日志；
- 资源导入、依赖管理、打包发布；
- 一个可发布的中小型样板游戏。

### 1.2 暂不追求

第一版不做以下内容：

- 主机平台；
- 大型开放世界流式加载；
- UE 级 Nanite/Lumen；
- 完整蓝图可视化编程；
- 自研物理引擎；
- 自研音频编解码器；
- 网络服务器集群；
- 多人协作编辑；
- 复杂材质节点编辑器；
- “支持所有格式”的资产导入。

### 1.3 成功标准

引擎是否成功，不以模块数量衡量，而以以下结果衡量：

1. 能否在编辑器中创建场景、放置对象、修改属性并保存；
2. 能否用 Lua 完成玩家控制、敌人 AI、UI、任务和存档；
3. 能否从原始资源生成可运行的发布包；
4. 是否能定位常见错误、查看性能和资源问题；
5. 是否能用它在 3–6 个月内完成一款小型游戏。

---

## 2. 从 UE5、Unity、Godot 借鉴什么

### 2.1 UE5：借鉴“运行时/编辑器分层”和专业工具链

借鉴：

- 引擎运行时与编辑器分离；
- 模块化插件；
- 反射/元数据驱动的属性面板；
- Actor/Component 的易用对象模型；
- 强调 profiling、日志、资源烘焙和打包；
- C++ 底层与高层脚本/可视化逻辑结合。

不照搬：

- 复杂的 UObject 反射体系；
- 过早建设大型渲染管线；
- 大量引擎级宏和隐式生命周期。

Rust 版本的对应设计：

```text
Rust 运行时
  ├── 明确的资源句柄
  ├── 类型化组件
  ├── 显式 World/Scene 生命周期
  └── 少量 derive 宏生成编辑器元数据

编辑器
  ├── 读取元数据
  ├── 展示属性
  └── 通过命令修改场景
```

### 2.2 Unity：借鉴“资产工作流”和“脚本迭代效率”

借鉴：

- 资源导入数据库；
- 场景、预制体、材质、动画作为可复用资产；
- 组件化开发；
- 编辑器扩展；
- Play Mode 快速运行；
- 游戏逻辑与引擎底层分离。

不照搬：

- 把所有逻辑都放在一个 MonoBehaviour 风格对象上；
- 依赖大量运行时反射；
- 让资源路径字符串承担引用关系。

Rust + Lua 版本的对应设计：

```text
源资源 -> 导入器 -> 中间资产 -> UUID 资源库 -> 运行时包

场景对象 = Entity + 组件
预制体 = 可实例化场景子树
Lua 脚本 = 组件或行为资源
```

### 2.3 Godot：借鉴“轻量、开源、场景即资产”

借鉴：

- 节点/场景组合；
- 小而完整的编辑器；
- 低门槛调试运行；
- 开源项目的可读性；
- 不强制用户理解复杂底层概念。

不照搬：

- 让节点树成为所有系统唯一的数据结构；
- 过早支持大量平台和功能；
- 使用字符串信号连接替代类型化接口。

推荐的折中：

```text
编辑器层：Scene + Node/Entity 树，重视易用性
运行时层：ECS/数据导向系统，重视性能
桥接层：稳定的 EntityId、Component API、事件总线
```

---

## 3. 总体技术架构

```text
┌─────────────────────────────────────┐
│              Game Project            │
│  Lua gameplay / scenes / prefabs     │
└──────────────────┬──────────────────┘
                   │
┌──────────────────▼──────────────────┐
│              Editor                  │
│ Scene | Inspector | Assets | Console │
└──────────────────┬──────────────────┘
                   │ Editor API / IPC or in-process
┌──────────────────▼──────────────────┐
│              Engine Runtime          │
│ World | ECS | Scheduler | Events     │
│ Render | Physics | Audio | Input     │
│ Assets | Save | Networking(optional) │
└──────────────┬───────────┬───────────┘
               │           │
      ┌────────▼─────┐ ┌───▼──────────┐
      │ Platform      │ │ GPU Backends │
      │ winit/SDL     │ │ wgpu         │
      └───────────────┘ └──────────────┘
```

### 3.1 推荐的 Rust 技术栈

| 领域 | 第一版建议 | 说明 |
|---|---|---|
| 构建与包管理 | Cargo workspace | 统一依赖、测试和工具链 |
| 图形 | `wgpu` | 优先跨平台和开发效率；需要时再下沉到原生后端 |
| 窗口/输入 | `winit` 或 SDL3 | 二选一；若重视手柄和平台能力，优先评估 SDL3 |
| 数学 | `glam` | 向量、矩阵、四元数、SIMD 友好 |
| ECS | 自研最小 ECS 或 `bevy_ecs`/`hecs` | 先用成熟库，除非 profiling 证明需要自研 |
| 物理 | `rapier2d`/`rapier3d` | 不自研物理；按需启用 2D/3D |
| 模型 | `gltf` | 第一版主格式；避免同时支持 FBX、OBJ、DAE |
| 图片 | `image` + KTX2/Basis 方案 | 运行时使用压缩纹理，导入时转码 |
| 音频 | `kira` 或 `rodio` | 先解决播放、混音和总线，再考虑高级空间音频 |
| Lua | `mlua` + Lua 5.4 或 LuaJIT | 默认 Lua 5.4；性能敏感场景再评估 LuaJIT |
| 序列化 | `serde` + RON/JSON/二进制格式 | 编辑资产用可读格式，发布包用二进制 |
| 文件监听 | `notify` | 资源热重载和编辑器刷新 |
| ID/句柄 | `uuid` + generation handle | 资源用 UUID，运行时对象用代数句柄 |
| 日志 | `tracing` | 支持 span、分层过滤和异步输出 |
| 错误 | `thiserror` + `anyhow` | 库层类型化错误，应用层上下文错误 |
| 性能分析 | Tracy、RenderDoc、平台 GPU 工具 | 第一版就埋点，不要最后才补 |
| 编辑器 UI | `egui` 起步 | 快速完成工具；复杂内容窗口再考虑 Qt/Web UI |
| 测试 | Rust unit/integration + Lua 测试场景 | 将引擎测试和样板游戏测试分开 |

### 3.2 目录结构

```text
engine/
├── Cargo.toml
├── crates/
│   ├── engine_core/        # 时间、ID、事件、资源句柄、日志
│   ├── engine_ecs/         # World、实体、组件、系统调度
│   ├── engine_math/        # 坐标、变换、颜色、包围盒
│   ├── engine_platform/    # 窗口、输入、文件、线程
│   ├── engine_render/      # wgpu 设备、资源、材质、渲染图
│   ├── engine_scene/       # 场景、预制体、序列化
│   ├── engine_assets/      # 导入、缓存、依赖、热重载
│   ├── engine_physics/     # Rapier 适配
│   ├── engine_audio/       # 音频设备、总线、资源
│   ├── engine_scripting/   # Lua VM、绑定、生命周期
│   ├── engine_ui/          # 运行时 UI
│   ├── engine_debug/       # 控制台、调试绘制、profiler
│   ├── editor/             # 编辑器应用
│   ├── assetc/             # 资源导入与烘焙命令行工具
│   └── game_runner/        # 独立运行器/发布入口
├── examples/
├── tests/
├── assets/
├── scripts/
└── docs/
```

---

## 4. 核心架构决策

### 4.1 运行时：数据导向，编辑器：易用对象模型

不要在 Rust 中硬套 Unity 的 MonoBehaviour，也不要把所有内容都设计成纯 ECS。

推荐：

- 场景文件保存为实体树和组件数据；
- 运行时将场景编译/加载为 ECS World；
- 编辑器通过命令修改场景；
- 所有编辑操作支持撤销/重做；
- 引擎对象只通过稳定 ID/句柄暴露，不暴露裸指针。

### 4.2 资源引用：UUID 优先，路径仅用于导入

```text
TextureAsset {
  uuid: 7b...
  source: "assets/art/player.png"
  imported: "cache/texture/7b...bin"
  dependencies: []
}
```

规则：

- 场景、预制体、材质引用 UUID；
- 文件移动不应破坏引用；
- 资源数据库记录源文件、导入结果、依赖和版本；
- 发布时只打包实际依赖；
- 导入器必须可重复执行。

### 4.3 坐标系和单位第一天就固定

建议：

- 右手坐标系；
- Y 轴向上；
- 1 个世界单位约等于 1 米；
- 四元数用于旋转存储；
- 编辑器、物理和渲染使用同一套约定；
- 明确颜色空间：纹理、材质、帧缓冲分别标注 sRGB/线性空间。

### 4.4 时间与更新顺序

```text
Frame Begin
  ├── 处理平台事件
  ├── 读取输入快照
  ├── 固定步长：脚本/游戏逻辑/物理
  ├── 可变步长：动画表现、摄像机平滑
  ├── 同步渲染世界
  ├── 构建 RenderGraph
  ├── 提交 GPU 命令
  └── 输出 profiler/调试信息
```

第一版采用：

- 固定逻辑步长 60Hz；
- 最大补偿步数，防止螺旋死亡；
- Lua 脚本不允许直接修改渲染资源；
- 修改通过 Command/Event/Component API 进入引擎。

### 4.5 脚本边界

Lua 负责：

- 游戏规则；
- 玩家控制；
- AI 状态；
- 任务和对话；
- UI 行为；
- 关卡触发器；
- 存档数据组织。

Rust 负责：

- 渲染；
- 物理；
- 音频底层；
- 资源加载；
- 线程调度；
- 平台和输入；
- 内存敏感系统；
- 网络传输底层。

禁止 Lua 直接持有 Rust 内部可变引用。Lua 只持有：

```text
EntityId
AssetId
ResourceHandle
轻量值类型
命令/事件对象
```

### 4.6 Lua API 草案

```lua
local player = entity("Player")

function Player:on_create()
    self.speed = 5.0
end

function Player:on_update(dt)
    local x = input.axis("move_x")
    local z = input.axis("move_z")

    transform.move(self.entity, vec3(x, 0, z) * self.speed * dt)

    if input.pressed("jump") then
        physics.jump(self.entity, 5.0)
    end
end

function Player:on_destroy()
end
```

第一版 API 原则：

- API 少而稳定；
- 命名面向游戏开发者；
- 错误返回清晰；
- 所有引擎对象都能通过调试器查看；
- 先提供手写绑定，后续用元数据生成绑定；
- Lua 类型提示文件同步生成，支持 Lua Language Server。

### 4.7 Lua 生命周期

```text
Project Start
  -> 创建 Lua VM
  -> 注册只读常量和 API
  -> 加载项目模块
  -> 创建场景
  -> on_create
  -> 每个固定步长调用 on_update
  -> 场景切换时 on_destroy
  -> Project Stop，回收 VM
```

热重载策略：

- 第一版只支持重新加载 Lua 模块；
- 运行时状态存放在引擎组件或显式 state 表中；
- 热重载前调用 `before_reload`；
- 热重载后调用 `after_reload`；
- 不承诺任意闭包和协程状态无损迁移；
- 发布版本关闭调试热重载。

---

## 5. 分阶段开发计划

以下按独立开发者每周 15–25 小时估算。若全职投入，可将日历时间压缩，但不要同时压缩学习、测试和文档阶段。

### Phase 0：准备与技术验证（2 周）

目标：验证工具链和最小风险。

任务：

- 安装 Rust stable、rustfmt、clippy、cargo-nextest；
- 建立 Cargo workspace；
- 创建窗口；
- 初始化 wgpu；
- 绘制三角形；
- 集成 Lua VM；
- Lua 调用 Rust 函数；
- 运行 CI：格式检查、静态检查、测试。

验收：

- `cargo run` 打开窗口并绘制三角形；
- Lua 调用 `log.info()`；
- Windows 下可重复构建；
- 新机器按 README 能完成构建。

不要在此阶段：做 ECS、编辑器、物理和完整资源系统。

### Phase 1：核心运行时（4–6 周）

任务：

- GameLoop；
- 时间系统；
- 输入快照；
- 日志与错误上下文；
- EntityId、AssetId、句柄；
- World 与最小组件注册；
- 系统调度；
- 事件队列；
- 调试绘制。

验收 Demo：

- 10000 个移动实体；
- WASD 控制摄像机；
- 固定步长运行；
- 能打印每个系统耗时；
- 删除和重建实体不出现悬空句柄。

### Phase 2：2D 渲染与资源系统（6–8 周）

任务：

- 纹理加载；
- 精灵渲染；
- 图集；
- 摄像机；
- SpriteBatch；
- 文本；
- 透明混合；
- 图片导入缓存；
- 资源 UUID；
- 热重载纹理。

验收 Demo：

- 2D 平台跳跃 Demo；
- 1000 个精灵；
- 图集批处理；
- 修改 PNG 后编辑器/运行时刷新；
- 发布包不依赖源文件目录。

### Phase 3：场景、预制体与 Lua 游戏脚本（8–10 周）

任务：

- 场景序列化；
- Entity/Component 属性保存；
- 预制体；
- Lua 脚本组件；
- 输入 API；
- 触发器；
- 事件和消息；
- 脚本错误定位到文件和行号；
- Lua 类型提示文件。

验收 Demo：

- 用 Lua 完成玩家、敌人、拾取物、门和胜负逻辑；
- 修改 Lua 后无需重启窗口即可重载；
- 脚本错误不会直接导致引擎崩溃；
- 场景保存后重启仍能还原。

### Phase 4：编辑器 MVP（8–12 周）

任务：

- 项目打开/创建；
- 场景树；
- Inspector；
- 属性编辑；
- 资源浏览器；
- 拖拽实例化；
- 场景保存；
- 撤销/重做；
- Play/Stop；
- 控制台；
- Gizmo；
- 选中对象高亮。

验收：

- 不写代码也能搭建一个简单关卡；
- 所有修改均可撤销；
- 编辑器崩溃后不会损坏主场景；
- 运行时错误可点击定位到 Lua 文件。

编辑器建议：

- 第一版使用 egui 快速交付；
- 复杂文本编辑器、资源预览和布局系统不要过早自研；
- 编辑器中的每项修改都转换成 Command，统一支持 Undo/Redo。

### Phase 5：2D 完整化与第一款游戏（10–16 周）

任务：

- Tilemap；
- 动画状态机；
- 粒子；
- UI；
- 音频总线；
- 存档；
- 设置系统；
- 关卡切换；
- 打包；
- 崩溃日志；
- Steam/桌面发行前的基础检查。

建议游戏类型：

- 俯视角动作；
- Roguelike；
- 卡牌/策略；
- 平台跳跃；
- 模拟经营。

不建议第一款游戏：

- 开放世界；
- 联机 MMO；
- 大型 3D 动作；
- 高复杂度物理沙盒。

### Phase 6：轻量 3D（12–20 周）

任务顺序：

1. glTF 导入；
2. Mesh 与材质；
3. 深度测试；
4. 透视相机；
5. 基础光照；
6. 阴影；
7. 骨骼动画；
8. PBR；
9. Frustum Culling；
10. Instancing；
11. 后处理；
12. 场景烘焙或简单光照探针。

验收：

- 加载一个完整 glTF 场景；
- 播放角色动画；
- 具备基础材质和阴影；
- 能显示渲染统计；
- 在目标机器上达到明确帧率目标。

### Phase 7：工程化与发布（6–10 周）

任务：

- 项目模板；
- 资源构建命令；
- Debug/Release 配置；
- 增量导入；
- 版本迁移；
- 自动化测试；
- 安装包；
- 日志收集；
- 崩溃报告；
- 文档和示例项目。

验收：

- 从干净机器构建并运行；
- 资源缓存可复用；
- 旧项目格式可迁移；
- 样板游戏可以独立发布；
- 有一份从创建项目到发布的完整教程。

---

## 6. 个人时间管理与交付策略

### 推荐周节奏

```text
周一：学习/阅读源码 2 小时
周二：核心功能开发 2–3 小时
周三：核心功能开发 2–3 小时
周四：工具/编辑器 2–3 小时
周五：测试、文档、整理 1–2 小时
周末：连续实现和 Demo 验收 5–8 小时
```

### 每个功能必须有四个产物

1. 设计说明；
2. 最小实现；
3. 自动化测试或可重复 Demo；
4. 使用文档。

### 每两周一个可见结果

不要使用“完成渲染器”“完善架构”作为任务。改成：

- 场景中出现一个可移动角色；
- 纹理修改后自动刷新；
- Lua 控制敌人追踪玩家；
- 编辑器可撤销一次属性修改；
- 运行时显示 Draw Call 和帧耗时。

---

## 7. 性能目标与测量方法

### 7.1 第一版性能目标

以 Windows 中端开发机为基准：

- 2D：10000 个简单精灵保持 60 FPS；
- 轻量 3D：10 万级简单实例具备批处理或实例化路径；
- Lua：游戏逻辑不在每帧产生大量临时表；
- 资源加载：启动场景加载过程可显示阶段和耗时；
- 编辑器：常规场景操作响应时间低于 100ms；
- 内存：长时间运行无持续增长。

这些是工程目标，不是跨硬件承诺。正式优化前建立目标机、场景规模和采样方式。

### 7.2 必须埋点的位置

- 每帧总耗时；
- 固定更新耗时；
- 各 ECS 系统耗时；
- Lua 调用耗时；
- 资源导入耗时；
- GPU pass 耗时；
- Draw Call；
- 三角形数量；
- 活跃实体数；
- 内存分配和显存估算；
- 场景加载和卸载。

### 7.3 优化顺序

```text
先测量
 -> 找到最大耗时
 -> 判断 CPU/GPU/IO/分配/锁竞争
 -> 写最小基准
 -> 修改
 -> 复测
 -> 记录结果
```

优先避免：

- 每帧字符串格式化；
- Lua 与 Rust 高频跨边界调用；
- 每个实体一个 GPU 提交；
- 资源加载阻塞主线程；
- 大量小对象堆分配；
- 未经测量的复杂线程模型。

---

## 8. 学习路线

## A. Rust 基础与工程能力

### 必学

1. The Rust Programming Language（官方 Book）；
2. Cargo Book；
3. Rust API Guidelines；
4. Rustonomicon 中的 unsafe、FFI、并发和内存布局；
5. `Result`、错误上下文、trait、泛型、生命周期；
6. 迭代器、闭包、智能指针、并发原语；
7. workspace、feature、build script、测试和 benchmark。

### 学习目标

- 能设计不依赖全局可变状态的模块；
- 能解释所有权和借用为何成立；
- 能安全封装少量 unsafe；
- 能写可测试的库；
- 能用 profiler 而不是猜测优化。

### 练习项目

- 资源句柄库；
- 事件总线；
- 代数生成索引；
- 简单任务调度器；
- arena allocator 实验；
- 文件缓存系统。

## B. 数学与计算机图形学

### 必学

- 向量和矩阵；
- 坐标空间；
- 四元数；
- 变换层级；
- 相机和投影；
- 光栅化；
- 深度测试；
- 法线和切线；
- Blinn-Phong 到 PBR；
- 纹理采样和 mipmap；
- alpha 混合；
- GPU/CPU 同步；
- 帧图和资源屏障的基本思想。

### 推荐资料

- 《Fundamentals of Computer Graphics》；
- 《Real-Time Rendering》；
- 《3D Math Primer for Graphics and Game Development》；
- LearnOpenGL；
- Scratchapixel；
- Vulkan Tutorial；
- WebGPU Fundamentals；
- GPU Gems；
- RenderDoc 官方文档。

### 练习项目

- 软件光栅化三角形；
- 2D SpriteBatch；
- 3D 相机；
- 法线可视化；
- 阴影贴图；
- 延迟渲染 Demo；
- GPU instancing Demo。

## C. 游戏架构

### 必学

- Game Loop；
- ECS；
- 组件生命周期；
- 状态机；
- 行为树基础；
- 事件系统；
- 对象池；
- 资源生命周期；
- 场景和预制体；
- Undo/Redo Command；
- 存档和版本迁移；
- 确定性与固定时间步；
- 线程与任务调度。

### 推荐资料

- [Game Programming Patterns](https://gameprogrammingpatterns.com/)；
- 《Game Engine Architecture》；
- 《Data-Oriented Design》相关资料；
- Unity DOTS / Entities 文档；
- Godot Scene、Node、Resource 文档；
- Bevy ECS 文档和示例；
- EnTT/flecs 设计资料。

## D. Lua 与脚本系统

### 必学

- Lua 5.4 Reference Manual；
- table、metatable、module；
- coroutine；
- error/pcall/xpcall；
- userdata；
- C API/FFI 基础；
- VM 生命周期；
- 沙箱和权限；
- 热重载；
- 调试钩子；
- 类型提示与 Lua Language Server。

### 练习项目

- Lua 配置驱动的敌人 AI；
- Rust 注册 `input`、`transform`、`spawn` API；
- Lua 错误定位；
- 模块热重载；
- 脚本协程驱动对话；
- Lua 单元测试。

## E. 工具链和编辑器

必学：

- 文件监听；
- 资源数据库；
- 序列化；
- UUID；
- Undo/Redo；
- 命令系统；
- Inspector 元数据；
- 日志和控制台；
- 崩溃恢复；
- 发布构建；
- 自动化测试。

推荐做法：

- 用 egui 先完成编辑器 MVP；
- 用 RON/JSON 保存可读项目数据；
- 用二进制格式保存运行时缓存；
- 使用外部 Git 管理项目；
- 任何不可逆导入都保留源资源。

---

## 9. 完整资料清单

### 9.1 官方引擎资料

- Unreal Engine C++：
  https://dev.epicgames.com/documentation/en-us/unreal-engine/programming-with-cplusplus-in-unreal-engine
- Unreal Blueprints：
  https://dev.epicgames.com/documentation/en-us/unreal-engine/overview-of-blueprints-visual-scripting-in-unreal-engine
- Unity Programming：
  https://docs.unity3d.com/6000.0/Documentation/Manual/scripting.html
- Unity Asset Workflow：
  https://docs.unity3d.com/6000.0/Documentation/Manual/AssetWorkflow.html
- Godot Step by Step：
  https://docs.godotengine.org/en/stable/getting_started/step_by_step/index.html
- Godot Best Practices：
  https://docs.godotengine.org/en/stable/tutorials/best_practices/index.html
- Bevy Introduction：
  https://bevyengine.org/learn/quick-start/introduction/

### 9.2 Rust 官方资料

- Rust Book： https://doc.rust-lang.org/book/
- Rustonomicon： https://doc.rust-lang.org/nomicon/
- Cargo Book： https://doc.rust-lang.org/cargo/
- Rust 标准库： https://doc.rust-lang.org/std/
- Rust API Guidelines： https://rust-lang.github.io/api-guidelines/
- Rust Reference： https://doc.rust-lang.org/reference/

### 9.3 关键 crate 文档

- wgpu： https://docs.rs/wgpu/latest/wgpu/
- Rapier： https://docs.rs/rapier3d/latest/rapier3d/
- mlua： https://docs.rs/mlua/latest/mlua/
- egui： https://docs.rs/egui/latest/egui/
- serde： https://docs.rs/serde/latest/serde/
- tracing： https://docs.rs/tracing/latest/tracing/
- gltf： https://docs.rs/gltf/latest/gltf/
- kira： https://docs.rs/kira/latest/kira/
- notify： https://docs.rs/notify/latest/notify/
- uuid： https://docs.rs/uuid/latest/uuid/
- slotmap： https://docs.rs/slotmap/latest/slotmap/
- anyhow： https://docs.rs/anyhow/latest/anyhow/
- thiserror： https://docs.rs/thiserror/latest/thiserror/

### 9.4 Lua 官方资料

- Lua 5.4 Reference Manual： https://www.lua.org/manual/5.4/
- Lua 官方主页： https://www.lua.org/
- Lua Language Server： https://github.com/LuaLS/lua-language-server
- mlua： https://github.com/mlua-rs/mlua

### 9.5 图形学资料

- LearnOpenGL： https://learnopengl.com/
- Scratchapixel： https://www.scratchapixel.com/
- WebGPU Fundamentals： https://webgpufundamentals.org/
- Vulkan Tutorial： https://vulkan-tutorial.com/
- GPU Gems： https://developer.nvidia.com/gpugems/gpugems/contributors
- RenderDoc： https://renderdoc.org/
- Tracy Profiler： https://github.com/wolfpld/tracy

### 9.6 架构与实践资料

- Game Programming Patterns： https://gameprogrammingpatterns.com/
- 《Game Engine Architecture》：Jason Gregory
- 《Real-Time Rendering》：Tomas Akenine-Möller 等
- 《The Rust Programming Language》：Steve Klabnik、Carol Nichols
- 《Crafting Interpreters》： https://craftinginterpreters.com/
- 《Designing Data-Intensive Applications》：Martin Kleppmann

---

## 10. 每个阶段的学习与实践配对

| 阶段 | 学习 | 实践产物 |
|---|---|---|
| 0 | Rust/Cargo、窗口、wgpu、Lua VM | 三角形 + Lua 日志 |
| 1 | ECS、游戏循环、并发 | 10000 实体运动 |
| 2 | 2D 渲染、纹理、批处理 | 2D 平台跳跃 Demo |
| 3 | 序列化、资源、脚本绑定 | Lua 控制完整关卡 |
| 4 | 编辑器、命令模式、撤销 | 可视化场景编辑器 |
| 5 | UI、音频、存档、打包 | 第一款可发布 2D 游戏 |
| 6 | 3D 数学、PBR、动画 | 轻量 3D Demo |
| 7 | 性能、CI、版本迁移 | 引擎模板和发布管线 |

原则：

> 每学习一个引擎概念，都必须把它做成一个能运行、能测量、能解释的 Demo。

---

## 11. 测试、质量与版本策略

### 11.1 测试分层

```text
单元测试：数学、ID、资源路径、序列化
集成测试：World、Lua 绑定、资源导入
场景测试：加载场景、运行脚本、保存存档
渲染测试：截图对比、shader 编译检查
性能测试：实体数量、资源加载、Lua 边界调用
发布测试：干净机器、不同分辨率、无开发资源运行
```

### 11.2 必须建立的规则

- `cargo fmt --check` 不通过禁止合并；
- `cargo clippy --all-targets --all-features` 定期执行；
- unsafe 代码集中到少数模块并写安全不变量；
- 每个资源导入器都有错误样本测试；
- 场景格式有版本号；
- 每次破坏性修改写迁移脚本或迁移说明；
- 重要功能有最小复现 Demo；
- Debug 和 Release 都要测试。

### 11.3 版本控制

```text
main       可运行
next       下一阶段集成
feature/*  单个功能
release/*  发布准备
```

每周至少一次可回退的稳定标签：

```text
v0.1-window
v0.2-sprite
v0.3-lua
v0.4-editor
```

---

## 12. 风险与应对

### 风险 1：项目范围失控

应对：

- 只支持一个平台起步；
- 只支持一个主资源格式；
- 每阶段只能新增少数核心能力；
- 每季度必须做一次范围削减。

### 风险 2：编辑器吞噬全部时间

应对：

- 先用文本场景 + 简单 Inspector；
- 先完成游戏，再反推工具；
- 不实现复杂节点图和可视化脚本；
- 让 Lua 文件和场景文件始终可以手动编辑。

### 风险 3：Rust 借用设计阻塞开发

应对：

- 优先使用 ID、句柄和命令队列；
- 避免跨系统长期持有引用；
- 不为了“零拷贝”牺牲架构清晰度；
- 对资源和实体采用明确的阶段性访问。

### 风险 4：Lua 脚本性能不稳定

应对：

- 限制每帧跨语言调用次数；
- 批量读取组件数据；
- 高频逻辑放 Rust 系统；
- 避免在 Update 中大量创建 table；
- 使用 profiler 识别真正热点。

### 风险 5：渲染器复杂度膨胀

应对：

- 先做 Forward Renderer；
- 再做材质和阴影；
- 最后考虑 RenderGraph、Deferred、GPU Driven；
- 每一步都有可运行 Demo。

### 风险 6：第三方库变化或停止维护

应对：

- 所有第三方依赖通过适配层接入；
- 业务代码不直接依赖 wgpu/rapier/mlua 的大量细节；
- 锁定版本并记录替换方案；
- 关键接口保留最小抽象。

---

## 13. 第一款游戏建议

建议把第一款游戏限制在：

- 1–3 个核心玩法；
- 10–30 个关卡或一张小地图；
- 2D 或低复杂度 3D；
- 不联网；
- 单人可完成；
- 资产数量可控；
- 有明确结束条件。

例如：

```text
俯视角 2D 动作 Roguelite

引擎能力覆盖：
- 场景和预制体
- Lua 玩家控制
- 状态机
- 敌人 AI
- 碰撞和伤害
- UI
- 音频
- 存档
- 随机房间
- 打包发布
```

这类游戏能覆盖引擎大多数基础能力，又不会被开放世界、联网和大型动画系统拖垮。

---

## 14. 立即开始的前 14 天清单

### 第 1–2 天

- 安装 Rust stable；
- 安装 IDE 和 rust-analyzer；
- 建立 Git 仓库；
- 建立 Cargo workspace；
- 写项目 README 和技术决策记录。

### 第 3–4 天

- 创建窗口；
- 创建 wgpu 设备、队列和表面；
- 绘制三角形；
- 添加窗口缩放处理。

### 第 5–6 天

- 接入 Lua；
- 注册 `log.info`、`time.delta`；
- 捕获 Lua 错误并打印文件行号。

### 第 7 天

- 写第一份 profiler 记录；
- 固定时间步；
- 做一个移动正方形 Demo；
- 打标签 `v0.1-foundation`。

### 第 8–10 天

- 实现 EntityId；
- 实现 Transform、Velocity；
- 实现移动系统；
- 实现系统耗时统计。

### 第 11–12 天

- 加载 PNG；
- 绘制 Sprite；
- 加入摄像机；
- 用 Lua 控制玩家。

### 第 13–14 天

- 写一份场景 JSON/RON；
- 加载场景；
- 保存场景；
- 完成第一个“从文件到可玩的 Demo”。

---

## 15. 最终技术路线结论

### 推荐默认方案

```text
Rust stable
wgpu
winit 或 SDL3
bevy_ecs / hecs
Rapier
mlua + Lua 5.4
egui
Serde + RON + 二进制缓存
GLTF
Kira
Tracing
Tracy + RenderDoc
Windows first
2D first, lightweight 3D second
```

### 关键架构原则

1. 运行时和编辑器分层；
2. 编辑器追求易用，运行时追求数据导向；
3. UUID 管理资产，句柄管理运行时对象；
4. Lua 只访问稳定 API，不接触内部借用；
5. 先用成熟库，不自研物理、音频和编解码；
6. 先做垂直切片，再抽象引擎；
7. 每个阶段交付可玩的 Demo；
8. 从第一天开始做日志、测试和性能埋点；
9. 只支持一个平台起步；
10. 引擎的最终价值由第一款游戏验证。

> 最现实的路线不是“先完成引擎，再做游戏”，而是“引擎和第一款游戏共同演化”：每个引擎功能都必须服务于一个可验证的游戏需求。

---

## 附录：资料使用顺序

### 第一轮：能跑起来

1. Rust Book；
2. Cargo Book；
3. wgpu 文档；
4. mlua 文档；
5. LearnOpenGL 前 1–6 章。

### 第二轮：建立引擎思维

1. Game Programming Patterns；
2. Game Engine Architecture；
3. Unity Asset Workflow；
4. Godot Step by Step；
5. Bevy ECS 示例。

### 第三轮：性能和图形深入

1. Real-Time Rendering；
2. GPU Gems；
3. RenderDoc；
4. Tracy；
5. wgpu 源码和示例。

### 第四轮：脚本与工具链

1. Lua 5.4 Reference Manual；
2. Crafting Interpreters；
3. Lua Language Server；
4. 资源导入器和编辑器源码；
5. 存档、版本迁移和发布工程。

---

## 资料调研说明

本文优先采用截至 2026-08-20 可访问的官方或一手资料：Epic Unreal Engine 文档、Unity 6 文档、Godot stable 文档、Rust 官方 Book/Cargo Book/Rustonomicon、Lua 5.4 官方手册，以及 docs.rs 上的 wgpu、Rapier、mlua、egui、Serde、Tracing、gltf、Kira、notify 等项目文档。具体 API 使用时仍应以项目锁定版本的文档为准。

