# M3-06：纹理图集

## 目标

将一张纹理中的多个矩形区域命名并复用到 Sprite，避免每个 Sprite 单独绑定纹理；图集区域通过现有 `SpriteBatch` 继续合批绘制。

## 源格式

`.atlas` 是行式文本资产（与 scene/prefab 同一族约定：`#` 注释、空行跳过、`key=value` 头部、tab 分隔记录），
解析器在 `rustlua-assets/src/atlas_format.rs`，完整约定与理由见 `docs/63`：

```text
atlas_version=1
texture=assets/atlases/hero-0.png
region	idle_0	0	0	64	64
```

- `texture` 是合并纹理相对工程根的路径（打包器写出时统一为 `/` 分隔）；
- `region` 记录为 `名称 x y 宽 高`，坐标单位是**像素**，原点为纹理左上角；
- 同名 region 后写覆盖先写（与 `SpriteAtlas::insert` 一致）。

## 坐标约定

- 像素坐标原点在纹理左上角。
- 区域采用半开区间：`[x, x + width) × [y, y + height)`。
- UV 使用左上角原点约定，转换公式为：`uv_min = (x / texture_width, y / texture_height)`，`uv_max = ((x + width) / texture_width, (y + height) / texture_height)`。
- 纹理尺寸、区域尺寸不能为 0，区域不能越界或发生整数溢出。

## API

`engine_core::atlas` 提供：

- `AtlasRegion::from_pixels`：把像素矩形转换为 UV；
- `AtlasRegion::from_asset`：从 `TextureAsset` 的尺寸生成区域；
- `AtlasRegion::apply`：把区域 UV 应用到 `Sprite`；
- `SpriteAtlas`：保存命名区域；
- `SpriteAtlas::insert`：插入区域，同名插入覆盖旧区域；
- `SpriteAtlas::get`：按名称查询区域；
- `SpriteAtlas::from_definition`：从 `.atlas` 解析结果 + 纹理真实尺寸构造（越界在这里拒）；
- `load_atlas(root, atlas_path)`：读 `.atlas` 并按 `texture` 取纹理头尺寸，产出 `LoadedAtlas { texture_path, atlas }`；
- `resolve_atlas_reference(root, raw)`：解析 `图集路径#区域名`，非图集引用返回 `None` 原样透传。

`root` 是工程根，路径解析规则与渲染器读纹理**完全一致**（有根则 `root.join(path)`，无根则用原始路径）；
返回的 `texture_path` 保持 `.atlas` 里写的那份工程根相对路径，交给 `SpriteAsset` 后由渲染器再 join 一次。
没设工程根时相对图集路径解析不到 → 装配期响亮报错，不会静默退化成整张合批图。

发布包侧：assetc 会把 `assets/` 下的 png 压成 `RustLua` 容器，所以 `load_atlas` 取纹理尺寸前先过
`pack::decode`（只猜格式读图片头，不解码像素）；`.atlas` 不在压缩清单里、包里仍是明文，
`AtlasDefinition::load` 因此保持明文读取。

消费点只有两处，都在**装配期**解析，运行期抠图/合批代码零改动：Scene/Prefab 的 Sprite `texture=` 字段
（`script/scene_spawn.rs`）与 Lua `sprite.set(id, "texture=图集路径#区域名;size=...")`。

导入侧：`AtlasImporter`（版本 `atlas-2`）在 `validate` 阶段做真实解析，空文件/BOM/非 UTF-8/语法损坏
（定位到行）/缺 `atlas_version`/缺 `texture`/无 region/零尺寸/空名一律拒绝，非法输入不落缓存。
依赖侧：`.atlas` 的 `texture=` 与场景里的 `图集路径#区域名` 都会建边（`scene → atlas → texture`），
源图变更沿边把图集和场景标脏，见 `docs/14`。

打包侧：`rustlua-assets/src/atlas_pack.rs` 提供 shelf 装箱（padding、POT、超限分页，完全确定性），
CLI 入口 `cargo run -p game --example atlas_pack -- <输出目录> <前缀> <输入 png ...>` 产出
`{前缀}-{页号}.png` + 同名 `.atlas` 成对文件。

越界、零尺寸和无效图集尺寸返回 `AtlasError`，不会静默生成错误 UV。

## 验收

单元测试覆盖 UV 转换、越界拒绝、命名区域查询、区域应用和 SpriteBatch UV 传递；`.atlas` 往返与坏语法定位、
导入器校验门、打包器 golden 排布、`#区域名` 端到端解析（装配期读到区域 UV、断链响亮报错）各有对应用例。
真实 GPU Demo 从**磁盘上的 `.atlas` + PNG** 加载（不再是手工 `insert`），程序生成的 4×4 四色图集注册
`red`、`green`、`blue`、`yellow` 四个 2×2 区域，以一个纹理绑定、一个绘制批次渲染 4 个 Sprite：

```powershell
cargo run -p game --example atlas_demo --release
```

预期输出包含 `regions=4 sprites=4 vertices=24 draw_calls=1 texture=4x4 target=128x128`。