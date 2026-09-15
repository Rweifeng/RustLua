[CmdletBinding()]
param(
    # Game project root to package.
    [string]$ProjectRoot = "",
    # Custom release name. Defaults to [project].name in project.toml.
    [string]$PublishName = "",
    # Target publish platform: pc, webgl or wechat-minigame.
    [ValidateSet("pc", "webgl", "wechat-minigame")]
    [string]$Platform = "pc",
    # Run the directory-isolation self test (copy to a temp dir with spaces,
    # launch the packaged game --pack without any source/target/.cache).
    [switch]$SelfTest,
    # Compress supported assets in the release pack (present = on).
    [switch]$Compress,
    # Encrypt Lua/scene resources in the release pack (present = on).
    [switch]$Encrypt,
    # M20-03: reuse existing release binaries (skip cargo builds) so repeated
    # publishes are fast and the staging/swap/rollback flow can be verified.
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$workspaceRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $workspaceRoot "engine"
if ([string]::IsNullOrWhiteSpace($ProjectRoot)) {
    # examples 閲嶇粍鍚庢牱鏉垮伐绋嬪湪瀛愮洰褰曢噷锛堟牴鐩綍宸叉棤 project.toml锛夈€?
    $ProjectRoot = Join-Path $workspaceRoot "examples\game_jhyt"
}
$gameRoot = (Resolve-Path -LiteralPath $ProjectRoot).Path
$projectToml = Join-Path $gameRoot "project.toml"
if (-not (Test-Path $projectToml)) { throw "Game project.toml was not found: $projectToml" }
if ([string]::IsNullOrWhiteSpace($PublishName)) {
    $nameLine = Get-Content -LiteralPath $projectToml | Where-Object { $_ -match '^\s*name\s*=\s*"(.+)"\s*$' } | Select-Object -First 1
    if ($nameLine -and $nameLine -match '^\s*name\s*=\s*"(.+)"\s*$') { $PublishName = $Matches[1] }
    if ([string]::IsNullOrWhiteSpace($PublishName)) { $PublishName = Split-Path -Leaf $gameRoot }
}
$invalidNameChars = [IO.Path]::GetInvalidFileNameChars()
foreach ($ch in $invalidNameChars) { $PublishName = $PublishName.Replace([string]$ch, "_") }
$PublishName = $PublishName.Trim()
if ([string]::IsNullOrWhiteSpace($PublishName)) { $PublishName = "game" }
# M31/P3锛氱帺瀹舵暟鎹洰褰曪紙瀛樻。 + 鐜╁璁剧疆锛夈€備笌寮曟搸 `core::user_dir` 鐨勮鍒欎竴鑷达細
# Windows 涓?`%APPDATA%\<publish_name>`锛屽叾瀹冨钩鍙板彇鍚勮嚜鐨勬暟鎹洰褰曪紱`RUSTLUA_USER_DIR` 鍙鐩栥€?
# 杩欓噷鍙敤浜?*鏂囨。涓庤嚜妫€鏂█**锛堝彂甯冨寘鐩綍閲屼笉鍐欎换浣曠帺瀹舵暟鎹級銆?
$PlayerDataDir = if ($env:RUSTLUA_USER_DIR) {
    $env:RUSTLUA_USER_DIR
} elseif ($env:APPDATA) {
    Join-Path $env:APPDATA $PublishName
} else {
    Join-Path $env:USERPROFILE (".local\share\" + $PublishName)
}
$distRoot = Join-Path $gameRoot ("builds\" + $Platform + "\" + $PublishName)
$packRoot = Join-Path $distRoot "pack"
# M20-03锛氬師瀛愬彂甯冦€傛墍鏈夊唴瀹瑰厛鍐欒繘 staging锛屽叏閮ㄦ垚鍔熷悗鍐嶅師瀛愭浛鎹㈡渶缁堢洰褰曪紱
# 澶辫触鏃舵渶缁堢洰褰曚繚鎸佹棫鐗堟湰涓嶅彉锛堜笉浼氬嚭鐜板崐鏇存柊鐘舵€侊級銆?
. (Join-Path $PSScriptRoot "release-atomic.ps1")

# M31/P3锛氬彂甯冪増杩愯鍣ㄦ槸 Windows GUI 瀛愮郴缁熺▼搴忥紝`& exe args` 涓嶇瓑寰呬篃涓嶆崟鑾疯緭鍑?
# 锛坄$LASTEXITCODE` 浼氫繚鐣欏墠涓€鏉″懡浠ょ殑鍊?鈬?鑷绌鸿窇锛夈€傜粺涓€鐢?Start-Process -Wait +
# 閲嶅畾鍚戝彇鍥炵湡瀹為€€鍑虹爜涓庤緭鍑恒€傝繑鍥?`[pscustomobject]@{ ExitCode; Output }`銆?
function Invoke-PackagedCheck {
    param(
        [Parameter(Mandatory = $true)][string]$Runner,
        [Parameter(Mandatory = $true)][string]$PackRoot
    )
    $stdoutFile = Join-Path $env:TEMP ("p2-pack-check-" + [guid]::NewGuid().ToString("N") + ".out")
    $stderrFile = $stdoutFile + ".err"
    try {
        # `Start-Process -ArgumentList` 浼氭妸鏁扮粍鎸夌┖鏍兼嫾鎴愬懡浠よ涓?*涓嶅姞寮曞彿**锛屽洜姝ゅ惈绌烘牸鐨?
        # 璺緞蹇呴』鑷繁甯﹀紩鍙凤紙鑷祴灏辨槸鏁呮剰鐢?"p2 release test 鈥? 杩欑甯︾┖鏍肩殑璺緞锛夈€?
        $quotedPack = '"' + $PackRoot + '"'
        $process = Start-Process -FilePath $Runner `
            -ArgumentList @("--pack", $quotedPack, "--check") `
            -PassThru -Wait -NoNewWindow `
            -RedirectStandardOutput $stdoutFile -RedirectStandardError $stderrFile
        $lines = @()
        if (Test-Path -LiteralPath $stdoutFile) { $lines += Get-Content -LiteralPath $stdoutFile }
        if (Test-Path -LiteralPath $stderrFile) { $lines += Get-Content -LiteralPath $stderrFile }
        return [pscustomobject]@{ ExitCode = $process.ExitCode; Output = $lines }
    }
    finally {
        Remove-Item -LiteralPath $stdoutFile -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $stderrFile -Force -ErrorAction SilentlyContinue
    }
}
$finalRoot = $distRoot
$staleRemoved = Clear-StaleReleaseStaging -FinalRoot $finalRoot -MaxAgeHours 24
foreach ($stale in $staleRemoved) {
    Write-Host "==> Removed stale release staging: $stale" -ForegroundColor DarkGray
}
$stageRoot = New-ReleaseStaging -FinalRoot $finalRoot
$distRoot = $stageRoot
$packRoot = Join-Path $stageRoot "pack"
$script:swapAttempted = $false
Write-Host "==> Staging release at $stageRoot (atomic swap on success)" -ForegroundColor Cyan
$timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Install Rust stable and ensure cargo is on PATH."
}

New-Item -ItemType Directory -Force -Path $distRoot | Out-Null
Push-Location $engineRoot
try {
    Write-Host "==> Release build ($Platform)" -ForegroundColor Cyan
    if ($SkipBuild) {
        # M20-03: repeated publish without rebuilding; require existing artifacts.
        $required = @(
            (Join-Path $engineRoot "target\release\assetc.exe"),
            (Join-Path $engineRoot "target\release\game.exe")
        )
        foreach ($artifact in $required) {
            if (-not (Test-Path -LiteralPath $artifact)) {
                throw "SkipBuild requires existing release artifact: $artifact"
            }
        }
        Write-Host "==> Skipping cargo build (-SkipBuild): reusing target\release artifacts" -ForegroundColor DarkGray
    }
    elseif ($Platform -eq "wechat-minigame") {`r`n        cargo build -p assetc --release`r`n        if ($LASTEXITCODE -ne 0) { throw "assetc release build failed." }`r`n        cargo build -p game_web --release --target wasm32-unknown-unknown`r`n        if ($LASTEXITCODE -ne 0) { throw "WeChat WASM build failed." }`r`n    } elseif ($Platform -eq "webgl") {
        rustup target add wasm32-unknown-unknown
        if ($LASTEXITCODE -ne 0) { throw "Install wasm32 target failed." }
        cargo build -p assetc --release
        if ($LASTEXITCODE -ne 0) { throw "assetc release build failed." }
        cargo build -p game_web --release --target wasm32-unknown-unknown
        if ($LASTEXITCODE -ne 0) { throw "WebGL WASM build failed." }
        if (-not (Get-Command wasm-bindgen -ErrorAction SilentlyContinue)) {
            Write-Host "==> Install wasm-bindgen-cli" -ForegroundColor Cyan
            cargo install wasm-bindgen-cli --version 0.2.127
            if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen-cli install failed." }
        }
    } else {
        cargo build --workspace --release
        if ($LASTEXITCODE -ne 0) { throw "Release build failed." }
    }

    Write-Host "==> assetc package (platform=$Platform, data resource pack, compress=$Compress encrypt=$Encrypt)" -ForegroundColor Cyan
    $assetc = Join-Path $engineRoot "target\release\assetc.exe"
    if (Test-Path $packRoot) { Remove-Item -LiteralPath $packRoot -Recurse -Force }
    $assetcArgs = @("package", $gameRoot, "-o", $packRoot)
    if ($Compress) { $assetcArgs += "--compress" }
    if ($Encrypt) { $assetcArgs += "--encrypt" }
    & $assetc @assetcArgs
    if ($LASTEXITCODE -ne 0) { throw "assetc package failed." }

    Write-Host "==> Copy runner and docs" -ForegroundColor Cyan
    # 鍙戝竷鍖呭彧鍚繍琛屽櫒锛沘ssetc 涓哄紑鍙戝伐鍏凤紝涓嶉殢鍖呭垎鍙戯紙骞舵竻鐞嗗巻鍙叉畫鐣欙級銆?
    Remove-Item -Path (Join-Path $distRoot "assetc.exe") -Force -ErrorAction SilentlyContinue
    if ($Platform -eq "wechat-minigame") {
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\wechat-adapter.js") (Join-Path $distRoot "wechat-adapter.js") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\game.js") (Join-Path $distRoot "game.js") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\game.json") (Join-Path $distRoot "game.json") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\project.config.json") (Join-Path $distRoot "project.config.json") -Force
        foreach ($f in @("game.js","game.json","project.config.json","wechat-adapter.js","pack\manifest.txt","pack\project.toml")) { if (-not (Test-Path (Join-Path $distRoot $f))) { throw "WeChat package is missing: $f" } }
        Write-Host "wechat_minigame_package_ok" -ForegroundColor Green
    } elseif ($Platform -eq "webgl") {
        Remove-Item -Path (Join-Path $distRoot "game.exe") -Force -ErrorAction SilentlyContinue
        $pkgRoot = Join-Path $distRoot "pkg"
        if (Test-Path $pkgRoot) { Remove-Item -LiteralPath $pkgRoot -Recurse -Force }
        New-Item -ItemType Directory -Force -Path $pkgRoot | Out-Null
        $wasmInput = Join-Path $engineRoot "target\wasm32-unknown-unknown\release\game_web.wasm"
        wasm-bindgen $wasmInput --out-dir $pkgRoot --target web --no-typescript
        if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen failed." }
        $webRuntimeTemplate = Join-Path $workspaceRoot "plugins\webgl\web-runtime.js"
        if (Test-Path $webRuntimeTemplate) { Copy-Item $webRuntimeTemplate (Join-Path $distRoot "web-runtime.js") -Force }
        # Fengari 随包分发（本地依赖）：发布包不再请求 CDN，离线可运行，也不会触发
        # 浏览器的第三方存储跟踪防护警告。
        $fengariSource = Join-Path $workspaceRoot "plugins\webgl\pkg\fengari-web.js"
        if (-not (Test-Path -LiteralPath $fengariSource)) {
            throw "WebGL package requires the vendored Lua runtime: plugins\webgl\pkg\fengari-web.js"
        }
        if ((Get-Item -LiteralPath $fengariSource).Length -le 0) {
            throw "Vendored Lua runtime is empty: plugins\webgl\pkg\fengari-web.js"
        }
        Copy-Item $fengariSource (Join-Path $pkgRoot "fengari-web.js") -Force
        @"
<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>$PublishName</title>
  <style>
    html, body { margin: 0; width: 100%; height: 100%; background: #151923; overflow: hidden; }
    canvas { display: block; width: 100vw; height: 100vh; }
    #status { position: fixed; left: 16px; top: 12px; color: #d0d7e2; font: 14px sans-serif; }
  </style>
</head>
<body data-p2-boot="pending">
  <canvas id="p2-canvas"></canvas>
  <div id="status">Loading $PublishName...</div>
  <script>window.__P2_FONT = './fonts/$fontName';</script>
  <script src="./pkg/fengari-web.js"></script>
  <script src="./web-runtime.js"></script>
  <script type="module">
    import init, { start } from './pkg/game_web.js';
    const publishName = $publishNameJson;
    const status = document.getElementById('status');
    const markBoot = (state, detail) => {
      document.body.setAttribute('data-p2-boot', state);
      if (detail) document.body.setAttribute('data-p2-boot-detail', String(detail).slice(0, 400));
    };
    try {
      await init();
      await start('p2-canvas', publishName);
      await window.p2RunLuaGame(status);
      status.textContent = publishName + ' - Lua running';
      markBoot('ok', publishName);
    } catch (error) {
      console.error(error);
      status.textContent = 'WebGL startup failed: ' + error;
      markBoot('failed', (error && error.message) || error);
    }
  </script>
</body>
</html>
"@ | Set-Content (Join-Path $distRoot "index.html") -Encoding UTF8
        @'
(function () {
  async function fetchText(path) {
    const response = await fetch(path, { cache: 'no-cache' });
    if (!response.ok) throw new Error(path + ' HTTP ' + response.status);
    return await response.text();
  }

  function keyValue(text, key, fallback) {
    for (const raw of text.split(/\r?\n/)) {
      const line = raw.trim().replace(/^\uFEFF/, '');
      const prefix = key + ' =';
      if (line.startsWith(prefix)) return line.slice(prefix.length).trim().replace(/^"|"$/g, '') || fallback;
    }
    return fallback;
  }

  function parseProps(text) {
    const out = {};
    for (const part of String(text || '').split(';')) {
      const i = part.indexOf('=');
      if (i > 0) out[part.slice(0, i)] = part.slice(i + 1);
    }
    return out;
  }

  function parseScene(text) {
    const entities = new Map();
    let name = 'main';
    for (const line of text.split(/\r?\n/)) {
      const parts = line.split('\t');
      if (line.startsWith('name=')) name = line.slice(5);
      if (parts[0] === 'entity') {
        entities.set(Number(parts[1]), { id: Number(parts[1]), name: parts[2] || '', alive: true, components: {} });
      } else if (parts[0] === 'component') {
        const entity = entities.get(Number(parts[1]));
        if (entity) entity.components[parts[2]] = parts.slice(3).join('\t');
      }
    }
    for (const entity of entities.values()) {
      if (entity.components.Transform) {
        const p = parseProps(entity.components.Transform);
        entity.transform = { x: Number(p.x || 0), y: Number(p.y || 0), rotation: Number(p.rotation || 0), scale_x: Number(p.scale_x || 1), scale_y: Number(p.scale_y || 1) };
      }
      if (entity.components.Tilemap) {
        const p = parseProps(entity.components.Tilemap);
        const tileSize = String(p.tile_size || '32x32').split('x').map(Number);
        entity.tilemap = { tile_size: tileSize[0] || 32, width: Number(p.width || 0), height: Number(p.height || 0), texture: p.texture || '', tiles: String(p.tiles || ''), solid: String(p.solid || '') };
      }
      if (entity.components.Animation) {
        const p = parseProps(entity.components.Animation);
        const clips = {};
        for (const part of String(entity.components.Animation).split(';')) {
          if (!part.startsWith('clip:')) continue;
          const fields = part.split(':');
          clips[fields[1]] = { texture: fields[2] || '', columns: Number(fields[3] || 1), frames: Number(fields[4] || 1), looping: fields[5] !== 'false' };
        }
        entity.animation = { current: p.default || 'idle', frame: 0, clips };
      }
      if (entity.components.Sprite) {
        const parts = String(entity.components.Sprite).split(';');
        const sizePart = parts.find(part => part.startsWith('size='));
        entity.sprite = { texture: parts[0] || '', size: sizePart ? Number(sizePart.slice(5)) || 32 : 32 };
      }
    }
    return { name, entities };
  }

  function loadImage(path) {
    return new Promise(resolve => {
      if (!path) {
        resolve(null);
        return;
      }
      const image = new Image();
      image.onload = () => resolve(image);
      image.onerror = () => resolve(null);
      image.src = 'pack/' + path;
    });
  }

  async function loadSceneImages(sceneState) {
    const images = new Map();
    const paths = new Set();
    for (const entity of sceneState.entities.values()) {
      if (entity.sprite && entity.sprite.texture) paths.add(entity.sprite.texture);
      if (entity.animation && entity.animation.clips) Object.values(entity.animation.clips).forEach(clip => { if (clip.texture) paths.add(clip.texture); });
      if (entity.tilemap && entity.tilemap.texture) paths.add(entity.tilemap.texture);
    }
    await Promise.all(Array.from(paths).map(async path => images.set(path, await loadImage(path))));
    return images;
  }

  function createWebInput(canvas) {
    const state = {
      keys: {},
      just: {},
      mouse: {},
      mouseJust: {},
      cursor: [0, 0],
      lastCursor: [0, 0],
      cursorDelta: [0, 0],
      scroll: 0
    };
    const keyName = event => {
       const key = String(event.key || '').toLowerCase();
       if (key === ' ') return 'space';
       if (key.startsWith('arrow')) return key.slice(5);
       return key;
     };
    window.addEventListener('keydown', event => {
      const key = keyName(event);
      if (!state.keys[key]) state.just[key] = true;
      state.keys[key] = true;
      if (['space', 'up', 'down', 'left', 'right'].includes(key)) event.preventDefault();
    });
    window.addEventListener('keyup', event => { state.keys[keyName(event)] = false; });
    canvas.addEventListener('mousedown', event => {
       const button = event.button === 2 ? 'right' : 'left';
       if (!state.mouse[button]) state.mouseJust[button] = true;
       state.mouse[button] = true;
     });
     window.addEventListener('mouseup', event => { state.mouse[event.button === 2 ? 'right' : 'left'] = false; });
    canvas.addEventListener('mousemove', event => {
      const rect = canvas.getBoundingClientRect();
      state.cursor = [event.clientX - rect.left, event.clientY - rect.top];
    });
    canvas.addEventListener('wheel', event => {
      state.scroll += event.deltaY;
      event.preventDefault();
    }, { passive: false });
    return {
      snapshot() {
        state.cursorDelta = [state.cursor[0] - state.lastCursor[0], state.cursor[1] - state.lastCursor[1]];
        state.lastCursor = [state.cursor[0], state.cursor[1]];
        const horizontal = (state.keys.d || state.keys.right ? 1 : 0) - (state.keys.a || state.keys.left ? 1 : 0);
         const vertical = (state.keys.s || state.keys.down ? 1 : 0) - (state.keys.w || state.keys.up ? 1 : 0);
        const out = { axis_x: horizontal, axis_y: vertical, keys: state.keys, just: state.just, mouse: state.mouse, mouse_just: state.mouseJust, cursor: state.cursor, cursor_delta: state.cursorDelta, scroll: state.scroll };
        state.just = {};
        state.mouseJust = {};
        state.scroll = 0;
        return out;
      }
    };
  }

  function syncLuaScene(sceneState, text) {
    for (const line of String(text || '').split(/\r?\n/)) {
      const parts = line.split('\t');
      const entity = sceneState.entities.get(Number(parts[1]));
      if (!entity) continue;
      if (parts[0] === 'T') {
        entity.transform = { x: Number(parts[2] || 0), y: Number(parts[3] || 0), rotation: Number(parts[4] || 0), scale_x: Number(parts[5] || 1), scale_y: Number(parts[6] || 1) };
      } else if (parts[0] === 'A') {
         const previous = entity.animation || { clips: {} };
         entity.animation = { current: parts[2] || 'idle', frame: Number(parts[3] || 0), clips: previous.clips || {} };
      } else if (parts[0] === 'D') {
        entity.alive = false;
      }
    }
  }

  function entityColor(entity) {
    const name = String(entity.name || '').toLowerCase();
    if (name.includes('player')) return '#4da3ff';
    if (name.includes('slime') || name.includes('enemy')) return '#7bdc65';
    if (name.includes('coin')) return '#ffd34d';
    return '#d0d7e2';
  }

  function drawScene(ctx, sceneState, images, projectName) {
    const canvas = ctx.canvas;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.fillStyle = '#151923';
    ctx.fillRect(0, 0, canvas.width, canvas.height);

    const tilemapEntity = Array.from(sceneState.entities.values()).find(entity => entity.tilemap);
    if (tilemapEntity) {
      const map = tilemapEntity.tilemap;
      const origin = tilemapEntity.transform || { x: 0, y: 0 };
      const tileset = images.get(map.texture);
      const columns = tileset ? Math.max(1, Math.floor(tileset.width / map.tile_size)) : 1;
      for (let y = 0; y < map.height; y += 1) {
        for (let x = 0; x < map.width; x += 1) {
          const i = y * map.width + x;
          const tile = Number(map.tiles[i] || 0);
          if (tileset && tile > 0) {
            const tileIndex = tile - 1;
            const sx = (tileIndex % columns) * map.tile_size;
            const sy = Math.floor(tileIndex / columns) * map.tile_size;
            ctx.drawImage(tileset, sx, sy, map.tile_size, map.tile_size, origin.x + x * map.tile_size, origin.y + y * map.tile_size, map.tile_size, map.tile_size);
          } else {
            const solid = map.solid[i] === '1';
            ctx.fillStyle = solid ? '#334155' : '#1f2937';
            ctx.fillRect(origin.x + x * map.tile_size, origin.y + y * map.tile_size, map.tile_size - 1, map.tile_size - 1);
          }
        }
      }
    }

    for (const entity of sceneState.entities.values()) {
      if (!entity.alive || entity.tilemap) continue;
      const pos = entity.transform || { x: 0, y: 0 };
      const size = entity.sprite ? entity.sprite.size : 24;
      const clip = entity.animation && entity.animation.clips ? entity.animation.clips[entity.animation.current] : null;
      const sprite = clip ? images.get(clip.texture) : (entity.sprite ? images.get(entity.sprite.texture) : null);
      if (sprite && clip) {
        const frame = clip.looping ? Math.floor(entity.animation.frame / 8) % Math.max(1, clip.frames) : Math.min(Math.floor(entity.animation.frame / 8), Math.max(1, clip.frames) - 1);
        const sx = (frame % Math.max(1, clip.columns)) * size;
        const sy = Math.floor(frame / Math.max(1, clip.columns)) * size;
        ctx.drawImage(sprite, sx, sy, size, size, pos.x - size / 2, pos.y - size / 2, size, size);
      } else if (sprite) {
        ctx.drawImage(sprite, pos.x - size / 2, pos.y - size / 2, size, size);
      } else {
        ctx.fillStyle = entityColor(entity);
        ctx.beginPath();
        ctx.arc(pos.x, pos.y, Math.max(6, size / 2), 0, Math.PI * 2);
        ctx.fill();
      }
    }

    ctx.fillStyle = '#d0d7e2';
    ctx.font = '14px sans-serif';
    ctx.fillText('Project: ' + projectName + ' | Scene: ' + sceneState.name + ' | Lua update running', 12, canvas.height - 16);
  }

  function luaQuote(text) {
    return '"' + String(text)
      .replace(/\\/g, '\\\\')
      .replace(/"/g, '\\"')
      .replace(/\r/g, '\\r')
      .replace(/\n/g, '\\n')
      .replace(/\t/g, '\\t') + '"';
  }

  function luaValue(value) {
    if (value === null || value === undefined) return 'nil';
    if (typeof value === 'number' || typeof value === 'boolean') return String(value);
    if (typeof value === 'string') return luaQuote(value);
    if (Array.isArray(value)) return '{' + value.map(luaValue).join(',') + '}';
    return '{' + Object.entries(value).map(([key, item]) => {
      const luaKey = /^\d+$/.test(key) ? '[' + key + ']' : '[' + luaQuote(key) + ']';
      return luaKey + '=' + luaValue(item);
    }).join(',') + '}';
  }

  function luaPrelude(sceneState) {
    const names = {};
    const transforms = {};
    const tilemaps = {};
    const animations = {};
    for (const entity of sceneState.entities.values()) {
      names[entity.name] = entity.id;
      if (entity.transform) transforms[entity.id] = entity.transform;
      if (entity.tilemap) tilemaps[entity.id] = entity.tilemap;
      if (entity.animation) animations[entity.id] = entity.animation;
    }
    return `
__p2_scene_name = ${luaQuote(sceneState.name)}
__p2_names = ${luaValue(names)}
__p2_alive = {}
__p2_transforms = ${luaValue(transforms)}
__p2_tilemaps = ${luaValue(tilemaps)}
__p2_animations = ${luaValue(animations)}
__p2_health = {}
__p2_save = {}
__p2_input = { axis_x = 0, axis_y = 0, keys = {}, just = {}, mouse = {}, mouse_just = {}, cursor = {0, 0}, cursor_delta = {0, 0}, scroll = 0 }
__p2_velocity = {}
for _, id in pairs(__p2_names) do __p2_alive[id] = true end
log = { info = function(v) print('[lua] ' .. tostring(v)) end, warn = function(v) print('[lua warn] ' .. tostring(v)) end, error = function(v) print('[lua error] ' .. tostring(v)) end }
time = { delta = function() return 1 / 60 end }
input = { axis = function(name) name = tostring(name or 'x'):lower(); if name == 'x' or name == 'horizontal' then return __p2_input.axis_x or 0 end; if name == 'y' or name == 'vertical' then return __p2_input.axis_y or 0 end; return 0 end, is_down = function(key) return __p2_input.keys[tostring(key):lower()] == true end, just_pressed = function(key) return __p2_input.just[tostring(key):lower()] == true end, mouse_is_down = function(button) return __p2_input.mouse[tostring(button or 0)] == true end, mouse_just_pressed = function(button) return __p2_input.mouse_just[tostring(button or 0)] == true end, cursor_position = function() return __p2_input.cursor or {0, 0} end, cursor_delta = function() return __p2_input.cursor_delta or {0, 0} end, scroll_delta = function() return __p2_input.scroll or 0 end }
scene = { reload = function() return __p2_scene_name end, load = function(name) __p2_scene_name = name return true end }
entity = { find = function(name) return __p2_names[name] end, is_alive = function(id) return __p2_alive[id] == true end, destroy = function(id) __p2_alive[id] = false end, spawn = function(name) local id = 1; for k, _ in pairs(__p2_alive) do if k >= id then id = k + 1 end end; __p2_names[name] = id; __p2_alive[id] = true; __p2_transforms[id] = { x = 0, y = 0 }; return id end }
transform = { get = function(id) return __p2_transforms[id] or { x = 0, y = 0 } end, move = function(id, x, y) local p = __p2_transforms[id] or { x = 0, y = 0 }; p.x = p.x + x; p.y = p.y + y; __p2_transforms[id] = p end }
transform3d = { attach = function(_) return true end, get = function(_) return { x = 0, y = 0, z = 0 } end, set = function(...) return true end, move = function(...) return true end }
tilemap = { get = function(id) local m = __p2_tilemaps[id]; if not m then return nil end; return { width = m.width, height = m.height, tile_size = m.tile_size } end, is_solid = function(id, x, y) local m = __p2_tilemaps[id]; if not m or x < 0 or y < 0 or x >= m.width or y >= m.height then return false end; local i = y * m.width + x + 1; return string.sub(m.solid, i, i) == '1' end, get_cell = function(id, x, y) local m = __p2_tilemaps[id]; if not m or x < 0 or y < 0 or x >= m.width or y >= m.height then return 0 end; local i = y * m.width + x + 1; return tonumber(string.sub(m.tiles, i, i)) or 0 end }
health = { get = function(id) return __p2_health[id] or 100 end, set = function(id, hp) __p2_health[id] = hp end, damage = function(id, amount) __p2_health[id] = (health.get(id) or 100) - amount; return __p2_health[id] end }
physics = { set_velocity = function(id, x, y) __p2_velocity[id] = { x = x or 0, y = y or 0 }; return true end, events = function() return {} end }
physics3d = { set_velocity = function(...) return true end, set_position = function(...) return true end, events = function() return {} end }
animation = { get = function(id) return (__p2_animations[id] and __p2_animations[id].current) or 'idle' end, set = function(id, name) local a = __p2_animations[id] or { current = 'idle', frame = 0 }; if a.current ~= name then a.current = name; a.frame = 0 end; __p2_animations[id] = a; return true end, ended = function(_) return true end, frame = function(id) return (__p2_animations[id] and __p2_animations[id].frame) or 0 end }
animation3d = { set = function(...) return true end }
audio = { volume = function(_) return true end, music = function(name) log.info('music ' .. tostring(name)) end, play = function(name) log.info('sound ' .. tostring(name)) end }
ui = { text_at = function(key, x, y, text, scale) __p2_ui_text = tostring(text) end, bar = function(...) return true end }
save = { read = function(key) return __p2_save[key] end, write = function(key, value) __p2_save[key] = tostring(value) end, load = function() return true end, commit = function() return true end }
particles = { burst = function(...) return true end }
camera = { set = function(...) return true end, set_active = function(...) return true end }
mesh = { set = function(...) return true end }
os = os or {}; os.getenv = function(_) return nil end
function __p2_export_state()
  local rows = {}
  for id, v in pairs(__p2_velocity) do local p = __p2_transforms[id] or { x = 0, y = 0 }; p.x = (p.x or 0) + (v.x or 0) * time.delta(); p.y = (p.y or 0) + (v.y or 0) * time.delta(); __p2_transforms[id] = p end
  for id, p in pairs(__p2_transforms) do rows[#rows + 1] = 'T\t' .. id .. '\t' .. tostring(p.x or 0) .. '\t' .. tostring(p.y or 0) .. '\t' .. tostring(p.rotation or 0) .. '\t' .. tostring(p.scale_x or 1) .. '\t' .. tostring(p.scale_y or 1) end
  for id, a in pairs(__p2_animations) do a.frame = (a.frame or 0) + 1; rows[#rows + 1] = 'A\t' .. id .. '\t' .. tostring(a.current or 'idle') .. '\t' .. tostring(a.frame or 0) end
  for id, alive in pairs(__p2_alive) do if alive ~= true then rows[#rows + 1] = 'D\t' .. id end end
  return table.concat(rows, '\\n')
end
`;
  }

  function doString(L, source, name) {
    const f = window.fengari;
    const rc = f.lauxlib.luaL_loadbuffer(L, f.to_luastring(source), source.length, f.to_luastring(name));
    if (rc !== f.lua.LUA_OK || f.lua.lua_pcall(L, 0, f.lua.LUA_MULTRET, 0) !== f.lua.LUA_OK) {
      const message = f.to_jsstring(f.lua.lua_tostring(L, -1));
      f.lua.lua_pop(L, 1);
      throw new Error(message);
    }
  }

  // 妗岄潰寮曟搸娉ㄥ唽鐨勯《灞?Lua 鍏ㄥ眬锛坄engine/src/core/script.rs`锛夈€俉ebGL 杩愯鏃跺彧瀹炵幇
  // 鍏朵腑涓€閮ㄥ垎锛坉emo 绾у瓙闆嗭級锛屾父鎴忚剼鏈竴鏃︾敤鍒版湭瀹炵幇鐨勯偅涓叏灞€锛屾姤閿欎細鏄竴鍙?
  // 娣卞湪 Lua 閲岀殑 "attempt to index a nil value"鈥斺€旂帺瀹朵笌楠屾敹閮芥棤娉曟嵁姝ゅ畾浣嶃€?
  // 杩欓噷鍦ㄥ惎鍔ㄥけ璐ユ椂琛ヤ竴鍙?缂哄摢浜涘涓?API"锛屽苟鍐欒繘 DOM 灞炴€т緵鑷姩鍖栨柇瑷€
  // 锛堣 docs/32 搂3 涓?scripts/verify-webgl-bundle.ps1锛夈€?
  const ENGINE_HOST_GLOBALS = [
    'animation', 'animation3d', 'app', 'asset', 'audio', 'calendar', 'camera', 'clock',
    'container', 'data', 'day', 'entity', 'event', 'grid', 'health', 'input', 'interact',
    'inv', 'item', 'light', 'log', 'map', 'mesh', 'particles', 'physics', 'physics3d',
    'save', 'scene', 'shop', 'sprite', 'tag', 'tilemap', 'time', 'transform', 'transform3d', 'ui'
  ];

  function missingHostGlobals(L) {
    const f = window.fengari;
    const missing = [];
    for (const name of ENGINE_HOST_GLOBALS) {
      f.lua.lua_getglobal(L, f.to_luastring(name));
      if (f.lua.lua_isnil(L, -1)) missing.push(name);
      f.lua.lua_pop(L, 1);
    }
    return missing;
  }

  function markBoot(state, detail) {
    const body = document.body || document.documentElement;
    if (!body) return;
    body.setAttribute('data-p2-boot', state);
    if (detail) body.setAttribute('data-p2-boot-detail', String(detail).slice(0, 400));
  }

  function callGlobal(L, name) {
    const f = window.fengari;
    f.lua.lua_getglobal(L, f.to_luastring(name));
    if (!f.lua.lua_isfunction(L, -1)) {
      f.lua.lua_pop(L, 1);
      return null;
    }
    if (f.lua.lua_pcall(L, 0, 1, 0) !== f.lua.LUA_OK) {
      const message = f.to_jsstring(f.lua.lua_tostring(L, -1));
      f.lua.lua_pop(L, 1);
      throw new Error(message);
    }
    const result = f.lua.lua_isstring(L, -1) ? f.to_jsstring(f.lua.lua_tostring(L, -1)) : null;
    f.lua.lua_pop(L, 1);
    return result;
  }

  window.p2RunLuaGame = async function (status) {
    if (!window.fengari) throw new Error('fengari-web not loaded');
    const project = await fetchText('pack/project.toml');
    const startup = keyValue(project, 'startup_scene', 'scenes/main.scene');
    const mainScript = keyValue(project, 'main_script', 'scripts/main.lua');
    const projectName = keyValue(project, 'name', 'game');
    const sceneText = await fetchText('pack/' + startup);
    const sceneState = parseScene(sceneText);
    const canvas = document.getElementById('p2-canvas');
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('2D canvas context is not available');
    const webInput = createWebInput(canvas);
    const images = await loadSceneImages(sceneState);

    const manifest = await fetchText('pack/manifest.txt');
    const modules = {};
    for (const line of manifest.split(/\r?\n/).slice(1)) {
      const path = line.split('\t')[0];
      if (path && path.startsWith('scripts/') && path.endsWith('.lua')) {
        const name = path.slice('scripts/'.length, -'.lua'.length).replace(/\//g, '.');
        modules[name] = await fetchText('pack/' + path);
      }
    }

    const preload = Object.keys(modules).sort().map(name => `package.preload[${luaQuote(name)}] = function(...)\n${modules[name]}\nend`).join('\n');
    const main = await fetchText('pack/' + mainScript);
    const f = window.fengari;
    const L = f.lauxlib.luaL_newstate();
    f.lualib.luaL_openlibs(L);
    try {
      doString(L, luaPrelude(sceneState) + '\n' + preload + '\n' + main, mainScript);
    } catch (error) {
      // fail fast锛氭妸"缂哄摢浜涘涓?API"鐩存帴璇村嚭鏉ワ紝鑰屼笉鏄妸 Lua 鏍堝洖婧涪缁欑帺瀹躲€?
      const missing = missingHostGlobals(L);
      const detail = 'missing engine host API: ' + missing.join(', ');
      markBoot('failed', detail);
      throw new Error(String((error && error.message) || error) + ' [' + detail + ']');
    }
    callGlobal(L, 'on_create');
    drawScene(ctx, sceneState, images, projectName);
    function frame() {
      try {
        doString(L, '__p2_input = ' + luaValue(webInput.snapshot()), '__p2_input');
        const result = callGlobal(L, 'update');
        syncLuaScene(sceneState, callGlobal(L, '__p2_export_state'));
        drawScene(ctx, sceneState, images, projectName);
        if (status && result) status.textContent = result;
        markBoot('running', sceneState.name);
      } catch (error) {
        console.error(error);
        if (status) status.textContent = 'Lua runtime failed: ' + error;
        markBoot('failed', (error && error.message) || error);
        return;
      }
      requestAnimationFrame(frame);
    }
    markBoot('booted', sceneState.name);
    requestAnimationFrame(frame);
    console.log('[lua] WebGL Lua runtime started:', startup, sceneState.name);
  };
})();
'@ | Set-Content (Join-Path $distRoot "web-runtime.js") -Encoding UTF8
    } else {
        Copy-Item -Path (Join-Path $engineRoot "target\release\game.exe") `
            -Destination (Join-Path $distRoot "game.exe") -Force
    }

    $fontLine = Get-Content -LiteralPath $projectToml | Where-Object { $_ -match '^\s*font\s*=\s*"(.+)"\s*$' } | Select-Object -First 1
    $fontRelative = if ($fontLine -and $fontLine -match '^\s*font\s*=\s*"(.+)"\s*$') { $Matches[1] } else { "assets/fonts/NotoSansSC-VF.ttf" }
    $fontSource = Join-Path $gameRoot ($fontRelative -replace '/', '\\')
    if (-not (Test-Path $fontSource)) { throw "Game font resource was not found: $fontSource" }
    if ((Get-Item $fontSource).Length -le 0) { throw "Game font resource is empty: $fontSource" }
    $fontName = Split-Path -Leaf $fontSource
    $fontTarget = Join-Path $distRoot ("fonts\\" + $fontName)
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $fontTarget) | Out-Null
    Copy-Item -Path $fontSource -Destination $fontTarget -Force
    if (-not (Test-Path $fontTarget) -or (Get-Item $fontTarget).Length -le 0) {
        throw "Release package font is missing or empty: fonts/$fontName"
    }

    # 宸ョ▼绾ц缃紙settings.toml锛氭墜鏌勬敼閿笌姝诲尯锛夐殢鍖呭垎鍙戯細鍙戝竷鍖呬互 pack 鐩綍浣滀负
    # 宸ョ▼鏍癸紝杩愯鍣ㄦ寜 `<root>/settings.toml` 璇诲彇锛屽惁鍒欐墦鍖呯増浼氬洖钀藉埌閿洏榛樿缁戝畾锛?
    # 鍑虹幇銆岀紪杈戝櫒閲屾敼濂界殑鎵嬫焺缁戝畾鍦ㄥ彂甯冨寘閲屽け鏁堛€嶃€?
    $settingsSource = Join-Path $gameRoot "settings.toml"
    $settingsPackaged = $false
    if (Test-Path -LiteralPath $settingsSource) {
        Copy-Item -Path $settingsSource -Destination (Join-Path $packRoot "settings.toml") -Force
        $settingsPackaged = $true
        Write-Host "==> Packaged settings.toml (input bindings / deadzone)" -ForegroundColor DarkGray
    }

    @(
        "publish_name = `"$PublishName`""
    ) | Set-Content (Join-Path $distRoot "release.toml") -Encoding UTF8

    if ($Platform -eq "wechat-minigame") {
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\wechat-adapter.js") (Join-Path $distRoot "wechat-adapter.js") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\game.js") (Join-Path $distRoot "game.js") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\game.json") (Join-Path $distRoot "game.json") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\project.config.json") (Join-Path $distRoot "project.config.json") -Force
        foreach ($f in @("game.js","game.json","project.config.json","wechat-adapter.js","pack\manifest.txt","pack\project.toml")) { if (-not (Test-Path (Join-Path $distRoot $f))) { throw "WeChat package is missing: $f" } }
        Write-Host "wechat_minigame_package_ok" -ForegroundColor Green
    } elseif ($Platform -eq "webgl") {
        $requiredWebFiles = @(
            (Join-Path $distRoot "index.html"),
                (Join-Path $distRoot "web-runtime.js"),
                (Join-Path $distRoot "pkg\game_web.js"),
            (Join-Path $distRoot "pkg\game_web_bg.wasm"),
            (Join-Path $packRoot "manifest.txt"),
            (Join-Path $packRoot "project.toml")
        )
        foreach ($file in $requiredWebFiles) {
            if (-not (Test-Path $file)) { throw "WebGL package is missing: $file" }
        }
        Write-Host "webgl_package_ok files=$($requiredWebFiles.Count)" -ForegroundColor Green
    } else {
        # 娉ㄦ剰锛氬彂甯冪増 `game.exe` 鏄?**Windows GUI 瀛愮郴缁?*绋嬪簭
        # 锛坄crates/game/src/main.rs` 鐨?`windows_subsystem = "windows"`锛屽弻鍑绘椂涓嶅脊鎺у埗鍙帮級銆?
        # 杩欑被绋嬪簭鐢?`& exe args` 璋冪敤鏃?PowerShell **鏃笉绛夊緟涔熶笉鎹曡幏 stdout**锛?
        # `$LASTEXITCODE` 浼氫繚鐣欎笂涓€鏉″懡浠ょ殑鍊尖€斺€斾簬鏄?鍙戝竷鍖呰嚜妫€"浼氶潤榛樼┖璺戞垚姘歌繙閫氳繃銆?
        # 蹇呴』鐢?Start-Process -Wait + 閲嶅畾鍚戞枃浠舵潵鐪熷疄鍙栧洖閫€鍑虹爜涓庤緭鍑恒€?
        $packCheck = Invoke-PackagedCheck -Runner (Join-Path $distRoot "game.exe") -PackRoot $packRoot
        Write-Host $packCheck.Output
        if ($packCheck.ExitCode -ne 0) {
            throw "Release package check failed (exit $($packCheck.ExitCode))."
        }
        $packRootsLine = $packCheck.Output | Where-Object { $_ -match "player_data_roots" } | Select-Object -First 1
        if (-not $packRootsLine) { throw "Release package check did not report player_data_roots." }
        if ($packRootsLine -notmatch "packaged=true") {
            throw "Release package did not report packaged=true: $packRootsLine"
        }
        Write-Host "Release package player data roots: $packRootsLine" -ForegroundColor DarkGray
    }

    $engineVersion = (Get-Content (Join-Path $engineRoot "Cargo.toml") | Select-String '^version = "0\.' | Select-Object -First 1).ToString().Trim().Split('=')[1].Trim().Trim('"')
    $toolchain = & rustc --version 2>$null
    cargo tree -e normal --prefix none 2>$null | Sort-Object -Unique | Set-Content (Join-Path $distRoot "deps.txt") -Encoding UTF8
    $packageContents = "game.exe, fonts/$fontName, pack/ (data), release.toml, run.txt, deps.txt"
    if ($settingsPackaged) {
        $packageContents = "game.exe, fonts/$fontName, pack/ (data, settings.toml), release.toml, run.txt, deps.txt"
    }
    if ($Platform -eq "wechat-minigame") {
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\wechat-adapter.js") (Join-Path $distRoot "wechat-adapter.js") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\game.js") (Join-Path $distRoot "game.js") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\game.json") (Join-Path $distRoot "game.json") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\project.config.json") (Join-Path $distRoot "project.config.json") -Force
        foreach ($f in @("game.js","game.json","project.config.json","wechat-adapter.js","pack\manifest.txt","pack\project.toml")) { if (-not (Test-Path (Join-Path $distRoot $f))) { throw "WeChat package is missing: $f" } }
        Write-Host "wechat_minigame_package_ok" -ForegroundColor Green
    } elseif ($Platform -eq "webgl") {
        $packageContents = "index.html, pkg/game_web.js, pkg/game_web_bg.wasm, fonts/$fontName, pack/ (data), release.toml, run.txt, deps.txt"
        if ($settingsPackaged) {
            $packageContents = "index.html, pkg/game_web.js, pkg/game_web_bg.wasm, fonts/$fontName, pack/ (data, settings.toml), release.toml, run.txt, deps.txt"
        }
    }
    @(
        "RustLua game engine release package",
        "publish_name: $PublishName",
        "build_time: $timestamp",
        "platform: $Platform",
        "engine version: $engineVersion",
        "toolchain: $toolchain",
        "contents: $packageContents",
        "font_check: fonts/$fontName present and non-empty",
        "settings_check: $(if ($settingsPackaged) { 'pack/settings.toml packaged from project root' } else { 'no project settings.toml; engine defaults apply' })",
        "player_data: $(if ($Platform -eq 'webgl') { 'browser storage; no filesystem player data' } else { "$PlayerDataDir (override: RUSTLUA_USER_DIR); pack/ is read-only and never written" })"
    ) | Set-Content (Join-Path $distRoot "build-info.txt") -Encoding UTF8

    if ($Platform -eq "wechat-minigame") {
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\wechat-adapter.js") (Join-Path $distRoot "wechat-adapter.js") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\game.js") (Join-Path $distRoot "game.js") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\game.json") (Join-Path $distRoot "game.json") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\project.config.json") (Join-Path $distRoot "project.config.json") -Force
        foreach ($f in @("game.js","game.json","project.config.json","wechat-adapter.js","pack\manifest.txt","pack\project.toml")) { if (-not (Test-Path (Join-Path $distRoot $f))) { throw "WeChat package is missing: $f" } }
        Write-Host "wechat_minigame_package_ok" -ForegroundColor Green
    } elseif ($Platform -eq "webgl") {
        @(
            "Run instructions",
            "1) Serve this directory with an HTTP server, for example: python -m http.server 8000",
            "2) Open http://localhost:8000/index.html in a WebGL2-capable browser",
            "3) Do not open index.html directly from file:// because browsers block WASM/resource fetches",
            "4) The page title and browser tab title use publish_name from release.toml"
        ) | Set-Content (Join-Path $distRoot "run.txt") -Encoding UTF8
    } else {
        @(
            "Run instructions",
            "1) Double-click game.exe, or run: game.exe --pack pack --title `"$PublishName`"",
            "2) Controls: WASD move, SPACE attack, F1 save, F2 load, F4 switch scene",
            "3) F3 toggles the developer fps/latency overlay (off by default in release builds)",
            "4) Logs and crash reports go to the player data directory: $PlayerDataDir\logs",
            "   (a release pack never writes into its own directory, so a read-only install is fine)",
            "5) This package has no source or toolchain dependency and runs without Rust/IDE",
            "6) Player data (savegames and settings) is written to: $PlayerDataDir",
            "   The pack/ directory is read-only for the runner and never receives player data;",
            "   set RUSTLUA_USER_DIR to relocate the player data directory (used by tests and portable installs)"
        ) | Set-Content (Join-Path $distRoot "run.txt") -Encoding UTF8
    }

    # M20-03锛氬叏閮ㄥ唴瀹逛笌鑷閫氳繃鍚庯紝鍘熷瓙鏇挎崲鏈€缁堢洰褰曪紙鏃х増鏈浠解啋绉诲叆鏂扮増鏈?
    # 鈫掓竻鐞嗗浠斤級銆備氦鎹㈠け璐ヤ細鍥炴粴鏃х増鏈苟鎶涢敊锛宻taging 淇濈暀浠ヤ究璇婃柇銆?
    Write-Host "==> Atomic release swap" -ForegroundColor Cyan
    $script:swapAttempted = $true
    $swap = Complete-ReleaseSwap -FinalRoot $finalRoot -StageRoot $stageRoot
    $distRoot = $finalRoot
    $packRoot = Join-Path $finalRoot "pack"
    Write-Host ("==> Release swapped in: {0} entries replaced, {1} entries moved aside" -f $swap.MovedIn.Count, $swap.MovedAside.Count) -ForegroundColor Cyan

    Write-Host "Release package ready: $distRoot" -ForegroundColor Green
    $total = Get-ChildItem -Recurse -File $distRoot | Measure-Object -Property Length -Sum
    Write-Host ("Package size: {0:N1} KB, {1} files" -f ($total.Sum / 1KB), $total.Count)
}
catch {
    # 浜ゆ崲涔嬪墠澶辫触锛氭渶缁堢洰褰曚粠鏈鏀瑰姩锛岀洿鎺ヤ涪寮?staging锛屽苟鏄庣‘鍛婄煡鏃х増鏈粛鍦ㄣ€?
    if (-not $script:swapAttempted -and (Test-Path -LiteralPath $stageRoot)) {
        Write-Warning "==> Release failed before swap; existing release kept at $finalRoot"
        Write-Warning "==> Discarding staging: $stageRoot"
        Undo-ReleaseStaging -StageRoot $stageRoot | Out-Null
    }
    elseif ($script:swapAttempted -and (Test-Path -LiteralPath $stageRoot)) {
        Write-Warning "==> Swap failed; old release restored. Staging kept for diagnosis: $stageRoot"
    }
    throw
}
finally {
    Pop-Location
}

# M20-03: best-effort removal of the isolated test copy. Windows may still hold
# the freshly launched runner (AV/indexer), so retry briefly and never fail the
# publish because of cleanup; leftovers are reported with their path.
function Remove-IsolatedRelease([string]$Path, [string]$Label) {
    for ($attempt = 1; $attempt -le 5; $attempt++) {
        if (-not (Test-Path -LiteralPath $Path)) { return }
        try {
            Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop
            return
        }
        catch {
            Start-Sleep -Milliseconds (200 * $attempt)
        }
    }
    Write-Warning "==> Leftover $Label not removed (file in use); remove manually: $Path"
}

if ($SelfTest) {
    Write-Host "`r`n==> Directory-isolation self test (no source/target/.cache, path with spaces)" -ForegroundColor Cyan
    $isolated = Join-Path $env:TEMP ("p2 release test " + (Get-Date -Format "HHmmss"))
    New-Item -ItemType Directory -Force -Path $isolated | Out-Null
    Copy-Item -LiteralPath $distRoot -Destination $isolated -Recurse -Force
    if ($Platform -eq "wechat-minigame") {
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\wechat-adapter.js") (Join-Path $distRoot "wechat-adapter.js") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\game.js") (Join-Path $distRoot "game.js") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\game.json") (Join-Path $distRoot "game.json") -Force
        Copy-Item (Join-Path $workspaceRoot "plugins\wechat-minigame\project.config.json") (Join-Path $distRoot "project.config.json") -Force
        foreach ($f in @("game.js","game.json","project.config.json","wechat-adapter.js","pack\manifest.txt","pack\project.toml")) { if (-not (Test-Path (Join-Path $distRoot $f))) { throw "WeChat package is missing: $f" } }
        Write-Host "wechat_minigame_package_ok" -ForegroundColor Green
    } elseif ($Platform -eq "webgl") {
        $isolatedRoot = Join-Path $isolated $PublishName
        foreach ($file in @(
            (Join-Path $isolatedRoot "index.html"),
            (Join-Path $isolatedRoot "web-runtime.js"),
            (Join-Path $isolatedRoot "pkg\game_web.js"),
            (Join-Path $isolatedRoot "pkg\game_web_bg.wasm"),
            (Join-Path $isolatedRoot "pack\manifest.txt")
        )) {
            if (-not (Test-Path $file)) { throw "isolated WebGL dir is missing: $file" }
        }
        Remove-IsolatedRelease -Path $isolated -Label "WebGL self-test copy"
        Write-Host "Directory-isolation self test passed (WebGL static package OK)" -ForegroundColor Green
    } else {
        $isolatedRunner = Join-Path $isolated ("$PublishName\game.exe")
        $isolatedPack = Join-Path $isolated ("$PublishName\pack")
        if (-not (Test-Path $isolatedRunner)) { throw "isolated dir is missing the runner" }
        $isolatedFont = Join-Path $isolated ("$PublishName\fonts\$fontName")
        if (-not (Test-Path $isolatedFont) -or (Get-Item $isolatedFont).Length -le 0) {
            throw "isolated dir is missing a non-empty fonts/$fontName"
        }

        Write-Host "Check: $isolatedRunner --pack $isolatedPack --check" -ForegroundColor DarkGray
        # 鐢?--check 鍋氭棤绐楀彛鏍￠獙锛氶€€鍑虹爜 0 琛ㄧず鍙戝竷鍖呭畬鏁达紙鍚┖鏍艰矾寰?+ 鏃犳簮鐮佺幆澧冿級銆?
        # M31/P3锛氬悓鏃舵妸鐜╁鏁版嵁鏍规寚鍒颁竴涓复鏃剁洰褰曪紝鏂█鍙戝竷鍖呭舰鎬佺‘瀹炶В鏋愬埌**鐢ㄦ埛鏁版嵁鐩綍**
        # 锛堣€屼笉鏄畨瑁呯洰褰曪級锛屽苟鏂█瀹夎鐩綍閲屼笉鍑虹幇 .cache/save/settings 绛夌帺瀹舵暟鎹€?
        $isolatedUserDir = Join-Path $env:TEMP ("p2 user data test " + (Get-Date -Format "HHmmss"))
        New-Item -ItemType Directory -Force -Path $isolatedUserDir | Out-Null
        $env:RUSTLUA_USER_DIR = $isolatedUserDir
        try {
            $isolatedCheck = Invoke-PackagedCheck -Runner $isolatedRunner -PackRoot $isolatedPack
            Write-Host $isolatedCheck.Output
            if ($isolatedCheck.ExitCode -ne 0) {
                throw "isolated check failed (exit $($isolatedCheck.ExitCode)): package incomplete or path incompatible."
            }
            $rootsLine = $isolatedCheck.Output | Where-Object { $_ -match "player_data_roots" } | Select-Object -First 1
            if (-not $rootsLine) { throw "isolated check did not report player_data_roots." }
            Write-Host "Player data roots: $rootsLine" -ForegroundColor DarkGray
            if ($rootsLine -notmatch "packaged=true") {
                throw "release package did not report packaged=true: $rootsLine"
            }
            if ($rootsLine -notmatch [regex]::Escape($isolatedUserDir)) {
                throw "release package ignored RUSTLUA_USER_DIR=$isolatedUserDir : $rootsLine"
            }
            $strayPlayerData = Get-ChildItem -LiteralPath (Join-Path $isolated $PublishName) -Recurse -Force -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -in @(".cache", ".data") -or $_.Extension -in @(".sav", ".log") }
            if ($strayPlayerData) {
                throw "release directory contains player data: $($strayPlayerData.FullName -join ', ')"
            }
        }
        finally {
            Remove-Item Env:\RUSTLUA_USER_DIR -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath $isolatedUserDir -Recurse -Force -ErrorAction SilentlyContinue
        }
        Remove-IsolatedRelease -Path $isolated -Label "self-test copy"
        Write-Host "Directory-isolation self test passed (spaces path + no-source check + user data dir OK)" -ForegroundColor Green
    }
}
