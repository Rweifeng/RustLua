# game_farm

用于整理“类星露谷物语”方向的免费农场像素资源集合目录。

当前目录主要用于资源收集与筛选，方便后续挑选、二次加工和接入现有引擎工程。

## 目录结构

- `project.toml`
  - `game_farm` 的最小项目清单
- `assets/third_party/environment/`
  - 场景地形、作物、农场物件等环境资源
- `assets/third_party/characters/`
  - 农夫/NPC 角色资源
- `assets/third_party/ui/`
  - HUD、按钮、图标等界面素材
- `assets/third_party/animals/`
  - 低约束动物资源（当前包含 CC0 动物表）
- `assets/third_party/interior/`
  - 家具和室内环境参考
- `assets/third_party/town/`
  - 小镇/街道/屋外装饰参考
- `assets/third_party/fish/`
  - 鱼类、水域与海洋生物资源
- `assets/third_party/mining/`
  - 矿石、矿洞采集物与相关图标
- `assets/third_party/foraging/`
  - 花草、蘑菇等野外采集物资源
- `assets/third_party/food/`
  - 食物、饮料、烹饪成品与厨房相关图标
- `assets/third_party/town_props/`
  - 路灯、指示牌等小镇门口和街道摆设
- `assets/third_party/kitchen/`
  - 餐具、桌子和室内厨房/餐厅摆设
- `assets/third_party/storage/`
  - 货盘、木箱和仓储杂物
- `assets/third_party/materials/`
  - 木料、石料、干草等建材与堆料贴图
- `assets/third_party/shop/`
  - 店铺货架、书架与陈列摆设
- `assets/third_party/licensed/`
  - 需要严格遵守署名或共享协议的参考资源，默认不直接并入主素材目录
- `assets/textures/`
  - 从第三方资源中挑选出的整理版主素材，命名已统一
- `assets/curated/`
  - 从整理版主素材中再次按玩法主题挑出的精选目录，便于快速做场景拼装和风格筛选
- `assets/final_candidate/`
  - 按 `主风格候选 A` 收敛后的准运行时候选层，便于后续正式接资源
- `assets/runtime_batch1/`
  - 批次 1“空间骨架”接入暂存层，准备继续拆成真正面向当前引擎的 runtime tileset
- `third_party/raw/`
  - 原始下载压缩包，便于追溯来源或重新解压
- `docs/THIRD_PARTY_ASSETS.md`
  - 资源来源、授权和使用建议
- `docs/STYLE_GUIDE.md`
  - 当前推荐主风格、混搭规则与不建议直接纳入主线的资源说明
- `docs/FINAL_ASSET_INDEX.md`
  - `assets/final_candidate/` 中各张图的预期用途索引
- `docs/RUNTIME_IMPORT_PLAN.md`
  - 从 `final_candidate` 接入运行时资源的批次、顺序和规则
- `docs/BATCH1_IMPORT_SPEC.md`
  - 批次 1“空间骨架”资源的具体接入草案与当前引擎约束说明
- `scripts/main.lua`
  - 最小项目脚本
- `scenes/resource_preview.scene`
  - 按主题分区摆放的资源展板场景
- `scenes/day_slice_preview.scene`
  - 更接近“农场一天切片”的组合预览场景，当前默认启动
- `scenes/farm_day_flow.scene`
  - 按“起床室内 -> 农场劳作 -> 镇上交易 -> 野外采集 -> 回家整理”串联的流程预览场景，当前默认启动
- `scenes/ui_preview.scene`
  - 将 HUD、按钮、菜单面板、库存与商店图标拼成的 UI 风格板场景
- `scenes/batch1_tilemap_preview.scene`
  - 使用 `runtime_batch1` first pass tileset 的 Tilemap 接入预览场景，当前默认启动

## 当前状态

已收集的资源均为可公开下载的免费资源，优先选择了授权清晰的 `CC0` 包。

当前已经额外整理出一批可直接引用的 PNG 到 `assets/textures/`：

- `environment/`：主 tileset、草地、泥地、板条箱、干草
- `crops/`：蔬菜、水果、谷物、种子，以及番茄/土豆/黄瓜单体与生长图
- `items/`：锄头、铁锹、浇水壶、金币、不同材质斧头
- `characters/`：农夫样例、马、女性角色待机图
- `animals/`：可直接候选使用的 CC0 动物表
- `interior/`：家具与室内墙体参考图
- `town/`：小镇 tilemap 与街景参考图
- `fish/`：鱼类、水草、水域地形与海洋小生物
- `mining/`：矿石与矿物图标
- `foraging/`：花草、蘑菇和野外采集物
- `food/`：食物、饮料、料理与厨房图标
- `town_props/`：路灯、指示牌和街道小物件
- `kitchen/`：餐具、桌子和厨房/餐厅摆设
- `storage/`：货盘、木箱和仓储杂物
- `materials/`：木料、石料、干草与建材贴图
- `shop/`：书架、货架和店铺陈列物
- `ui/`：按钮、菜单、HUD 条、图标、告示牌

另外新增了一个二次整理层 `assets/curated/`，它不替代 `assets/textures/`，而是从中按玩法主题挑出一批更容易直接拿来做原型和预览的小集合：

