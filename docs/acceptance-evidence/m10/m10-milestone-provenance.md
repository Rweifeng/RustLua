# M10 里程碑 provenance 与增量边界

日期：2026-08-24

## 标签

| 标签 | 目标提交 | 含义 |
|---|---|---|
| `v0.9-3d-game` | `ecc4000a847dfab274636d2f3fa37fefc65926d5` | M9-05 3D 性能、发布与验收基线 |
| `v0.10-editor` | `86095edd064039be10664bb3a6a04ae85cd15dd8` | M10-10 编辑器验收与性能基线 |

## 标签之后的增量

- `0b781a6`：M10-11 编辑器面板拆分与四行布局；
- `63ebff4`：M10-12 编辑器优化；
- `cdf7448`：M10-12 TMP 风格 SDF 字体；
- `a8a6aba`：M10-04 3D 透视视口与导航文档修改。

因此，`v0.10-editor` 是 M10-10 的历史基线标签，不等于当前 HEAD，也不自动覆盖 M10-11/M10-12 的后续功能和验收状态。

## 验证命令

```powershell
git tag --list 'v0.9-3d-game' 'v0.10-editor'
git rev-parse v0.9-3d-game^{commit}
git rev-parse v0.10-editor^{commit}
git show -s --format='%H %s' v0.9-3d-game
git show -s --format='%H %s' v0.10-editor
```

## 验收边界

M10-11 和 M10-12 的自动化测试、代码范围和当前未覆盖项分别记录在：

- `docs/acceptance-evidence/m10/m10-11-editor-layout.md`；
- `docs/acceptance-evidence/m10/m10-12-editor-sdf.md`。

两份证据均不把 `--check`、单元测试或 offscreen 数据结构检查描述成真实 GUI/GPU 视觉验收。
