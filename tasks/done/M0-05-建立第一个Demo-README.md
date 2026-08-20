# M0-05 建立第一个 Demo README

## 状态

ACCEPTED

## 目标

建立一个独立、可被新 AI 会话和开发者直接使用的最小 Demo README，说明运行方式、预期输出、验收命令和当前边界。

## 验收标准

- [x] 独立 Demo README 已创建；
- [x] README 说明 Debug/Release 运行命令；
- [x] README 给出预期输出；
- [x] README 给出完整质量门命令和故障排查方法；
- [x] README 明确 M0 不包含窗口、wgpu、Lua、ECS 和真实游戏玩法；
- [x] 文档 UTF-8 可读且无控制字符。

## 验收证据

- `docs/demos/README.md`；
- Debug/Release 运行输出：`p2 game runner 0.1.0`；
- 质量日志目录：`artifacts/logs/quality/`。

## 已知限制

当前 Demo 是命令行运行器，不是图形窗口 Demo。窗口和渲染能力属于 M1。