- `farm/`：农场地表、作物、农具、主角与动物
- `town/`：镇子 tilemap、摊位、路灯和店招
- `interior/`：室内 tileset、家具、桌椅和餐具
- `shop/`：书架、食物和开业牌等店铺陈列候选
- `workshop/`：木料、石料、货盘、木箱和作坊工具
- `gathering/`：鱼、矿石、蘑菇、花草和水域采集物

目前目录关系可以理解为三层：

- `assets/third_party/`：保留原始来源和授权信息
- `assets/textures/`：统一命名后的整理版候选主素材
- `assets/curated/`：面向具体玩法主题的精选小集合

在这三层之上，当前又补了一层更靠近正式接入的目录：

- `assets/final_candidate/`
  - 只保留 `docs/STYLE_GUIDE.md` 中“主风格候选 A”明确选中的资源
  - 适合后续做运行时图集、规范命名和正式接入引擎

已补一个最小项目骨架用于后续继续接入，但它目前仍以资源整理和预览为主，不是完整玩法样板。

当前场景层已经分成两类：

- `scenes/resource_preview.scene`
  - 用于按 `farm / town / interior / shop / workshop / gathering` 六个主题浏览代表素材
- `scenes/day_slice_preview.scene`
  - 用于把农场、镇子、采集、作坊和室内元素拼成一个更接近实际游戏气氛的组合切片
- `scenes/farm_day_flow.scene`
  - 用于按一天内的活动顺序浏览素材组合，便于判断“室内起步、下地、进镇、采集、回家”这条节奏线是否顺畅
- `scenes/ui_preview.scene`
  - 用于集中查看 `ui/ + food/ + items/ + shop/ + town_props/` 组合后的界面气质，便于后续选定 HUD 和商店 UI 方向
- `scenes/batch1_tilemap_preview.scene`
  - 用于验证 `farm_town_tileset_runtime.png` 与 `interior_tileset_runtime.png` 是否能在当前 Tilemap 工作流下成立

当前已经补了一份风格收敛文档：

- `docs/STYLE_GUIDE.md`
  - 把现阶段最协调的一套组合固定为“主风格候选 A”
  - 明确了 `Kenney + ScratchIO + Anwar + 若干 CC0 单体物件` 的混搭边界
  - 同时说明了为什么暂不把 `LPC` 与等距资源纳入默认主线风格

当前也已经把这套主风格候选真正落成一个目录：

- `assets/final_candidate/`
  - 当前包含地形、作物、角色、动物、室内、镇子、采集、作坊、店铺和 UI 的准运行时候选
  - 后续如果开始做真正的运行时图集，建议优先从这里取图
- `docs/FINAL_ASSET_INDEX.md`
  - 把 `final_candidate` 中每张图准备承担的用途先固定下来
  - 后续拆图集、做资源注册或命名规范时可以直接参考
- `docs/RUNTIME_IMPORT_PLAN.md`
  - 把“先导空间骨架、再导农场循环、再导采集作坊、最后导 UI”这条接入顺序固定下来
  - 后续开始真正做图集和资源注册时，可以直接按批次推进
- `assets/runtime_batch1/`
  - 已经把 `environment + town + interior` 这批空间骨架单独拎出来
  - 用于继续拆成当前引擎更容易消费的 tileset 与 Sprite 资源
- `docs/BATCH1_IMPORT_SPEC.md`
  - 明确了批次 1 当前还处在“接入前整理”阶段
  - 同时把现有 `Tilemap` 单纹理、按列取样的约束写清楚了
  - 当前也已经产出第一版 `farm_town_tileset_runtime.png` 与 `interior_tileset_runtime.png`

另外新增了一批“扩展农场题材”的参考资源：

- `Kenney Isometric Miniature Farm`：CC0，适合参考建筑/围栏/农田组织方式
- `Isometric Furniture and Walls`：CC0，适合补家具和室内布局参考
- `Kenney Roguelike Indoor Pack`：CC0，适合补厨房、桌椅、沙发等屋内布置
- `Kenney Pico-8 City`：CC0，可参考小镇街道、房屋和路面组织
- `Market Stand`：CC0，适合补集市摊位或商店外摆
- `Kenney Fish Pack`：CC0，可补钓鱼、鱼类图鉴和水域装饰
- `CC0 Mineral Icons`：CC0，可补矿石、矿物和采集系统图标
- `Flowers` 与 `Mushroom Pixel Art Pack`：CC0，适合补采集物、地表装饰和季节性小道具
- `Food Pack 16x16`、`32px Food Items` 与 `CC0 Food Icons`：CC0，适合补厨房、料理、餐桌和背包食物图标
- `Lamp post` 与 `Signpost 32px`：CC0，适合补村镇街道和店铺门口的识别物
- `CC0 Tableware Icons` 与 `Tables and Misc Props`：CC0，适合补厨房餐具、餐桌和店内摆设
- `Pallet Sprite` 与 `Wooden box`：CC0，适合补作坊、仓库、店铺后场和农场储物区
- `CC0 Sign Icons`：CC0，适合补店招、开业牌、方向牌和公告牌变体
- `16x16 Block Texture Set` 与 `Pixel Art Bookcase`：CC0，适合补木料、石料、干草和店内货架感
- `LPC farm animals / crops / fruit trees`：内容很强，但授权要求更严格，当前只作为参考素材保留
