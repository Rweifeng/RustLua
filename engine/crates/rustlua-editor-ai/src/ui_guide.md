# RustLua UI 预设（UiSpec）契约

本文件由 MCP resource `rustlua://ui/guide` 提供，是「一句话创建一个游戏 UI 预设」的唯一契约。
按本文件构造 `UiSpec`，再用 `ui_validate_spec`、`ui_preview_spec`、`ui_create_preset` 三个 tool 落盘。

## 调用顺序

1. `ui_validate_spec { spec }`：有 `severity: "error"` 就按 `fix` 修，直到 error 为 0。
2. `ui_preview_spec { spec, width?, height? }`：看布局是否越界/重叠（返回 MCP image 内容块）。
3. `ui_create_preset { document, spec, preview? }`：创建**新**文件；已存在会拒绝，换一个文件名或改用 `ui_apply_patch`。

`document` 必须是工程内相对路径且以 `.ui.json` 结尾，例如 `ui/tavern.ui.json`。建议用
`ui/<英文小写 slug>.ui.json`，`document.name` 用同一个 slug。

## 顶层字段

```json
{
  "spec_version": 1,
  "document": {"kind": "widget", "name": "tavern",
               "canvas": {"reference": [1280, 720], "scale_mode": "fit_both", "safe_area": true}},
  "tokens":  {"color.surface": "#141018", "color.ink": "#f2e6d0"},
  "styles":  {"panel": {"base": {"bg": "@color.surface"}}},
  "root":    {"key": "root", "type": "panel"},
  "assets":  [],
  "bindings": []
}
```

- `document.kind`：`widget`（默认，推荐做 UI 预设）/ `prefab` / `scene`。
- `tokens`：扁平具名值（颜色、字号、间距、圆角、时长）。样式里用 `@token` 引用，未知 token 是 error。
- `styles`：`{ "base": {属性: 值}, "states": {"hover"|"pressed"|"disabled"|"focused": {属性: 值}} }`。
- `assets`：只有引用真实存在的图片时才填；第一版预设建议留空，用纯 panel/text/button 就不会缺资源。
- `bindings`：可选，`{target, property, source, mode?, default?}`；source 形如 `lua:<path>` / `save:<key>`。

## 节点（当前运行时只支持这 6 种）

| type | 必填 | 可选 |
|---|---|---|
| `panel` | — | `src`（图片引用） |
| `container` | — | `direction`: `v_box`/`h_box`/`grid`/`wrap`/`margin`/`center`、`padding`、`spacing`、`justify`、`align`、`columns` |
| `text` | `text` | — |
| `image` | `src` | `nine_slice` |
| `button` | — | `text`、`src` |
| `scroll_view` | — | `horizontal` |

**不要使用** `toggle`/`slider`/`progress`/`input`/`list`/`grid`（节点）/`tabs`/`modal`/`tooltip`/`spacer`/`safe_area`：
它们是 `planned()` 能力，当前 `Capabilities::core()` 会拒绝。`grid` 只能作为 `container.direction` 的值。

公共字段：

- `key`：节点唯一标识，不能重复、不能为空；AI 生成时用稳定的英文短名（`title`、`actions`、`buy`）。
- `layout`：`anchor` + `size`/`position`/`offsets`/`percent`/`grow`/`shrink`/`align_self` 等。
  `anchor` 取值：`top_left`、`top_center`、`top_right`、`center_left`、`center`、`center_right`、
  `bottom_left`、`bottom_center`、`bottom_right`、`stretch_horizontal`、`stretch_vertical`、`full_rect`。
- `style`：引用 `styles` 里的名字；不存在是 error。
- `events`：`click`/`pointer_enter`/`pointer_exit`/`pointer_down`/`pointer_up`/`focus`/`submit` →
  Lua 函数名（`module.function` 或标识符）。没有真实 Lua 函数时不要写 events。
- `children`：数组；`text`/`image` 是叶子，不能挂子节点。

## 样式属性白名单

`bg`、`background`、`color`、`font`、`font_size`、`radius`、`opacity`、`border`、`border_width`、
`border_color`、`shadow`、`nine_slice`、`padding`、`margin`、`spacing`、`align`、`justify`、
`text_align`、`tint`、`cursor`、`transition`。

不在白名单里的属性是 error；`transition` 当前运行时未消费，别用。

## 完整示例：酒馆对话预设

```json
{
  "spec_version": 1,
  "document": {
    "kind": "widget",
    "name": "tavern",
    "canvas": {"reference": [1280, 720], "scale_mode": "fit_both", "safe_area": true}
  },
  "tokens": {
    "color.surface": "#141018",
    "color.ink": "#f2e6d0",
    "color.accent": "#c98b3c"
  },
  "styles": {
    "panel": {"base": {"bg": "@color.surface", "radius": 12, "padding": [20, 20, 20, 20]}},
    "title": {"base": {"color": "@color.accent", "font_size": 28}},
    "body": {"base": {"color": "@color.ink", "font_size": 18}},
    "button": {
      "base": {"bg": "@color.accent", "radius": 8, "padding": [8, 16, 8, 16]},
      "states": {"hover": {"opacity": 0.9}, "pressed": {"opacity": 0.8}}
    }
  },
  "root": {
    "key": "root",
    "type": "panel",
    "style": "panel",
    "layout": {"anchor": "center", "size": [640, 360]},
    "children": [
      {"key": "title", "type": "text", "text": "酒馆", "style": "title",
       "layout": {"anchor": "top_center", "position": [0, 24], "size": [260, 40]}},
      {"key": "body", "type": "text", "text": "今晚想喝点什么？", "style": "body",
       "layout": {"anchor": "top_center", "position": [0, 80], "size": [420, 28]}},
      {"key": "actions", "type": "container", "direction": "v_box", "spacing": 12,
       "layout": {"anchor": "center", "position": [0, 40], "size": [220, 160]},
       "children": [
         {"key": "ale", "type": "button", "text": "来一杯麦酒", "style": "button",
          "layout": {"size": [220, 44]}},
         {"key": "stew", "type": "button", "text": "点一份炖肉", "style": "button",
          "layout": {"size": [220, 44]}},
         {"key": "leave", "type": "button", "text": "离开", "style": "button",
          "layout": {"size": [220, 44]}}
       ]}
    ]
  }
}
```

## 生成规则（减少返工）

- 先结构后视觉：`panel` 作根 → `container` 分组 → `text`/`button` 叶子；容器优先，绝对定位只用于标题/角标。
- 所有尺寸用设计稿的 1280×720 坐标；根面板 `size` 控制在 480–960 宽、240–540 高，居中放置。
- 文案用用户语言；key 用英文；颜色/字号先进 `tokens`，不要在每个节点写魔法值。
- 不引用不存在的 `asset:`；需要配图时先在 `assets[]` 里声明 `{key, role, path}` 并确认文件存在。
- 同一 `key` 不能重复；`text` 不能有 children；`button` 可以没有 `text`（纯图标按钮）但必须有可区分 key。
- 生成后先 `ui_validate_spec`，再 `ui_preview_spec`，最后 `ui_create_preset`；不要跳过校验直接落盘。
