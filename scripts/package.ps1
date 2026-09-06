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
    [switch]$Encrypt
)

$ErrorActionPreference = "Stop"
$workspaceRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $workspaceRoot "engine"
if ([string]::IsNullOrWhiteSpace($ProjectRoot)) {
    $ProjectRoot = Join-Path $workspaceRoot "examples"
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
$distRoot = Join-Path $gameRoot ("builds\" + $Platform + "\" + $PublishName)
$packRoot = Join-Path $distRoot "pack"
$timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Install Rust stable and ensure cargo is on PATH."
}

New-Item -ItemType Directory -Force -Path $distRoot | Out-Null
Push-Location $engineRoot
try {
    Write-Host "==> Release build ($Platform)" -ForegroundColor Cyan
    if ($Platform -eq "webgl") {
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
    # 发布包只含运行器；assetc 为开发工具，不随包分发（并清理历史残留）。
    Remove-Item -Path (Join-Path $distRoot "assetc.exe") -Force -ErrorAction SilentlyContinue
    if ($Platform -eq "webgl") {
        Remove-Item -Path (Join-Path $distRoot "game.exe") -Force -ErrorAction SilentlyContinue
        $pkgRoot = Join-Path $distRoot "pkg"
        if (Test-Path $pkgRoot) { Remove-Item -LiteralPath $pkgRoot -Recurse -Force }
        New-Item -ItemType Directory -Force -Path $pkgRoot | Out-Null
        $wasmInput = Join-Path $engineRoot "target\wasm32-unknown-unknown\release\game_web.wasm"
        wasm-bindgen $wasmInput --out-dir $pkgRoot --target web --no-typescript
        if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen failed." }
        $publishNameJson = $PublishName | ConvertTo-Json -Compress
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
<body>
  <canvas id="p2-canvas"></canvas>
  <div id="status">Loading $PublishName...</div>
  <script src="https://cdn.jsdelivr.net/npm/fengari-web@0.1.4/dist/fengari-web.js"></script>
  <script src="./web-runtime.js"></script>
  <script type="module">
    import init, { start } from './pkg/game_web.js';
    const publishName = $publishNameJson;
    const status = document.getElementById('status');
    try {
      await init();
      await start('p2-canvas', publishName);
      await window.p2RunLuaGame(status);
      status.textContent = publishName + ' - Lua running';
    } catch (error) {
      console.error(error);
      status.textContent = 'WebGL startup failed: ' + error;
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
    doString(L, luaPrelude(sceneState) + '\n' + preload + '\n' + main, mainScript);
    callGlobal(L, 'on_create');
    drawScene(ctx, sceneState, images, projectName);
    function frame() {
      try {
        doString(L, '__p2_input = ' + luaValue(webInput.snapshot()), '__p2_input');
        const result = callGlobal(L, 'update');
        syncLuaScene(sceneState, callGlobal(L, '__p2_export_state'));
        drawScene(ctx, sceneState, images, projectName);
        if (status && result) status.textContent = result;
      } catch (error) {
        console.error(error);
        if (status) status.textContent = 'Lua runtime failed: ' + error;
        return;
      }
      requestAnimationFrame(frame);
    }
    requestAnimationFrame(frame);
    console.log('[lua] WebGL Lua runtime started:', startup, sceneState.name);
  };
})();
'@ | Set-Content (Join-Path $distRoot "web-runtime.js") -Encoding UTF8
    } else {
        Copy-Item -Path (Join-Path $engineRoot "target\release\game.exe") `
            -Destination (Join-Path $distRoot "game.exe") -Force
    }

    $fontSource = Join-Path $gameRoot "assets\fonts\NotoSansSC-VF.ttf"
    if (-not (Test-Path $fontSource)) { throw "Game CJK font resource was not found: $fontSource" }
    if ((Get-Item $fontSource).Length -le 0) { throw "Game CJK font resource is empty: $fontSource" }
    $fontTarget = Join-Path $distRoot "fonts\NotoSansSC-VF.ttf"
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $fontTarget) | Out-Null
    Copy-Item -Path $fontSource -Destination $fontTarget -Force
    if (-not (Test-Path $fontTarget) -or (Get-Item $fontTarget).Length -le 0) {
        throw "Release package font is missing or empty: fonts/NotoSansSC-VF.ttf"
    }

    @(
        "publish_name = `"$PublishName`""
    ) | Set-Content (Join-Path $distRoot "release.toml") -Encoding UTF8

    if ($Platform -eq "webgl") {
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
        $packCheckOutput = & (Join-Path $distRoot "game.exe") --pack $packRoot --check 2>&1
        $packCheckExit = $LASTEXITCODE
        Write-Host $packCheckOutput
        if ($packCheckExit -ne 0) { throw "Release package check failed (exit $packCheckExit)." }
    }

    $engineVersion = (Get-Content (Join-Path $engineRoot "Cargo.toml") | Select-String '^version = "0\.' | Select-Object -First 1).ToString().Trim().Split('=')[1].Trim().Trim('"')
    $toolchain = & rustc --version 2>$null
    cargo tree -e normal --prefix none 2>$null | Sort-Object -Unique | Set-Content (Join-Path $distRoot "deps.txt") -Encoding UTF8
    $packageContents = "game.exe, fonts/NotoSansSC-VF.ttf, pack/ (data), release.toml, run.txt, deps.txt"
    if ($Platform -eq "webgl") {
        $packageContents = "index.html, pkg/game_web.js, pkg/game_web_bg.wasm, fonts/NotoSansSC-VF.ttf, pack/ (data), release.toml, run.txt, deps.txt"
    }
    @(
        "RustLua game engine release package",
        "publish_name: $PublishName",
        "build_time: $timestamp",
        "platform: $Platform",
        "engine version: $engineVersion",
        "toolchain: $toolchain",
        "contents: $packageContents",
        "font_check: fonts/NotoSansSC-VF.ttf present and non-empty"
    ) | Set-Content (Join-Path $distRoot "build-info.txt") -Encoding UTF8

    if ($Platform -eq "webgl") {
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
            "3) Logs and crash reports are written next to game.exe",
            "4) This package has no source or toolchain dependency and runs without Rust/IDE"
        ) | Set-Content (Join-Path $distRoot "run.txt") -Encoding UTF8
    }

    Write-Host "Release package ready: $distRoot" -ForegroundColor Green
    $total = Get-ChildItem -Recurse -File $distRoot | Measure-Object -Property Length -Sum
    Write-Host ("Package size: {0:N1} KB, {1} files" -f ($total.Sum / 1KB), $total.Count)
}
finally {
    Pop-Location
}

