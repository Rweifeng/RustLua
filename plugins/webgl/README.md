# WebGL 平台插件

浏览器 WebGL 发布所需的平台侧文件都放在这个目录，`scripts/package.ps1 -Platform webgl`
会把它们复制/生成进发布包。

- `web-runtime.js`：浏览器运行时。负责从 `pack/` 读取工程与场景、用 Fengari 执行 Lua、
  把 `ui.*` 调用与场景静态 UI（`UiLayout` + `Image`/`Text`）画到 2D canvas，并接入
  键盘/鼠标输入与启动自检标记（`data-p2-boot` / `data-p2-drawn` / `data-p2-ui-*`）。
- `pkg/fengari-web.js`：随包分发的 Lua VM（本地依赖，不请求 CDN，离线可运行）。
- `pkg/game_web.js`、`pkg/game_web_bg.wasm`：发布时由 `wasm-bindgen` 生成（不入库）。

## 发布与验证

```powershell
# 发布
powershell -NoProfile -File scripts\package.ps1 -Platform webgl

# 验证产物：启动静态服务器 + 无头浏览器，断言 boot 与画面像素
powershell -NoProfile -File scripts\verify-webgl-bundle.ps1 -BundleRoot examples\game_jhyt\builds\webgl\jianghu_youtian
```

发布包目录下还带一个 `test-webgl.bat`：双击即验证并起本地服务器，
然后浏览器打开 `http://127.0.0.1:8137/` 即可试玩。

## 画面自检

`verify-webgl-bundle.ps1` 除了断言 `data-p2-boot`，还读取运行时写入的画布采样：

- `data-p2-drawn`：非背景色采样点数量。`-1` 表示像素不可读，`0` 表示"启动了但黑屏"（判定失败）。
- `data-p2-ui-elements` / `data-p2-ui-summary`：当前帧由 Lua `ui.*` 绘制的元素数量与 id 摘要。

这样可以区分"Lua 在跑"和"真的画出了画面"，避免出现启动成功却没有任何画面的情况。
