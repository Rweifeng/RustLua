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

  // 场景组件的字段分隔符是 `;`，但 `Text.text` 里可能出现 `;` 或 `=`（中文文案、标点）。
  // 因此按「`;` + 合法字段名 + `=`」切分，只有真正的字段边界才算分隔符。
  function parseComponentFields(raw) {
    const text = String(raw || '');
    const pattern = /([A-Za-z_][A-Za-z0-9_]*)=/g;
    const marks = [];
    let match;
    while ((match = pattern.exec(text)) !== null) {
      const boundary = match.index === 0 ? 0 : (text[match.index - 1] === ';' ? match.index - 1 : -1);
      marks.push({ key: match[1], valueStart: match.index + match[0].length, boundary });
    }
    const out = {};
    for (let i = 0; i < marks.length; i += 1) {
      const end = i + 1 < marks.length && marks[i + 1].boundary >= 0 ? marks[i + 1].boundary : text.length;
      out[marks[i].key] = text.slice(marks[i].valueStart, end);
    }
    return out;
  }

  function parseColor(value, fallback) {
    const parts = String(value || '').split(',').map(Number);
    if (parts.length < 3 || parts.some(Number.isNaN)) return fallback;
    return parts;
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
      // 场景静态 UI：`UiLayout` 给出屏幕矩形（设计分辨率 1280x720），
      // `Image`/`Text` 给出要画的内容。菜单、HUD 底图、按钮都走这条路径，
      // 不是 Lua 每帧的 `ui.*` 调用。
      if (entity.components.UiLayout) {
        const fields = parseComponentFields(entity.components.UiLayout);
        const offsets = String(fields.offsets || '').split(',').map(Number);
        if (offsets.length === 4 && offsets.every(value => !Number.isNaN(value))) {
          entity.uiRect = { x: offsets[0], y: offsets[1], w: offsets[2] - offsets[0], h: offsets[3] - offsets[1] };
        }
      }
      if (entity.components.Image) {
        const fields = parseComponentFields(entity.components.Image);
        entity.image = {
          texture: fields.texture || '',
          color: parseColor(fields.color, [1, 1, 1]),
          nineSlice: fields.nine_slice ? String(fields.nine_slice).split(',').map(Number) : null
        };
      }
      if (entity.components.Text) {
        const fields = parseComponentFields(entity.components.Text);
        entity.text = {
          value: fields.text || '',
          size: Number(fields.font_size) || 24,
          color: parseColor(fields.color, [1, 1, 1])
        };
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
      if (entity.image && entity.image.texture) paths.add(entity.image.texture);
      if (entity.animation && entity.animation.clips) Object.values(entity.animation.clips).forEach(clip => { if (clip.texture) paths.add(clip.texture); });
      if (entity.tilemap && entity.tilemap.texture) paths.add(entity.tilemap.texture);
    }
    await Promise.all(Array.from(paths).map(async path => images.set(path, await loadImage(path))));
    return images;
  }

  // 贴图按需加载：首帧可能还没解出来，返回 null 并登记请求，后续帧会命中缓存。
  function ensureTexture(images, pending, path) {
    if (!path) return null;
    const cached = images.get(path);
    if (cached) return cached;
    if (!pending.has(path)) {
      pending.add(path);
      loadImage(path).then(result => {
        pending.delete(path);
        if (result) images.set(path, result);
      });
    }
    return null;
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

  let GAME_FONT = 'sans-serif';

  // 游戏在 fonts/ 下随包分发中文 TTF；这里用 FontFace 注册，保证 HUD/菜单中文可见。
  async function loadGameFont() {
    const url = (typeof window !== 'undefined' && window.__P2_FONT) || '';
    if (!url || typeof FontFace !== 'function' || !document.fonts) return;
    try {
      const face = new FontFace('RustLuaGameFont', 'url("' + url + '")');
      await face.load();
      document.fonts.add(face);
      GAME_FONT = 'RustLuaGameFont, sans-serif';
    } catch (error) {
      console.warn('[lua] game font could not be loaded:', url, error);
    }
  }

  // `ui.*` 是游戏的主要绘制接口（面板/文本/图片/进度条）。Lua 侧把每帧的 UI 状态
  // 序列化成制表符分隔的行，这里解析后画到 2D canvas 上。
  function parseUiLines(text) {
    const panels = [];
    const images = [];
    const bars = [];
    const texts = [];
    for (const line of String(text || '').split(/\r?\n/)) {
      if (!line) continue;
      const parts = line.split('\t');
      if (parts[0] === 'P') {
        panels.push({ id: parts[1], x: +parts[2], y: +parts[3], w: +parts[4], h: +parts[5], r: +parts[6], g: +parts[7], b: +parts[8], a: parts[9] === undefined ? 1 : +parts[9] });
      } else if (parts[0] === 'I') {
        images.push({ id: parts[1], x: +parts[2], y: +parts[3], w: +parts[4], h: +parts[5], texture: parts[6] || '', r: +parts[7], g: +parts[8], b: +parts[9] });
      } else if (parts[0] === 'B') {
        bars.push({ id: parts[1], x: +parts[2], y: +parts[3], w: +parts[4], h: +parts[5], ratio: +parts[6], r: +parts[7], g: +parts[8], b: +parts[9] });
      } else if (parts[0] === 'X') {
        texts.push({ id: parts[1], x: +parts[2], y: +parts[3], size: +parts[4], scale: +parts[5], text: parts.slice(6).join('\t') });
      }
    }
    return { panels, images, bars, texts };
  }

  function uiColor(r, g, b, a) {
    const clamp = value => Math.max(0, Math.min(255, Math.round((Number(value) || 0) * 255)));
    const alpha = a === undefined || Number.isNaN(a) ? 1 : Math.max(0, Math.min(1, a));
    return 'rgba(' + clamp(r) + ',' + clamp(g) + ',' + clamp(b) + ',' + alpha + ')';
  }

  // 画面自检：采样 canvas 像素，统计“非背景色”的采样点数量。用于回答
  // “页面启动了但没有画面”这类问题——`data-p2-boot=running` 只说明 Lua 在跑，
  // 不代表真的画出了东西。返回 -1 表示像素不可读（canvas 被跨域贴图污染）。
  function sampleDrawnPixels(ctx) {
    try {
      const width = ctx.canvas.width;
      const height = ctx.canvas.height;
      const data = ctx.getImageData(0, 0, width, height).data;
      let painted = 0;
      for (let i = 0; i < data.length; i += 4 * 53) {
        if (Math.abs(data[i] - 0x15) > 8 || Math.abs(data[i + 1] - 0x19) > 8 || Math.abs(data[i + 2] - 0x23) > 8) painted += 1;
      }
      return painted;
    } catch (error) {
      return -1;
    }
  }

  let LAST_UI_SUMMARY = '';

  function drawUi(ctx, uiText, images, pending) {
    const ui = parseUiLines(uiText);
    LAST_UI_SUMMARY = ui.panels.map(item => 'P:' + item.id)
      .concat(ui.images.map(item => 'I:' + item.id + ':' + item.texture), ui.bars.map(item => 'B:' + item.id), ui.texts.map(item => 'X:' + item.id))
      .join(',');
    for (const panel of ui.panels) {
      ctx.fillStyle = uiColor(panel.r, panel.g, panel.b, panel.a);
      ctx.fillRect(panel.x, panel.y, panel.w, panel.h);
    }
    for (const image of ui.images) {
      const texture = ensureTexture(images, pending, image.texture);
      if (!texture) continue;
      ctx.drawImage(texture, image.x, image.y, image.w, image.h);
    }
    for (const bar of ui.bars) {
      ctx.fillStyle = uiColor(bar.r, bar.g, bar.b, 1);
      ctx.fillRect(bar.x, bar.y, Math.max(0, bar.w * bar.ratio), bar.h);
    }
    for (const text of ui.texts) {
      const size = text.size > 0 ? text.size : 20;
      const scale = text.scale > 0 ? text.scale : 1;
      ctx.font = (size * scale) + 'px ' + GAME_FONT;
      ctx.textBaseline = 'top';
      ctx.fillStyle = '#ffffff';
      ctx.fillText(text.text, text.x, text.y);
    }
    return ui.panels.length + ui.images.length + ui.bars.length + ui.texts.length;
  }

  function colorCss(color, alpha) {
    const r = Math.max(0, Math.min(255, Math.round((color[0] || 0) * 255)));
    const g = Math.max(0, Math.min(255, Math.round((color[1] || 0) * 255)));
    const b = Math.max(0, Math.min(255, Math.round((color[2] || 0) * 255)));
    const a = alpha === undefined ? 1 : Math.max(0, Math.min(1, alpha));
    return 'rgba(' + r + ',' + g + ',' + b + ',' + a + ')';
  }

  // 场景静态 UI 实体（UiLayout + Image/Text）。九宫格按目标矩形拉伸绘制：
  // 发布包的按钮/条框都是规则矩形，拉伸的视觉误差可接受。
  function drawSceneUiEntity(ctx, entity, images, pending) {
    const rect = entity.uiRect;
    if (!rect || rect.w <= 0 || rect.h <= 0) return false;
    let drew = false;
    if (entity.image && entity.image.texture) {
      const texture = ensureTexture(images, pending, entity.image.texture);
      if (texture) {
        ctx.globalAlpha = entity.image.color[3] === undefined ? 1 : entity.image.color[3];
        ctx.drawImage(texture, rect.x, rect.y, rect.w, rect.h);
        ctx.globalAlpha = 1;
        drew = true;
      }
    }
    if (entity.text && entity.text.value) {
      ctx.font = entity.text.size + 'px ' + GAME_FONT;
      ctx.textBaseline = 'top';
      ctx.fillStyle = colorCss(entity.text.color);
      ctx.fillText(entity.text.value, rect.x, rect.y);
      drew = true;
    }
    return drew;
  }

  function drawScene(ctx, sceneState, images, projectName, uiText, pending) {
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

    const ordered = Array.from(sceneState.entities.values()).sort((a, b) => a.id - b.id);
    for (const entity of ordered) {
      if (!entity.alive) continue;
      // 场景静态 UI（菜单底图/按钮/标签）按 UiLayout 的屏幕矩形绘制。
      if (entity.uiRect) {
        drawSceneUiEntity(ctx, entity, images, pending);
        continue;
      }
      if (entity.tilemap) continue;
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

    if (typeof window !== 'undefined' && window.__P2_DEBUG) {
      ctx.fillStyle = '#d0d7e2';
      ctx.font = '14px sans-serif';
      ctx.fillText('Project: ' + projectName + ' | Scene: ' + sceneState.name + ' | Lua update running', 12, canvas.height - 16);
    }
    return drawUi(ctx, uiText, images, pending);
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
__p2_ui_panels = {}
__p2_ui_texts = {}
__p2_ui_bars = {}
__p2_ui_images = {}
ui = {}
function ui.panel(id, x, y, w, h, r, g, b, a)
  __p2_ui_panels[id] = { x = x, y = y, w = w, h = h, r = r, g = g, b = b, a = a, visible = true }
end
function ui.image(id, x, y, w, h, texture, r, g, b)
  __p2_ui_images[id] = { x = x, y = y, w = w, h = h, texture = tostring(texture or ''), r = r or 1, g = g or 1, b = b or 1, visible = true }
end
function ui.bar(id, x, y, w, h, ratio, r, g, b)
  local value = tonumber(ratio) or 0
  if value < 0 then value = 0 elseif value > 1 then value = 1 end
  __p2_ui_bars[id] = { x = x, y = y, w = w, h = h, ratio = value, r = r or 1, g = g or 1, b = b or 1, visible = true }
end
function ui.text_at(id, x, y, text, scale, size)
  __p2_ui_texts[id] = { x = x, y = y, text = tostring(text or ''), scale = scale or 1, size = size, visible = true }
end
function ui.text(id, text)
  __p2_ui_texts[id] = { x = 10, y = 10, text = tostring(text or ''), scale = 1, visible = true }
end
function ui.hide(id)
  for _, store in ipairs({ __p2_ui_panels, __p2_ui_texts, __p2_ui_bars, __p2_ui_images }) do
    local entry = store[id]
    if entry then entry.visible = false end
  end
end
function ui.show(id)
  for _, store in ipairs({ __p2_ui_panels, __p2_ui_texts, __p2_ui_bars, __p2_ui_images }) do
    local entry = store[id]
    if entry then entry.visible = true end
  end
end
function ui.clear(id)
  for _, store in ipairs({ __p2_ui_panels, __p2_ui_texts, __p2_ui_bars, __p2_ui_images }) do
    store[id] = nil
    store[id .. ':title'] = nil
    store[id .. ':tooltip'] = nil
  end
end
function ui.panel_get(id) return __p2_ui_panels[id] end
function ui.image_get(id) return __p2_ui_images[id] end
function ui.get(id) return __p2_ui_texts[id] end
function ui.grid(...) return true end
function ui.measure_text(text, size)
  local s = tostring(text or '')
  local count = 0
  for _ in s:gmatch('[^\\128-\\191]') do count = count + 1 end
  local px = tonumber(size) or 16
  return { width = count * px * 0.6, height = px }
end
function __p2_export_ui()
  local rows = {}
  local function num(v) return tostring(v or 0) end
  local function flat(v) return string.gsub(tostring(v or ''), '[\\t\\r\\n]', ' ') end
  for id, p in pairs(__p2_ui_panels) do
    if p.visible then
      rows[#rows + 1] = 'P\\t' .. flat(id) .. '\\t' .. num(p.x) .. '\\t' .. num(p.y) .. '\\t' .. num(p.w) .. '\\t' .. num(p.h) .. '\\t' .. num(p.r) .. '\\t' .. num(p.g) .. '\\t' .. num(p.b) .. '\\t' .. num(p.a)
    end
  end
  for id, i in pairs(__p2_ui_images) do
    if i.visible then
      rows[#rows + 1] = 'I\\t' .. flat(id) .. '\\t' .. num(i.x) .. '\\t' .. num(i.y) .. '\\t' .. num(i.w) .. '\\t' .. num(i.h) .. '\\t' .. flat(i.texture) .. '\\t' .. num(i.r) .. '\\t' .. num(i.g) .. '\\t' .. num(i.b)
    end
  end
  for id, b in pairs(__p2_ui_bars) do
    if b.visible then
      rows[#rows + 1] = 'B\\t' .. flat(id) .. '\\t' .. num(b.x) .. '\\t' .. num(b.y) .. '\\t' .. num(b.w) .. '\\t' .. num(b.h) .. '\\t' .. num(b.ratio) .. '\\t' .. num(b.r) .. '\\t' .. num(b.g) .. '\\t' .. num(b.b)
    end
  end
  for id, t in pairs(__p2_ui_texts) do
    if t.visible then
      rows[#rows + 1] = 'X\\t' .. flat(id) .. '\\t' .. num(t.x) .. '\\t' .. num(t.y) .. '\\t' .. num(t.size) .. '\\t' .. num(t.scale) .. '\\t' .. flat(t.text)
    end
  end
  return table.concat(rows, '\\n')
end
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
      const hostCompat = ENGINE_HOST_GLOBALS.map(name => `if ${name} == nil then ${name} = {} end`).join('\n');
      const methodCompat = `
item = item or {}
local definitions = {}
function item.define(id, definition)
  assert(type(id) == 'string' and id ~= '', 'item.define id must not be empty')
  assert(type(definition) == 'table', 'item.define expects a table')
  definitions[id] = {
    id=id, name=definition.name or id, description=definition.description or '',
    kind=definition.kind or '', texture=definition.texture or '',
    stack=math.max(1, definition.stack or 99), buy=definition.buy or 0, sell=definition.sell or 0
  }
  return true
end
function item.def(id)
  local definition = definitions[id]
  if not definition then return nil end
  local result = {}
  for key, value in pairs(definition) do result[key] = value end
  return result
end
for _, api in ipairs({audio, input}) do
  if not api.load_settings then
    api.load_settings = function() return false, 'WebGL settings backend unavailable; defaults retained' end
  end
  if not api.save_settings then
    api.save_settings = function() return false, 'WebGL settings persistence unavailable' end
  end
end
`;
      const genericCompat = `
local rate, currentDay, minute = 0, 1, 0
function clock.set_rate(value) rate = assert(tonumber(value)) end
function clock.set(dayValue, minuteValue) currentDay = dayValue; minute = minuteValue end
function clock.now() return {day=currentDay, minute=minute, minute_of_day=minute} end
local bindings = {confirm={'enter','space'},cancel={'escape'},move_up={'w','up'},move_down={'s','down'},move_left={'a','left'},move_right={'d','right'}}
function input.bindings(action) local result = {}; for i,k in ipairs(bindings[action] or {}) do result[i]=k end; return result end
function input.action_pressed(action)
  for _,key in ipairs(bindings[action] or {}) do
    if __p2_input and __p2_input.just and __p2_input.just[key] then return true end
  end
  return false
end
`;
      doString(L, hostCompat, 'engine-host-compat');
      doString(L, luaPrelude(sceneState) + '\n' + preload, 'engine-host-prelude');
      doString(L, methodCompat + '\n' + genericCompat, 'engine-host-method-compat');
      doString(L, main, mainScript);
    } catch (error) {
      const missing = missingHostGlobals(L);
      const detail = 'missing engine host API: ' + missing.join(', ');
      markBoot('failed', detail);
      throw new Error(String((error && error.message) || error) + ' [' + detail + ']');
    }
    callGlobal(L, 'on_create');
    const pendingTextures = new Set();
    await loadGameFont();
    let uiElements = drawScene(ctx, sceneState, images, projectName, callGlobal(L, '__p2_export_ui'), pendingTextures);
    let frameCount = 0;
    function frame() {
      try {
        doString(L, '__p2_input = ' + luaValue(webInput.snapshot()), '__p2_input');
        const result = callGlobal(L, 'update');
        syncLuaScene(sceneState, callGlobal(L, '__p2_export_state'));
        uiElements = drawScene(ctx, sceneState, images, projectName, callGlobal(L, '__p2_export_ui'), pendingTextures);
        frameCount += 1;
        if (frameCount <= 5 || frameCount % 30 === 0) {
          const body = document.body || document.documentElement;
          if (body) {
            body.setAttribute('data-p2-drawn', String(sampleDrawnPixels(ctx)));
            body.setAttribute('data-p2-ui-elements', String(uiElements));
            body.setAttribute('data-p2-ui-summary', String(LAST_UI_SUMMARY).slice(0, 400));
          }
        }
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
