# WebGL 平台插件

浏览器 WebGL 发布所需的平台侧文件都放在这个目录，`scripts/package.ps1 -Platform webgl`
会把它们复制/生成进发布包。

- `web-runtime.js`：浏览器运行时。负责从 `pack/` 读取工程与场景、经 **wasm Lua 桥**（`__RUSTLUA_LUA_BRIDGE`，执行层在 wasm 侧 mlua）执行 Lua、
  把 `ui.*` 调用与场景静态 UI（`UiLayout` + `Image`/`Text`）画到 2D canvas，并接入
  键盘/鼠标输入、音频后端（浏览器 `new Audio` / 微信 `wx.createInnerAudioContext`，
  验收读数写在 `data-rustlua-audio`）与启动自检标记（`data-rustlua-boot` / `data-rustlua-drawn` / `data-rustlua-ui-*`）。
  **微信小游戏发布共用这一份文件**（见 `plugins/wechat-minigame/README.md`）。
- `index.html`：浏览器页面模板。`package.ps1` 负责替换 `__PUBLISH_NAME__` /
  `__PUBLISH_NAME_JSON__` / `__FONT_PATH__` 三个占位符（标题、脚本字符串与字体路径）。
  `__FONT_PATH__` 当前替换为**空串**：webgl 不随包带 17 MB 完整字体，`web-runtime.js` 的
  `loadGameFont()` 检测到空路径直接跳过，中文由浏览器系统字体（`sans-serif`）渲染。
  若日后要字体保真，可改为打包期子集字体（微信侧已是 247 KB 子集方案）。
- `test-webgl.bat`：**包内双击壳模板**。`package.ps1` 把它复制进每个 webgl 产物目录；
  它不写死游戏工程名或目录深度，而是从自身位置向上找到仓库根（含 `scripts\test-bundle.ps1` 的目录）。
  必须由发布脚本复制、不能手工丢进产物目录：`scripts/release-atomic.ps1` 的原子替换会把发布根下
  所有非 `.staging-*`/`.backup-*` 条目移入备份后删除，手工文件活不过一次发布。
- `pkg/game_web.js`、`pkg/game_web_bg.wasm`：发布时由 `wasm-bindgen` 生成（不入库）。
  wasm 即 Lua 执行层（mlua 移植，Lua 5.4 + vendored），另导出 6 个 Lua 桥函数
  （`lua_boot` / `lua_do_string` / `lua_call_global` / `lua_register_js_callback` /
  `lua_missing_globals` / `lua_execute_main`）收编全部 Lua 交互。

## 限制

`web-runtime.js` 只读**明文** `pack/`：它不做 RustLua 解包，也不做 gzip 解压（那是 Rust 运行时的能力）。
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

# 完整玩法剧本验收：URL `?rustlua-drive=jhyt` 在真实浏览器里驱动 game_jhyt 跑完整玩法链路
# （标题确认 → 新游戏(manor) → 移动 → 背包/任务面板开合 → 山庄↔云泽村真实切图 → 回山庄），
# M1–M6 里程碑逐段落定，`data-rustlua-drive=done` 才算通过；已挂 verify.ps1 总门
# （webgl_playthrough_ok）。与 -Drive 互斥（后者是 `?rustlua-drive=auto` 的最小剧本）。
powershell -NoProfile -File scripts\verify-webgl-bundle.ps1 -DriveJhyt
```

产物的定位规则收在 `scripts/bundle-discovery.ps1`：扫 `examples\<工程>\builds\webgl`，
标记文件清单与 `package.ps1` 的产物校验数组**逐项一致**（含 `pkg/game_web_bg.wasm`——正是它把随包
wasm 核心之前的历史产物挡在发现之外，否则一个「index.html 与 wasm 都在、却在浏览器里进不了 Lua」
的包会被选中）。本机有多个合格产物时不做猜测，要求显式 `-BundleRoot`。

发布包目录下还带一个 `test-webgl.bat`：双击即验证并起本地服务器，
然后浏览器打开 `http://127.0.0.1:8137/` 即可试玩。

## 画面自检

`verify-webgl-bundle.ps1` 除了断言 `data-rustlua-boot`，还读取运行时写入的画布采样：

- `data-rustlua-drawn`：非背景色采样点数量。`-1` 表示像素不可读，`0` 表示"启动了但黑屏"（判定失败）。
- `data-rustlua-ui-elements` / `data-rustlua-ui-summary`：当前帧由 Lua `ui.*` 绘制的元素数量与 id 摘要。
- `data-rustlua-world`：**世界状态**，每帧刷新：
  `scene=<场景声明实体数> map=<当前地图> spawn=<落点|none> alive=<存活实体数>
  player=<主角名>@x,y/<当前动画> drawn=<渲染器侧主角>x,y`。
  前四项由 Lua 侧给，`scene=`/`drawn=` 由渲染器侧补。

这样可以区分"Lua 在跑"和"真的画出了画面"，避免出现启动成功却没有任何画面的情况。

`data-rustlua-world` 再往前一步，区分的是"画面在动"和**世界真的在动**：一个只有背景与 HUD 的
画面同样能让 `drawn` 与 `ui_elements` 全绿。`alive` 少于 `scene` 说明实体表在场景切换中丢了，
`alive` 等于 `scene` 说明"运行期生成角色"（`entity.spawn` + `sprite.set` + `animation.attach`）
这条链路没生效，`player=` 与 `drawn=` 不一致说明**回写通道**（Lua → 渲染器）断了。
微信小游戏的无头 harness 用这三个量做质量门（`--drive-new-game --expect-runtime-spawns
--drive-move`），WebGL 与微信共用同一份运行时，因此两边读到的字段含义完全一致。
