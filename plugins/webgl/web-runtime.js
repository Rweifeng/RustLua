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
    // declared = 场景文件里**声明**的实体数。运行期 entity.spawn 会往 entities 里加人，
    // 所以这个数要单独留存，否则"运行期生成实体"这条断言永远自洽（实测踩过）。
    const declared = entities.size;
    for (const entity of entities.values()) applyComponents(entity);
    return { name, entities, declared };
  }

  // 组件串 → 实体字段。**parseScene 与运行期 `sprite.set` / `animation.attach` 共用这一份**：
  // 引擎里这两条 API 收的正是同一种组件串（`Sprite=..` / `Animation=..`，见
  // core/script.rs 的 sprite.set 与 animation.attach），各写一份解析必然漂移。
  // 运行期路径把 spec 塞回 `entity.components` 再调这里，行为与场景声明完全一致。
  function applyComponents(entity) {
    if (entity.components.Transform) {
      const p = parseProps(entity.components.Transform);
      entity.transform = { x: Number(p.x || 0), y: Number(p.y || 0), rotation: Number(p.rotation || 0), scale_x: Number(p.scale_x || 1), scale_y: Number(p.scale_y || 1) };
    }
    if (entity.components.Tilemap) {
      const p = parseProps(entity.components.Tilemap);
      const tileSize = String(p.tile_size || '32x32').split('x').map(Number);
      const previous = entity.tilemap || {};
      entity.tilemap = {
        tile_size: tileSize[0] || 32,
        width: Number(p.width || previous.width || 0),
        height: Number(p.height || previous.height || 0),
        texture: p.texture || previous.texture || '',
        tiles: String(p.tiles !== undefined ? p.tiles : (previous.tiles || '')),
        solid: String(p.solid !== undefined ? p.solid : (previous.solid || '')),
      };
    }
    if (entity.components.Animation) {
      // clip 字段顺序是 **name:sheet:cols:fps:looped**（引擎 AnimationClip::from_component_str），
      // 第 4 个字段是**帧率**，不是帧数。早先按 "cols:frames:looping" 解析，把 4 列 5fps 的
      // 条带当成 5 帧播，源矩形会越过首行取到别的动作的帧（主角/NPC 走路动画整体错位）。
      // 帧序号由 Lua 侧按引擎 current_frame 的算法（floor(time*fps)，循环取模/单次夹紧）
      // 算好后经 A 行回写，这里只保留条带布局。
      const p = parseProps(entity.components.Animation);
      const clips = {};
      for (const part of String(entity.components.Animation).split(';')) {
        if (!part.startsWith('clip:')) continue;
        const fields = part.slice(5).split(':');
        if (fields.length < 5) continue;
        clips[fields[0]] = { sheet: fields[1] || '', cols: Math.max(1, Number(fields[2]) || 1), fps: Number(fields[3]) || 8, looping: fields[4] === 'true' };
      }
      entity.animation = { current: p.default || 'idle', time: 0, ended: false, clips };
    }
    if (entity.components.Sprite) {
      // 场景里 Sprite 写成 `texture=...;size=224x224;pivot=...`：取字段必须走
      // parseProps，不能拿 `parts[0]`——那样贴图路径会带上 `texture=` 前缀
      // （实测：`loadImage('pack/texture=assets/art/buildings/manor_house.png')` 全 404），
      // 尺寸也会被 `Number('224x224')` 变成 NaN 退化成 32 的方图。
      const p = parseProps(entity.components.Sprite);
      const size = String(p.size || '').split('x').map(Number);
      const width = size[0] > 0 ? size[0] : 32;
      const height = size[1] > 0 ? size[1] : width;
      entity.sprite = { texture: p.texture || '', size: width, size_y: height };
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
      if (entity.animation && entity.animation.clips) Object.values(entity.animation.clips).forEach(clip => { if (clip.sheet) paths.add(clip.sheet); });
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

  // Lua → 渲染器的状态回写。行格式（Lua 侧 __p2_export_state 生成）：
  //   R  spawn（运行期实体，渲染器必须先认识它）
  //   S  sprite.set（Sprite 组件串）        X  animation.attach（Animation 组件串）
  //   C  tilemap.set_cell                   U  tilemap.set_texture
  //   D  destroy                            T  Transform
  //   A  动画当前 clip + **帧序号**（不是累计 tick，引擎 current_frame 的算法在 Lua 侧）
  // 结构行（R/S/X/C/U/D）由 op 列表一次性发出，T/A 每帧对存活实体重发。
  function syncLuaScene(sceneState, text) {
    for (const line of String(text || '').split(/\r?\n/)) {
      if (!line) continue;
      const parts = line.split('\t');
      const id = Number(parts[1]);
      if (parts[0] === 'R') {
        // 运行期实体：entity.spawn 只建实体不建外观，后续的 S/X/T 行才把它画出来。
        const existing = sceneState.entities.get(id);
        if (existing) {
          existing.alive = true;
          if (parts[2]) existing.name = parts[2];
        } else {
          sceneState.entities.set(id, {
            id,
            name: parts[2] || '',
            alive: true,
            components: {},
            transform: { x: 0, y: 0, rotation: 0, scale_x: 1, scale_y: 1 },
          });
        }
        continue;
      }
      const entity = sceneState.entities.get(id);
      if (!entity) continue;
      if (parts[0] === 'T') {
        entity.transform = { x: Number(parts[2] || 0), y: Number(parts[3] || 0), rotation: Number(parts[4] || 0), scale_x: Number(parts[5] || 1), scale_y: Number(parts[6] || 1) };
      } else if (parts[0] === 'A') {
        const previous = entity.animation || { clips: {} };
        entity.animation = { current: parts[2] || 'idle', frame: Number(parts[3] || 0), clips: previous.clips || {} };
      } else if (parts[0] === 'S') {
        entity.components.Sprite = parts.slice(2).join('\t');
        applyComponents(entity);
      } else if (parts[0] === 'X') {
        entity.components.Animation = parts.slice(2).join('\t');
        applyComponents(entity);
      } else if (parts[0] === 'C') {
        // 单字符编码与引擎一致（Tilemap::to_component_str 的 char::from(b'0' + cell)）。
        const map = entity.tilemap;
        const x = Number(parts[2]);
        const y = Number(parts[3]);
        if (!map || !Number.isFinite(x) || !Number.isFinite(y)) continue;
        const index = y * map.width + x;
        if (index < 0 || index >= map.tiles.length) continue;
        map.tiles = map.tiles.slice(0, index) + String.fromCharCode(48 + (Number(parts[4]) || 0)) + map.tiles.slice(index + 1);
      } else if (parts[0] === 'U') {
        if (entity.tilemap) entity.tilemap.texture = parts.slice(2).join('\t');
      } else if (parts[0] === 'D') {
        entity.alive = false;
      }
    }
  }

  // 渲染器**自己**这一侧的实体位置（不是 Lua 侧）：世界摘要里的 `drawn=` 用它。
  // 为什么必须单独报一条：Lua 侧的世界表与渲染器的实体表是两份数据，靠回写通道
  // （T 行）同步。只报 Lua 侧的话，"游戏世界里主角在走、画面上的主角不动"这类
  // 回写断链完全看不见——两份都报，验收就能断言它们一致。
  function rendererPlayerState(sceneState) {
    for (const entity of sceneState.entities.values()) {
      if (entity.alive === false) continue;
      const name = String(entity.name || '');
      if (!name.toLowerCase().includes('player')) continue;
      const t = entity.transform || { x: 0, y: 0 };
      return name + '@' + Math.round(t.x) + ',' + Math.round(t.y);
    }
    return '';
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
      // 地表贴图可能被 Lua 的 tilemap.set_texture 换掉（换季/换图），不能只靠
      // 场景载入时预加载的那一张，走按需加载（首帧未解出时先退化成纯色格）。
      const tileset = ensureTexture(images, pending, map.texture);
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
      // 非方形贴图（场景里 size 写成 `160x128`）：源帧仍按方格切，目标矩形按实际高画。
      const sizeY = entity.sprite && entity.sprite.size_y ? entity.sprite.size_y : size;
      const clip = entity.animation && entity.animation.clips ? entity.animation.clips[entity.animation.current] : null;
      // 运行期挂上去的形象（sprite.set / animation.attach）不在场景预加载清单里，
      // 必须走按需加载，否则 NPC/敌人只会画成一个色块圆点。
      const sprite = clip ? ensureTexture(images, pending, clip.sheet) : (entity.sprite ? ensureTexture(images, pending, entity.sprite.texture) : null);
      if (sprite && clip) {
        // frame 是**帧序号**（Lua 侧按引擎 current_frame 算好回写），不是累计 tick：
        // 引擎的 current_frame_uv 只取条带首行（frame/cols, 0），所以 sy 恒为 0。
        const index = Math.max(0, Math.floor(Number(entity.animation.frame) || 0));
        const sx = (index % Math.max(1, clip.cols)) * size;
        ctx.drawImage(sprite, sx, 0, size, size, pos.x - size / 2, pos.y - sizeY / 2, size, sizeY);
      } else if (sprite) {
        ctx.drawImage(sprite, pos.x - size / 2, pos.y - sizeY / 2, size, sizeY);
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

  // 场景数据在 Lua 侧的绑定：`__p2_names/transforms/tilemaps/animations` 对应引擎里
  // "当前场景的实体表"，`__p2_alive` 对应实体存活集合。**启动与换场景共用同一段生成
  // 代码**——换场景时只重绑这四张表并重建存活集合，别的（存档 `__p2_save`、地图状态
  // `__p2_map_state`、`__map`/`__map:spawn`）都是引擎 `save_data` 里的持久数据，
  // 跨场景必须保留，与引擎一致。
  function sceneBindings(sceneState) {
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
__p2_transforms = ${luaValue(transforms)}
__p2_tilemaps = ${luaValue(tilemaps)}
__p2_animations = ${luaValue(animations)}
__p2_health = {}
__p2_velocity = {}
-- UI 注册表同样属于"当前场景"：标题菜单留下的面板/标签不能渗进下一张场景的第一帧
-- （main.lua 的 on_click 也专门 hide 一次，这里是运行时侧的兜底）。
__p2_ui_panels = {}
__p2_ui_texts = {}
__p2_ui_bars = {}
__p2_ui_images = {}
__p2_alive = {}
-- 运行期改写的渲染数据（syncLuaScene 每帧取走）：sprite.set / animation.attach /
-- tilemap.set_cell / tilemap.set_texture / entity.spawn。引擎换场景时整张 world 重建
-- （script.rs 的场景载入分支 world = World::new() 并清 names/persist/health），
-- 所以这些注册表也必须跟着场景走，否则新场景会带上上一张图的贴图、格子与持久 id 映射。
__p2_persist = {}
__p2_sprites = {}
-- 运行期**结构变更**（spawn/destroy/sprite.set/animation.attach/tilemap.set_cell/
-- set_texture）走一次性 op 列表：JS 每帧取走即清空。每帧状态（位置、动画帧序号）
-- 走 T/A 行实时回写，两者分开是刻意的——动画组件串一条就有几 KB，逐帧重发
-- 在 Fengari 里是纯粹的浪费。
__p2_ops = {}
function __p2_emit(row) __p2_ops[#__p2_ops + 1] = row end
for _, id in pairs(__p2_names) do __p2_alive[id] = true end`;
  }

  function luaPrelude(sceneState, knownScenes) {
    return `
${sceneBindings(sceneState)}
__p2_save = {}
__p2_input = { axis_x = 0, axis_y = 0, keys = {}, just = {}, mouse = {}, mouse_just = {}, cursor = {0, 0}, cursor_delta = {0, 0}, scroll = 0 }
log ={ info = function(v) print('[lua] ' .. tostring(v)) end, warn = function(v) print('[lua warn] ' .. tostring(v)) end, error = function(v) print('[lua error] ' .. tostring(v)) end, debug = function(v) print('[lua debug] ' .. tostring(v)) end }
time = { delta = function() return 1 / 60 end }
input = { axis = function(name) name = tostring(name or 'x'):lower(); if name == 'x' or name == 'horizontal' then return __p2_input.axis_x or 0 end; if name == 'y' or name == 'vertical' then return __p2_input.axis_y or 0 end; return 0 end, is_down = function(key) return __p2_input.keys[tostring(key):lower()] == true end, just_pressed = function(key) return __p2_input.just[tostring(key):lower()] == true end, mouse_is_down = function(button) return __p2_input.mouse[tostring(button or 0)] == true end, mouse_just_pressed = function(button) return __p2_input.mouse_just[tostring(button or 0)] == true end, cursor_position = function() return __p2_input.cursor or {0, 0} end, cursor_delta = function() return __p2_input.cursor_delta or {0, 0} end, scroll_delta = function() return __p2_input.scroll or 0 end }
-- 场景与地图宿主 API。引擎事实（core/script.rs 的 scene.load / map.goto）：
--   * 切换是**排队的**：写 save_data["__map"]、记录 __map:spawn:x/y、把名字排进
--     scene_pending_load；真正换场景由随后的 process_scene_transition 完成，
--     然后全局 on_create 在**新场景**上重跑。所以这里也只排队、不改名字。
--   * 目标场景存在才切换（引擎按 scenes/<name>.scene 是否存在判定），不存在
--     返回 false 且不改任何状态。
--   * map.state(name[, value]) 是**地图作用域的持久键**（引擎侧落在
--     save_data["__mapstate:<name>"]）：读缺失键返回 nil，写返回 true。
-- 此前这里是 load = function(name) __p2_scene_name = name end 加一张空表 map
-- （hostCompat 留下的）：改名不换场景，update() 当帧就走"已在玩法地图"分支，
-- 而新场景的实体/瓦片从未加载；map.current() 则直接崩在空表上
-- （实测：标题界面点「开始新游戏」后第 5 帧 hud 报 attempt to call a nil value）。
__p2_scene_known = ${luaValue(knownScenes || {})}
__p2_scene_pending = nil
__p2_map_current = nil
__p2_map_spawn = nil
__p2_map_state = {}

function __p2_scene_request(name)
  name = tostring(name or '')
  if name == '' or not __p2_scene_known[name] then return false end
  __p2_scene_pending = name
  return true
end

-- 运行时每帧取一次待切换场景（取走即清空，与引擎"只应用一次"一致）。
function __p2_take_pending_scene()
  local name = __p2_scene_pending
  __p2_scene_pending = nil
  return name
end

scene = {
  reload = function() return __p2_scene_name end,
  load = function(name) return __p2_scene_request(name) end
}

-- 网格换算宿主 API（引擎 core/grid.rs）：世界坐标 ↔ 格子坐标、朝向格、越界判定。
-- 返回形状与引擎一致：world_to_cell/facing_cell 是**数组** {cx, cy}（调用侧写 cell[1], cell[2]），
-- cell_to_world 是 {x, y}（向量）。tile_size 必须为正有限值（引擎会报错，这里同样报）。
function __p2_tile_size(value)
  local size = tonumber(value)
  if not size or size ~= size or size == math.huge or size == -math.huge or size <= 0 then
    error('tile_size must be a positive finite number', 2)
  end
  return size
end
__p2_facing_step = { up = { 0, 1 }, n = { 0, 1 }, north = { 0, 1 }, down = { 0, -1 }, s = { 0, -1 }, south = { 0, -1 }, left = { -1, 0 }, w = { -1, 0 }, west = { -1, 0 }, right = { 1, 0 }, e = { 1, 0 }, east = { 1, 0 } }
grid = {
  world_to_cell = function(x, y, tile_size)
    local size = __p2_tile_size(tile_size)
    return { math.floor(x / size), math.floor(y / size) }
  end,
  cell_to_world = function(cx, cy, tile_size)
    local size = __p2_tile_size(tile_size)
    return { (cx + 0.5) * size, (cy + 0.5) * size }
  end,
  facing_cell = function(x, y, facing, tile_size)
    local size = __p2_tile_size(tile_size)
    -- 未知朝向回落 down（引擎 Facing::parse 的默认分支）
    local step = __p2_facing_step[tostring(facing or ''):lower():match('^%s*(.-)%s*$')] or { 0, -1 }
    return { math.floor(x / size) + step[1], math.floor(y / size) + step[2] }
  end,
  in_bounds = function(cx, cy, width, height)
    return cx >= 0 and cy >= 0 and cx < width and cy < height
  end
}

-- 库存宿主 API（引擎 core/script.rs 的 inv 表 + core/gameplay.rs 的 Inventory）。
-- 数据模型：**以字符串 id 为键**的库存注册表（游戏侧 state.inventory = "player"）；
-- 每个库存 = 定长槽位数组（空槽 item 为空串）+ 具名装备槽。加物品先填同类槽再占空槽、
-- 受堆叠上限钳制（上限取 item.define 的 stack，缺省 99）；移除从**后往前**扫。
-- 返回值语义逐条对齐引擎：add 返回装不下的剩余、remove/count 返回数量、
-- 目标库存不存在时 add 隐式建一个 24 格的（引擎如此，不报错）。
__p2_inventories = {}
function __p2_stack_limit(item_id)
  local definition = type(item) == 'table' and type(item.def) == 'function' and item.def(item_id) or nil
  local limit = definition and tonumber(definition.stack) or nil
  if not limit or limit < 1 then return 99 end
  return math.floor(limit)
end
function __p2_inventory_new(capacity)
  capacity = math.max(1, math.floor(tonumber(capacity) or 24))
  local slots = {}
  for index = 1, capacity do slots[index] = { item = '', count = 0 } end
  return { capacity = capacity, slots = slots, equipment = {} }
end
function __p2_inventory_add(inventory, item_id, count, limit)
  local remaining = count
  for _, slot in ipairs(inventory.slots) do
    if remaining == 0 then break end
    if slot.item == item_id then
      local moved = math.min(remaining, math.max(0, limit - slot.count))
      slot.count = slot.count + moved
      remaining = remaining - moved
    end
  end
  if remaining > 0 then
    for _, slot in ipairs(inventory.slots) do
      if remaining == 0 then break end
      if slot.item == '' then
        local moved = math.min(remaining, limit)
        slot.item = item_id
        slot.count = moved
        remaining = remaining - moved
      end
    end
  end
  return remaining
end
function __p2_inventory_remove(inventory, item_id, count)
  local remaining = count
  for index = #inventory.slots, 1, -1 do
    if remaining == 0 then break end
    local slot = inventory.slots[index]
    if slot.item == item_id then
      local removed = math.min(remaining, slot.count)
      slot.count = slot.count - removed
      remaining = remaining - removed
      if slot.count == 0 then slot.item = '' end
    end
  end
  return count - remaining
end
function __p2_inventory_get(id)
  local inventory = __p2_inventories[id]
  if not inventory then error('inventory ' .. tostring(id) .. ' not found', 2) end
  return inventory
end
function __p2_inventory_slot(inventory, index)
  index = math.floor(tonumber(index) or 0)
  if index <= 0 then error('inventory slot index must be positive', 3) end
  if index > inventory.capacity then error('inventory slot out of bounds', 3) end
  return inventory.slots[index]
end
function __p2_inventory_movable(source, target, requested, limit)
  if target.item == source.item then
    return math.min(math.max(0, limit - target.count), requested)
  elseif target.item == '' then
    return requested
  end
  return 0
end
inv = {
  create = function(id, capacity)
    id = tostring(id or '')
    if id == '' then error('inv.create id must not be empty', 2) end
    __p2_inventories[id] = __p2_inventory_new(capacity or 24)
    return id
  end,
  hotbar = function(id, size)
    id = tostring(id or '')
    if id == '' then error('inv.hotbar id must not be empty', 2) end
    __p2_inventories[id] = __p2_inventory_new(size or 9)
    return id
  end,
  capacity = function(id) return __p2_inventory_get(tostring(id)).capacity end,
  exists = function(id) return __p2_inventories[tostring(id)] ~= nil end,
  add = function(id, item_id, count)
    id, item_id = tostring(id), tostring(item_id or '')
    if item_id == '' then error('inv.add item id must not be empty', 2) end
    count = math.floor(tonumber(count) or 0)
    if count <= 0 then error('inv.add count must be positive', 2) end
    local inventory = __p2_inventories[id]
    if not inventory then
      inventory = __p2_inventory_new(24)
      __p2_inventories[id] = inventory
    end
    return __p2_inventory_add(inventory, item_id, count, __p2_stack_limit(item_id))
  end,
  remove = function(id, item_id, count)
    count = math.floor(tonumber(count) or 0)
    if count <= 0 then error('inv.remove count must be positive', 2) end
    local inventory = __p2_inventories[tostring(id)]
    if not inventory then return 0 end
    return __p2_inventory_remove(inventory, tostring(item_id), count)
  end,
  count = function(id, item_id)
    local inventory = __p2_inventories[tostring(id)]
    if not inventory then return 0 end
    local total = 0
    for _, slot in ipairs(inventory.slots) do
      if slot.item == tostring(item_id) then total = total + slot.count end
    end
    return total
  end,
  get = function(id)
    id = tostring(id)
    local inventory = __p2_inventory_get(id)
    local slots, occupied = {}, 0
    for index, slot in ipairs(inventory.slots) do
      if slot.item ~= '' then
        slots[index] = { item = slot.item, count = slot.count }
        occupied = occupied + 1
      end
    end
    return { id = id, capacity = inventory.capacity, slots = slots, occupied = occupied }
  end,
  slot = function(id, index)
    local slot = __p2_inventory_slot(__p2_inventory_get(tostring(id)), index)
    if slot.item == '' then return nil end
    return { item = slot.item, count = slot.count }
  end,
  set = function(id, index, item_id, count)
    local inventory = __p2_inventory_get(tostring(id))
    local slot = __p2_inventory_slot(inventory, index)
    count = math.floor(tonumber(count) or 0)
    if item_id ~= nil and tostring(item_id) ~= '' and count > 0 then
      slot.item = tostring(item_id)
      slot.count = count
    else
      slot.item, slot.count = '', 0
    end
  end,
  swap = function(from_id, from_index, to_id, to_index)
    local from = __p2_inventory_get(tostring(from_id))
    local left = __p2_inventory_slot(from, from_index)
    if tostring(from_id) == tostring(to_id) then
      local right = __p2_inventory_slot(from, to_index)
      left.item, right.item = right.item, left.item
      left.count, right.count = right.count, left.count
      return true
    end
    local to = __p2_inventory_get(tostring(to_id))
    local right = __p2_inventory_slot(to, to_index)
    left.item, right.item = right.item, left.item
    left.count, right.count = right.count, left.count
    return true
  end,
  move = function(from_id, from_index, to_id, to_index, count)
    local from = __p2_inventory_get(tostring(from_id))
    local source = __p2_inventory_slot(from, from_index)
    if source.item == '' then return 0 end
    local limit = __p2_stack_limit(source.item)
    local same = tostring(from_id) == tostring(to_id)
    if to_index == nil then
      -- 无目标格：退回"移出再放回"，净效果是重新堆叠，返回净移动量（引擎同义）。
      local requested = math.max(1, math.floor(tonumber(count) or source.count))
      local removed = __p2_inventory_remove(from, source.item, requested)
      local leftover = __p2_inventory_add(from, source.item, removed, limit)
      return removed - leftover
    end
    local to = same and from or __p2_inventory_get(tostring(to_id))
    local target = __p2_inventory_slot(to, to_index)
    local requested = math.max(0, math.min(math.floor(tonumber(count) or source.count), source.count))
    local moved = __p2_inventory_movable(source, target, requested, limit)
    if moved > 0 then
      source.count = source.count - moved
      if source.count == 0 then source.item = '' end
      if target.item == source.item then
        target.count = target.count + moved
      else
        target.item, target.count = source.item, moved
      end
      -- 同库存互换槽位时 source 与 target 可能是同一个槽（index 相同）：
      -- 上面的赋值顺序已按引擎逐句对应，不做额外特判。
    end
    return moved
  end,
  equip = function(id, slot, item_id)
    id, slot = tostring(id or ''), tostring(slot or '')
    if id == '' or slot == '' then error('inv.equip needs an inventory id and a non-empty slot', 2) end
    local inventory = __p2_inventory_get(id)
    local value = item_id ~= nil and tostring(item_id) or ''
    if value == '' then
      inventory.equipment[slot] = nil
    else
      inventory.equipment[slot] = value
    end
  end,
  equip_get = function(id, slot) return __p2_inventory_get(tostring(id)).equipment[tostring(slot)] end,
  equip_list = function(id)
    local result = {}
    for slot, item_id in pairs(__p2_inventory_get(tostring(id)).equipment) do result[slot] = item_id end
    return result
  end,
  -- 引擎的 inv.open 面向容器 UI，游戏未使用；保留同名空实现以免宿主 API 表缺项。
  open = function() end
}
map = {
  current = function() return __p2_map_current end,
  spawn = function() return __p2_map_spawn end,
  set_spawn = function(x, y) __p2_map_spawn = { x = x, y = y } end,
  -- goto 是 Lua 5.2+ 的保留字，只能以 map["goto"](...) 形式使用（游戏侧正是这么调的）。
  ["goto"] = function(name, x, y)
    if not __p2_scene_request(name) then return false end
    __p2_map_current = tostring(name)
    if x ~= nil and y ~= nil then __p2_map_spawn = { x = x, y = y } end
    return true
  end,
  enter = function(name, x, y) return map["goto"](name, x, y) end,
  state = function(name, value)
    name = tostring(name)
    if value == nil then return __p2_map_state[name] end
    __p2_map_state[name] = value
    return true
  end,
  state_clear = function(name)
    local removed = 0
    if name == nil then
      for _ in pairs(__p2_map_state) do removed = removed + 1 end
      __p2_map_state = {}
    elseif __p2_map_state[tostring(name)] ~= nil then
      __p2_map_state[tostring(name)] = nil
      removed = 1
    end
    return removed
  end,
  -- 引擎的 map.schema 是"写入前校验"；web 运行时不带 schema 校验器，接受即忽略。
  schema = function(_name, _spec) end
}
-- 存活校验（引擎 require_alive 的同构）：对未知/已销毁实体**报错**而不是静默改数据。
-- 静默 no-op 会让"实体早就没了"这类问题一路潜伏到画面异常才被发现。
function __p2_require_alive(id)
  if __p2_alive[id] ~= true then
    error('entity is not alive: ' .. tostring(id), 3)
  end
end

-- 实体 id 分配：取现有存活 id 之外的最小正整数（与引擎 world.spawn 的递增 id 形态一致）。
function __p2_next_entity_id()
  local id = 1
  for k in pairs(__p2_alive) do
    if type(k) == 'number' and k >= id then id = k + 1 end
  end
  return id
end

-- 运行期实体：entity.spawn(name?, persist_id?) 在引擎里是"新开一个只有 Transform 的实体"，
-- 渲染数据由随后的 sprite.set / animation.attach 补上，所以这里也必须让**渲染器**
-- 见到这个实体（只写 Lua 侧的表，画面里不会出现任何东西）。
-- persist_id 是跨场景重挂用的稳定键（NPC/敌人靠它避免重复生成）：find_id 查它、
-- destroy 清它，与引擎 core/script.rs 的 persist 表逐条对齐（含重复注册报错）。
entity = {
  find = function(name) return __p2_names[name] end,
  is_alive = function(id) return __p2_alive[id] == true end,
  find_id = function(persist_id) return __p2_persist[tostring(persist_id or '')] end,
  persist_id = function(id)
    for key, value in pairs(__p2_persist) do
      if value == id then return key end
    end
    return nil
  end,
  destroy = function(id)
    __p2_require_alive(id)
    __p2_alive[id] = false
    __p2_emit('D\t' .. id)
    for key, value in pairs(__p2_persist) do
      if value == id then __p2_persist[key] = nil end
    end
    return true
  end,
  spawn = function(name, persist_id)
    local key = persist_id ~= nil and tostring(persist_id) or ''
    local taken = key ~= '' and __p2_persist[key] or nil
    if taken ~= nil and __p2_alive[taken] == true then
      error('persist_id already in use: ' .. key, 2)
    end
    local id = __p2_next_entity_id()
    __p2_alive[id] = true
    __p2_transforms[id] = { x = 0, y = 0, rotation = 0, scale_x = 1, scale_y = 1 }
    if name ~= nil and name ~= '' then __p2_names[tostring(name)] = id end
    if key ~= '' then __p2_persist[key] = id end
    __p2_emit('R\t' .. id .. '\t' .. tostring(name or ''))
    return id
  end
}
transform = {
  get = function(id)
    local p = __p2_transforms[id]
    if not p then return nil end
    return { x = p.x or 0, y = p.y or 0, rotation = p.rotation or 0, scale_x = p.scale_x or 1, scale_y = p.scale_y or 1 }
  end,
  set = function(id, x, y, rotation, scale_x, scale_y)
    __p2_require_alive(id)
    local p = __p2_transforms[id] or { x = 0, y = 0, rotation = 0, scale_x = 1, scale_y = 1 }
    p.x, p.y = tonumber(x) or 0, tonumber(y) or 0
    if rotation ~= nil then p.rotation = tonumber(rotation) or 0 end
    if scale_x ~= nil then p.scale_x = tonumber(scale_x) or 1 end
    if scale_y ~= nil then p.scale_y = tonumber(scale_y) or 1 end
    __p2_transforms[id] = p
    return true
  end,
  move = function(id, dx, dy)
    __p2_require_alive(id)
    local p = __p2_transforms[id] or { x = 0, y = 0, rotation = 0, scale_x = 1, scale_y = 1 }
    p.x = (p.x or 0) + (tonumber(dx) or 0)
    p.y = (p.y or 0) + (tonumber(dy) or 0)
    __p2_transforms[id] = p
    return true
  end
}
transform3d = { attach = function(_) return true end, get = function(_) return { x = 0, y = 0, z = 0 } end, set = function(...) return true end, move = function(...) return true end }
__p2_tilemap_of = function(id)
  local m = __p2_tilemaps[id]
  if not m then return nil end
  return m
end
__p2_cell_index = function(m, x, y)
  x, y = tonumber(x), tonumber(y)
  if not x or not y or x < 0 or y < 0 or x >= m.width or y >= m.height then return nil end
  return math.floor(y) * m.width + math.floor(x) + 1
end
tilemap = {
  -- 引擎的 tilemap.get 带 texture（season_visual 靠它判断"是否已是目标地表"，
  -- 少了这个字段就会每帧重复写组件、网格反复重建）。
  get = function(id)
    local m = __p2_tilemap_of(id)
    if not m then return nil end
    return { width = m.width, height = m.height, tile_size = m.tile_size, texture = m.texture }
  end,
  is_solid = function(id, x, y)
    local m = __p2_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local i = __p2_cell_index(m, x, y)
    if not i then return false end
    return string.sub(m.solid, i, i) == '1'
  end,
  get_cell = function(id, x, y)
    local m = __p2_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local i = __p2_cell_index(m, x, y)
    if not i then return 0 end
    return tonumber(string.sub(m.tiles, i, i)) or 0
  end,
  set_cell = function(id, x, y, value)
    local m = __p2_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local cx, cy = math.floor(tonumber(x) or -1), math.floor(tonumber(y) or -1)
    local i = __p2_cell_index(m, cx, cy)
    if not i then error('tilemap cell (' .. tostring(x) .. ', ' .. tostring(y) .. ') is out of bounds', 2) end
    -- Lua 侧自身也要跟着变：tilemap.get_cell 必须读到刚写进去的值（farm 依赖它
    -- 判断「这块地已经锄过了」，只发渲染 op 会让两边数据分叉）。
    -- 单字符编码与引擎一致（Tilemap::to_component_str 的 char::from(b'0' + cell)）。
    local code = math.floor(tonumber(value) or 0)
    m.tiles = string.sub(m.tiles, 1, i - 1) .. string.char(48 + code) .. string.sub(m.tiles, i + 1)
    __p2_emit('C\t' .. id .. '\t' .. cx .. '\t' .. cy .. '\t' .. tostring(value))
    return true
  end,
  set_texture = function(id, texture)
    local m = __p2_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local path = tostring(texture or '')
    if path:match('^%s*$') then error('tilemap texture must not be empty', 2) end
    m.texture = path
    __p2_emit('U\t' .. id .. '\t' .. path)
    return true
  end,
  -- A* 寻路（引擎 core/tilemap.rs 的 Map::find_path 同构）：返回 **1 基** 的
  -- {x=,y=} 格数组（含起终点），走不通返回 nil + 原因字符串。
  -- 开放集用二叉堆：Fengari 是 JS 里的解释器，线性取最小在 64x64 图上单次寻路
  -- 要百万级比较，NPC/敌人每帧重寻路会把帧时间吃光。
  find_path = function(id, from_x, from_y, to_x, to_y, diagonal)
    local m = __p2_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local sx, sy = math.floor(tonumber(from_x) or -1), math.floor(tonumber(from_y) or -1)
    local gx, gy = math.floor(tonumber(to_x) or -1), math.floor(tonumber(to_y) or -1)
    local function out_of_bounds(x, y)
      return type(x) ~= 'number' or type(y) ~= 'number' or x < 0 or y < 0 or x >= m.width or y >= m.height
    end
    -- 越界当作**不可走**（引擎走的是 index() 的 Result，这里等价表达）
    local function solid_at(x, y)
      if out_of_bounds(x, y) then return true end
      local i = y * m.width + x + 1
      return string.sub(m.solid, i, i) == '1'
    end
    if out_of_bounds(sx, sy) then return nil, 'path endpoint (' .. sx .. ', ' .. sy .. ') is out of bounds' end
    if out_of_bounds(gx, gy) then return nil, 'path endpoint (' .. gx .. ', ' .. gy .. ') is out of bounds' end
    if solid_at(sx, sy) then return nil, 'path endpoint (' .. sx .. ', ' .. sy .. ') is solid' end
    if solid_at(gx, gy) then return nil, 'path endpoint (' .. gx .. ', ' .. gy .. ') is solid' end
    if sx == gx and sy == gy then return { { x = sx, y = sy } } end
    local allow = diagonal == true
    local width = m.width
    local function heuristic(x, y)
      local dx, dy = math.abs(x - gx), math.abs(y - gy)
      if allow then return math.max(dx, dy) end
      return dx + dy
    end
    local best, came = {}, {}
    local heap, heap_size = {}, 0
    -- 小顶堆按 f；同 f 时 g 更大者优先（引擎 BinaryHeap 的 tie-break）
    local function heap_push(node)
      heap_size = heap_size + 1
      heap[heap_size] = node
      local i = heap_size
      while i > 1 do
        local parent = math.floor(i / 2)
        local a, b = heap[i], heap[parent]
        if a.f < b.f or (a.f == b.f and a.g > b.g) then
          heap[i], heap[parent] = heap[parent], heap[i]
          i = parent
        else
          break
        end
      end
    end
    local function heap_pop()
      local top = heap[1]
      heap[1] = heap[heap_size]
      heap[heap_size] = nil
      heap_size = heap_size - 1
      local i = 1
      while true do
        local left, right, pick = i * 2, i * 2 + 1, i
        if left <= heap_size and (heap[left].f < heap[pick].f or (heap[left].f == heap[pick].f and heap[left].g > heap[pick].g)) then pick = left end
        if right <= heap_size and (heap[right].f < heap[pick].f or (heap[right].f == heap[pick].f and heap[right].g > heap[pick].g)) then pick = right end
        if pick == i then break end
        heap[i], heap[pick] = heap[pick], heap[i]
        i = pick
      end
      return top
    end
    local start_index = sy * width + sx + 1
    local goal_index = gy * width + gx + 1
    best[start_index] = 0
    heap_push({ x = sx, y = sy, g = 0, f = heuristic(sx, sy) })
    while heap_size > 0 do
      local current = heap_pop()
      local current_index = current.y * width + current.x + 1
      if current.g <= (best[current_index] or math.huge) then
        if current_index == goal_index then
          local path = { { x = gx, y = gy } }
          local cursor = goal_index
          while cursor ~= start_index do
            local previous = came[cursor]
            if not previous then return nil, 'no path from (' .. sx .. ', ' .. sy .. ') to (' .. gx .. ', ' .. gy .. ')' end
            cursor = previous
            table.insert(path, 1, { x = (cursor - 1) % width, y = math.floor((cursor - 1) / width) })
          end
          return path
        end
        local west = not solid_at(current.x - 1, current.y)
        local east = not solid_at(current.x + 1, current.y)
        local north = not solid_at(current.x, current.y - 1)
        local south = not solid_at(current.x, current.y + 1)
        local neighbours = {}
        if west then neighbours[#neighbours + 1] = { current.x - 1, current.y } end
        if east then neighbours[#neighbours + 1] = { current.x + 1, current.y } end
        if north then neighbours[#neighbours + 1] = { current.x, current.y - 1 } end
        if south then neighbours[#neighbours + 1] = { current.x, current.y + 1 } end
        -- 斜向不允许贴角穿越（引擎 walkable_neighbors 要求两个正交邻格都可走）
        if allow then
          if west and north and not solid_at(current.x - 1, current.y - 1) then neighbours[#neighbours + 1] = { current.x - 1, current.y - 1 } end
          if east and north and not solid_at(current.x + 1, current.y - 1) then neighbours[#neighbours + 1] = { current.x + 1, current.y - 1 } end
          if west and south and not solid_at(current.x - 1, current.y + 1) then neighbours[#neighbours + 1] = { current.x - 1, current.y + 1 } end
          if east and south and not solid_at(current.x + 1, current.y + 1) then neighbours[#neighbours + 1] = { current.x + 1, current.y + 1 } end
        end
        for _, cell in ipairs(neighbours) do
          local tentative = current.g + 1
          local i = cell[2] * width + cell[1] + 1
          if tentative < (best[i] or math.huge) then
            best[i] = tentative
            came[i] = current_index
            heap_push({ x = cell[1], y = cell[2], g = tentative, f = tentative + heuristic(cell[1], cell[2]) })
          end
        end
      end
    end
    return nil, 'no path from (' .. sx .. ', ' .. sy .. ') to (' .. gx .. ', ' .. gy .. ')'
  end
}
health = { get = function(id) return __p2_health[id] or 100 end, set = function(id, hp) __p2_health[id] = hp end, damage = function(id, amount) __p2_health[id] = (health.get(id) or 100) - amount; return __p2_health[id] end }
physics = {
  set_velocity = function(id, x, y) __p2_velocity[id] = { x = x or 0, y = y or 0 }; return true end,
  -- 2D 传送（引擎 M17-01）：**刚体与 ECS 同步落位**，只改 transform 会被下一帧
  -- 物理回写覆盖成旧位置（player.teleport 的注释里就写着这条）。这里两者都写。
  set_position = function(id, x, y)
    local nx, ny = tonumber(x), tonumber(y)
    if not nx or not ny or nx ~= nx or ny ~= ny or nx == math.huge or nx == -math.huge or ny == math.huge or ny == -math.huge then
      error('physics.set_position must be finite', 2)
    end
    __p2_require_alive(id)
    local p = __p2_transforms[id] or { rotation = 0, scale_x = 1, scale_y = 1 }
    p.x, p.y = nx, ny
    __p2_transforms[id] = p
    __p2_velocity[id] = { x = 0, y = 0 }
    return true
  end,
  events = function() return {} end
}
physics3d = { set_velocity = function(...) return true end, set_position = function(...) return true end, events = function() return {} end }
-- 单帧贴图：入参是 Sprite **组件串**（texture=..;size=..），与场景里写的是同一种，
-- 解析交给渲染侧同一份 applyComponents（这里只登记，syncLuaScene 每帧取走）。
sprite = {
  set = function(id, component)
    __p2_require_alive(id)
    local spec = tostring(component or '')
    if spec:match('^%s*$') then error('sprite component must not be empty', 2) end
    __p2_sprites[id] = spec
    __p2_emit('S\t' .. id .. '\t' .. spec)
    return true
  end,
  get = function(id)
    __p2_require_alive(id)
    local spec = __p2_sprites[id]
    if not spec then return nil end
    local size = spec:match('size=([^;]+)') or '32x32'
    local w, h = size:match('^(%d+)x(%d+)$')
    return {
      texture = spec:match('texture=([^;]+)'),
      size_x = tonumber(w) or 32, size_y = tonumber(h) or tonumber(w) or 32
    }
  end
}
-- 序列帧：clip 定义与引擎 AnimationClip 同构（name:sheet:cols:fps:looped）。
-- 组件串在 Lua 侧解析一次（帧序号要按 fps 算，必须知道 cols/fps/looping）。
function __p2_parse_animation(spec)
  local machine = { current = 'idle', time = 0, ended = false, clips = {} }
  for field in tostring(spec or ''):gmatch('[^;]+') do
    local default = field:match('^default=(.*)$')
    if default then
      machine.current = default
    else
      local rest = field:match('^clip:(.*)$')
      if rest then
        local parts = {}
        for piece in (rest .. ':'):gmatch('([^:]*):') do parts[#parts + 1] = piece end
        if #parts >= 5 then
          machine.clips[parts[1]] = {
            sheet = parts[2], cols = math.max(1, tonumber(parts[3]) or 1),
            fps = tonumber(parts[4]) or 8, looping = parts[5] == 'true'
          }
        end
      end
    end
  end
  return machine
end
-- 步进一帧：引擎 AnimationStateMachine 的同构（time 累加、非循环播完置 ended）；
-- 帧序号用 current_frame 的算法：循环取模、单次夹紧到 cols-1。
function __p2_advance_animation(a, delta)
  if not a then return nil end
  local clip = a.clips and a.clips[a.current]
  if not clip then return nil end
  local fps = tonumber(clip.fps) or 1
  local cols = math.max(1, tonumber(clip.cols) or 1)
  if not a.ended then
    a.time = (a.time or 0) + delta
    if clip.looping ~= true and math.floor(a.time * fps) >= cols then a.ended = true end
  end
  local raw = math.floor((a.time or 0) * fps)
  local frame
  if clip.looping == true and not a.ended then
    frame = raw % cols
  else
    frame = math.min(raw, cols - 1)
  end
  a.frame = frame
  return frame
end
animation = {
  get = function(id)
    __p2_require_alive(id)
    local a = __p2_animations[id]
    if not a then error('entity has no animation', 2) end
    return a.current
  end,
  set = function(id, name)
    __p2_require_alive(id)
    local a = __p2_animations[id]
    if not a then error('entity has no animation', 2) end
    local clip = a.clips and a.clips[name]
    if not clip then error('unknown animation clip: ' .. tostring(name), 2) end
    a.current, a.time, a.ended, a.frame = name, 0, false, 0
    return true
  end,
  -- 与引擎一致：**读取并清除** ended 标志（take_ended 语义），不是常真。
  ended = function(id)
    __p2_require_alive(id)
    local a = __p2_animations[id]
    if not a then error('entity has no animation', 2) end
    local value = a.ended == true
    a.ended = false
    return value
  end,
  frame = function(id)
    __p2_require_alive(id)
    local a = __p2_animations[id]
    if not a then error('entity has no animation', 2) end
    return a.frame or 0
  end,
  -- 运行期挂动画（引擎 M21）：spawn 出来的实体只有 Transform，形象靠这里补上。
  attach = function(id, component)
    __p2_require_alive(id)
    local spec = tostring(component or '')
    local machine = __p2_parse_animation(spec)
    if next(machine.clips) == nil then
      error('animation component has no clip: ' .. spec, 2)
    end
    __p2_animations[id] = machine
    __p2_emit('X\t' .. id .. '\t' .. spec)
    return true
  end
}
animation3d = { set = function(...) return true end }
-- 音频宿主 API。web 平台没有音频后端（浏览器/小游戏侧都没有接入混音器），
-- 但**返回值语义必须与引擎一致**：audio.play_on 播放失败要返回 false + 原因，
-- 游戏侧 core/audio.lua 据此把该 bus:name 记进 failed 表、不再重试；
-- 若这里返回无意义的成功，游戏会每帧重试同一段缺资源的音效。
__p2_audio_buses = { master = true, bgm = true, sfx = true, ui = true }
__p2_audio_volume = { master = 1, bgm = 1, sfx = 1, ui = 1 }
audio = {
  volume = function(_) return true end,
  music = function(name) log.info('music ' .. tostring(name)) end,
  play = function(name) log.info('sound ' .. tostring(name)); return true end,
  set_bus_volume = function(bus, level)
    bus = tostring(bus or '')
    if not __p2_audio_buses[bus] then error('unknown audio bus: ' .. bus, 2) end
    local value = tonumber(level)
    if not value then error('audio.set_bus_volume level must be a number', 2) end
    if value < 0 then value = 0 elseif value > 1 then value = 1 end
    __p2_audio_volume[bus] = value
    return true
  end,
  bus_volume = function(bus)
    bus = tostring(bus or '')
    if not __p2_audio_buses[bus] then error('unknown audio bus: ' .. bus, 2) end
    return __p2_audio_volume[bus]
  end,
  effective_volume = function(bus)
    bus = tostring(bus or '')
    if not __p2_audio_buses[bus] then error('unknown audio bus: ' .. bus, 2) end
    if bus == 'master' then return __p2_audio_volume.master end
    return __p2_audio_volume.master * __p2_audio_volume[bus]
  end,
  -- web 平台没有混音器：播放一律失败，返回 false + 原因（游戏侧会因此走静默降级，
  -- 与「缺音频资源」同一条路径，不会每帧重试）。
  play_on = function(bus, name)
    bus = tostring(bus or '')
    if not __p2_audio_buses[bus] then error('unknown audio bus: ' .. bus, 2) end
    if type(name) ~= 'string' or name == '' then return false, 'audio name must not be empty' end
    return false, 'web platform has no audio backend'
  end
}
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
save = {
  read = function(key) return __p2_save[key] end,
  write = function(key, value) __p2_save[key] = tostring(value) end,
  load = function() return true end,
  commit = function() return true end,
  -- 「开始新游戏」必须能丢弃引擎侧持久键：__map / __map:spawn:* / __mapstate:*。
  -- 游戏侧 state.reset() 只清 Lua 表，残留会让新档读回上一局的地图与地图状态
  -- （同进程内「继续 → 玩 → 返回标题 → 开始新游戏」即可复现，main.lua 的注释写着
  -- 顺序固定：先清引擎键，再让各模块写自己的空快照）。返回清掉的键数，与引擎一致。
  clear = function()
    local removed = 0
    for _ in pairs(__p2_save) do removed = removed + 1 end
    for _ in pairs(__p2_map_state) do removed = removed + 1 end
    __p2_save = {}
    __p2_map_state = {}
    __p2_map_current = nil
    __p2_map_spawn = nil
    log.info('cleared ' .. removed .. ' in-memory key(s)')
    return removed
  end
}
particles = { burst = function(...) return true end }
camera = { set = function(...) return true end, set_active = function(...) return true end }
mesh = { set = function(...) return true end }
os = os or {}; os.getenv = function(_) return nil end
-- 每帧把 Lua 侧的场景状态交给渲染器（syncLuaScene 解析）。
--   step=false 用于 on_create 之后的"只看不改"快照：不推进速度积分与动画帧，
--   否则一次切换会白白多走一帧动画。
-- 行格式：R=spawn（建实体） S=sprite.set（Sprite 组件串） X=animation.attach
--        C=set_cell  U=set_texture  D=destroy  T=Transform  A=动画当前 clip + 帧序号
-- R/D 这类结构变更只发一次（op 列表取走即清）；T/A 每帧对**存活**实体重新发。
function __p2_export_state(step)
  local rows = __p2_ops
  __p2_ops = {}
  local advance = step ~= false
  if advance then
    for id, v in pairs(__p2_velocity) do
      if __p2_alive[id] == true then
        local p = __p2_transforms[id] or { x = 0, y = 0 }
        p.x = (p.x or 0) + (v.x or 0) * time.delta()
        p.y = (p.y or 0) + (v.y or 0) * time.delta()
        __p2_transforms[id] = p
      end
    end
    for _, a in pairs(__p2_animations) do __p2_advance_animation(a, time.delta()) end
  end
  for id, p in pairs(__p2_transforms) do
    if __p2_alive[id] == true then
      rows[#rows + 1] = 'T\t' .. id .. '\t' .. tostring(p.x or 0) .. '\t' .. tostring(p.y or 0) .. '\t' .. tostring(p.rotation or 0) .. '\t' .. tostring(p.scale_x or 1) .. '\t' .. tostring(p.scale_y or 1)
    end
  end
  for id, a in pairs(__p2_animations) do
    if __p2_alive[id] == true then
      rows[#rows + 1] = 'A\t' .. id .. '\t' .. tostring(a.current or 'idle') .. '\t' .. tostring(a.frame or 0)
    end
  end
  return table.concat(rows, '\\n')
end

-- 一行世界自报，供宿主诊断覆盖层显示：真机/开发者工具里"看不见世界状态"是排查黑屏
-- 与"游戏在跑却没进图"的最大障碍。写法保持通用：不认工程名，主角按名字里含 player
-- 识别（渲染器 entityColor 判定主角用的是同一套启发）。
function __p2_world_summary()
  local alive, player_name, player_id = 0, nil, nil
  -- 两个 O(n) 遍历，不做嵌套：这个函数每帧都要出结果（宿主覆盖层与验收都要读实时值）。
  for _, value in pairs(__p2_alive) do
    if value == true then alive = alive + 1 end
  end
  for name, mapped in pairs(__p2_names) do
    if type(name) == 'string' and __p2_alive[mapped] == true and name:lower():find('player', 1, true) then
      player_name, player_id = name, mapped
    end
  end
  local parts = {
    'map=' .. tostring(__p2_map_current or __p2_scene_name),
    'spawn=' .. (__p2_map_spawn and (math.floor((__p2_map_spawn.x or 0) + 0.5) .. ',' .. math.floor((__p2_map_spawn.y or 0) + 0.5)) or 'none'),
    'alive=' .. alive
  }
  if player_id and __p2_transforms[player_id] then
    local p = __p2_transforms[player_id]
    local machine = __p2_animations[player_id]
    parts[#parts + 1] = 'player=' .. tostring(player_name)
      .. '@' .. math.floor((p.x or 0) + 0.5) .. ',' .. math.floor((p.y or 0) + 0.5)
      .. (machine and ('/' .. tostring(machine.current)) or '')
  end
  return table.concat(parts, ' ')
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

  // 引擎里存在、但 web 运行时**未实现**的宿主全局清单。
  //
  // 为什么必须有这份清单：下面的 `hostCompat` 会把名单里缺的全局换成"自报家门"的代理，
  // 而 `missingHostGlobals()` 用**同一份名单**判断"这条路径到底缺哪些 API"。
  // **不在名单里的缺失全局不会得到代理**——它保持 nil，脚本一碰就是
  // `attempt to index a nil value (global 'x')`，而诊断清单里**一个字都不提**它，
  // 于是根因看起来像"游戏脚本写错了"，而不是"这个平台没有这个 API"。
  //
  // `camera2d`（2026-09-23 补）：桌面引擎有、web/微信两边都没有。它此前**不**在名单里，
  // 于是第一个用到它的脚本会以"取 nil"的姿态炸掉，且 `missing-host-api` 通道保持沉默
  // （实测：`examples/game_jhyt` 的 `world/camera_view.lua` 在微信形态点「开始新游戏」后
  // 启动失败）。**给引擎新增宿主能力后，这里要一起想**：要么实现它，要么列进这份名单，
  // 让平台缺口在诊断里可见。
  const ENGINE_HOST_GLOBALS = [
    'animation', 'animation3d', 'app', 'asset', 'audio', 'calendar', 'camera', 'camera2d',
    'clock', 'container', 'data', 'day', 'entity', 'event', 'grid', 'health', 'input',
    'interact', 'inv', 'item', 'light', 'log', 'map', 'mesh', 'particles', 'physics',
    'physics3d', 'save', 'scene', 'shop', 'sprite', 'tag', 'tilemap', 'time', 'transform',
    'transform3d', 'ui'
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

  function callGlobal(L, name, args) {
    const f = window.fengari;
    f.lua.lua_getglobal(L, f.to_luastring(name));
    if (!f.lua.lua_isfunction(L, -1)) {
      f.lua.lua_pop(L, 1);
      return null;
    }
    const list = args || [];
    for (const value of list) {
      if (typeof value === 'boolean') f.lua.lua_pushboolean(L, value);
      else if (typeof value === 'number') f.lua.lua_pushnumber(L, value);
      else if (value === null || value === undefined) f.lua.lua_pushnil(L);
      else f.lua.lua_pushstring(L, f.to_luastring(String(value)));
    }
    if (f.lua.lua_pcall(L, list.length, 1, 0) !== f.lua.LUA_OK) {
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
    // 场景可切换 ⇒ 这两项必须可重新赋值（切图时整体换掉）。
    let sceneState = parseScene(sceneText);
    const canvas = document.getElementById('p2-canvas');
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('2D canvas context is not available');
    const webInput = createWebInput(canvas);
    let images = await loadSceneImages(sceneState);

    const manifest = await fetchText('pack/manifest.txt');
    // 可切换的场景清单来自 manifest（发布时真实随包/随资源包分发的那批），
    // 而不是"磁盘上看着有"——与引擎按 scenes/<name>.scene 是否存在判定同源。
    const knownScenes = {};
    const modules = {};
    for (const line of manifest.split(/\r?\n/).slice(1)) {
      const path = line.split('\t')[0];
      if (path && path.startsWith('scenes/') && path.endsWith('.scene')) {
        knownScenes[path.slice('scenes/'.length, -'.scene'.length)] = true;
      }
      if (path && path.startsWith('scripts/') && path.endsWith('.lua')) {
        const name = path.slice('scripts/'.length, -'.lua'.length).replace(/\//g, '.');
        modules[name] = await fetchText('pack/' + path);
      }
    }

    // 用 `load(源码, "@<模块路径>.lua")` 而不是 `function(...) <源码> end` 包装：
    // 两者对 require 等价（chunk 同样收到 `...`），但后者把**所有模块**的行号都折进
    // 同一个 `engine-host-prelude` chunk，报错只能给出拼接后的绝对行号
    // （实测：`engine-host-prelude:14566` 得手工反推才知是哪个模块哪一行）。
    // 挂上 chunkname 后 Lua 直接报 `@scripts/world/xxx.lua:42`，定位一步到位。
    const preload = Object.keys(modules).sort().map(name => {
      const chunkName = '@' + name.replace(/\./g, '/') + '.lua';
      return `package.preload[${luaQuote(name)}] = assert(load(${luaQuote(modules[name])}, ${luaQuote(chunkName)}))`;
    }).join('\n');
    const main = await fetchText('pack/' + mainScript);
    const f = window.fengari;
    const L = f.lauxlib.luaL_newstate();
    f.lualib.luaL_openlibs(L);
    try {
      const hostCompat = ENGINE_HOST_GLOBALS.map(name => `if ${name} == nil then ${name} = __p2_missing_host_global(${luaQuote(name)}) end`).join('\n');
      // 引擎里有、web 运行时没实现的宿主 API：**不再静默变成空表**。
      // 旧写法（if map == nil then map = {} end）把"未实现的全局"伪装成一张空表，
      // 于是 `map.current()` 报的是 `attempt to call a nil value (field 'current')`——
      // 看上去像引擎缺一个字段，实际是整个 map 表都没实现（实测把人误导到"引擎 API 缺失"）。
      // 现在换成会**自报家门**的代理：每次访问记一条 `全局.字段` 路径并打印，同时返回
      // 可调用/可索引的替身让执行继续——一条用例就能把所有缺口探全，而不是撞一个修一个。
      // `__p2_export_missing_api()` 给出清单，供宿主/harness 断言"这条路径不缺 API"。
      const missingApiCompat = `
__p2_missing_api = {}
function __p2_record_missing(path)
  if not __p2_missing_api[path] then
    __p2_missing_api[path] = true
    print('[lua missing-host-api] ' .. path)
  end
end
function __p2_missing_proxy(prefix)
  return setmetatable({}, {
    __index = function(_, key)
      local child = prefix .. '.' .. tostring(key)
      __p2_record_missing(child)
      return __p2_missing_proxy(child)
    end,
    __call = function() return __p2_missing_proxy(prefix) end,
    __tostring = function() return '<missing host API ' .. prefix .. '>' end
  })
end
-- 整个宿主全局都没实现（如 inv / grid / day）：换成会自报家门的代理。
function __p2_missing_host_global(name)
  return __p2_missing_proxy(name)
end
-- 宿主表存在但缺方法（如引擎 save 有 clear，shim 里没有）：同样点名。
-- 只在表自身没有元表时安装，避免盖掉上面代理的 __call。
function __p2_trap_host_table(name, api)
  if type(api) == 'table' and getmetatable(api) == nil then
    setmetatable(api, {
      __index = function(_, key)
        local child = name .. '.' .. tostring(key)
        __p2_record_missing(child)
        return __p2_missing_proxy(child)
      end
    })
  end
end
function __p2_export_missing_api()
  local rows = {}
  for path in pairs(__p2_missing_api) do rows[#rows + 1] = path end
  table.sort(rows)
  return table.concat(rows, '\\n')
end
`;
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
-- 动作层（引擎 core/input.rs 的 ActionMap::with_defaults 与 InputState::action_axis）。
-- 这里镜像默认绑定的**键盘**部分：web 平台没有手柄设备，引擎默认表里的
-- GamepadAxis/GamepadButton 项在 web 侧无对应输入，故省略；虚拟摇杆由适配层映射成
-- w/a/s/d（harness 断言 stick_key=w），仍走同一张表，所以玩法的轴向与 PC 一致。
-- 键名与运行时 keyName() 的产出对齐：小写字母、'space'、'escape'、'up/down/left/right'、数字。
-- 此前只硬编码了 6 个动作（confirm/cancel/move_*），于是 web 端 interact/attack/
-- inventory/pause/slot_* 等**全是死的**（按了没反应，且不报错）。
local bindings = {
  move_up = {'w'}, move_down = {'s'}, move_left = {'a'}, move_right = {'d'},
  interact = {'e'}, attack = {'j'}, skill = {'k'}, gift = {'g'}, sleep = {'t'},
  practice = {'q'}, inventory = {'i'}, pause = {'p'}, build = {'b'}, place = {'f'},
  demolish = {'x'}, use = {'u'}, quest = {'l'}, respawn = {'r'}, seed = {'c'},
  -- 引擎把 + / - 按**字符**语义绑到 US-QWERTY 的 = / - 物理键，两种写法都收。
  volume_up = {'=', '+'}, volume_down = {'-', '_'},
  slot_1 = {'1'}, slot_2 = {'2'}, slot_3 = {'3'}, slot_4 = {'4'}, slot_5 = {'5'},
  slot_6 = {'6'}, slot_7 = {'7'}, slot_8 = {'8'}, slot_9 = {'9'},
  confirm = {'space'}, cancel = {'escape'},
  ui_up = {'up'}, ui_down = {'down'}, ui_left = {'left'}, ui_right = {'right'},
}
function input.bindings(action) local result = {}; for i,k in ipairs(bindings[tostring(action)] or {}) do result[i]=k end; return result end
function input.action_pressed(action)
  for _,key in ipairs(bindings[tostring(action)] or {}) do
    if __p2_input and __p2_input.just and __p2_input.just[key] then return true end
  end
  return false
end
-- 轴 = 正动作绑定值之和 − 负动作绑定值之和，钳到 [-1, 1]（引擎 InputState::action_axis）。
-- **首参是负方向**动作，所以垂直轴写作 action_axis("move_down", "move_up")：
-- 写成 ("move_up","move_down") 会让按键方向整体反号（引擎测试里有这条断言）。
function input.action_axis(negative, positive)
  local value = 0
  for _,key in ipairs(bindings[tostring(negative)] or {}) do
    if __p2_input and __p2_input.keys and __p2_input.keys[key] then value = value - 1 end
  end
  for _,key in ipairs(bindings[tostring(positive)] or {}) do
    if __p2_input and __p2_input.keys and __p2_input.keys[key] then value = value + 1 end
  end
  if value < -1 then return -1 elseif value > 1 then return 1 end
  return value
end
function input.actions()
  local names = {}
  for name in pairs(bindings) do names[#names + 1] = name end
  table.sort(names)
  return names
end
`;
      // 先记下"这一侧到底缺哪些宿主全局"，再装兼容层——顺序不能反：`hostCompat` 会把缺的
      // 全局换成代理，之后 `missingHostGlobals()` 只会看到"不是 nil 了"（那是它给 catch
      // 分支用的兜底诊断，见下面 `catch`）。
      //
      // 这份**静态清单**要交给游戏脚本：代理是可调用/可索引的，所以
      // `pcall(camera2d.get)` 会"成功"并返回一张真表——脚本单靠"调用报不报错"或
      // "返回值形状"都判不出宿主其实没有这个 API（`examples/game_jhyt` 的
      // `world/camera_view.lua` 实测踩到）。有了它，脚本可以**零噪声**地判能力：
      // 不碰代理就不会在诊断日志里留下假缺口。
      //
      // 用**下标**写法而不是 `{name=true}`：表里会出现 `time`/`log` 这类**Lua 保留字**
      // 或非标识符，`{time=true}` 直接是语法错误（实测把启动打成
      // `'}' expected near '='`）；`["time"]=true` 对任何名字都成立。
      const eagerlyMissingGlobals = missingHostGlobals(L);
      const missingGlobalTable = '__p2_missing_globals = {' +
        eagerlyMissingGlobals.map(name => `[${luaQuote(name)}]=true`).join(',') + '}';
      doString(L, missingApiCompat + '\n' + missingGlobalTable + '\n' + hostCompat, 'engine-host-compat');
      doString(L, luaPrelude(sceneState, knownScenes) + '\n' + preload, 'engine-host-prelude');
      doString(L, methodCompat + '\n' + genericCompat, 'engine-host-method-compat');
      // 在**真正的宿主表**上装缺失方法陷阱（放在 methodCompat 之后：那批默认实现
      // 必须先落地，否则陷阱会让 `if not api.load_settings` 之类的探测误判为"有"）。
      doString(L, ENGINE_HOST_GLOBALS.map(name => `__p2_trap_host_table(${luaQuote(name)}, ${name})`).join('\n'), 'engine-host-table-traps');
      doString(L, main, mainScript);
    } catch (error) {
      const missing = missingHostGlobals(L);
      const detail = 'missing engine host API: ' + missing.join(', ');
      markBoot('failed', detail);
      throw new Error(String((error && error.message) || error) + ' [' + detail + ']');
    }
    callGlobal(L, 'on_create');
    const pendingTextures = new Set();
    // on_create 里就会挂形象/生成 NPC（`sprite.set` + `animation.attach` + `entity.spawn`），
    // 这些是**一次性 op**：不在这里立刻取走，首帧画的就是"还没挂外观"的场景。
    // 传 false = 只取快照、不推进速度与动画帧（首帧不该白走一次积分）。
    syncLuaScene(sceneState, callGlobal(L, '__p2_export_state', [false]));
    await loadGameFont();
    let uiElements = drawScene(ctx, sceneState, images, projectName, callGlobal(L, '__p2_export_ui'), pendingTextures);
    let frameCount = 0;
    // '' 空闲 / 'loading' 切换在途 / 'failed' 切换失败（停链，让失败可见而不是无限重绘）
    let sceneSwitch = '';

    // 换场景，对齐引擎的 process_scene_transition：读新场景文件 → 换掉运行时持有的
    // 场景数据与贴图 → 把新实体表绑进 Lua → **重跑 `on_create`**。最后这步是游戏侧
    // 进图的唯一入口（`on_create` 里才走 boot_game/enter_playable_map 完成落点、
    // 农田、建筑、NPC、UI 复位）：只换名字不重跑它，`update()` 会按"已在玩法地图"
    // 执行而新场景数据从未加载（实测点「开始新游戏」第 5 帧崩在 `map.current()`）。
    async function applySceneSwitch(name) {
      try {
        const next = parseScene(await fetchText('pack/scenes/' + name + '.scene'));
        const nextImages = await loadSceneImages(next);
        doString(L, sceneBindings(next), 'engine-host-scene');
        sceneState = next;
        images = nextImages;
        pendingTextures.clear();
        callGlobal(L, 'on_create');
        // 换场景后的 on_create 会重新落位、挂形象、生成 NPC —— 与启动路径同理，
        // 立刻取一次快照，别让新场景的第一帧画成"只有场景本体"。
        syncLuaScene(sceneState, callGlobal(L, '__p2_export_state', [false]));
        uiElements = drawScene(ctx, sceneState, images, projectName, callGlobal(L, '__p2_export_ui'), pendingTextures);
        markBoot('running', sceneState.name);
        console.log('[lua] 切换场景:', name, '实体', next.entities.size);
        sceneSwitch = '';
        // P0-①：场景切换落定 = drive 的**事件驱动入口**。wait 阶段若靠虚拟时间下的
        // 帧轮询判据，加载期间的 rAF 帧会吞噬虚拟时间预算、轮询迟迟等不到判据满足
        // （实测有日志拖慢帧率时 done、清理日志后 absent 的竞态）。切换完成的瞬间
        // 直接进 move，与微信 harness"触发一次 → 等切换 → 再移动"完全同构。
        if (driveAuto && driveAuto.phase === 'wait') {
          driveAuto.phase = 'move';
          driveAuto.frames = 0;
          markDrivePhase('moving');
        }
      } catch (error) {
        console.error(error);
        if (status) status.textContent = 'scene load failed: ' + error;
        markBoot('failed', (error && error.message) || error);
        sceneSwitch = 'failed';
      }
    }

    // P0-①：URL `?p2-drive=auto` 驱动"完整剧本"（标题按确认 → 进玩法地图 → 按住方向键移动），
    // 结果写进 `data-p2-drive`（done/failed）供无头浏览器验收断言。驱动走**真实输入路径**
    // （window KeyboardEvent），与微信 harness 的 --drive-new-game/--drive-move 同构——
    // 两个平台共用同一份 web-runtime.js，字段含义一致，可以直接对照。
    let driveAuto = null;
    try {
      const search = (typeof window !== 'undefined' && window.location && window.location.search) || '';
      if (search.indexOf('p2-drive=auto') >= 0) {
        driveAuto = {
          phase: 'confirm',
          frames: 0,
          confirmFrames: 2,
          sceneBefore: sceneState.name,
          waitLimit: 150,
          moveKey: 'w',
          moveFrames: 12,
          positions: [],
          first: null
        };
      }
    } catch (e) { driveAuto = null; }
    if (driveAuto) {
    }

    function dispatchDriveKey(type, key) {
      try { window.dispatchEvent(new KeyboardEvent(type, { key, bubbles: true })); } catch (e) { }
    }

    function markDrivePhase(phase) {
      const body = document.body || document.documentElement;
      if (body) body.setAttribute('data-p2-drive-phase', phase);
    }

    function markDrive(state, detail) {
      const body = document.body || document.documentElement;
      if (!body) return;
      body.setAttribute('data-p2-drive', state);
      if (detail) body.setAttribute('data-p2-drive-detail', String(detail).slice(0, 400));
      console.log('[drive] ' + state + ' ' + detail);
    }

    function stepDrive() {
      if (!driveAuto || driveAuto.phase === 'done' || driveAuto.phase === 'failed') return;
      const d = driveAuto;
      d.frames += 1;
      if (d.phase === 'confirm') {
        // 与微信 harness 同构：真实空格键值 ' '（keyName 归一到 'space'），按 2 帧后松开。
        // 只按 1 帧标题不响应（Lua 确认需要按住/连续帧）；但也不能按太久——新场景起来后
        // 继续按 space 会触发场景里的交互（无限 loading）。2 帧是两者间的稳定值。
        if (d.frames < d.confirmFrames) {
          dispatchDriveKey('keydown', ' ');
        }
        if (d.frames >= d.confirmFrames) {
          dispatchDriveKey('keyup', ' ');
          d.phase = 'wait';
          d.frames = 0;
        }
        return;
      }
      if (d.phase === 'wait') {
        const summary = String(callGlobal(L, '__p2_world_summary') || '');
        const mapMatch = summary.match(/\bmap=([^\s]+)/);
        const sceneSwitched = sceneState.name !== d.sceneBefore ||
          (mapMatch && mapMatch[1] !== 'main' && mapMatch[1] !== d.sceneBefore);
        if (sceneSwitched) {
          d.phase = 'move';
          d.frames = 0;
        } else if (d.frames >= d.waitLimit) {
          markDrive('failed', 'scene switch did not happen within ' + d.waitLimit + ' frames (scene=' + sceneState.name + ' frames=' + d.frames + ')');
          d.phase = 'failed';
        }
        return;
      }
      if (d.phase === 'move') {
        dispatchDriveKey('keydown', d.moveKey);
        const summary = String(callGlobal(L, '__p2_world_summary') || '');
        const match = summary.match(/player=[^ ]*@(-?\d+),(-?\d+)/);
        if (match) {
          const pos = [Number(match[1]), Number(match[2])];
          if (d.first === null) d.first = pos;
          d.positions.push(pos);
          // 位移出现即完成：虚拟时间预算在场景加载期被 rAF 帧吞噬后所剩无几，
          // 固定帧数轮询在这种环境下不可靠（实测多轮 absent）；坐标一变化就收口。
          if (d.first && (pos[0] !== d.first[0] || pos[1] !== d.first[1])) {
            dispatchDriveKey('keyup', d.moveKey);
            const dx = pos[0] - d.first[0];
            const dy = pos[1] - d.first[1];
            const detail = 'scene=' + sceneState.name + ' before=' + d.first.join(',') + ' after=' + pos.join(',') + ' d=' + dx + ',' + dy + ' samples=' + d.positions.length;
            markDrivePhase('done');
            markDrive('done', detail);
            d.phase = 'done';
            return;
          }
        }
        if (d.frames >= d.moveFrames) {
          dispatchDriveKey('keyup', d.moveKey);
          const first = d.first || d.positions[0];
          const last = d.positions[d.positions.length - 1];
          let detail;
          if (!first || !last) detail = 'no player position observed while moving (frames=' + d.frames + ')';
          else detail = 'scene=' + sceneState.name + ' before=' + first.join(',') + ' after=' + last.join(',') + ' d=' + (last[0]-first[0]) + ',' + (last[1]-first[1]) + ' samples=' + d.positions.length;
          markDrivePhase('failed');
          markDrive('failed', detail);
          d.phase = 'failed';
        }
        return;
      }
    }

    function frame() {
      try {
        // 切换在途：只重绘当前画面（表现上"画面不跳"），**不推 update**——新场景数据
        // 尚未就绪时跑 Lua 只会拿到半初始化状态。rAF 链保持不断，切换完成后自动恢复。
        if (sceneSwitch === 'loading') {
          drawScene(ctx, sceneState, images, projectName, callGlobal(L, '__p2_export_ui'), pendingTextures);
          requestAnimationFrame(frame);
          return;
        }
        if (sceneSwitch === 'failed') return;
        // 场景切换是**排队**的（`scene.load` / `map.goto` 只排队、不改名字）；每帧开头
        // 取一次，取走即应用——与引擎"排队 → transition → 重跑 on_create"同构。
        const pendingScene = callGlobal(L, '__p2_take_pending_scene');
        if (pendingScene) {
          sceneSwitch = 'loading';
          applySceneSwitch(pendingScene);
          drawScene(ctx, sceneState, images, projectName, callGlobal(L, '__p2_export_ui'), pendingTextures);
          requestAnimationFrame(frame);
          return;
        }
        stepDrive();
        doString(L, '__p2_input = ' + luaValue(webInput.snapshot()), '__p2_input');
        const result = callGlobal(L, 'update');
        syncLuaScene(sceneState, callGlobal(L, '__p2_export_state'));
        uiElements = drawScene(ctx, sceneState, images, projectName, callGlobal(L, '__p2_export_ui'), pendingTextures);
        frameCount += 1;
        const body = document.body || document.documentElement;
        if (body) {
          // 世界状态（场景实体数/地图/落点/存活实体数/主角坐标与当前动画）每帧刷新：
          // 宿主诊断覆盖层与无头验收都读它，"游戏在跑但世界是空的"这类问题不必再猜。
          // scene=/drawn= 由 JS 侧补（场景里声明的实体数、渲染器侧的主角位置），其余由 Lua 侧给。
          const drawnPlayer = rendererPlayerState(sceneState);
          const world = 'scene=' + (sceneState.declared || sceneState.entities.size) + ' '
            + String(callGlobal(L, '__p2_world_summary') || '')
            + (drawnPlayer ? ' drawn=' + drawnPlayer : '');
          body.setAttribute('data-p2-world', world.slice(0, 300));
          if (frameCount <= 5 || frameCount % 30 === 0) {
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
