# WebGL 平台插件

浏览器 WebGL 发布所需的平台侧文件都放在这个目录，`scripts/package.ps1 -Platform webgl`
会把它们复制/生成进发布包。

- `web-runtime.js`：浏览器运行时。负责从 `pack/` 读取工程与场景、用 Fengari 执行 Lua、
  把 `ui.*` 调用与场景静态 UI（`UiLayout` + `Image`/`Text`）画到 2D canvas，并接入
  键盘/鼠标输入与启动自检标记（`data-p2-boot` / `data-p2-drawn` / `data-p2-ui-*`）。
  **微信小游戏发布共用这一份文件**（见 `plugins/wechat-minigame/README.md`）。
- `index.html`：浏览器页面模板。`package.ps1` 负责替换 `__PUBLISH_NAME__` /
  `__PUBLISH_NAME_JSON__` / `__FONT_PATH__` 三个占位符（标题、脚本字符串与字体路径），
  因此字体路径与工程名不会再出现"生成时变量还没赋值"的空值。
- `test-webgl.bat`：**包内双击壳模板**。`package.ps1` 把它复制进每个 webgl 产物目录；
  它不写死游戏工程名或目录深度，而是从自身位置向上找到仓库根（含 `scripts\test-bundle.ps1` 的目录）。
  必须由发布脚本复制、不能手工丢进产物目录：`scripts/release-atomic.ps1` 的原子替换会把发布根下
  所有非 `.staging-*`/`.backup-*` 条目移入备份后删除，手工文件活不过一次发布。
- `pkg/fengari-web.js`：随包分发的 Lua VM（本地依赖，不请求 CDN，离线可运行）。
- `pkg/game_web.js`、`pkg/game_web_bg.wasm`：发布时由 `wasm-bindgen` 生成（不入库）。

## 限制

`web-runtime.js` 只读**明文** `pack/`：它不做 P2PK1 解包，也不做 gzip 解压（那是 Rust 运行时的能力）。
因此 `package.ps1 -Platform webgl|wechat-minigame` 会拒绝 `-Encrypt` / `-Compress`，
避免产出"打包成功但运行不起来"的发布包。

## 发布与验证

```powershell
# 发布
powershell -NoProfile -File scripts\package.ps1 -Platform webgl

# 一条命令试玩：自动发现本机唯一的合格产物 → 验证 → 起本地服务
powershell -NoProfile -File scripts\test-bundle.ps1

# 只做产物层验证（启动静态服务器 + 无头浏览器，断言 boot 与画面像素）；
# 省略 -BundleRoot 时同样自动发现
powershell -NoProfile -File scripts\verify-webgl-bundle.ps1
```

产物的定位规则收在 `scripts/bundle-discovery.ps1`：扫 `examples\<工程>\builds\webgl\<publish_name>`，
标记文件清单与 `package.ps1` 的产物校验数组**逐项一致**（含 `pkg/fengari-web.js`——正是它把随包
Lua VM 之前的历史产物挡在发现之外，否则一个「index.html 与 wasm 都在、却在浏览器里进不了 Lua」
的包会被选中）。本机有多个合格产物时不做猜测，要求显式 `-BundleRoot`。

发布包目录下还带一个 `test-webgl.bat`：双击即验证并起本地服务器，
然后浏览器打开 `http://127.0.0.1:8137/` 即可试玩。

## 画面自检

`verify-webgl-bundle.ps1` 除了断言 `data-p2-boot`，还读取运行时写入的画布采样：

- `data-p2-drawn`：非背景色采样点数量。`-1` 表示像素不可读，`0` 表示"启动了但黑屏"（判定失败）。
- `data-p2-ui-elements` / `data-p2-ui-summary`：当前帧由 Lua `ui.*` 绘制的元素数量与 id 摘要。
- `data-p2-world`：**世界状态**，每帧刷新：
  `scene=<场景声明实体数> map=<当前地图> spawn=<落点|none> alive=<存活实体数>
  player=<主角名>@x,y/<当前动画> drawn=<渲染器侧主角>x,y`。
  前四项由 Lua 侧给，`scene=`/`drawn=` 由渲染器侧补。

这样可以区分"Lua 在跑"和"真的画出了画面"，避免出现启动成功却没有任何画面的情况。

`data-p2-world` 再往前一步，区分的是"画面在动"和**世界真的在动**：一个只有背景与 HUD 的
画面同样能让 `drawn` 与 `ui_elements` 全绿。`alive` 少于 `scene` 说明实体表在场景切换中丢了，
`alive` 等于 `scene` 说明"运行期生成角色"（`entity.spawn` + `sprite.set` + `animation.attach`）
这条链路没生效，`player=` 与 `drawn=` 不一致说明**回写通道**（Lua → 渲染器）断了。
微信小游戏的无头 harness 用这三个量做质量门（`--drive-new-game --expect-runtime-spawns
--drive-move`），WebGL 与微信共用同一份运行时，因此两边读到的字段含义完全一致。
