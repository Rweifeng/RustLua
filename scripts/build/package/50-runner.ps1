# -----------------------------------------------------------------------------
# 打包阶段：运行器与文档（品牌图、wasm-opt、no-modules 胶水补丁）
# 自 scripts/build/package.ps1 原样迁出（原 L873-L1465，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

    Write-Host "==> Copy runner and docs" -ForegroundColor Cyan
    # 发布包只含运行器；assetc 为开发工具，不随包分发（并清理历史残留）。
    Remove-Item -Path (Join-Path $distRoot "assetc.exe") -Force -ErrorAction SilentlyContinue

    if ($isWebPlatform) {
        # 品牌图（早启动 icon/splash）：`package-branding.ps1`，退出码/输出行契约与旧 .py 一致。
        $savedEapBrand = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $brandArgs = @{
                ProjectRoot = $gameRoot
                Out = (Join-Path $distRoot "branding")
            }
            if ($Platform -eq "wechat-minigame") { $brandArgs["SplashFormat"] = "jpg" }
            $brandOutput = & (Join-Path $packageScriptRoot "package-branding.ps1") @brandArgs 2>&1
            $brandExit = $LASTEXITCODE
        } finally { $ErrorActionPreference = $savedEapBrand }
        if ($brandExit -ne 0) { throw "Branding packaging failed: $($brandOutput -join ' ')" }
        Write-Host ($brandOutput -join ' ') -ForegroundColor DarkGray
        Remove-Item -Path (Join-Path $distRoot "game.exe") -Force -ErrorAction SilentlyContinue
        # pkg/：随包分发的 Lua VM（不请求 CDN，离线可运行）。webgl 的胶水/内核留在 pkg/；
        # 微信的胶水生成后挪到 launcher 根（wasm 内核在分包形态下再搬去 pack/）。
        $pkgRoot = Join-Path $distRoot "pkg"
        if (Test-Path $pkgRoot) { Remove-Item -LiteralPath $pkgRoot -Recurse -Force }
        New-Item -ItemType Directory -Force -Path $pkgRoot | Out-Null
        $wasmInput = Join-Path $wasmTargetRoot "game_web.wasm"
        if (-not (Test-Path -LiteralPath $wasmInput)) {
            throw "wasm artifact missing: $wasmInput"
        }
        # wasm-bindgen CLI 在这里**就地补齐**，而不是只依赖上面构建分支里的解析结果：
        # 那句解析写在 `if ($SkipBuild) { … } else { <构建> }` 的 else 里，于是
        # `-SkipBuild` + web 平台必然带着一个空的 $wasmBindgenCli 走到下面的 `& $wasmBindgenCli`，
        # 报"管道元素中的 '&' 后面的表达式生成无效的对象"（实测 2026-10-03）——
        # 而这条错误离真正的原因隔了几百行，看起来像产物坏了。跳过构建不代表可以跳过工具解析。
        if (-not $wasmBindgenCli) { $wasmBindgenCli = Resolve-WasmBindgenCli -Provision }
        # 解析发生在构建分支里，真正用它在几百行之后；中间发生的事（清目录、扫描器拿走新装的
        # exe）会让这条路径失效，而报错是 `& : ... is not recognized`，看起来像产物问题。
        # 用之前再确认一次：不在了就重新解析（此时会走临时副本那条路）。
        if ($wasmBindgenCli -and -not (Test-Path -LiteralPath $wasmBindgenCli -PathType Leaf)) {
            Write-Host "==> wasm-bindgen CLI 路径已失效（$wasmBindgenCli），重新解析" -ForegroundColor Yellow
            $wasmBindgenCli = $null
        }
        if (-not $wasmBindgenCli) { $wasmBindgenCli = Resolve-WasmBindgenCli -Provision }
        if (-not $wasmBindgenCli) { throw "wasm-bindgen CLI（版本需与 Cargo.lock 的 wasm-bindgen 一致）解析失败。" }
        if ($Platform -eq "webgl") {
            # 浏览器：`--target web` 的 **ES Module** 胶水，由 index.html 的 `<script type=module>` 加载。
            # --remove-name-section：release 构建为保留 target_features 段关闭了 strip，name
            # section（纯调试信息）可达 1.2 MB+，部署前移除（对运行无任何影响）。
            & $wasmBindgenCli $wasmInput --out-dir $pkgRoot --target web --no-typescript --remove-name-section
            if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen failed." }
            # wasm-bindgen 对 `--target web` 会额外生成一个 `index.html` 覆盖说明页（它靠
            # **文件系统里已存在 index.html** 来判断"输出目录是不是给浏览器用的"，因此平台
            # 目录里那个 index.html 会被浏览器分支的产物**整体覆盖**）。我们自己的那份
            # `index.html` 是发布模板（含 __PUBLISH_NAME__ 等多种占位符替换），必须由下面的
            # 复制步骤赢，所以这里把 wasm-bindgen 顺带生成的那份删掉，避免"谁最后写谁生效"
            # 这种靠顺序的隐式约定。
            Remove-Item -LiteralPath (Join-Path $pkgRoot "index.html") -Force -ErrorAction SilentlyContinue
            # 体积：webgl 与微信**共用** `Optimize-WasmBin`（同一条 binaryen 133 `-Oz`）。
            #
            # 这里此前**故意不跑** wasm-opt，理由是"binaryen 的任意 pass 都会把 try_table 转回
            # legacy try，造成新旧 EH 混合、Edge 拒绝编译（2026-09-29 实测）"。2026-10-11 复核
            # 后推翻该口径：当时用的是**其它版本**的 binaryen，而本仓已把版本锁死在 133
            # （`scripts/build/minify-toolchain.ps1` 只认 version_133）。
            # 实测（同一次 A/B，`+1.88.0` + `opt-level=z` + `lto=fat` + `codegen-units=1`）：
            #   wasm-bindgen 产物 2,932,495 B → `wasm-opt -Oz` 后 **2,708,453 B（省 224 KB / 7.6%）**，
            #   产物仍是纯 `try_table`（无 legacy `try`/`catch`），Node V8 `WebAssembly.compile` 通过、
            #   138 个导出不变。此后每次发布都会由下面的 `verify-wasm-host.mjs`（`--platform=webgl`）
            #   与 `scripts/verify/verify-webgl-eh.ps1`（真实浏览器 fetch + compile）各兜一次——
            #   这条链正是把上面四类"wasm 装不进来"的回归挡在发布期的守卫。
            # 微信侧独有的三步（剥 Global / 剥 Tag / noextern 降级）**不适用于 webgl**：
            # 那是 WXWebAssembly 的能力缺口（iOS 不支持 Global 导出、没有 WebAssembly.Tag
            # 构造器、不认 GC 终版 typed null），浏览器三样都支持，做了反而平白丢功能。
            Optimize-WasmBin (Join-Path $pkgRoot "game_web_bg.wasm")
        }
        else {
            # 微信小游戏：**必须用 `--target no-modules`**。
            #
            # 为什么不能沿用 `--target web`：那份胶水是浏览器 ESM（顶层 `export { … }`），
            # 小游戏运行时按 CommonJS 解析，加载即报
            #   WAGame.js:1 SyntaxError: MiniProgramError Unexpected token 'export'
            # `--target no-modules` 的产物是**纯脚本**（无 import/export，全部包在一个 IIFE 里），
            # 因此可以被 CommonJS 直接 `require`——`wasm-bridge.js` 就是那层包壳。
            # 这也让 `verify-wechat-minigame.ps1` 的"逐脚本按脚本模式解析"守卫保持有牙齿。
            & $wasmBindgenCli $wasmInput --out-dir $pkgRoot --target no-modules --no-typescript --remove-name-section
            if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen (no-modules) failed." }
            # no-modules 胶水开头会算 `script_src`（用于"不给参数时按 `<脚本名>_bg.wasm` 找模块"）：
            #   if (typeof document !== 'undefined' && document.currentScript !== null) {
            #       script_src = new URL(document.currentScript.src, location.href).toString();
            #   }
            # 小游戏里这段**必然抛错**：适配层那份 `document` shim 只保证运行时直接调用的成员
            # （`getElementById`），没有 `currentScript` 与 `location` —— 实测报
            #   Cannot read properties of undefined (reading 'src')
            # 而它出现在 IIFE 的**第一句**，于是整个 require 失败、wasm 装不进来。
            # 修法：构建期把这一句包进 try/catch。**为什么不改适配层的 shim**：给 shim 补
            # `location`/`currentScript` 是"为了绕过一个我们根本不走的代码路径"而伪造宿主能力，
            # 而这个分支只服务于"不加参数按脚本 URL 猜 wasm 位置"——bridge 永远**显式给参数**
            # （小游戏走 `WXWebAssembly.instantiate(代码包路径, imports)`，字节兜底走 `initSync(bytes)`），
            # 那条路径一次都不会用到。包一层 try 是诚实的做法：
            # 这条降级在浏览器里永远不会触发（那里 `document.currentScript` 是真的）。
            $gluePath = Join-Path $pkgRoot "game_web.js"
            $glueText = [System.IO.File]::ReadAllText($gluePath)
            $needle = "script_src = new URL(document.currentScript.src, location.href).toString();"
            if ($glueText.Contains($needle)) {
                $patched = $glueText.Replace($needle, "try { $needle } catch (error) { /* 小游戏宿主没有 currentScript/location，见 scripts/build/package.ps1 的说明 */ }")
                # 必须与源文件同编码写出（no BOM）：wasm-bindgen 的产物是 UTF-8 无 BOM。
                [System.IO.File]::WriteAllText($gluePath, $patched, (New-Object System.Text.UTF8Encoding($false)))
                Write-Host "==> Patched no-modules glue: script_src derivation guarded for the mini game host"
            }
            else {
                # 胶水换了写法就**响亮失败**，不要静默放过——那意味着"小游戏里 wasm 装不进来"
                # 这个坑又回来了，而且会以"启动照常、wasm 静默缺席"的形态出现。
                throw "no-modules glue shape changed: 'script_src = new URL(document.currentScript.src, ...)' not found in $gluePath. 小游戏宿主没有 currentScript/location，请重新确认这条降级要不要保留（见本文件注释）。"
            }
            # 第二个**必需**的胶水补丁：wasm-bindgen 的 call guard 读的 Global 导出，会被下面的
            # Global 剔除步骤一起拿掉。
            #
            # 每个导出函数的开头都被插了 `__wbg_call_guard()`，它无条件读
            #   __wbg_terminated_addr ??= wasm.__instance_terminated.value / 4;
            # 而 `strip-wasm-globals.mjs` 剔除 wasm 的**全部** Global 导出（iOS 的 WXWebAssembly
            # 不支持 Global 导出，docs/52 R12）——那个名字在 `wasm` 上根本不存在，于是
            # **第一次跨 wasm 调用**就抛
            #   TypeError: Cannot read properties of undefined (reading 'value')
            # 实测 2026-10-03：微信包 boot=failed，形态是"实例化成功（111 项导出）→ 宿主桥 error →
            # 一帧都不跑"，即 `hostApi=error:Cannot read properties of undefined (reading 'value')`。
            # 修法：global 缺席时守卫直接返回。它回答的是"实例已终止就别再调了"，读不到标记时
            # 本来就无从判断；同份胶水里的 abort hook 自己就包在 try/catch 内，说明 wasm-bindgen
            # 也把这类读取当可选路径。webgl 分支不需要补：那边不做 Global 剔除，导出是真的。
            $glueText = [System.IO.File]::ReadAllText($gluePath)
            $guardNeedle = "__wbg_terminated_addr ??= wasm.__instance_terminated.value / 4;"
            # 守卫必须写成 **上传器档位（ES2019）内的等价形式**，不能原样保留 `??=`：微信**上传器**的
            # 解析器比开发者工具低一档，实测（2026-10-03，稳定版 2.02.2608070）上传直接拒——
            #   Error: invalid file: game_web.js, 989:72 SyntaxError: Unexpected token ?
            # 而同一份包在开发者工具里跑得好好的（只有上传炸），所以这条既有下面的改写，也有
            # verify-wechat-minigame.ps1 的 ECMA 档位门兜住"胶水又升级出新语法"这一类。
            # 为什么 `=== undefined` 与 `??=` 在这里同义：左值是 `let __wbg_terminated_addr;`
            # （初值 undefined），右值是数值除法（永远不会是 null）。**不能**图省事写成
            # `||=` 或 `if (!x)`：地址算出来是 0 时那也是合法值，两种写法都会每次重算。
            $guardPlain = "if (__wbg_terminated_addr === undefined) __wbg_terminated_addr = wasm.__instance_terminated.value / 4;"
            if ($glueText.Contains($guardNeedle)) {
                $glueText = $glueText.Replace($guardNeedle, "if (!wasm.__instance_terminated) return; $guardPlain")
                [System.IO.File]::WriteAllText($gluePath, $glueText, (New-Object System.Text.UTF8Encoding($false)))
                Write-Host "==> Patched no-modules glue: wasm-bindgen call guard tolerates the stripped Global exports"
            }
            elseif (-not $glueText.Contains('__instance_terminated')) {
                # 形态变了：wasm-bindgen 0.2.129 在**纯 try_table**（rustc 1.88）产物上不再生成这段
                # call guard——胶水根本不读 `__instance_terminated`，所以没有可补的东西。
                # 依据（2026-10-08 实测）：`--target no-modules --no-typescript --remove-name-section`
                # 的产物里 call_guard / __instance_terminated / WebAssembly.Tag 全是 0 处，
                # 与**上一次成功打包**的发布产物（2026-09-30 的 launcher/game_web.js）形态一致。
                # 注意区分"没有读"与"换了写法读"：下面 else 分支才是后者，仍然响亮失败。
                Write-Host "==> Checked no-modules glue: no call guard present (glue reads no stripped Global), nothing to patch"
            }
            else {
                # 守卫换了写法同样响亮失败：静默放过等于微信包退回"装得进来、一调就死"。
                throw "no-modules glue shape changed: '__wbg_terminated_addr ??= wasm.__instance_terminated.value' not found in $gluePath，但胶水**确实引用了** __instance_terminated。这句读取依赖 Global 导出，而下面的步骤会把它们全部剔掉（iOS）——请重新确认守卫要不要补、怎么补（见本文件注释）。"
            }
            # 微信的 WXWebAssembly 没有 WebAssembly.Tag / WebAssembly.Exception 构造器，
            # wasm-bindgen 0.2 的胶水会在**求值期** `new WebAssembly.Tag(...)`，导致 require 即崩。
            # strip-wasm-tags.mjs 已经把 wasm 的 tag 导入剔掉、throw 指令替换成 unreachable，
            # 这里把胶水侧对应的 Tag 构造、Exception 包装、tag 引用一起简化掉：
            #   * 删掉两行 `const ... = new WebAssembly.Tag(...)`（求值期就 ReferenceError）
            #   * 删掉 __wbg_get_imports 里的 tag 字段（wasm 已经不导入它们了）
            #   * wrapError 改成直接透传（不再包 WebAssembly.Exception）
            #   * __wbg_handle_catch 简化（不再 instanceof WebAssembly.Exception 判断）
            #   * __wbg___wbindgen_throw 改成直接 throw Error（不再包 Exception）
            # 成对约束见 strip-wasm-tags.mjs 头部注释。
            $glueText = [System.IO.File]::ReadAllText($gluePath)
            # 1) 删掉两行 Tag 构造
            $glueText = $glueText -replace '(?m)^\s*const __wbindgen_jstag_polyfill = new WebAssembly\.Tag\(\{ parameters: \[''externref''\] \}\);\s*\r?\n?', ''
            $glueText = $glueText -replace '(?m)^\s*const __wbindgen_wrapped_jstag = new WebAssembly\.Tag\(\{ parameters: \[''externref''\] \}\);\s*\r?\n?', ''
            # 2) 删掉 __wbg_get_imports 里的 tag 字段
            # 0.2.129 的新胶水把 jstag 写成 `WebAssembly.JSTag`（不是 polyfill 变量），两种形态都要删；
            # 删完 wasm 侧 strip-wasm-tags 也已不再导入它，JSTag 不会进入 facade 检查。
            $glueText = $glueText -replace '(?m)^\s*__wbindgen_jstag:\s*(?:__wbindgen_jstag_polyfill|WebAssembly\.JSTag),\s*\r?\n?', ''
            $glueText = $glueText -replace '(?m)^\s*__wbindgen_wrapped_jstag: __wbindgen_wrapped_jstag,\s*\r?\n?', ''
            # 3) wrapError 改成直接透传
            $wrapErrorOld = 'function wrapError(f, args) {
        try {
            return f.apply(this, args);
        } catch (e) {
            if (e instanceof WebAssembly.Exception) throw e;
            throw new WebAssembly.Exception(__wbindgen_jstag_polyfill, [e], { traceStack: true });
        }
    }'
            $wrapErrorNew = 'function wrapError(f, args) {
        return f.apply(this, args);
    }'
            $glueText = $glueText.Replace($wrapErrorOld, $wrapErrorNew)
            # 4) __wbg_handle_catch 简化
            $handleCatchOld = 'function __wbg_handle_catch(e) {
        if (e instanceof WebAssembly.Exception && e.is(__wbindgen_wrapped_jstag)) {
            throw e.getArg(__wbindgen_wrapped_jstag, 0);
        }
        getInt32ArrayMemory0()[__wbg_terminated_addr] = 1;
        __wbg_call_abort_hook();
        throw e;
    }'
            $handleCatchNew = 'function __wbg_handle_catch(e) {
        getInt32ArrayMemory0()[__wbg_terminated_addr] = 1;
        __wbg_call_abort_hook();
        throw e;
    }'
            $glueText = $glueText.Replace($handleCatchOld, $handleCatchNew)
            # 5) __wbg___wbindgen_throw 改成直接 throw Error
            $throwOld = '__wbg___wbindgen_throw_41e9ee4f547fc59a: function() { return wrapError(function (arg0, arg1) {
                throw new WebAssembly.Exception(__wbindgen_wrapped_jstag, [new Error(getStringFromWasm0(arg0, arg1))]);
            }, arguments); },'
            $throwNew = '__wbg___wbindgen_throw_41e9ee4f547fc59a: function() { return wrapError(function (arg0, arg1) {
                throw new Error(getStringFromWasm0(arg0, arg1));
            }, arguments); },'
            $glueText = $glueText.Replace($throwOld, $throwNew)
            # 0.2.129 的新胶水把 throw 直接写在导入函数体里（没有 wrapError 包壳）：
            #   throw new WebAssembly.Exception(__wbindgen_wrapped_jstag, [new Error(getStringFromWasm0(...))]);
            # 上面的 Tag 构造已删，这个变量不再存在；直接抛普通 Error 即可。
            $throwDirectOld = 'throw new WebAssembly.Exception(__wbindgen_wrapped_jstag, [new Error(getStringFromWasm0(arg0, arg1))]);'
            $throwDirectNew = 'throw new Error(getStringFromWasm0(arg0, arg1));'
            $glueText = $glueText.Replace($throwDirectOld, $throwDirectNew)
            [System.IO.File]::WriteAllText($gluePath, $glueText, (New-Object System.Text.UTF8Encoding($false)))
            Write-Host "==> Patched no-modules glue: removed WebAssembly.Tag/Exception usage for WeChat WXWebAssembly compatibility"
            # **必须补上 CommonJS 导出**：`--target no-modules` 的产物是一句
            #   let wasm_bindgen = (function (exports) { … })({ __proto__: null });
            # 顶层 `let` 是**脚本作用域**绑定，不挂全局、也不是 `module.exports`，
            # 所以 `require('./game_web.js')` 拿到的是**空对象**——wasm-bridge 的
            # "胶水形态不对"就是这么来的（实测：`typeof require(...) === 'object'`，keys 为空）。
            # 补一行导出即可让 bridge 正常拿到那个函数（它同时带 `initSync` / `start` / `start_wechat`）。
            $glueText = [System.IO.File]::ReadAllText($gluePath)
            $glueText += "`n// 微信分支：补 CommonJS 导出（见 scripts/build/package.ps1）。no-modules 产物只声明了顶层 let，`n// 不挂全局也不是 module.exports，直接 require 会拿到空对象。`nmodule.exports = wasm_bindgen;`n"
            [System.IO.File]::WriteAllText($gluePath, $glueText, (New-Object System.Text.UTF8Encoding($false)))
            # 第三条**必需**的胶水依赖清单：胶水里出现的每一个 `WebAssembly.<名字>` 都必须由
            # `wasm-bridge.js` 的 facade 转发得到（宿主只给 `WXWebAssembly` 域名，标准全局缺席）。
            # 这条不是假设：2026-10-03 开发者工具实测 `[wasm=glue-missing/WXWebAssembly
            # detail=WebAssembly is not defined]`——胶水的两条 tag 导入在**求值期**就要
            # `new WebAssembly.Tag(...)`，而本机 Node 有真全局，所以本机全绿、工具里起不来。
            # 现在 bridge 转发 Tag/Exception/Memory/Table/Global，`instantiate`/`Instance` 由路径通道的
            # shim 提供，`Module`/`initSync` 只在字节通道里用（那条自己会先判宿主有没有）。
            # 这里把"胶水又新读了一个我们没转发的标准域名"变成发布期错误，而不是下一次真机/工具启动错误。
            $facadeCovered = @(
                "Tag", "Exception", "Memory", "Table", "Global",           # wasm-bridge.js 的 GLUE_EVAL_CTORS
                "instantiate", "Instance", "Module", "instantiateStreaming" # 通道逻辑自己覆盖的两条
            )
            $usedNames = @([regex]::Matches($glueText, "WebAssembly\.([A-Za-z_][\w$]*)") |
                ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
            $uncovered = @($usedNames | Where-Object { $facadeCovered -notcontains $_ })
            if ($uncovered.Count -gt 0) {
                throw ("no-modules 胶水读了 facade 未转发的标准 WebAssembly 域名：{0}（清单里已覆盖 {1}）。" -f `
                    ($uncovered -join ','), ($facadeCovered -join ',')) +
                    " 小游戏宿主只有 WXWebAssembly、没有标准 WebAssembly 全局，运行时会在 require 胶水时抛 ReferenceError。" +
                    " 要么在 plugins/wechat-minigame/wasm-bridge.js 的 GLUE_EVAL_CTORS 里补转发，要么把这条用法从胶水里去掉，并同步这里的清单。"
            }
            Write-Host ("==> Checked no-modules glue WebAssembly surface: {0}" -f (($usedNames | Sort-Object) -join ',')) -ForegroundColor DarkGray
            Optimize-WasmBin (Join-Path $pkgRoot "game_web_bg.wasm")
            # iOS 的 WXWebAssembly 不支持 Global 导出（docs/52 R12 尾段），带 Global 的模块
            # 会被整块拒绝。当前 wasm-bindgen 会把 __abort_handler/__instance_terminated
            # 塞进 Export 段——从 Export 段削掉、Global 定义本身留着，模块仍然自洽。
            # **但胶水确实读它们**（2026-10-03 实测纠正本注释的旧口径"一条也没落进 game_web.js"）：
            # `__wbg_call_guard` 读 `wasm.__instance_terminated.value`，abort hook 读
            # `wasm.__abort_handler.value`。所以剔除 Global 必须和上面那条守卫补丁成对出现，
            # 两者任一单独缺失都会让微信包"实例化成功、第一次调用就死"。
            # 只作用在微信分支；webgl 保留原样（浏览器都支持 Global）。
            $stripGlobals = Join-Path $packageScriptRoot "..\lib\wasm-lua\strip-wasm-globals.mjs"
            if (-not (Test-Path -LiteralPath $stripGlobals)) {
                throw "缺少 wasm Global 剔除脚本: $stripGlobals（发布不能在没有它的情况下继续）"
            }
            $nodeForStrip = Get-Command node -ErrorAction SilentlyContinue
            if (-not $nodeForStrip) { throw "Global 剔除需要 node" }
            $wasmForStrip = Join-Path $pkgRoot "game_web_bg.wasm"
            $savedEapStrip = $ErrorActionPreference
            $ErrorActionPreference = "Continue"
            try {
                $stripOutput = & $nodeForStrip.Source $stripGlobals $wasmForStrip $wasmForStrip 2>&1
                $stripExit = $LASTEXITCODE
            }
            finally {
                $ErrorActionPreference = $savedEapStrip
            }
            if ($stripExit -ne 0) {
                throw ("wasm Export 段 Global 剔除失败（{0}）：{1}" -f `
                    $wasmForStrip, (($stripOutput | ForEach-Object { "$_" }) -join ' '))
            }
            Write-Host (($stripOutput | Where-Object { $_ -match '^strip_wasm_globals_ok' } | Select-Object -First 1)) -ForegroundColor DarkGray
            # 微信的 WXWebAssembly 没有 WebAssembly.Tag 构造器，wasm-bindgen 0.2 的产物会导入
            # 两个 tag（__wbindgen_jstag / __wbindgen_wrapped_jstag），实测 polyfill 也被拒绝。
            # strip-wasm-tags.mjs 把 tag 导入从 Import 段剔掉，同时把 Code 段里引用被删 tag 的
            # throw/throw_ref 替换成 unreachable、try_table 降级成普通 block。
            # 成对约束：胶水侧的 Tag 构造、Exception 包装、tag 引用已经在上面的补丁里简化掉。
            $stripTags = Join-Path $packageScriptRoot "..\lib\wasm-lua\strip-wasm-tags.mjs"
            if (-not (Test-Path -LiteralPath $stripTags)) {
                throw "缺少 wasm Tag 剔除脚本: $stripTags（发布不能在没有它的情况下继续）"
            }
            $savedEapStripTags = $ErrorActionPreference
            $ErrorActionPreference = "Continue"
            try {
                $stripTagsOutput = & $nodeForStrip.Source $stripTags $wasmForStrip $wasmForStrip 2>&1
                $stripTagsExit = $LASTEXITCODE
            }
            finally {
                $ErrorActionPreference = $savedEapStripTags
            }
            if ($stripTagsExit -ne 0) {
                throw ("wasm Import 段 Tag 剔除失败（{0}）：{1}" -f `
                    $wasmForStrip, (($stripTagsOutput | ForEach-Object { "$_" }) -join ' '))
            }
            Write-Host (($stripTagsOutput | Where-Object { $_ -match '^strip_wasm_tags_ok' } | Select-Object -First 1)) -ForegroundColor DarkGray
            # 降级 `ref.null noextern`(D0 72) → `ref.null extern`(D0 6F)：真机 WXWebAssembly 不认
            # GC 终版 typed null（实测报 `Unknown heap type -14`，heap type -14 == noextern），
            # 而本机 V8 / 开发者工具都认——门全绿也照样真机白屏。放在 strip-tags 之后、宿主校验之前，
            # 是对**已剥 tag/global 的最终产物**做最小重编码（实测只改 6 字节、大小不变），细节与两个
            # 反例（--all-features 会写非法 import kind、自动检测因 target-features 已剥而退回 MVP）
            # 都在 Downgrade-WasmNoExtern 的注释里。
            Downgrade-WasmNoExtern $wasmForStrip
            # EH 真机探针：53 字节，零导入 + 本地 tag + legacy try/catch，走
            # WXWebAssembly.compile('pkg/eh-probe.wasm') 问设备引擎认不认 EH。
            # 由发布链路里同一个 binaryen-133 `wasm-as --enable-exception-handling` 汇编自：
            #   (module (tag $t (param i32))
            #     (func $f (result i32)
            #       (try (result i32) (do (throw $t (i32.const 42))) (catch $t (drop) (i32.const 7)))))
            #     (export "f" (func $f))
            $ehProbeHex = '0061736d0100000001090260017f006000017f030201010d03010000070501016600000a10010e00067f412a080007001a41070b0b'
            $ehProbePath = Join-Path $pkgRoot 'eh-probe.wasm'
            $ehProbeBytes = [byte[]] -split ($ehProbeHex -replace '..', '0x$& ')
            [System.IO.File]::WriteAllBytes($ehProbePath, $ehProbeBytes)
            Write-Host ("==> 已写入 EH 真机探针 {0}（{1} 字节，pkg/，零导入 + 本地 tag + legacy try/catch）" -f `
                $ehProbePath, $ehProbeBytes.Length) -ForegroundColor DarkGray
        }
        # wasm 产物必须**真的能被宿主的 WebAssembly 实现接受**（两个 web 平台都在发布期验一次）。
        # 这条守卫把四类回归挡在发布期，而不是等产物上线：
        #   * toolchain 换成 LLVM 19+ → 新旧 EH 指令混用（V8 报 `Invalid opcode 0x1f`）；
        #   * wasm-opt/binaryen 版本变化 → 改写 EH 编码；
        #   * 构建参数（如 RUSTFLAGS 的 build-eh 相关）漂移；
        #   * 微信侧 wasm 里还残留 Global 导出（iOS WXWebAssembly 硬拒绝，docs/52 R12；
        #     上面的 strip-wasm-globals.mjs 已把两条内部 global 从 Export 段剔除，这里再兜
        #     一次防漏更）。
        # 症状在运行时一律塌成同一句 `wasm lua bridge not loaded` + `boot=failed`、0 帧，
        # 现场看不出是"wasm 装不进来"还是"桥没接线"（实测 2026-09-30 就是这么漏出去的）。
        # 宿主选择：优先 `WXWebAssembly`（小游戏那份），没有则退到 `WebAssembly`
        # ——与 wasm-bridge.js 的 resolveWebAssembly 同一口径。本机 Node 的 V8 就是这里测的宿主，
        # 它**证明不了**微信真机 WXWebAssembly 的行为，但能证明"连标准 V8 都拒绝"这种硬坏。
        $wasmHostCheck = Join-Path $packageScriptRoot "..\lib\wasm-lua\verify-wasm-host.mjs"
        if (-not (Test-Path -LiteralPath $wasmHostCheck)) {
            throw "缺少 wasm 宿主校验脚本: $wasmHostCheck（发布不能在没有这条守卫的情况下继续）"
        }
        $nodeForWasm = Get-Command node -ErrorAction SilentlyContinue
        if (-not $nodeForWasm) { throw "wasm 宿主校验需要 node" }
        $savedEapWasm = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $wasmHostOutput = & $nodeForWasm.Source $wasmHostCheck (Join-Path $pkgRoot "game_web_bg.wasm") "--platform=$Platform" 2>&1
            $wasmHostExit = $LASTEXITCODE
        }
        finally {
            $ErrorActionPreference = $savedEapWasm
        }
        if ($wasmHostExit -ne 0) {
            throw ("wasm 产物无法被宿主的 WebAssembly 接受（{0}）：{1}" -f `
                (Join-Path $pkgRoot "game_web_bg.wasm"), (($wasmHostOutput | ForEach-Object { "$_" }) -join ' '))
        }
        Write-Host (($wasmHostOutput | Where-Object { $_ -match '^wasm_host_ok' } | Select-Object -First 1)) -ForegroundColor DarkGray
        # 分包形态把 **wasm 内核本体**搬进独立的 pack-wasm/ 根：微信主包只有 4 MB，而这份内核 2.5 MB（占主包
        # 一半以上），内容侧（Lua + 字体 + 首屏贴图）实测已到 2 MB 量级——内核留在主包就没有任何
        # 出路（逐目录归属见 docs/52「主包构成」）。让分包替它腾地方是代价最小的一条：其余杠杆
        # （feature 裁剪、wasm 再瘦身）动的都是引擎本体，改一处要重验一整条链。
        # **代价是启动次序变了**：胶水 `game_web.js` 放在 launcher 根（它被 `require('./game_web.js')`
        # 同步加载，进分包就等于换了一套 require 语义，本机验证不到真机行为），但 wasm 本体属分包 ⇒
        # 适配层必须等 `wx.loadSubpackage('pack')` 挂载完成才能实例化，标题画面因此排在分包之后。
        # 于是"下载进度"从 Lua 侧的东西变成 boot **之前**就得出现的 JS 侧画面，见 wechat-adapter.js
        # 的 `ensurePackForBoot` + `startPreBootUi`（进度条与失败横幅都画在 canvas 上，不依赖 wasm/Lua）。
        # 只动分包形态：full 下 pack/ 本身就在主包（搬了等于没搬，主包字节数不变）。
        # remote 下 pack/ 会被下面的瘦身裁成骨架——正是这一点让 `-WasmInSubpackage`（第 53 轮）有了
        # 明确语义：**搬内核之后才裁骨架**，于是主包只剩胶水/运行时/骨架/子集字体，重资源全在 CDN，
        # 分包里只有内核 + 骨架（谁都装得下）。不搬的话 remote 的主包必然超门（内核一个人就 > 4 MB）。
        # 落在 pack/ 根而不是 pack/pkg/：分包 root 下已有微信强制的 game.js 入口，同级放内核不与
        # 任何主包文件争抢（主包根下没有同名 .wasm），`subpackage-relative` 方言去前缀也读不到别的东西。
        if ($wasmInSubpackage) {
            $wasmForSubpackage = Join-Path $pkgRoot "game_web_bg.wasm"
            if (-not (Test-Path -LiteralPath $wasmForSubpackage)) {
                throw "内核入分包找不到待搬运的 wasm: $wasmForSubpackage"
            }
            New-Item -ItemType Directory -Force -Path $packWasmRoot | Out-Null
            $wasmSubpackagePath = Join-Path $packWasmRoot "game_web_bg.wasm"
            Move-Item -LiteralPath $wasmForSubpackage -Destination $wasmSubpackagePath -Force
            Write-Host ("==> 内核已搬进 $packWasmDirName/（{0:N0} KB 不再计入主包；exporter={1}），胶水 pkg/game_web.js 留在主包" -f `
                ((Get-Item -LiteralPath $wasmSubpackagePath).Length / 1KB), $AssetPack) -ForegroundColor DarkGray
        }
        # 胶水留在 pkg/：`require('./pkg/game_web.js')` 是同步加载，pkg/ 是主包目录。
        # subpackage 下内核已搬去 pack/，pkg/ 里只剩这一份胶水（刻意的单文件目录）。
        $glueAtPkg = Join-Path $pkgRoot "game_web.js"
        if (-not (Test-Path -LiteralPath $glueAtPkg)) {
            throw "缺少 wasm 胶水: $glueAtPkg"
        }
        if (Test-Path -LiteralPath (Join-Path $distRoot "game_web.js")) {
            throw "launcher 根不应再有 game_web.js：胶水固定在 pkg/（旧布局残留）"
        }
        # WP10 迁移（fengari → mlua-wasm）：Lua 执行层在 wasm 侧 mlua（LuaRuntime 桥），
        # pkg/fengari-web.js 已从发布链路移除，此处不再复制。
        #
        # **B8：旧世界 `plugins/webgl/web-runtime.js` 不再复制，源文件也已删除。** 启动编排在
        # 适配层/宿主（`lua_boot_with_reader` 预装 reader + `lua_execute_main` 执行 main.lua +
        # 宿主帧泵）；原来两条"解析它当源码"的静态门（输入绑定表 / 瓦片哈希）已随 B8 各自改判或
        # 退休（逐条理由见 `scripts/verify/verify.ps1` 里的退休注释与 `tasks/plan/M73-03`）。

        if ($Platform -eq "wechat-minigame") {
            # 微信小游戏入口：game.js（入口）+ wechat-adapter.js（wx 宿主适配）+ startup-branding.js
            # （适配层 `require('./startup-branding.js')`，见 wechat-adapter.js 的 showStartupBranding）
            # + 两个配置。平台文件都在 plugins/wechat-minigame/，发布脚本只负责复制、改名与校验。
            #
            # 这份清单**必须是穷举**（缺一个就 throw），而且**必须跟着适配层的 require 走** ——
            # 2026-10-16 实测踩到：适配层新增了 `require('./startup-branding.js')` 而清单没跟上，
            # 产物因此缺文件，**开发者工具里直接起不来**（表现是探针收不到任何上报）。
            $wechatPlugin = Join-Path $workspaceRoot "plugins\wechat-minigame"
            foreach ($pluginFile in @("game.js", "game.json", "project.config.json", "wechat-adapter.js", "startup-branding.js", "wasm-bridge.js", "bundle-roots.js")) {
                $pluginSource = Join-Path $wechatPlugin $pluginFile
                if (-not (Test-Path -LiteralPath $pluginSource)) {
                    throw "WeChat platform plugin is missing: $pluginSource"
                }
                Copy-Item -LiteralPath $pluginSource -Destination (Join-Path $distRoot $pluginFile) -Force
                # 发布产物 minify（源码保持 plugins/wechat-minigame/*.js 可读）。
                # `startup-branding.js` 与适配层同样处理：它也是运行期被 require 的产物文件，
                # 同样要过上传的 ECMA 档（`check-upload-ecma.mjs` 逐文件检）。
                if ($pluginFile -eq "wechat-adapter.js" -or $pluginFile -eq "startup-branding.js") { Minify-JsFile (Join-Path $distRoot $pluginFile) }
            }
            # game.json 的 subpackages **按形态注入，不写死在模板里**：
            # 模板带上分包声明会让另外两种形态（full / remote）被微信直接拒开——声明的 root
            # 必须真的存在且含入口，否则开发者工具报「未找到 ["subpackages"][0]["root"] 对应的
            # /pack/game.js 文件」并拒绝打开工程（实测踩过，也是历史上取消分包的直接原因）。
            $gameConfigPath = Join-Path $distRoot "game.json"
            $gameConfig = Get-Content -LiteralPath $gameConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
            # 两种拼写都要清：小游戏用 subpackages，小程序用 subPackages，宿主对拼写敏感。
            $gameConfig.PSObject.Properties.Remove("subpackages")
            $gameConfig.PSObject.Properties.Remove("subPackages")
            if ($wasmInSubpackage) {
                # pack/：非启动资源（容器 + 骨架）；pack-wasm/：只有内核。拆成两个 root 后
                # boot 只等 pack-wasm，pack 由适配层在 boot 后后台挂载。
                $subpackageEntries = @()
                $packEntry = New-Object PSObject
                $packEntry | Add-Member -NotePropertyName name -NotePropertyValue $packDirName
                $packEntry | Add-Member -NotePropertyName root -NotePropertyValue $packDirName
                $subpackageEntries += $packEntry
                $wasmEntry = New-Object PSObject
                $wasmEntry | Add-Member -NotePropertyName name -NotePropertyValue $packWasmDirName
                $wasmEntry | Add-Member -NotePropertyName root -NotePropertyValue $packWasmDirName
                $subpackageEntries += $wasmEntry
                $gameConfig | Add-Member -NotePropertyName subpackages -NotePropertyValue $subpackageEntries -Force
            }
            $gameConfigJson = $gameConfig | ConvertTo-Json -Depth 10
            [IO.File]::WriteAllText($gameConfigPath, $gameConfigJson, (New-Object Text.UTF8Encoding($false)))
            # 分包入口 pack/game.js：微信要求 subpackages[i].root 下有 game.js，挂载后会**执行**它
            # （不只是存在性占位）。它由发布脚本生成而不是 assetc 打进去的：assetc 只认工程目录，
            # 而这是发布形态的产物。包内每个 .js 都会被下面三处按 CommonJS 解析，必须合法。
            $subpackageEntryPath = Join-Path $packRoot "game.js"
            if ($wasmInSubpackage) {
                $subpackageEntryBody = @(
                    "'use strict';",
                    "// 微信小游戏的**分包入口**：subpackages[i].root === '$packDirName'，挂载后由宿主执行。",
                    "// 这个文件不加载任何资源——分包里的资源由主包侧 launcher/wechat-adapter.js 按",
                    $(if ($isSubpackagePack) {
                            "// pack.json 静态路由后读取。它存在的意义只有一个：微信强制要求 root 目录下有"
                        }
                        else {
                            "// pack.json 的空清单路由后读取（本包是 remote 形态：重资源在 CDN，分包里只有内核与骨架）。"
                        }),
                    "// game.js，缺了开发者工具连工程都打不开（报错形如「未找到 subpackages[0].root",
                    "// 对应的 /$packDirName/game.js 文件」）。",
                    "module.exports = { subpackage: '$packDirName', mount: 'resources-only' };",
                    ""
                ) -join "`r`n"
                [IO.File]::WriteAllText($subpackageEntryPath, $subpackageEntryBody, (New-Object Text.UTF8Encoding($false)))
                # pack-wasm 也是微信分包根，必须有入口 game.js（内容无关，挂载时执行）。
                $wasmEntryPath = Join-Path $packWasmRoot "game.js"
                $wasmEntryBody = (Get-Content -LiteralPath $subpackageEntryPath -Raw -Encoding UTF8).
                    Replace("'$packDirName'", "'$packWasmDirName'")
                [IO.File]::WriteAllText($wasmEntryPath, $wasmEntryBody, (New-Object Text.UTF8Encoding($false)))
            }
            elseif (Test-Path -LiteralPath $subpackageEntryPath) {
                # 非分包形态下 pack/game.js 是个**危险的假货**：它既不是主包入口，又会被
                # `subpackage-relative` 方言读成主包入口 `game.js`（见适配层的方言注释）。
                Remove-Item -LiteralPath $subpackageEntryPath -Force
                Write-Warning "==> 已移除 pack/game.js（-AssetPack $AssetPack 不是分包形态，留着会盖住主包入口）"
            }
            # 主包首屏目录（pack/）只属于分包形态：其它形态下留着会让开发者工具按目录结构误判。
            $mainPackRoot = Join-Path $distRoot $mainPackDirName
            $mainPackManifest = Join-Path $distRoot "pack.json"
            if (-not $isSubpackagePack) {
                foreach ($stalePath in @($mainPackRoot, $mainPackManifest)) {
                    if (Test-Path -LiteralPath $stalePath) {
                        Remove-Item -LiteralPath $stalePath -Recurse -Force
                        Write-Warning "==> 已移除残留的 $stalePath（-AssetPack $AssetPack 不是分包形态）"
                    }
                }
            }
            # remote + -WasmInSubpackage：**必须**发一份首屏清单，适配层就是靠它判断"内核在分包"
            # （`wechat-adapter.js` 的 `isWasmInPack()` ← `pack.json` 的 `layout.subpackage_root`）。
            # 缺了它，适配层会去主包读 `pkg/game_web_bg.wasm`（内核已经不在那儿了）并启动失败。
            # entries 为空是**对的**：remote 形态的首屏资源在 CDN，包内没有 mainpack/ 路由可走；
            # 于是所有 `pack/` 读取都归到分包侧（挂载后本地读，读不到再走远端回落）。
            if ($wasmInSubpackage -and -not $isSubpackagePack) {
                $emptyFirstScreen = [pscustomobject]@{
                    entries = @()
                    layout  = [pscustomobject]@{ subpackage_root = $packDirName; wasm_root = $packWasmDirName }
                }
                [IO.File]::WriteAllText($mainPackManifest, ($emptyFirstScreen | ConvertTo-Json -Depth 6),
                    (New-Object Text.UTF8Encoding($false)))
                Write-Host "==> remote + 内核入分包：已写 pack.json（layout.subpackage_root=$packDirName、entries 为空）" -ForegroundColor DarkGray
            }
            # project.config.json 的工程名跟随 publish_name，方便开发者工具里区分工程。
            $projectConfigPath = Join-Path $distRoot "project.config.json"
            # PS 5.1 的 Get-Content 默认按 ANSI 解码：必须显式 UTF8，否则中文工程名会被写坏。
            $projectConfig = Get-Content -LiteralPath $projectConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
            $projectConfig.projectname = $PublishName
            # appid 来自编辑器「发布项目」面板（随工程记在 project.toml 的 wechat_appid）。
            # 不传就保留模板的 touristappid —— 那是微信的"游客"占位，能本地预览但不能上传，
            # 所以填了 appid 时这里是唯一能让产物直接可上传的地方（历史上要手工改这个文件）。
            if ($WechatAppId -notmatch '^(wx[0-9a-fA-F]{16}|touristappid)$') {
                Write-Warning ("==> 微信 appid 形状不像正式 appid（应为 wx + 16 位十六进制）: '{0}' —— 仍按原样写入，上传前请在微信开发者工具里核对" -f $WechatAppId)
            }
            $projectConfig.appid = $WechatAppId
            Write-Host ("==> project.config.json appid = {0}" -f $WechatAppId) -ForegroundColor DarkGray
            $projectConfigJson = $projectConfig | ConvertTo-Json -Depth 10
            # 开发者工具的 JSON 解析对 BOM 敏感：写无 BOM 的 UTF-8。
            [IO.File]::WriteAllText($projectConfigPath, $projectConfigJson, (New-Object Text.UTF8Encoding($false)))
            # 资源远端回落配置：只在显式给了 -RemotePackBaseUrl 时落盘；没给就**必须清掉**，
            # 否则上一次发布的远端配置会悄悄留在包里（行为不一致且难以排查）。
            $remoteConfigPath = Join-Path $distRoot "remote-assets.json"
            if ([string]::IsNullOrWhiteSpace($RemotePackBaseUrl)) {
                if (Test-Path -LiteralPath $remoteConfigPath) { Remove-Item -LiteralPath $remoteConfigPath -Force }
            } else {
                if ($RemotePackBaseUrl -notmatch '^https?://') {
                    throw "-RemotePackBaseUrl 必须以 http:// 或 https:// 开头: $RemotePackBaseUrl"
                }
                # 归一化：基址指向 pack/ 的**内容**目录（适配层会把 pack/ 前缀去掉再拼接）。
                $baseUrl = $RemotePackBaseUrl.TrimEnd('/')
                if ($baseUrl -notmatch '/pack$') { $baseUrl = "$baseUrl/pack" }
                $baseUrl = "$baseUrl/"
                $remoteConfig = [ordered]@{ packBaseUrl = $baseUrl; verifyChecksum = $true; timeoutMs = 15000 }
                # ASCII 内容 + 无 BOM：JSON.parse 与开发者工具都不会踩到编码坑。
                [IO.File]::WriteAllText($remoteConfigPath, ($remoteConfig | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding($false)))
                Write-Host ("==> 远端资源回落: {0}（包内仍是首选，读不到才下载）" -f $baseUrl) -ForegroundColor DarkGray
            }
            # 小游戏包不复制完整字体：assetc 已把用到的字符子集化进 pack/；
            # 没有 font/font_fallback 声明的字体二进制由上方 --drop-undeclared-fonts 在包侧物理删除。
            $packedFont = Join-Path $packRoot ("assets\fonts\" + $fontName)
            if (-not (Test-Path -LiteralPath $packedFont)) {
                throw "WeChat package requires the subset font inside pack/: $packedFont"
            }
            if ((Get-Item -LiteralPath $packedFont).Length -le 0) {
                throw "WeChat subset font is empty: $packedFont"
            }
            Write-Host ("==> WeChat Mini Game entry staged (subset font {0:N0} KB; full font not shipped)" -f ((Get-Item -LiteralPath $packedFont).Length / 1KB)) -ForegroundColor DarkGray
        }
        else {
            # 浏览器页面来自平台插件模板：占位符替换 publish_name 与真实字体路径。
            $indexTemplate = Join-Path $workspaceRoot "plugins\webgl\index.html"
            if (-not (Test-Path -LiteralPath $indexTemplate)) {
                throw "WebGL platform plugin is missing: plugins\webgl\index.html"
            }
            $indexHtml = Get-Content -LiteralPath $indexTemplate -Raw -Encoding UTF8
            $indexHtml = $indexHtml.Replace('__PUBLISH_NAME_JSON__', $publishNameJson)
            $indexHtml = $indexHtml.Replace('__PUBLISH_NAME__', $PublishName)
            # WP10 之后 webgl **不随包复制 17 MB 完整字体**：`__FONT_PATH__` 替换为空串。
            # 字体由引擎侧按 project.toml 的 font 键经资源管线读取（B8 之前那句"运行时
            # loadGameFont 检测空路径跳过"已随旧世界删除而失效）。若后续要字体保真，
            # 可改为打包期子集字体（微信侧已是 247 KB 子集方案）。
            $indexHtml = $indexHtml.Replace('__FONT_PATH__', '')
            [IO.File]::WriteAllText((Join-Path $distRoot "index.html"), $indexHtml, (New-Object Text.UTF8Encoding($false)))
            # 包内双击壳：与 index.html 一样来自平台插件模板，必须由发布脚本生成——
            # Complete-ReleaseSwap 会把发布根下所有非 .staging-*/.backup-* 条目移入备份后删除，
            # 手工丢进产物目录的脚本每次发布会消失。模板本身与游戏工程名、目录深度无关
            # （它从自身位置向上找仓库根），因此任何工程发布 webgl 包都自动带上它。
            $testBatTemplate = Join-Path $workspaceRoot "plugins\webgl\test-webgl.bat"
            if (-not (Test-Path -LiteralPath $testBatTemplate)) {
                throw "WebGL platform plugin is missing: plugins\webgl\test-webgl.bat"
            }
            Copy-Item -LiteralPath $testBatTemplate -Destination (Join-Path $distRoot "test-webgl.bat") -Force
        }
    }
    else {
        $packagedGame = Join-Path $distRoot "game.exe"
        Copy-Item -LiteralPath (Join-Path $nativeTargetRoot "game.exe") `
            -Destination $packagedGame -Force
        # Project file icon is a packaging-only override. Never patch the shared
        # Cargo artifact: other projects and the engine tools keep their default.
        # Run this even with -SkipBuild, before self-checks and the atomic swap.
        # `package-icon.ps1` 自己处理"工程没声明 icon_image"这一档（原样保留编译期图标）。
        $savedEapIcon = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $iconOutput = & (Join-Path $packageScriptRoot "package-icon.ps1") `
                -ProjectRoot $gameRoot -Exe $packagedGame 2>&1
            $iconExit = $LASTEXITCODE
        }
        finally {
            $ErrorActionPreference = $savedEapIcon
        }
        if ($iconExit -ne 0) {
            throw ("PC icon packaging failed (exit {0}): {1}" -f $iconExit, (($iconOutput | ForEach-Object { "$_" }) -join ' '))
        }
        Write-Host (($iconOutput | ForEach-Object { "$_" }) -join ' ') -ForegroundColor DarkGray
    }

    # 工程级设置（project.toml 的 [input]/[audio] 段：手柄改键与死区/音量）随包分发：发布包以 pack 目录作为
    # 工程根，运行器按 `<root>/project.toml` 读取，否则打包版会回落到键盘默认绑定，
    # 出现「编辑器里改好的手柄绑定在发布包里失效」。
