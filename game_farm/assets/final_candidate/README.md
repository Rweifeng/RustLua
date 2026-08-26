# 最终候选素材

这里存放的是基于 `docs/STYLE_GUIDE.md` 中“主风格候选 A”拎出来的一组准运行时候选素材。

和其它目录的关系：

- `assets/third_party/`
  - 原始来源层，保留来源与授权信息
- `assets/textures/`
  - 整理候选层，保留更完整的可选池
- `assets/curated/`
  - 主题精选层，便于做预览场景和玩法原型
- `assets/final_candidate/`
  - 风格收敛后的准运行时候选层，优先作为后续正式接入的起点

当前目录只保留主风格候选 A 明确点名的资源，不再把所有相近变体一起放进来。

配套索引文档：

- `docs/FINAL_ASSET_INDEX.md`
  - 记录这里每张图的预期用途与当前接入优先级

## 当前收录范围

- `environment/`
  - 农场地形主结构与近景草地/泥地
- `town/`
  - 小镇 tilemap 与摊位
- `crops/`
  - 作物总表与少量单体焦点作物
- `items/`
  - 当前默认农具
- `characters/`
  - 当前农夫样例
- `animals/`
  - 当前动物占位表
- `interior/`
  - 室内主结构与家具参考表
- `kitchen/`
  - 餐具和桌椅摆设
- `fish/`
  - 水域与钓鱼主识别图
- `foraging/`
  - 花草与蘑菇采集物
- `mining/`
  - 当前默认矿石识别图
- `materials/`
  - 作坊/材料主 tilemap
- `storage/`
  - 仓储摆设
- `shop/`
  - 店铺陈列候选
- `town_props/`
  - 路灯与店招
- `ui/`
  - 当前默认 HUD、面板、按钮与货币图标

## 使用建议

1. 后续做真正的运行时图集，优先从这里开始，而不是重新回到全量 `assets/textures/`
2. 如果某类素材仍然不够，再回到 `assets/textures/` 或 `assets/curated/` 补充
3. 若后面决定做“主风格候选 B”，建议平行新增 `assets/final_candidate_b/`，不要直接覆盖这一层