if ($SelfTest) {
    Write-Host "`n==> Directory-isolation self test (no source/target/.cache, path with spaces)" -ForegroundColor Cyan
    $isolated = Join-Path $env:TEMP ("p2 release test " + (Get-Date -Format "HHmmss"))
    New-Item -ItemType Directory -Force -Path $isolated | Out-Null
    Copy-Item -LiteralPath $distRoot -Destination $isolated -Recurse -Force
    if ($Platform -eq "webgl") {
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
        Remove-Item -LiteralPath $isolated -Recurse -Force
        Write-Host "Directory-isolation self test passed (WebGL static package OK)" -ForegroundColor Green
    } else {
        $isolatedRunner = Join-Path $isolated ("$PublishName\game.exe")
        $isolatedPack = Join-Path $isolated ("$PublishName\pack")
        if (-not (Test-Path $isolatedRunner)) { throw "isolated dir is missing the runner" }
        $isolatedFont = Join-Path $isolated ("$PublishName\fonts\NotoSansSC-VF.ttf")
        if (-not (Test-Path $isolatedFont) -or (Get-Item $isolatedFont).Length -le 0) {
            throw "isolated dir is missing a non-empty fonts/NotoSansSC-VF.ttf"
        }

        Write-Host "Check: $isolatedRunner --pack $isolatedPack --check" -ForegroundColor DarkGray
        # 用 --check 做无窗口校验：退出码 0 表示发布包完整（含空格路径 + 无源码环境）。
        $checkOutput = & $isolatedRunner --pack $isolatedPack --check 2>&1
        $checkExit = $LASTEXITCODE
        Write-Host $checkOutput
        Remove-Item -LiteralPath $isolated -Recurse -Force
        if ($checkExit -ne 0) {
            throw "isolated check failed (exit $checkExit): package incomplete or path incompatible."
        }
        Write-Host "Directory-isolation self test passed (spaces path + no-source check OK)" -ForegroundColor Green
    }
}
