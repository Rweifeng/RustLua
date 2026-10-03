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
      // 场景文件里的 Transform 是 **Transform3D 统一格式**（`position=x,y,z;rotation=x,y,z,w;
      // scale=x,y,z`，见 core/transform.rs 的 Transform3D::from_component_str），运行期实体经
      // `transform.set` 走的才是 `x=;y=` 的 2D 格式。两种都必须认——只认 `x=` 会把静态
      // 建筑/物件的坐标整成 (0,0)，画面表现就是"所有建筑都堆在世界原点"。
      const p = parseProps(entity.components.Transform);
      const pos = String(p.position || '').split(',').map(Number);
      const scl = String(p.scale || '').split(',').map(Number);
      const rot = Number(p.rotation || 0);
      entity.transform = {
        x: Number.isFinite(pos[0]) ? pos[0] : Number(p.x || 0),
        y: Number.isFinite(pos[1]) ? pos[1] : Number(p.y || 0),
        rotation: Number.isFinite(rot) ? rot : 0,
        scale_x: Number.isFinite(scl[0]) ? scl[0] : Number(p.scale_x || 1),
        scale_y: Number.isFinite(scl[1]) ? scl[1] : Number(p.scale_y || 1),
      };
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
      // 锚点 pivot（原生 Sprite 默认 0.5,0.5；静态建筑/物件写 `pivot=0,0` = 左下贴地、向上生长）。
      // 不解析 pivot 会按中心点画在坐标上，建筑整体偏移半个身位（且与翻转前的画法不一致）。
      const pivot = String(p.pivot || '').split(',').map(Number);
      entity.sprite = {
        texture: p.texture || '',
        size: width,
        size_y: height,
        pivot_x: Number.isFinite(pivot[0]) ? pivot[0] : 0.5,
        pivot_y: Number.isFinite(pivot[1]) ? pivot[1] : 0.5,
      };
    }
    if (entity.components.Collider) {
      // 碰撞体：`shape=ball;radius=..` / `shape=cuboid;half_width=..;half_height=..`，
      // 可选 `sensor=true`、`offset=x,y`（字段与 `rustlua-physics` 的
      // `ColliderDesc::from_component_str` 一致）。
      // 为什么要解析：web 侧没有物理世界，移动靠 Lua 自己积分——没有这半边信息，
      // 主角会直接穿过院墙/水面/屋基（实机 2026-10-03：web 上"可以穿越任何阻碍点"）。
      const c = parseProps(entity.components.Collider);
      const shape = String(c.shape || 'ball').toLowerCase();
      const offset = String(c.offset || '').split(',').map(Number);
      const collider = {
        shape,
        sensor: c.sensor === 'true' || c.sensor === true || c.sensor === '1',
        ox: Number.isFinite(offset[0]) ? offset[0] : 0,
        oy: Number.isFinite(offset[1]) ? offset[1] : 0,
        hx: 0,
        hy: 0,
      };
      if (shape === 'cuboid') {
        collider.hx = Number(c.half_width) > 0 ? Number(c.half_width) : 0;
        collider.hy = Number(c.half_height) > 0 ? Number(c.half_height) : 0;
      } else {
        const radius = Number(c.radius);
        collider.hx = Number.isFinite(radius) && radius > 0 ? radius : 0;
        collider.hy = collider.hx;
      }
      entity.collider = collider;
    }
    if (entity.components.Camera) {
      // 相机组件：`projection=orthographic;center=x,y;zoom=z;bounds=min_x,min_y,max_x,max_y`。
      // 原生与 web 共用同一份视觉口径（docs/51 §2.2：zoom=2 对标星露谷）。
      // bounds 与地图矩形一致，computeCamera 直接按地图尺寸夹取即可，无需单独解析。
      const p = parseProps(entity.components.Camera);
      entity.camera = {
        zoom: Number(p.zoom) > 0 ? Number(p.zoom) : 1,
      };
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
      // 键盘**抬起边沿**（`input.just_released` / `input.released` / `input.action_released`）。
      // 与 `just` 对称、每帧随快照清空：引擎的 `InputSnapshot` 里 pressed/released 是两个
      // 独立集合（见上面的 `mouseUp` 注释），而此前 web 只把按下边沿送进 Lua，抬起边沿根本没
      // 有来源——补丁层因此没法实现 `action_released`，L3 契约的 `input.action.no_device`
      // 直接报「缺宿主 API」。让 `up` 真真存在，才有资格谈"和桌面同语义"。
      up: {},
      mouse: {},
      mouseJust: {},
      // 鼠标**抬起边沿**（`input.mouse_just_released`）。与 `mouseJust` 分开：引擎
      // `InputSnapshot` 的 pressed/released 也是两个独立集合，合成一个会让"刚按下"
      // 与"刚抬起"互相冒充。
      mouseUp: {},
      cursor: [0, 0],
      pressCursor: [0, 0],
      lastCursor: [0, 0],
      cursorDelta: [0, 0],
      // 滚轮累计增量，**两个分量**（引擎 `InputState::scroll_event(dx, dy)` 与
      // `input.scroll_delta()` 返回的 `{x, y}` 表同形；此前只攒 deltaY 且当标量交给
      // Lua，形状就与桌面不同）。浏览器 `wheel` 事件本身就带 deltaX（横向滚轮/触控板）。
      scrollX: 0,
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
    window.addEventListener('keyup', event => {
      const key = keyName(event);
      // 按下过才算一次抬起边沿：宿主可能补发没按过的 keyup，与下面 `mouseup` 同一判据。
      if (state.keys[key]) state.up[key] = true;
      state.keys[key] = false;
    });
    canvas.addEventListener('mousedown', event => {
       const button = event.button === 2 ? 'right' : 'left';
       // 每次 mousedown 都记按下边沿（不只首按）：多指场景里第二根手指按按钮也必须产生
       // 一次边沿，否则"按住摇杆再点按钮"按钮永远收不到按下（微信端虚拟按钮失效的根因③）。
       // 按下位置单独记进 press_cursor：光标可能正跟手（摇杆），虚拟控件按按下位置命中测试。
       state.mouseJust[button] = true;
       state.mouse[button] = true;
       const rect = canvas.getBoundingClientRect();
       // 画布坐标 → 设计坐标：与 drawScene 的 contain 变换互逆（(x - ox) / s）。
       const fitS = Math.min(canvas.width / 1280, canvas.height / 720);
       const fitOx = (canvas.width - 1280 * fitS) / 2;
       const fitOy = (canvas.height - 720 * fitS) / 2;
       state.pressCursor = [(event.clientX - rect.left - fitOx) / fitS, (event.clientY - rect.top - fitOy) / fitS];
     });
     window.addEventListener('mouseup', event => {
       const button = event.button === 2 ? 'right' : 'left';
       // 有按下过才记边沿：宿主（微信适配层）在触摸链上可能补发 mouseup，
       // 没按过的"抬起"不该产生一次幽灵点击。
       if (state.mouse[button]) state.mouseUp[button] = true;
       state.mouse[button] = false;
     });
    canvas.addEventListener('mousemove', event => {
      const rect = canvas.getBoundingClientRect();
      // 画布坐标 → 设计坐标（与 drawScene 的 contain 变换互逆）。
      const fitS = Math.min(canvas.width / 1280, canvas.height / 720);
      const fitOx = (canvas.width - 1280 * fitS) / 2;
      const fitOy = (canvas.height - 720 * fitS) / 2;
      state.cursor = [(event.clientX - rect.left - fitOx) / fitS, (event.clientY - rect.top - fitOy) / fitS];
    });
    canvas.addEventListener('wheel', event => {
      state.scrollX += event.deltaX;
      state.scroll += event.deltaY;
      event.preventDefault();
    }, { passive: false });
    return {
      snapshot() {
        state.cursorDelta = [state.cursor[0] - state.lastCursor[0], state.cursor[1] - state.lastCursor[1]];
        state.lastCursor = [state.cursor[0], state.cursor[1]];
        const horizontal = (state.keys.d || state.keys.right ? 1 : 0) - (state.keys.a || state.keys.left ? 1 : 0);
         const vertical = (state.keys.s || state.keys.down ? 1 : 0) - (state.keys.w || state.keys.up ? 1 : 0);
        const out = { axis_x: horizontal, axis_y: vertical, keys: state.keys, just: state.just, released: state.up, mouse: state.mouse, mouse_just: state.mouseJust, mouse_just_released: state.mouseUp, cursor: state.cursor, press_cursor: state.pressCursor, cursor_delta: state.cursorDelta, scroll: [state.scrollX, state.scroll] };
        state.just = {};
        state.up = {};
        state.mouseJust = {};
        state.mouseUp = {};
        state.scrollX = 0;
        state.scroll = 0;
        return out;
      }
    };
  }

  // Lua → 渲染器的状态回写。行格式（Lua 侧 __rustlua_export_state 生成）：
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
      } else if (parts[0] === 'Z') {
        // `tilemap.set_solid`（运行期放置/拆除建筑）：Lua 侧的碰撞表与渲染侧的
        // `map.solid` 必须同步，否则"放置的建筑不挡路"（两侧数据分叉）。
        const map = entity.tilemap;
        const x = Number(parts[2]);
        const y = Number(parts[3]);
        if (!map || !Number.isFinite(x) || !Number.isFinite(y)) continue;
        const index = y * map.width + x;
        if (index < 0 || index >= map.solid.length) continue;
        const on = Number(parts[4]) ? '1' : '0';
        map.solid = map.solid.slice(0, index) + on + map.solid.slice(index + 1);
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
    const url = (typeof window !== 'undefined' && window.__RUSTLUA_FONT) || '';
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
        panels.push({ id: parts[1], x: +parts[2], y: +parts[3], w: +parts[4], h: +parts[5], r: +parts[6], g: +parts[7], b: +parts[8], a: parts[9] === undefined ? 1 : +parts[9], shape: parts[10] === undefined ? 0 : +parts[10] });
      } else if (parts[0] === 'I') {
        images.push({ id: parts[1], x: +parts[2], y: +parts[3], w: +parts[4], h: +parts[5], texture: parts[6] || '', r: +parts[7], g: +parts[8], b: +parts[9], slice: parts[10] || '' });
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
  // “页面启动了但没有画面”这类问题——`data-rustlua-boot=running` 只说明 Lua 在跑，
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

  // 9-slice 绘制（与引擎 nine_slice_cells 对齐）：corners 固定像素、edges 单向拉伸、
  // center 双向拉伸。slice = "l,r,t,b" 纹理像素；目标比边距和还窄时边距等比收缩。
  function drawNineSliceImage(ctx, tex, dx, dy, dw, dh, sliceStr) {
    const tw = tex.naturalWidth || tex.width;
    const th = tex.naturalHeight || tex.height;
    if (!(tw > 0) || !(th > 0)) return;
    const parts = String(sliceStr || '').split(',').map(Number);
    if (parts.length !== 4 || parts.some(v => !Number.isFinite(v) || v < 0)) {
      ctx.drawImage(tex, dx, dy, dw, dh);
      return;
    }
    let [l, r, t, b] = parts;
    // 目标比左右边距和还窄时等比压缩边距（与 Rust dst_col_f 逻辑一致）。
    const fx = (l + r > dw && l + r > 0) ? dw / (l + r) : 1;
    const fy = (t + b > dh && t + b > 0) ? dh / (t + b) : 1;
    l *= fx; r *= fx; t *= fy; b *= fy;
    const sw = tw - l - r, sh = th - t - b;      // source center 尺寸
    const dwc = dw - l - r, dhc = dh - t - b;     // dest center 尺寸
    // [sx, sy, sWidth, sHeight, dx, dy, dWidth, dHeight]
    const cells = [
      [0, 0, l, t, dx, dy, l, t],                                  // TL
      [l, 0, sw, t, dx + l, dy, dwc, t],                           // top edge
      [tw - r, 0, r, t, dx + l + dwc, dy, r, t],                   // TR
      [0, t, l, sh, dx, dy + t, l, dhc],                           // left edge
      [l, t, sw, sh, dx + l, dy + t, dwc, dhc],                    // center
      [tw - r, t, r, sh, dx + l + dwc, dy + t, r, dhc],            // right edge
      [0, th - b, l, b, dx, dy + t + dhc, l, b],                   // BL
      [l, th - b, sw, b, dx + l, dy + t + dhc, dwc, b],           // bottom edge
      [tw - r, th - b, r, b, dx + l + dwc, dy + t + dhc, r, b],    // BR
    ];
    for (const c of cells) {
      if (c[6] <= 0 || c[7] <= 0 || c[2] <= 0 || c[3] <= 0) continue;
      ctx.drawImage(tex, c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]);
    }
  }

  function drawUi(ctx, uiText, images, pending) {
    const ui = parseUiLines(uiText);
    LAST_UI_SUMMARY = ui.panels.map(item => (item.shape === 1 ? 'C:' : 'P:') + item.id)
      .concat(ui.images.map(item => 'I:' + item.id + ':' + item.texture), ui.bars.map(item => 'B:' + item.id), ui.texts.map(item => 'X:' + item.id))
      .join(',');
    for (const panel of ui.panels) {
      ctx.fillStyle = uiColor(panel.r, panel.g, panel.b, panel.a);
      // shape=1（引擎 UiPanel.shape）：在外接矩形内画填充圆。半径取 min(w,h)/2，
      // 圆心取矩形中心——与 runtime_scene.rs 的三角扇几何一致；退化半径跳过，
      // 不让 arc 拿到 0/负半径抛异常（那会让整帧绘制中断）。
      if (panel.shape === 1) {
        const radius = Math.min(panel.w, panel.h) / 2;
        if (!(radius > 0)) continue;
        ctx.beginPath();
        ctx.arc(panel.x + panel.w / 2, panel.y + panel.h / 2, radius, 0, Math.PI * 2);
        ctx.fill();
        continue;
      }
      ctx.fillRect(panel.x, panel.y, panel.w, panel.h);
    }
    for (const image of ui.images) {
      const texture = ensureTexture(images, pending, image.texture);
      if (!texture) continue;
      if (image.slice) {
        drawNineSliceImage(ctx, texture, image.x, image.y, image.w, image.h, image.slice);
      } else {
        ctx.drawImage(texture, image.x, image.y, image.w, image.h);
      }
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
    if (entity.text && entity.text.value && !p2SceneTextHidden.has(String(entity.name || ''))) {
      ctx.font = entity.text.size + 'px ' + GAME_FONT;
      ctx.textBaseline = 'top';
      ctx.fillStyle = colorCss(entity.text.color);
      ctx.fillText(entity.text.value, rect.x, rect.y);
      drew = true;
    }
    return drew;
  }

  // 相机（世界 → 画布），与原生引擎同一套约定（camera.rs 的 constrained_center /
  // camera_view.lua 的 follow_player）：
  //   * 世界 y 朝上、tilemap row 0 在地图最底；画布 y 向下 ⇒
  //     `画布 = 视口/2 + (世界 - 相机中心)` 的 x 轴、`画布 = 视口/2 - (世界 - 相机中心)` 的
  //     y 轴，正好等于引擎 `world_to_screen`（屏幕空间、左上原点），画布即屏幕。
  //   * 相机中心默认跟随玩家实体（名字含 `player`，即 JianghuPlayer），无玩家时取地图中心。
  //   * 夹取规则与原生一致：中心 ∈ [半视口, 地图尺寸 - 半视口]；区间非法（地图比视口短）
  //     时取两条界的中点（横版下地图 704 < 视口 720，y 轴因此钉在地图纵向中心）。
  // 没有 tilemap 的场景（标题门面）返回 null ⇒ 世界元素保持原样（屏幕空间，无翻转无相机）。
  function computeCamera(sceneState, viewportW, viewportH) {
    let map = null;
    let player = null;
    let camZoom = 1;
    for (const entity of sceneState.entities.values()) {
      if (entity.alive === false) continue;
      if (!map && entity.tilemap && entity.tilemap.width > 0 && entity.tilemap.height > 0) {
        map = entity.tilemap;
      }
      if (entity.camera && Number(entity.camera.zoom) > 0) {
        camZoom = Number(entity.camera.zoom);
      }
      if (!player && entity.transform) {
        const name = String(entity.name || '');
        if (name === 'JianghuPlayer' || name.toLowerCase().indexOf('player') !== -1) {
          player = entity;
        }
      }
    }
    if (!map) return null;
    const mapW = map.width * map.tile_size;
    const mapH = map.height * map.tile_size;
    // 半视口按 zoom 缩放（与原生 Camera2D::visible_extent = 视口/2/zoom 一致）：
    // zoom=2 时可见域 = 640×360 世界单位（20×11.25 格），屏幕 tile 64px。
    const halfW = viewportW / 2 / camZoom;
    const halfH = viewportH / 2 / camZoom;
    let cx = mapW / 2;
    let cy = mapH / 2;
    if (player && player.transform) {
      cx = player.transform.x;
      cy = player.transform.y;
    }
    const clampAxis = (value, min, max) => (min > max ? (min + max) / 2 : Math.max(min, Math.min(max, value)));
    cx = clampAxis(cx, halfW, mapW - halfW);
    cy = clampAxis(cy, halfH, mapH - halfH);
    return { x: cx, y: cy, zoom: camZoom };
  }

  // 场景文字实体隐藏集：title_menu 把按钮标签接管为脚本绘制（ui.scene_text_visible(false)），
  // 场景里的 Text 实体（BtnStartLabel 等）不再画，避免「场景层 + 脚本层」两层文字重叠。
  let p2SceneTextHidden = new Set();
  function refreshSceneTextHidden(L) {
    p2SceneTextHidden = new Set();
    try {
      const raw = String(callGlobal(L, '__rustlua_export_scene_text_hidden') || '');
      if (raw) raw.split(',').forEach(name => { if (name) p2SceneTextHidden.add(name); });
    } catch (e) { /* 兼容层缺失时保持空集 */ }
  }

  function drawScene(ctx, sceneState, images, projectName, uiText, pending) {
    const canvas = ctx.canvas;
    // 等比适配（contain）：设计画布 1280×720 → 实际 canvas（窗口/屏幕物理像素）。
    // 微信把 canvas fill 铺满窗口，画布尺寸若与窗口比例不一致会被**非等比拉伸**
    // （开发者工具 1380×652 下横 1.08×、纵 0.91×，文字随之变形发糊）。画布已跟随
    // 窗口（wechat-adapter），这里把 1280×720 设计空间等比缩放到居中区域（黑边），
    // 场景/UI/文字不再变形；相机视口同步用设计尺寸（在缩放后的坐标系里计算）。
    const fitS = Math.min(canvas.width / 1280, canvas.height / 720);
    const fitOx = (canvas.width - 1280 * fitS) / 2;
    const fitOy = (canvas.height - 720 * fitS) / 2;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.fillStyle = '#151923';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.save();
    ctx.translate(fitOx, fitOy);
    ctx.scale(fitS, fitS);

    const camera = computeCamera(sceneState, 1280, 720);
    // 世界 → 画布（屏幕空间）：x 同向、y 反向（世界朝上 ↔ 画布朝下）。无相机时原样直绘。
    // 注意：这里已在 contain 的 scale 变换**内**，坐标系 = 1280×720 设计空间，视口中心必须用
    // 设计尺寸（640/360）而不是真实 canvas 尺寸——用 canvas.width/2 会把场景中心整体右移、
    // 与屏幕空间 UI（面板/HUD/按钮）分离（1785×832 窗口实测：场景中心偏到千分比 663）。
    const toCanvas = (wx, wy) => {
      if (!camera) return { x: wx, y: wy };
      return {
        x: 640 + (wx - camera.x) * camera.zoom,
        y: 360 - (wy - camera.y) * camera.zoom,
      };
    };
    const camZoom = camera ? camera.zoom : 1;

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
          // 单元格世界矩形 [y*ts, (y+1)*ts]；画布以单元格**上沿**为锚点直绘，无需翻转贴图。
          const cell = toCanvas(origin.x + x * map.tile_size, origin.y + (y + 1) * map.tile_size);
          if (tileset && tile > 0) {
            const tileIndex = tile - 1;
            const sx = (tileIndex % columns) * map.tile_size;
            const sy = Math.floor(tileIndex / columns) * map.tile_size;
            ctx.drawImage(tileset, sx, sy, map.tile_size, map.tile_size, cell.x, cell.y, map.tile_size * camZoom, map.tile_size * camZoom);
          } else {
            const solid = map.solid[i] === '1';
            ctx.fillStyle = solid ? '#334155' : '#1f2937';
            ctx.fillRect(cell.x, cell.y, map.tile_size * camZoom - 1, map.tile_size * camZoom - 1);
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
      // 世界实体走相机变换（与瓦片同一套）。锚点按 Sprite.pivot（原生默认 0.5,0.5；
      // 静态建筑/物件 0,0 = **世界里**左下贴地向上长，与 sprite.rs 的
      // `local_corners`（x∈[-pivot.x*w,(1-pivot.x)*w]、y∈[-pivot.y*h,(1-pivot.y)*h]）
      // 同一套约定）。变换里 y 已反向，无需再翻转贴图本身。
      //
      // 画布 y 向下、世界 y 向上 ⇒ "锚点 → 画布上沿"的距离是 **(1 - pivot.y) × 高**：
      //   * pivot.y=0（静态建筑/木牌/树等 134 处）：上沿 = 锚点 - 整高，贴图底边压在锚点上；
      //   * pivot.y=0.5（角色/NPC 默认）：上沿 = 锚点 - 半高，等价于中心对齐；
      //   * pivot.y=1：上沿 = 锚点，贴图自锚点向下挂。
      // 曾经写成 `p.y - pivot.y * 高`，等价于把 pivot.y 当"从画布上沿量"，于是
      // 0,0 的建筑被整体下移**一整张贴图高度**（实机 2026-10-03：山庄里建筑全部偏移，
      // 药庐/练功房/仓房/木牌/树全部压在药田里；原生渲染与场景数据都是底左锚点）。
      const p = toCanvas(pos.x, pos.y);
      const pivotX = entity.sprite && entity.sprite.pivot_x !== undefined ? entity.sprite.pivot_x : 0.5;
      const pivotY = entity.sprite && entity.sprite.pivot_y !== undefined ? entity.sprite.pivot_y : 0.5;
      const dx = p.x - pivotX * size * camZoom;
      const dy = p.y - (1 - pivotY) * sizeY * camZoom;
      if (sprite && clip) {
        // frame 是**帧序号**（Lua 侧按引擎 current_frame 算好回写），不是累计 tick：
        // 引擎的 current_frame_uv 只取条带首行（frame/cols, 0），所以 sy 恒为 0。
        const index = Math.max(0, Math.floor(Number(entity.animation.frame) || 0));
        const sx = (index % Math.max(1, clip.cols)) * size;
        ctx.drawImage(sprite, sx, 0, size, size, dx, dy, size * camZoom, sizeY * camZoom);
      } else if (sprite) {
        ctx.drawImage(sprite, dx, dy, size * camZoom, sizeY * camZoom);
      } else {
        ctx.fillStyle = entityColor(entity);
        ctx.beginPath();
        ctx.arc(p.x, p.y, Math.max(6, size / 2) * camZoom, 0, Math.PI * 2);
        ctx.fill();
      }
    }

    if (typeof window !== 'undefined' && window.__RUSTLUA_DEBUG) {
      ctx.fillStyle = '#d0d7e2';
      ctx.font = '14px sans-serif';
      ctx.fillText('Project: ' + projectName + ' | Scene: ' + sceneState.name + ' | Lua update running', 12, 704);
    }
    // 动态 UI（面板/按钮/文字）也必须在 contain 变换**内**绘制：它与场景共用 1280×720
    // 设计坐标系。若在 restore 之后画，微信窗口比例≠16:9 时动态 UI 按原始设计坐标铺在
    // 画布左上（未缩放居中），与场景错位分离（1785×832 实机截图）。
    const uiCount = drawUi(ctx, uiText, images, pending);
    ctx.restore();
    return uiCount;
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

  // 场景数据在 Lua 侧的绑定：`__rustlua_names/transforms/tilemaps/animations` 对应引擎里
  // "当前场景的实体表"，`__rustlua_alive` 对应实体存活集合。**启动与换场景共用同一段生成
  // 代码**——换场景时只重绑这四张表并重建存活集合，别的（存档 `__rustlua_save`、地图状态
  // `__rustlua_map_state`、`__map`/`__map:spawn`）都是引擎 `save_data` 里的持久数据，
  // 跨场景必须保留，与引擎一致。
  function sceneBindings(sceneState) {
    const names = {};
    const transforms = {};
    const tilemaps = {};
    const animations = {};
    const colliders = {};
    for (const entity of sceneState.entities.values()) {
      names[entity.name] = entity.id;
      if (entity.transform) transforms[entity.id] = entity.transform;
      if (entity.tilemap) tilemaps[entity.id] = entity.tilemap;
      if (entity.animation) animations[entity.id] = entity.animation;
      if (entity.collider) colliders[entity.id] = entity.collider;
    }
    return `
__rustlua_scene_name = ${luaQuote(sceneState.name)}
__rustlua_names = ${luaValue(names)}
__rustlua_transforms = ${luaValue(transforms)}
__rustlua_tilemaps = ${luaValue(tilemaps)}
-- 碰撞体表（scene 里的 Collider 组件）：Lua 侧的移动积分靠它决定"谁参与格碰撞"。
__rustlua_colliders = ${luaValue(colliders)}
__rustlua_animations = ${luaValue(animations)}
__rustlua_health = {}
__rustlua_velocity = {}
-- UI 注册表同样属于"当前场景"：标题菜单留下的面板/标签不能渗进下一张场景的第一帧
-- （main.lua 的 on_click 也专门 hide 一次，这里是运行时侧的兜底）。
__rustlua_ui_panels = {}
__rustlua_ui_texts = {}
__rustlua_ui_bars = {}
__rustlua_ui_images = {}
__rustlua_scene_text_hidden = {}
__rustlua_alive = {}
-- 运行期改写的渲染数据（syncLuaScene 每帧取走）：sprite.set / animation.attach /
-- tilemap.set_cell / tilemap.set_texture / entity.spawn。引擎换场景时整张 world 重建
-- （script.rs 的场景载入分支 world = World::new() 并清 names/persist/health），
-- 所以这些注册表也必须跟着场景走，否则新场景会带上上一张图的贴图、格子与持久 id 映射。
__rustlua_persist = {}
__rustlua_sprites = {}
-- 运行期**结构变更**（spawn/destroy/sprite.set/animation.attach/tilemap.set_cell/
-- set_texture）走一次性 op 列表：JS 每帧取走即清空。每帧状态（位置、动画帧序号）
-- 走 T/A 行实时回写，两者分开是刻意的——动画组件串一条就有几 KB，逐帧重发
__rustlua_ops = {}
function __rustlua_emit(row) __rustlua_ops[#__rustlua_ops + 1] = row end
for _, id in pairs(__rustlua_names) do __rustlua_alive[id] = true end`;
  }

  function luaPrelude(sceneState, knownScenes) {
    return `
${sceneBindings(sceneState)}
__rustlua_save = {}
-- 虚拟键（触屏摇杆/虚拟按钮注入）是**跨帧保持**的按住态：引擎 InputSnapshot::
-- inherit_virtual_keys 每帧把上一份快照的虚拟键集合并入新快照。宿主每帧都整份替换
-- __rustlua_input（设备层重新采样），不并回来就会退化成「摇杆能动、角色不动」——
-- 同帧注入、下一帧被清空，input.action_axis 恒为 0（引擎侧踩过同一个坑）。
__rustlua_virtual_keys = {}
__rustlua_virtual_just = {}
__rustlua_input = { axis_x = 0, axis_y = 0, keys = {}, just = {}, released = {}, mouse = {}, mouse_just = {}, mouse_just_released = {}, cursor = {0, 0}, cursor_delta = {0, 0}, scroll = {0, 0} }
log ={ info = function(v) print('[lua] ' .. tostring(v)) end, warn = function(v) print('[lua warn] ' .. tostring(v)) end, error = function(v) print('[lua error] ' .. tostring(v)) end, debug = function(v) print('[lua debug] ' .. tostring(v)) end }
time = { delta = function() return 1 / 60 end }
-- 引擎物理键名 → web 键名：native 引擎的动作表用 keyw/keya/keyj...（core/input.rs 的
-- ActionMap），web 的动作层（下方 bindings 表）用 w/a/j...。虚拟控件
-- （ui/virtual_pad.lua）注入的是引擎名，不翻译就永远命中不了任何动作（微信实测：
-- 可见摇杆/按钮全部"按了没反应"）。
local function __rustlua_virtual_name(key)
  local name = tostring(key):lower()
  local letter = name:match('^key([a-z])$')
  if letter then return letter end
  -- native 引擎把"关闭"作为独立物理键 cancel；web 键盘关闭键是 Escape，
  -- bindings.cancel 绑在 'escape' 上。虚拟控件注入 "cancel" 时须翻成 'escape'，
  -- 否则 just["cancel"] 永远不被 action_pressed("cancel") 读到（关闭按钮点了没反应）。
  if name == 'cancel' then return 'escape' end
  return name
end
-- 这张表是**整表替换**引擎注册的 input（37 个函数），所以写在这里的每一个函数都是
-- 补丁层自己的实现：漏一个名字，web 上它就是 nil（不是代理、不在控制台留痕，游戏侧
-- type(x)=="function" 的守卫会安静地跳过——input.last_device 就这样让 hints.lua
-- 的设备提示在 web 上从来没跑过）。抬起边沿一族（just_released/released/
-- action_released）此前整族缺失：JS 侧的 keyup 只写按住态、没送边沿进 Lua。现在
-- 快照带 released（createWebInput 的 state.up），而触摸宿主（微信）没有 keyup，
-- 抬起边沿从 virtual_key 的 true→false 变化里取，与 just 同一条纪律：只在发生
-- 这一帧可见一次，不跨帧重放。
input = { axis = function(name) name = tostring(name or 'x'):lower(); if name == 'x' or name == 'horizontal' then return (__rustlua_input.axis_x or 0) + 0.0 end; if name == 'y' or name == 'vertical' then return (__rustlua_input.axis_y or 0) + 0.0 end; return 0.0 end, is_down = function(key) return __rustlua_input.keys[tostring(key):lower()] == true end, just_pressed = function(key) return __rustlua_input.just[tostring(key):lower()] == true end, just_released = function(key) return __rustlua_input.released[tostring(key):lower()] == true end, pressed = function(key) return __rustlua_input.just[tostring(key):lower()] == true end, released = function(key) return __rustlua_input.released[tostring(key):lower()] == true end, mouse_is_down = function(button) return __rustlua_input.mouse[tostring(button or 0)] == true end, mouse_just_pressed = function(button) return __rustlua_input.mouse_just[tostring(button or 0)] == true end, cursor_position = function() return __rustlua_input.cursor or {0, 0} end, press_position = function() return __rustlua_input.press_cursor or __rustlua_input.cursor or {0, 0} end, cursor_delta = function() return __rustlua_input.cursor_delta or {0, 0} end, scroll_delta = function() local s = __rustlua_input.scroll; return { s[1] + 0.0, s[2] + 0.0 } end, mouse_just_released = function(button) return __rustlua_input.mouse_just_released and __rustlua_input.mouse_just_released[tostring(button or 0)] == true end, virtual_key = function(key, down) local name = __rustlua_virtual_name(key); local was_down = __rustlua_input.keys and __rustlua_input.keys[name] == true; if down then __rustlua_virtual_keys[name] = true else __rustlua_virtual_keys[name] = nil end; if __rustlua_input.keys then __rustlua_input.keys[name] = down and true or false end; if not down and was_down then __rustlua_input.released[name] = true end end, virtual_pressed = function(key) local name = __rustlua_virtual_name(key); __rustlua_virtual_keys[name] = true; __rustlua_virtual_just[name] = true; if __rustlua_input.keys then __rustlua_input.keys[name] = true end; if __rustlua_input.just then __rustlua_input.just[name] = true end end }
-- 每帧把上一帧注入的虚拟键并回新快照（宿主在帧首刚整份替换过 __rustlua_input）。
-- 按下边沿（virtual_pressed）只在**被消费的这一帧**可见一次，读完即清——与引擎
-- 「边沿只在一个快照周期内为真」一致；重发边沿会让 action_pressed 系动作连发。
function __rustlua_apply_virtual_input()
  if type(__rustlua_input) ~= 'table' then return end
  if type(__rustlua_input.keys) ~= 'table' then __rustlua_input.keys = {} end
  if type(__rustlua_input.just) ~= 'table' then __rustlua_input.just = {} end
  if type(__rustlua_input.released) ~= 'table' then __rustlua_input.released = {} end
  for name in pairs(__rustlua_virtual_keys) do __rustlua_input.keys[name] = true end
  -- 按下边沿不跨帧重放：input.virtual_pressed 注入当帧已把键写进 __rustlua_input.just，
  -- 这里若再把 __rustlua_virtual_just 并入 just，toggle 类动作（inventory/quest/pause 等绑定
  -- 到虚拟按钮的键）会在下一帧重放一次边沿——背包打开后立刻被同一动作关闭，面板只存活
  -- 一帧（微信实机"点击背包按钮没有渲染背包UI"的根因）。__rustlua_virtual_just 仅作注入登记，
  -- 清空即可，不写回 just。
  __rustlua_virtual_just = {}
end
-- 场景与地图宿主 API。引擎事实（core/script.rs 的 scene.load / map.goto）：
--   * 切换是**排队的**：写 save_data["__map"]、记录 __map:spawn:x/y、把名字排进
--     scene_pending_load；真正换场景由随后的 process_scene_transition 完成，
--     然后全局 on_create 在**新场景**上重跑。所以这里也只排队、不改名字。
--   * 目标场景存在才切换（引擎按 scenes/<name>.scene 是否存在判定），不存在
--     返回 false 且不改任何状态。
--   * map.state(name[, value]) 是**地图作用域的持久键**（引擎侧落在
--     save_data["__mapstate:<name>"]）：读缺失键返回 nil，写返回 true。
-- 此前这里是 load = function(name) __rustlua_scene_name = name end 加一张空表 map
-- （hostCompat 留下的）：改名不换场景，update() 当帧就走"已在玩法地图"分支，
-- 而新场景的实体/瓦片从未加载；map.current() 则直接崩在空表上
-- （实测：标题界面点「开始新游戏」后第 5 帧 hud 报 attempt to call a nil value）。
__rustlua_scene_known = ${luaValue(knownScenes || {})}
__rustlua_scene_pending = nil
__rustlua_map_current = nil
__rustlua_map_spawn = nil
__rustlua_map_state = {}

function __rustlua_scene_request(name)
  name = tostring(name or '')
  if name == '' or not __rustlua_scene_known[name] then return false end
  __rustlua_scene_pending = name
  return true
end

-- 运行时每帧取一次待切换场景（取走即清空，与引擎"只应用一次"一致）。
function __rustlua_take_pending_scene()
  local name = __rustlua_scene_pending
  __rustlua_scene_pending = nil
  return name
end

scene = {
  reload = function() return __rustlua_scene_name end,
  load = function(name) return __rustlua_scene_request(name) end
}

-- 网格换算宿主 API（引擎 core/grid.rs）：世界坐标 ↔ 格子坐标、朝向格、越界判定。
-- 返回形状与引擎一致：world_to_cell/facing_cell 是**数组** {cx, cy}（调用侧写 cell[1], cell[2]），
-- cell_to_world 是 {x, y}（向量）。tile_size 必须为正有限值（引擎会报错，这里同样报）。
function __rustlua_tile_size(value)
  local size = tonumber(value)
  if not size or size ~= size or size == math.huge or size == -math.huge or size <= 0 then
    error('tile_size must be a positive finite number', 2)
  end
  return size
end
__rustlua_facing_step = { up = { 0, 1 }, n = { 0, 1 }, north = { 0, 1 }, down = { 0, -1 }, s = { 0, -1 }, south = { 0, -1 }, left = { -1, 0 }, w = { -1, 0 }, west = { -1, 0 }, right = { 1, 0 }, e = { 1, 0 }, east = { 1, 0 } }
grid = {
  world_to_cell = function(x, y, tile_size)
    local size = __rustlua_tile_size(tile_size)
    return { math.floor(x / size), math.floor(y / size) }
  end,
  cell_to_world = function(cx, cy, tile_size)
    local size = __rustlua_tile_size(tile_size)
    return { (cx + 0.5) * size, (cy + 0.5) * size }
  end,
  facing_cell = function(x, y, facing, tile_size)
    local size = __rustlua_tile_size(tile_size)
    -- 未知朝向回落 down（引擎 Facing::parse 的默认分支）
    local step = __rustlua_facing_step[tostring(facing or ''):lower():match('^%s*(.-)%s*$')] or { 0, -1 }
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
__rustlua_inventories = {}
function __rustlua_stack_limit(item_id)
  local definition = type(item) == 'table' and type(item.def) == 'function' and item.def(item_id) or nil
  local limit = definition and tonumber(definition.stack) or nil
  if not limit or limit < 1 then return 99 end
  return math.floor(limit)
end
function __rustlua_inventory_new(capacity)
  capacity = math.max(1, math.floor(tonumber(capacity) or 24))
  local slots = {}
  for index = 1, capacity do slots[index] = { item = '', count = 0 } end
  return { capacity = capacity, slots = slots, equipment = {} }
end
function __rustlua_inventory_add(inventory, item_id, count, limit)
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
function __rustlua_inventory_remove(inventory, item_id, count)
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
function __rustlua_inventory_get(id)
  local inventory = __rustlua_inventories[id]
  if not inventory then error('inventory ' .. tostring(id) .. ' not found', 2) end
  return inventory
end
function __rustlua_inventory_slot(inventory, index)
  index = math.floor(tonumber(index) or 0)
  if index <= 0 then error('inventory slot index must be positive', 3) end
  if index > inventory.capacity then error('inventory slot out of bounds', 3) end
  return inventory.slots[index]
end
function __rustlua_inventory_movable(source, target, requested, limit)
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
    __rustlua_inventories[id] = __rustlua_inventory_new(capacity or 24)
    return id
  end,
  hotbar = function(id, size)
    id = tostring(id or '')
    if id == '' then error('inv.hotbar id must not be empty', 2) end
    __rustlua_inventories[id] = __rustlua_inventory_new(size or 9)
    return id
  end,
  capacity = function(id) return __rustlua_inventory_get(tostring(id)).capacity end,
  exists = function(id) return __rustlua_inventories[tostring(id)] ~= nil end,
  add = function(id, item_id, count)
    id, item_id = tostring(id), tostring(item_id or '')
    if item_id == '' then error('inv.add item id must not be empty', 2) end
    count = math.floor(tonumber(count) or 0)
    if count <= 0 then error('inv.add count must be positive', 2) end
    local inventory = __rustlua_inventories[id]
    if not inventory then
      inventory = __rustlua_inventory_new(24)
      __rustlua_inventories[id] = inventory
    end
    return __rustlua_inventory_add(inventory, item_id, count, __rustlua_stack_limit(item_id))
  end,
  remove = function(id, item_id, count)
    count = math.floor(tonumber(count) or 0)
    if count <= 0 then error('inv.remove count must be positive', 2) end
    local inventory = __rustlua_inventories[tostring(id)]
    if not inventory then return 0 end
    return __rustlua_inventory_remove(inventory, tostring(item_id), count)
  end,
  count = function(id, item_id)
    local inventory = __rustlua_inventories[tostring(id)]
    if not inventory then return 0 end
    local total = 0
    for _, slot in ipairs(inventory.slots) do
      if slot.item == tostring(item_id) then total = total + slot.count end
    end
    return total
  end,
  get = function(id)
    id = tostring(id)
    local inventory = __rustlua_inventory_get(id)
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
    local slot = __rustlua_inventory_slot(__rustlua_inventory_get(tostring(id)), index)
    if slot.item == '' then return nil end
    return { item = slot.item, count = slot.count }
  end,
  set = function(id, index, item_id, count)
    local inventory = __rustlua_inventory_get(tostring(id))
    local slot = __rustlua_inventory_slot(inventory, index)
    count = math.floor(tonumber(count) or 0)
    if item_id ~= nil and tostring(item_id) ~= '' and count > 0 then
      slot.item = tostring(item_id)
      slot.count = count
    else
      slot.item, slot.count = '', 0
    end
  end,
  swap = function(from_id, from_index, to_id, to_index)
    local from = __rustlua_inventory_get(tostring(from_id))
    local left = __rustlua_inventory_slot(from, from_index)
    if tostring(from_id) == tostring(to_id) then
      local right = __rustlua_inventory_slot(from, to_index)
      left.item, right.item = right.item, left.item
      left.count, right.count = right.count, left.count
      return true
    end
    local to = __rustlua_inventory_get(tostring(to_id))
    local right = __rustlua_inventory_slot(to, to_index)
    left.item, right.item = right.item, left.item
    left.count, right.count = right.count, left.count
    return true
  end,
  move = function(from_id, from_index, to_id, to_index, count)
    local from = __rustlua_inventory_get(tostring(from_id))
    local source = __rustlua_inventory_slot(from, from_index)
    if source.item == '' then return 0 end
    local limit = __rustlua_stack_limit(source.item)
    local same = tostring(from_id) == tostring(to_id)
    if to_index == nil then
      -- 无目标格：退回"移出再放回"，净效果是重新堆叠，返回净移动量（引擎同义）。
      local requested = math.max(1, math.floor(tonumber(count) or source.count))
      local removed = __rustlua_inventory_remove(from, source.item, requested)
      local leftover = __rustlua_inventory_add(from, source.item, removed, limit)
      return removed - leftover
    end
    local to = same and from or __rustlua_inventory_get(tostring(to_id))
    local target = __rustlua_inventory_slot(to, to_index)
    local requested = math.max(0, math.min(math.floor(tonumber(count) or source.count), source.count))
    local moved = __rustlua_inventory_movable(source, target, requested, limit)
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
    local inventory = __rustlua_inventory_get(id)
    local value = item_id ~= nil and tostring(item_id) or ''
    if value == '' then
      inventory.equipment[slot] = nil
    else
      inventory.equipment[slot] = value
    end
  end,
  equip_get = function(id, slot) return __rustlua_inventory_get(tostring(id)).equipment[tostring(slot)] end,
  equip_list = function(id)
    local result = {}
    for slot, item_id in pairs(__rustlua_inventory_get(tostring(id)).equipment) do result[slot] = item_id end
    return result
  end,
  -- 引擎的 inv.open 面向容器 UI，游戏未使用；保留同名空实现以免宿主 API 表缺项。
  open = function() end
}
map = {
  current = function() return __rustlua_map_current end,
  spawn = function() return __rustlua_map_spawn end,
  set_spawn = function(x, y) __rustlua_map_spawn = { x = x, y = y } end,
  -- goto 是 Lua 5.2+ 的保留字，只能以 map["goto"](...) 形式使用（游戏侧正是这么调的）。
  ["goto"] = function(name, x, y)
    if not __rustlua_scene_request(name) then return false end
    __rustlua_map_current = tostring(name)
    if x ~= nil and y ~= nil then __rustlua_map_spawn = { x = x, y = y } end
    return true
  end,
  enter = function(name, x, y) return map["goto"](name, x, y) end,
  state = function(name, value)
    name = tostring(name)
    if value == nil then return __rustlua_map_state[name] end
    __rustlua_map_state[name] = value
    return true
  end,
  state_clear = function(name)
    local removed = 0
    if name == nil then
      for _ in pairs(__rustlua_map_state) do removed = removed + 1 end
      __rustlua_map_state = {}
    elseif __rustlua_map_state[tostring(name)] ~= nil then
      __rustlua_map_state[tostring(name)] = nil
      removed = 1
    end
    return removed
  end,
  -- 引擎的 map.schema 是"写入前校验"；web 运行时不带 schema 校验器，接受即忽略。
  schema = function(_name, _spec) end
}
-- 存活校验（引擎 require_alive 的同构）：对未知/已销毁实体**报错**而不是静默改数据。
-- 静默 no-op 会让"实体早就没了"这类问题一路潜伏到画面异常才被发现。
function __rustlua_require_alive(id)
  if __rustlua_alive[id] ~= true then
    error('entity is not alive: ' .. tostring(id), 3)
  end
end

-- 实体 id 分配：取现有存活 id 之外的最小正整数（与引擎 world.spawn 的递增 id 形态一致）。
function __rustlua_next_entity_id()
  local id = 1
  for k in pairs(__rustlua_alive) do
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
  find = function(name) return __rustlua_names[name] end,
  is_alive = function(id) return __rustlua_alive[id] == true end,
  find_id = function(persist_id) return __rustlua_persist[tostring(persist_id or '')] end,
  persist_id = function(id)
    for key, value in pairs(__rustlua_persist) do
      if value == id then return key end
    end
    return nil
  end,
  destroy = function(id)
    __rustlua_require_alive(id)
    __rustlua_alive[id] = false
    __rustlua_emit('D\t' .. id)
    for key, value in pairs(__rustlua_persist) do
      if value == id then __rustlua_persist[key] = nil end
    end
    return true
  end,
  spawn = function(name, persist_id)
    local key = persist_id ~= nil and tostring(persist_id) or ''
    local taken = key ~= '' and __rustlua_persist[key] or nil
    if taken ~= nil and __rustlua_alive[taken] == true then
      error('persist_id already in use: ' .. key, 2)
    end
    local id = __rustlua_next_entity_id()
    __rustlua_alive[id] = true
    __rustlua_transforms[id] = { x = 0, y = 0, rotation = 0, scale_x = 1, scale_y = 1 }
    if name ~= nil and name ~= '' then __rustlua_names[tostring(name)] = id end
    if key ~= '' then __rustlua_persist[key] = id end
    __rustlua_emit('R\t' .. id .. '\t' .. tostring(name or ''))
    return id
  end
}
transform = {
  get = function(id)
    local p = __rustlua_transforms[id]
    if not p then return nil end
    return { x = p.x or 0, y = p.y or 0, rotation = p.rotation or 0, scale_x = p.scale_x or 1, scale_y = p.scale_y or 1 }
  end,
  set = function(id, x, y, rotation, scale_x, scale_y)
    __rustlua_require_alive(id)
    local p = __rustlua_transforms[id] or { x = 0, y = 0, rotation = 0, scale_x = 1, scale_y = 1 }
    p.x, p.y = tonumber(x) or 0, tonumber(y) or 0
    if rotation ~= nil then p.rotation = tonumber(rotation) or 0 end
    if scale_x ~= nil then p.scale_x = tonumber(scale_x) or 1 end
    if scale_y ~= nil then p.scale_y = tonumber(scale_y) or 1 end
    __rustlua_transforms[id] = p
    return true
  end,
  move = function(id, dx, dy)
    __rustlua_require_alive(id)
    local p = __rustlua_transforms[id] or { x = 0, y = 0, rotation = 0, scale_x = 1, scale_y = 1 }
    p.x = (p.x or 0) + (tonumber(dx) or 0)
    p.y = (p.y or 0) + (tonumber(dy) or 0)
    __rustlua_transforms[id] = p
    return true
  end
}
transform3d = { attach = function(_) return true end, get = function(_) return { x = 0, y = 0, z = 0 } end, set = function(...) return true end, move = function(...) return true end }
__rustlua_tilemap_of = function(id)
  local m = __rustlua_tilemaps[id]
  if not m then return nil end
  return m
end
__rustlua_cell_index = function(m, x, y)
  x, y = tonumber(x), tonumber(y)
  if not x or not y or x < 0 or y < 0 or x >= m.width or y >= m.height then return nil end
  return math.floor(y) * m.width + math.floor(x) + 1
end
tilemap = {
  -- 引擎的 tilemap.get 带 texture（season_visual 靠它判断"是否已是目标地表"，
  -- 少了这个字段就会每帧重复写组件、网格反复重建）。
  get = function(id)
    local m = __rustlua_tilemap_of(id)
    if not m then return nil end
    return { width = m.width, height = m.height, tile_size = m.tile_size, texture = m.texture }
  end,
  is_solid = function(id, x, y)
    local m = __rustlua_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local i = __rustlua_cell_index(m, x, y)
    if not i then return false end
    return string.sub(m.solid, i, i) == '1'
  end,
  get_cell = function(id, x, y)
    local m = __rustlua_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local i = __rustlua_cell_index(m, x, y)
    if not i then return 0 end
    return tonumber(string.sub(m.tiles, i, i)) or 0
  end,
  set_cell = function(id, x, y, value)
    local m = __rustlua_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local cx, cy = math.floor(tonumber(x) or -1), math.floor(tonumber(y) or -1)
    local i = __rustlua_cell_index(m, cx, cy)
    if not i then error('tilemap cell (' .. tostring(x) .. ', ' .. tostring(y) .. ') is out of bounds', 2) end
    -- Lua 侧自身也要跟着变：tilemap.get_cell 必须读到刚写进去的值（farm 依赖它
    -- 判断「这块地已经锄过了」，只发渲染 op 会让两边数据分叉）。
    -- 单字符编码与引擎一致（Tilemap::to_component_str 的 char::from(b'0' + cell)）。
    local code = math.floor(tonumber(value) or 0)
    m.tiles = string.sub(m.tiles, 1, i - 1) .. string.char(48 + code) .. string.sub(m.tiles, i + 1)
    __rustlua_emit('C\t' .. id .. '\t' .. cx .. '\t' .. cy .. '\t' .. tostring(value))
    return true
  end,
  -- 阻挡层改写（建筑放置/拆除走这里）：引擎侧会重建该图的静态刚体
  -- （refresh_tilemap_physics），web 侧同样要**当场**影响移动——Lua 的碰撞表
  -- 与渲染侧的 map.solid 一起改，否则"放下去的建筑不挡路"。
  set_solid = function(id, x, y, value)
    local m = __rustlua_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local cx, cy = math.floor(tonumber(x) or -1), math.floor(tonumber(y) or -1)
    local i = __rustlua_cell_index(m, cx, cy)
    if not i then error('tilemap cell (' .. tostring(x) .. ', ' .. tostring(y) .. ') is out of bounds', 2) end
    local on = value and value ~= false and value ~= 0
    m.solid = string.sub(m.solid, 1, i - 1) .. (on and '1' or '0') .. string.sub(m.solid, i + 1)
    __rustlua_emit('Z\t' .. id .. '\t' .. cx .. '\t' .. cy .. '\t' .. (on and 1 or 0))
    return true
  end,
  set_texture = function(id, texture)
    local m = __rustlua_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local path = tostring(texture or '')
    if path:match('^%s*$') then error('tilemap texture must not be empty', 2) end
    m.texture = path
    __rustlua_emit('U\t' .. id .. '\t' .. path)
    return true
  end,
  -- A* 寻路（引擎 core/tilemap.rs 的 Map::find_path 同构）：返回 **1 基** 的
  -- {x=,y=} 格数组（含起终点），走不通返回 nil + 原因字符串。
  -- 开放集用二叉堆：线性取最小在 64x64 图上单次寻路
  -- 要百万级比较，NPC/敌人每帧重寻路会把帧时间吃光。
  find_path = function(id, from_x, from_y, to_x, to_y, diagonal)
    local m = __rustlua_tilemap_of(id)
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
health = { get = function(id) return __rustlua_health[id] or 100 end, set = function(id, hp) __rustlua_health[id] = hp end, damage = function(id, amount) __rustlua_health[id] = (health.get(id) or 100) - amount; return __rustlua_health[id] end }
physics = {
  -- 场景空间**宽相**：按实体 Transform 位置做圆查询，与 native（core/script.rs
  -- 同名 API）语义一致：返回半径内所有存活实体 id 的 1 基数组（含调用者自己）。
  -- 此前 web 端缺失该 API：combat.lua 判定窗内每帧调用它，每帧打一条
  -- [lua missing-host-api] physics.positions_in_radius（微信端实测点一次攻击
  -- 后控制台刷屏 2.6 万条）。实现后缺口消失、战斗宽相真正生效。
  positions_in_radius = function(x, y, radius)
    local cx, cy, r = tonumber(x), tonumber(y), tonumber(radius)
    local finite = function(v) return v ~= nil and v == v and v ~= math.huge and v ~= -math.huge end
    if not finite(cx) or not finite(cy) or not finite(r) or r < 0 then
      error('physics.positions_in_radius arguments must be finite', 2)
    end
    local result = {}
    local n = 1
    local r2 = r * r
    for id in pairs(__rustlua_alive) do
      local pos = __rustlua_transforms[id]
      if pos then
        local dx = (pos.x or 0) - cx
        local dy = (pos.y or 0) - cy
        if dx * dx + dy * dy <= r2 then
          result[n] = id
          n = n + 1
        end
      end
    end
    return result
  end,
  set_velocity = function(id, x, y) __rustlua_velocity[id] = { x = x or 0, y = y or 0 }; return true end,
  -- 2D 传送（引擎 M17-01）：**刚体与 ECS 同步落位**，只改 transform 会被下一帧
  -- 物理回写覆盖成旧位置（player.teleport 的注释里就写着这条）。这里两者都写。
  set_position = function(id, x, y)
    local nx, ny = tonumber(x), tonumber(y)
    if not nx or not ny or nx ~= nx or ny ~= ny or nx == math.huge or nx == -math.huge or ny == math.huge or ny == -math.huge then
      error('physics.set_position must be finite', 2)
    end
    __rustlua_require_alive(id)
    local p = __rustlua_transforms[id] or { rotation = 0, scale_x = 1, scale_y = 1 }
    p.x, p.y = nx, ny
    __rustlua_transforms[id] = p
    __rustlua_velocity[id] = { x = 0, y = 0 }
    return true
  end,
  events = function() return {} end
}
physics3d = { set_velocity = function(...) return true end, set_position = function(...) return true end, events = function() return {} end }
-- 单帧贴图：入参是 Sprite **组件串**（texture=..;size=..），与场景里写的是同一种，
-- 解析交给渲染侧同一份 applyComponents（这里只登记，syncLuaScene 每帧取走）。
sprite = {
  set = function(id, component)
    __rustlua_require_alive(id)
    local spec = tostring(component or '')
    if spec:match('^%s*$') then error('sprite component must not be empty', 2) end
    __rustlua_sprites[id] = spec
    __rustlua_emit('S\t' .. id .. '\t' .. spec)
    return true
  end,
  get = function(id)
    __rustlua_require_alive(id)
    local spec = __rustlua_sprites[id]
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
function __rustlua_parse_animation(spec)
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
function __rustlua_advance_animation(a, delta)
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
    __rustlua_require_alive(id)
    local a = __rustlua_animations[id]
    if not a then error('entity has no animation', 2) end
    return a.current
  end,
  set = function(id, name)
    __rustlua_require_alive(id)
    local a = __rustlua_animations[id]
    if not a then error('entity has no animation', 2) end
    local clip = a.clips and a.clips[name]
    if not clip then error('unknown animation clip: ' .. tostring(name), 2) end
    a.current, a.time, a.ended, a.frame = name, 0, false, 0
    return true
  end,
  -- 与引擎一致：**读取并清除** ended 标志（take_ended 语义），不是常真。
  ended = function(id)
    __rustlua_require_alive(id)
    local a = __rustlua_animations[id]
    if not a then error('entity has no animation', 2) end
    local value = a.ended == true
    a.ended = false
    return value
  end,
  frame = function(id)
    __rustlua_require_alive(id)
    local a = __rustlua_animations[id]
    if not a then error('entity has no animation', 2) end
    return a.frame or 0
  end,
  -- 运行期挂动画（引擎 M21）：spawn 出来的实体只有 Transform，形象靠这里补上。
  attach = function(id, component)
    __rustlua_require_alive(id)
    local spec = tostring(component or '')
    local machine = __rustlua_parse_animation(spec)
    if next(machine.clips) == nil then
      error('animation component has no clip: ' .. spec, 2)
    end
    __rustlua_animations[id] = machine
    __rustlua_emit('X\t' .. id .. '\t' .. spec)
    return true
  end
}
animation3d = { set = function(...) return true end }
-- 音频宿主 API。真实后端在 JS 侧（installLuaAudioBridge，见 web-runtime.js）：
-- 微信小游戏走 wx.createInnerAudioContext，浏览器走 HTML5 Audio，统一 MP3 格式。
-- **返回值语义必须与引擎一致**：audio.play_on 播放失败要返回 false + 原因，
-- 游戏侧 core/audio.lua 据此把该 bus:name 记进 failed 表、不再重试；
-- 若这里返回无意义的成功，游戏会每帧重试同一段缺资源的音效。
-- 桥没接上（后端没装）时仍然返回 false，而不是 error 出去——音频缺失不能中断玩法。
__rustlua_audio_buses = { master = true, bgm = true, sfx = true, ui = true }
__rustlua_audio_volume = { master = 1, bgm = 1, sfx = 1, ui = 1 }
audio = {
  volume = function(value)
    if value ~= nil then
      local v = tonumber(value)
      if v then
        if v < 0 then v = 0 elseif v > 1 then v = 1 end
        __rustlua_audio_volume.master = v
      end
    end
    if __rustlua_audio_js_set_volume then __rustlua_audio_js_set_volume('master', __rustlua_audio_volume.master) end
    return true
  end,
  music = function(name)
    if type(name) ~= 'string' or name == '' then return false, 'audio name must not be empty' end
    if not __rustlua_audio_js_music then return false, 'web platform has no audio backend' end
    return __rustlua_audio_js_music(name)
  end,
  play = function(name) return audio.play_on('sfx', name) end,
  stop_music = function()
    if __rustlua_audio_js_stop_music then __rustlua_audio_js_stop_music() end
    return true
  end,
  set_bus_volume = function(bus, level)
    bus = tostring(bus or '')
    if not __rustlua_audio_buses[bus] then error('unknown audio bus: ' .. bus, 2) end
    local value = tonumber(level)
    if not value then error('audio.set_bus_volume level must be a number', 2) end
    if value < 0 then value = 0 elseif value > 1 then value = 1 end
    __rustlua_audio_volume[bus] = value
    if __rustlua_audio_js_set_volume then __rustlua_audio_js_set_volume(bus, value) end
    return true
  end,
  bus_volume = function(bus)
    bus = tostring(bus or '')
    if not __rustlua_audio_buses[bus] then error('unknown audio bus: ' .. bus, 2) end
    return __rustlua_audio_volume[bus]
  end,
  effective_volume = function(bus)
    bus = tostring(bus or '')
    if not __rustlua_audio_buses[bus] then error('unknown audio bus: ' .. bus, 2) end
    if bus == 'master' then return __rustlua_audio_volume.master end
    return __rustlua_audio_volume.master * __rustlua_audio_volume[bus]
  end,
  -- 真实播放（微信 InnerAudioContext / 浏览器 HTML5 Audio）；文件不存在或后端桥没接上才失败。
  play_on = function(bus, name)
    bus = tostring(bus or '')
    if not __rustlua_audio_buses[bus] then error('unknown audio bus: ' .. bus, 2) end
    if type(name) ~= 'string' or name == '' then return false, 'audio name must not be empty' end
    if not __rustlua_audio_js_play_on then return false, 'web platform has no audio backend' end
    return __rustlua_audio_js_play_on(bus, name)
  end
}
ui = {}
function ui.panel(id, x, y, w, h, r, g, b, a)
  __rustlua_ui_panels[id] = { x = x, y = y, w = w, h = h, r = r, g = g, b = b, a = a, visible = true, shape = 0 }
end
-- 圆形与 ui.panel **共用同一张表**：引擎 ui.circle 写的就是 ui_panels，只多一个
-- shape = 1（core/ui.rs 的 UiPanel.shape：0 = 矩形、1 = 圆形）。因此两边共享 id
-- 命名空间，ui.panel_get / ui.hide / ui.clear 对圆同样生效。
-- position/size 存的是**外接矩形**（引擎存 [cx-r, cy-r] / [2r, 2r]），绘制侧取
-- center = position + size/2、r = min(w,h)/2——与 runtime_scene.rs push_ui_panels
-- 的三角扇几何逐项对齐（32 段填充圆）。
function ui.circle(id, cx, cy, radius, r, g, b, a)
  local value = tonumber(radius) or 0
  __rustlua_ui_panels[id] = { x = cx - value, y = cy - value, w = value * 2, h = value * 2, r = r, g = g, b = b, a = a, visible = true, shape = 1 }
end
function ui.image(id, x, y, w, h, texture, r, g, b, slice)
  local s = nil
  if slice ~= nil and tostring(slice) ~= '' then s = tostring(slice) end
  __rustlua_ui_images[id] = { x = x, y = y, w = w, h = h, texture = tostring(texture or ''), r = r or 1, g = g or 1, b = b or 1, slice = s, visible = true }
end
function ui.bar(id, x, y, w, h, ratio, r, g, b)
  local value = tonumber(ratio) or 0
  if value < 0 then value = 0 elseif value > 1 then value = 1 end
  __rustlua_ui_bars[id] = { x = x, y = y, w = w, h = h, ratio = value, r = r or 1, g = g or 1, b = b or 1, visible = true }
end
function ui.text_at(id, x, y, text, scale, size)
  __rustlua_ui_texts[id] = { x = x, y = y, text = tostring(text or ''), scale = scale or 1, size = size, visible = true }
end
function ui.text(id, text)
  __rustlua_ui_texts[id] = { x = 10, y = 10, text = tostring(text or ''), scale = 1, visible = true }
end
function ui.hide(id)
  for _, store in ipairs({ __rustlua_ui_panels, __rustlua_ui_texts, __rustlua_ui_bars, __rustlua_ui_images }) do
    local entry = store[id]
    if entry then entry.visible = false end
  end
end
function ui.show(id)
  for _, store in ipairs({ __rustlua_ui_panels, __rustlua_ui_texts, __rustlua_ui_bars, __rustlua_ui_images }) do
    local entry = store[id]
    if entry then entry.visible = true end
  end
end
-- 场景文字实体显隐（按实体名，如 BtnStartLabel）：true/省略=显示，false=隐藏。
-- 标题界面把按钮标签接管为脚本绘制后，场景 Text 实体须隐藏，否则两层文字重叠。
__rustlua_scene_text_hidden = {}
function ui.scene_text_visible(name, visible)
  local key = tostring(name or '')
  if visible == false then __rustlua_scene_text_hidden[key] = true else __rustlua_scene_text_hidden[key] = nil end
end
function __rustlua_export_scene_text_hidden()
  local out = {}
  for k in pairs(__rustlua_scene_text_hidden) do out[#out + 1] = k end
  return table.concat(out, ',')
end
function ui.clear(id)
  for _, store in ipairs({ __rustlua_ui_panels, __rustlua_ui_texts, __rustlua_ui_bars, __rustlua_ui_images }) do
    store[id] = nil
    store[id .. ':title'] = nil
    store[id .. ':tooltip'] = nil
  end
end
function ui.panel_get(id) return __rustlua_ui_panels[id] end
function ui.image_get(id) return __rustlua_ui_images[id] end
function ui.get(id) return __rustlua_ui_texts[id] end
function ui.grid(...) return true end
function ui.measure_text(text, size)
  local s = tostring(text or '')
  local count = 0
  for _ in s:gmatch('[^\\128-\\191]') do count = count + 1 end
  local px = tonumber(size) or 16
  return { width = count * px * 0.6, height = px }
end
function __rustlua_export_ui()
  local rows = {}
  local function num(v) return tostring(v or 0) end
  local function flat(v) return string.gsub(tostring(v or ''), '[\\t\\r\\n]', ' ') end
  for id, p in pairs(__rustlua_ui_panels) do
    if p.visible then
      rows[#rows + 1] = 'P\\t' .. flat(id) .. '\\t' .. num(p.x) .. '\\t' .. num(p.y) .. '\\t' .. num(p.w) .. '\\t' .. num(p.h) .. '\\t' .. num(p.r) .. '\\t' .. num(p.g) .. '\\t' .. num(p.b) .. '\\t' .. num(p.a) .. '\\t' .. num(p.shape or 0)
    end
  end
  for id, i in pairs(__rustlua_ui_images) do
    if i.visible then
      rows[#rows + 1] = 'I\\t' .. flat(id) .. '\\t' .. num(i.x) .. '\\t' .. num(i.y) .. '\\t' .. num(i.w) .. '\\t' .. num(i.h) .. '\\t' .. flat(i.texture) .. '\\t' .. num(i.r) .. '\\t' .. num(i.g) .. '\\t' .. num(i.b) .. '\\t' .. flat(i.slice or '')
    end
  end
  for id, b in pairs(__rustlua_ui_bars) do
    if b.visible then
      rows[#rows + 1] = 'B\\t' .. flat(id) .. '\\t' .. num(b.x) .. '\\t' .. num(b.y) .. '\\t' .. num(b.w) .. '\\t' .. num(b.h) .. '\\t' .. num(b.ratio) .. '\\t' .. num(b.r) .. '\\t' .. num(b.g) .. '\\t' .. num(b.b)
    end
  end
  for id, t in pairs(__rustlua_ui_texts) do
    if t.visible then
      rows[#rows + 1] = 'X\\t' .. flat(id) .. '\\t' .. num(t.x) .. '\\t' .. num(t.y) .. '\\t' .. num(t.size) .. '\\t' .. num(t.scale) .. '\\t' .. flat(t.text)
    end
  end
  return table.concat(rows, '\\n')
end
save = {
  read = function(key) return __rustlua_save[key] end,
  write = function(key, value) __rustlua_save[key] = tostring(value) end,
  load = function() return true end,
  commit = function() return true end,
  -- 「开始新游戏」必须能丢弃引擎侧持久键：__map / __map:spawn:* / __mapstate:*。
  -- 游戏侧 state.reset() 只清 Lua 表，残留会让新档读回上一局的地图与地图状态
  -- （同进程内「继续 → 玩 → 返回标题 → 开始新游戏」即可复现，main.lua 的注释写着
  -- 顺序固定：先清引擎键，再让各模块写自己的空快照）。返回清掉的键数，与引擎一致。
  clear = function()
    local removed = 0
    for _ in pairs(__rustlua_save) do removed = removed + 1 end
    for _ in pairs(__rustlua_map_state) do removed = removed + 1 end
    __rustlua_save = {}
    __rustlua_map_state = {}
    __rustlua_map_current = nil
    __rustlua_map_spawn = nil
    log.info('cleared ' .. removed .. ' in-memory key(s)')
    return removed
  end
}
particles = { burst = function(...) return true end }
camera = { set = function(...) return true end, set_active = function(...) return true end }
mesh = { set = function(...) return true end }
os = os or {}; os.getenv = function(_) return nil end
-- ---------------------------------------------------------------------------
-- 固体格碰撞（web 侧的第二套实现，几何与引擎一致）
-- ---------------------------------------------------------------------------
-- 引擎侧的事实（rustlua-scripting/src/script/mod.rs）：
--   * solid_cell_colliders：每个 Tilemap.solid 格 → 一个静态 cuboid，
--     半宽/半高 = 格尺寸/2，偏移 = 该格中心；refresh_tilemap_physics 保证运行期
--     tilemap.set_solid 立刻重建静态体。
--   * 实体按场景 Collider 组件（ball/cuboid）生成刚体，只有**有刚体的实体**参与碰撞
--     （本工程的玩法地图里只有主角 JianghuPlayer 有 Collider）。
-- web 侧没有物理世界：physics.set_velocity 只是一张速度表，位移靠下面这段自己积分。
-- 因此这里必须自己实现同一套阻挡——否则主角会直接穿过院墙/水面/屋基
-- （实机 2026-10-03：web 上"角色可以穿越任何阻碍点"）。
--
-- 做法：把实体近似成以自身为心的 AABB，按**分轴 + 子步**移动（每轴最多 1px 一步）：
-- 该轴撞上就停、另一轴照走 ⇒ 贴墙滑动，与 Rapier 的手感一致；只对带 Collider 的
-- 实体生效（没刚体的实体照旧自由移动，与引擎"没刚体不参与碰撞"等价）。
function __rustlua_cell_solid(m, cx, cy)
  if cx < 0 or cy < 0 or cx >= m.width or cy >= m.height then return true end
  local i = cy * m.width + cx + 1
  return string.sub(m.solid, i, i) == '1'
end

function __rustlua_blocked_at(id, x, y)
  local colliders = __rustlua_colliders
  local col = colliders and colliders[id]
  if not col or col.sensor then return false end
  local hx, hy = col.hx or 0, col.hy or 0
  local px, py = x + (col.ox or 0), y + (col.oy or 0)
  for map_id, m in pairs(__rustlua_tilemaps) do
    local t = __rustlua_transforms[map_id] or { x = 0, y = 0 }
    local ts = m.tile_size or 32
    local ox, oy = t.x or 0, t.y or 0
    -- 与格边重合不算撞（引擎里球贴着实心面停住，不会越界到邻格）。
    local cx0 = math.floor((px - hx - ox) / ts)
    local cx1 = math.floor((px + hx - ox) / ts - 1e-4)
    local cy0 = math.floor((py - hy - oy) / ts)
    local cy1 = math.floor((py + hy - oy) / ts - 1e-4)
    for cy = cy0, cy1 do
      for cx = cx0, cx1 do
        if __rustlua_cell_solid(m, cx, cy) then return true end
      end
    end
  end
  return false
end

function __rustlua_move_with_collision(id, dx, dy)
  local p = __rustlua_transforms[id]
  if not p then return end
  local colliders = __rustlua_colliders
  if not (colliders and colliders[id]) then
    p.x, p.y = (p.x or 0) + dx, (p.y or 0) + dy
    __rustlua_transforms[id] = p
    return
  end
  local n = math.max(1, math.ceil(math.max(math.abs(dx), math.abs(dy))))
  local sx, sy = dx / n, dy / n
  local x, y = p.x or 0, p.y or 0
  for _ = 1, n do
    if not __rustlua_blocked_at(id, x + sx, y) then x = x + sx end
    if not __rustlua_blocked_at(id, x, y + sy) then y = y + sy end
  end
  p.x, p.y = x, y
  __rustlua_transforms[id] = p
end

-- 每帧把 Lua 侧的场景状态交给渲染器（syncLuaScene 解析）。
--   step=false 用于 on_create 之后的"只看不改"快照：不推进速度积分与动画帧，
--   否则一次切换会白白多走一帧动画。
-- 行格式：R=spawn（建实体） S=sprite.set（Sprite 组件串） X=animation.attach
--        C=set_cell  Z=set_solid  U=set_texture  D=destroy  T=Transform  A=动画当前 clip + 帧序号
-- R/D 这类结构变更只发一次（op 列表取走即清）；T/A 每帧对**存活**实体重新发。
function __rustlua_export_state(step)
  local rows = __rustlua_ops
  __rustlua_ops = {}
  local advance = step ~= false
  if advance then
    for id, v in pairs(__rustlua_velocity) do
      if __rustlua_alive[id] == true then
        __rustlua_move_with_collision(id, (v.x or 0) * time.delta(), (v.y or 0) * time.delta())
      end
    end
    for _, a in pairs(__rustlua_animations) do __rustlua_advance_animation(a, time.delta()) end
  end
  for id, p in pairs(__rustlua_transforms) do
    if __rustlua_alive[id] == true then
      rows[#rows + 1] = 'T\t' .. id .. '\t' .. tostring(p.x or 0) .. '\t' .. tostring(p.y or 0) .. '\t' .. tostring(p.rotation or 0) .. '\t' .. tostring(p.scale_x or 1) .. '\t' .. tostring(p.scale_y or 1)
    end
  end
  for id, a in pairs(__rustlua_animations) do
    if __rustlua_alive[id] == true then
      rows[#rows + 1] = 'A\t' .. id .. '\t' .. tostring(a.current or 'idle') .. '\t' .. tostring(a.frame or 0)
    end
  end
  return table.concat(rows, '\\n')
end

-- 一行世界自报，供宿主诊断覆盖层显示：真机/开发者工具里"看不见世界状态"是排查黑屏
-- 与"游戏在跑却没进图"的最大障碍。写法保持通用：不认工程名，主角按名字里含 player
-- 识别（渲染器 entityColor 判定主角用的是同一套启发）。
function __rustlua_world_summary()
  local alive, player_name, player_id = 0, nil, nil
  -- 两个 O(n) 遍历，不做嵌套：这个函数每帧都要出结果（宿主覆盖层与验收都要读实时值）。
  for _, value in pairs(__rustlua_alive) do
    if value == true then alive = alive + 1 end
  end
  for name, mapped in pairs(__rustlua_names) do
    if type(name) == 'string' and __rustlua_alive[mapped] == true and name:lower():find('player', 1, true) then
      player_name, player_id = name, mapped
    end
  end
  local parts = {
    'map=' .. tostring(__rustlua_map_current or __rustlua_scene_name),
    'spawn=' .. (__rustlua_map_spawn and (math.floor((__rustlua_map_spawn.x or 0) + 0.5) .. ',' .. math.floor((__rustlua_map_spawn.y or 0) + 0.5)) or 'none'),
    'alive=' .. alive
  }
  if player_id and __rustlua_transforms[player_id] then
    local p = __rustlua_transforms[player_id]
    local machine = __rustlua_animations[player_id]
    parts[#parts + 1] = 'player=' .. tostring(player_name)
      .. '@' .. math.floor((p.x or 0) + 0.5) .. ',' .. math.floor((p.y or 0) + 0.5)
      .. (machine and ('/' .. tostring(machine.current)) or '')
  end
  return table.concat(parts, ' ')
end
`;
  }


  // ---------- 音频后端（微信小游戏：InnerAudioContext；浏览器：HTML5 Audio） ----------
  //
  // Lua 侧 audio 表经 __rustlua_audio_js_* 全局调用到这里。微信端用
  // wx.createInnerAudioContext() 播放（官方推荐 API，支持流式、适合 BGM 与短音效），
  // 浏览器端用 new Audio()——**两侧对外语义必须一致**（缺文件返回 (false, 原因)，
  // 游戏侧 core/audio.lua 据此把 bus:name 记进 failed 表、不再重试；返回无意义的
  // 成功会让它每帧重试同一段缺资源的音效）。
  // 格式统一为 MP3：InnerAudioContext 在 iOS 和 Android 均支持（ogg 在 iOS 不支持，
  // wav 体积过大）；浏览器三种格式都能放，但扩展名次序与引擎 sound_bytes 对齐
  // （mp3 → wav → ogg，见 rustlua-audio/src/audio.rs），这样"同一个名字在 PC 放哪
  // 个文件"在 web 上也放同一个。
  //
  // `audioIndex` 是 pack/manifest.txt 里的资源路径集合（发布时真实随包分发的那批）。
  // 浏览器没有微信的 fs.accessSync，"文件在不在"**只能问 manifest**：先查再播才能
  // 同步返回 false，否则只能先乐观返回 true、等 onerror 落地，那期间游戏已经按成功
  // 记过账了。manifest 缺失时退回乐观模式（onerror 会把该名字补进 missing）。
  function installLuaAudioBridge(audioIndex) {
    const isWechat = typeof wx !== 'undefined' && typeof wx.createInnerAudioContext === 'function';
    const isBrowser = !isWechat && typeof Audio === 'function';
    // 音量状态（与 Lua 侧 __rustlua_audio_volume 保持一致；Lua 是事实来源，这里只用于播放时应用）。
    const volumes = { master: 1, bgm: 1, sfx: 1, ui: 1 };
    const clamp01 = function (v) { const n = Number(v); if (!(n >= 0)) return 0; return n > 1 ? 1 : n; };
    const effective = function (bus) {
      if (bus === 'master') return volumes.master;
      const group = volumes[bus] === undefined ? 1 : volumes[bus];
      return volumes.master * group;
    };
    // 一次性音效池：[{bus,name,ctx,alive}, ...]（同一音效允许重叠播放）。
    // 浏览器侧 ctx 就是 HTMLAudioElement，下面的 destroyCtx/applyVolume 两侧共用。
    const sfxPool = [];
    // 循环 BGM（同时只有一个）。
    let musicCtx = null;
    // 播放失败过的音频名：与微信侧 accessSync 探不到文件同源，之后一律返回 false，
    // 让游戏侧把它记进 failed 表（浏览器的"不存在"是异步 onerror，这里缓存成同步判据）。
    const missing = new Set();
    // 浏览器自动播放策略：首次用户手势之前 play() 一律 reject（NotAllowedError）。
    // 音效都发生在交互之后，只有标题 BGM 在手势前就要响，所以记下来等手势补播。
    let pendingMusicEl = null;
    let gestureSeen = !isBrowser;
    // 验收接缝：微信侧由 harness 的 wx mock 统计（audio_ctx/audio_played），浏览器
    // 侧没有 mock 可计，后端自己记同样的三条读数供 CDP 断言。游戏逻辑不读它。
    const backendStats = { kind: isWechat ? 'wechat' : (isBrowser ? 'browser' : 'none'), created: 0, played: 0, gated: 0, music: null, musicPlayed: 0, srcs: [] };
    if (typeof window !== 'undefined') window.__RUSTLUA_AUDIO = backendStats;
    // 浏览器侧多给一个探针：`play()` 的 Promise resolve 只说明"开始放"，currentTime
    // 在前进才说明"真的在出声"。验收与"玩家说没声音"时的排查读的是同一个数。
    if (isBrowser) {
      backendStats.musicProbe = function () {
        if (!musicCtx) return null;
        return {
          src: musicCtx.currentSrc, currentTime: musicCtx.currentTime,
          paused: musicCtx.paused, volume: musicCtx.volume, loop: musicCtx.loop,
          readyState: musicCtx.readyState, networkState: musicCtx.networkState
        };
      };
    }
    // 后端读数抄到 body 的 data-rustlua-audio 上：产物层验收只 dump DOM
    // （见 scripts/webgl-cdp-wait.js），没有这个属性，"浏览器到底有没有出声"就永远
    // 进不了门——与微信侧 harness 的 audio_ctx/audio_played 是同一个接缝。
    function syncAudioMarker() {
      if (typeof document === 'undefined' || !document.body) return;
      document.body.setAttribute('data-rustlua-audio',
        'kind=' + backendStats.kind + ' created=' + backendStats.created +
        ' played=' + backendStats.played + ' gated=' + backendStats.gated +
        ' music_played=' + backendStats.musicPlayed +
        ' music=' + String(backendStats.music || 'none'));
    }
    syncAudioMarker();

    // 候选 src：微信宿主对不同路径形式的接受程度不一致（分包根相对等），
    // 与 wechat-adapter.js 的 readStrategies 同理，逐个尝试（pack/ 是资源根）。
    function srcCandidates(name) {
      const logical = 'assets/audio/' + name + '.mp3';
      return ['pack/' + logical, logical, './pack/' + logical];
    }
    function fileExists(path) {
      if (!isWechat) return false;
      try {
        const fs = (typeof wx.getFileSystemManager === 'function') && wx.getFileSystemManager();
        if (!fs || typeof fs.accessSync !== 'function') return true; // 无法探测：假定存在，交给 onError
        fs.accessSync(path);
        return true;
      } catch (error) { return false; }
    }
    function resolveSrc(name) {
      if (!isWechat) return null;
      const list = srcCandidates(name);
      for (let i = 0; i < list.length; i += 1) {
        if (fileExists(list[i])) return list[i];
      }
      return null;
    }
    // 浏览器侧：路径就是"页面根 + pack/"（与 loadImage / fetchText 同一套，不需要
    // 微信那三种路径方言），存在性问 manifest，扩展名次序与引擎一致。
    const AUDIO_EXTS = ['mp3', 'wav', 'ogg'];
    function browserSrc(name) {
      for (let i = 0; i < AUDIO_EXTS.length; i += 1) {
        const logical = 'assets/audio/' + name + '.' + AUDIO_EXTS[i];
        if (!audioIndex || audioIndex.has(logical)) return encodeURI('pack/' + logical);
      }
      return null;
    }
    function resolveAnySrc(name) {
      if (missing.has(name)) return null;
      return isWechat ? resolveSrc(name) : (isBrowser ? browserSrc(name) : null);
    }
    function destroyCtx(ctx) {
      // 先立"已释放"标记：下面摘 src 会 load()，浏览器规范里"没有 src 的 load()"会
      // 补发一个 error 事件，不挡掉它的话被淘汰的音效会被误记成"文件不存在"，
      // 同一个名字下次就永远不响了。
      try { ctx.__rustlua_released = true; } catch (error) { }
      try { if (typeof ctx.offEnded === 'function') ctx.offEnded(); } catch (error) { }
      try { if (typeof ctx.offError === 'function') ctx.offError(); } catch (error) { }
      try { if (typeof ctx.stop === 'function') ctx.stop(); } catch (error) { }
      // 浏览器：pause 之后还要摘掉 src 并 load()，否则一个正在下载的音频不会因为
      // 元素被丢弃就停——切图时淘汰旧音效池会白占带宽。
      try { if (typeof ctx.pause === 'function') ctx.pause(); } catch (error) { }
      try { if (typeof ctx.removeAttribute === 'function') { ctx.removeAttribute('src'); ctx.load(); } } catch (error) { }
      try { if (typeof ctx.destroy === 'function') ctx.destroy(); } catch (error) { }
    }
    function applyVolume(ctx, bus) {
      try { ctx.volume = clamp01(effective(bus)); } catch (error) { /* 宿主只读时忽略 */ }
    }
    function pruneSfxPool() {
      for (let i = sfxPool.length - 1; i >= 0; i -= 1) {
        if (!sfxPool[i].alive) sfxPool.splice(i, 1);
      }
    }
    // 浏览器一次性音效。ended/error 用 addEventListener（微信侧是 onEnded/onError 回调），
    // 淘汰与池管理仍走共用的 sfxPool。
    function playBrowserSfx(bus, name, src) {
      let el;
      try { el = new Audio(src); } catch (error) { return { ok: false, reason: 'Audio constructor failed: ' + error }; }
      const entry = { bus: bus, name: name, ctx: el, alive: true };
      const markDead = function () { entry.alive = false; destroyCtx(el); };
      try {
        el.addEventListener('ended', markDead);
        el.addEventListener('error', function () { if (el.__rustlua_released) return; missing.add(name); markDead(); });
      } catch (error) { /* 宿主不提供事件时忽略 */ }
      applyVolume(el, bus);
      backendStats.created += 1;
      if (backendStats.srcs.length < 8) backendStats.srcs.push(src);
      // 音效被手势闸门拦下时直接淘汰（补播一个"三秒前该响的点击声"比不响更怪），
      // BGM 才需要等手势（见 playBrowserMusic）。
      startPlayback(el.play(), markDead);
      syncAudioMarker();
      sfxPool.push(entry);
      pruneSfxPool();
      // 并发音效过多时淘汰最旧的（防止长时间运行内存泄漏）。
      if (sfxPool.length > 48) {
        const victim = sfxPool.shift();
        if (victim) destroyCtx(victim.ctx);
      }
      return { ok: true, src: src };
    }
    // play() 的三种结局：真正开播（played 计数）、被自动播放策略拦下（gated 计数，
    // 由 onNotAllowed 把元素留着等手势补播）、源出错（error 监听器已记 missing）。
    // 收不到 Promise 的老宿主按"已发起"计，与微信侧同步 play() 的口径一致。
    function startPlayback(promise, onNotAllowed, onPlayed) {
      if (!promise || typeof promise.then !== 'function') { backendStats.played += 1; if (onPlayed) onPlayed(); syncAudioMarker(); return; }
      promise.then(function () {
        backendStats.played += 1;
        if (onPlayed) onPlayed();
        syncAudioMarker();
      }).catch(function (error) {
        if (error && error.name === 'NotAllowedError') {
          backendStats.gated += 1;
          if (onNotAllowed) onNotAllowed();
          syncAudioMarker();
        }
      });
    }
    function playOn(bus, name) {
      const src = resolveAnySrc(name);
      if (!src) {
        if (!isWechat && !isBrowser) return { ok: false, reason: 'web platform has no audio backend' };
        return { ok: false, reason: isWechat ? 'audio file not found: ' + name : 'no .mp3/.wav/.ogg clip found for ' + name };
      }
      if (isBrowser) return playBrowserSfx(bus, name, src);
      let ctx;
      try { ctx = wx.createInnerAudioContext(); } catch (error) { return { ok: false, reason: 'createInnerAudioContext failed: ' + error }; }
      try { ctx.src = src; } catch (error) { destroyCtx(ctx); return { ok: false, reason: 'set src failed: ' + error }; }
      const entry = { bus: bus, name: name, ctx: ctx, alive: true };
      const onEnd = function () { entry.alive = false; destroyCtx(ctx); };
      try {
        ctx.onEnded(onEnd);
        ctx.onError(onEnd);
      } catch (error) { /* 宿主未提供回调时忽略 */ }
      applyVolume(ctx, bus);
      backendStats.created += 1;
      try { ctx.play(); } catch (error) { destroyCtx(ctx); return { ok: false, reason: 'play failed: ' + error }; }
      // 微信的 play() 是同步调用，没有 Promise 可等：发起即计入 played。
      backendStats.played += 1;
      syncAudioMarker();
      sfxPool.push(entry);
      pruneSfxPool();
      // 并发音效过多时淘汰最旧的（防止长时间运行内存泄漏）。
      if (sfxPool.length > 48) {
        const victim = sfxPool.shift();
        if (victim) destroyCtx(victim.ctx);
      }
      return { ok: true, src: src };
    }
    // 浏览器 BGM：单个循环元素。被自动播放策略拦下时把元素存进 pendingMusicEl，
    // 首次手势到了再 play() 一次（元素留着才不用重新下载）。
    function playBrowserMusic(name, src) {
      if (musicCtx) { destroyCtx(musicCtx); musicCtx = null; }
      pendingMusicEl = null;
      let el;
      try { el = new Audio(src); } catch (error) { return { ok: false, reason: 'Audio constructor failed: ' + error }; }
      try { el.loop = true; } catch (error) { /* 宿主只读时忽略 */ }
      try {
        el.addEventListener('error', function () {
          // 换曲/停播会摘 src，那次补发的 error 不代表文件不存在（见 destroyCtx 的标记）。
          if (el.__rustlua_released) return;
          missing.add(name);
          destroyCtx(el);
          if (musicCtx === el) musicCtx = null;
          if (pendingMusicEl === el) pendingMusicEl = null;
        });
      } catch (error) { /* 同上 */ }
      applyVolume(el, 'bgm');
      backendStats.created += 1;
      backendStats.music = src;
      const started = el.play();
      startPlayback(started, function () { pendingMusicEl = el; }, function () { backendStats.musicPlayed += 1; });
      syncAudioMarker();
      musicCtx = el;
      return { ok: true, src: src };
    }
    function music(name) {
      const src = resolveAnySrc(name);
      if (!src) {
        if (!isWechat && !isBrowser) return { ok: false, reason: 'web platform has no audio backend' };
        return { ok: false, reason: isWechat ? 'audio file not found: ' + name : 'no .mp3/.wav/.ogg clip found for ' + name };
      }
      if (isBrowser) return playBrowserMusic(name, src);
      if (musicCtx) { destroyCtx(musicCtx); musicCtx = null; }
      let ctx;
      try { ctx = wx.createInnerAudioContext(); } catch (error) { return { ok: false, reason: 'createInnerAudioContext failed: ' + error }; }
      try { ctx.loop = true; ctx.src = src; } catch (error) { destroyCtx(ctx); return { ok: false, reason: 'music setup failed: ' + error }; }
      try { ctx.onError(function () { /* BGM 播放失败仅静默（下一次切图会重试） */ }); } catch (error) { }
      applyVolume(ctx, 'bgm');
      backendStats.created += 1;
      backendStats.music = src;
      try { ctx.play(); } catch (error) { destroyCtx(ctx); return { ok: false, reason: 'music play failed: ' + error }; }
      backendStats.played += 1;
      backendStats.musicPlayed += 1;
      syncAudioMarker();
      musicCtx = ctx;
      return { ok: true, src: src };
    }
    function stopMusic() {
      pendingMusicEl = null;
      backendStats.music = null;
      if (musicCtx) { destroyCtx(musicCtx); musicCtx = null; }
      syncAudioMarker();
    }
    function setVolume(bus, value) {
      volumes[bus] = clamp01(value);
      for (let i = 0; i < sfxPool.length; i += 1) {
        if (sfxPool[i].alive) applyVolume(sfxPool[i].ctx, sfxPool[i].bus);
      }
      if (musicCtx) applyVolume(musicCtx, 'bgm');
    }
    // 自动播放解锁：BGM 在手势前就被 reject 过（pendingMusicEl）时，首次 pointerdown/
    // keydown 立刻补播。监听不摘掉，靠 gestureSeen 短路——标题界面之后还有整局游戏，
    // 少一个监听器不值得为此维护摘除逻辑。
    if (isBrowser && typeof window !== 'undefined') {
      const unlockAudio = function () {
        if (gestureSeen) return;
        gestureSeen = true;
        const el = pendingMusicEl;
        if (!el) return;
        pendingMusicEl = null;
        startPlayback(el.play(), null, function () { backendStats.musicPlayed += 1; });
      };
      window.addEventListener('pointerdown', unlockAudio, { capture: true });
      window.addEventListener('keydown', unlockAudio, { capture: true });
    }
    // WP10 迁移：JS 回调经 wasm 桥注册成 Lua 全局。回调收 Lua 参数（string/number/
    // boolean/nil → JS 基本值），返回 **数组 = 多返回值**（`[ok]` / `[ok, reason]`
    // 对应 fengari 版的多 push 语义），空数组 = 无返回。
    function registerLuaFunction(name, fn) {
      const bridge = window.__RUSTLUA_LUA_BRIDGE;
      if (!bridge) throw new Error('wasm lua bridge not loaded');
      bridge.lua_register_js_callback(name, fn);
    }
    registerLuaFunction('__rustlua_audio_js_play_on', function (bus, name) {
      bus = String(bus == null ? '' : bus);
      name = String(name == null ? '' : name);
      const result = playOn(bus, name);
      if (result.ok) return [true];
      return [false, result.reason || 'audio play failed'];
    });
    registerLuaFunction('__rustlua_audio_js_music', function (name) {
      name = String(name == null ? '' : name);
      const result = music(name);
      if (result.ok) return [true];
      return [false, result.reason || 'music failed'];
    });
    registerLuaFunction('__rustlua_audio_js_stop_music', function () { stopMusic(); return []; });
    registerLuaFunction('__rustlua_audio_js_set_volume', function (bus, value) {
      bus = String(bus == null ? '' : bus);
      const parsed = Number(value == null ? 1 : value);
      setVolume(bus, Number.isFinite(parsed) ? parsed : 1);
      return [];
    });
  }


function doString(L, source, name) {
    // WP10 迁移（fengari → mlua-wasm）：L 参数保留仅为调用点兼容（bridge 模式忽略），
    // 执行走 wasm 桥 `lua_do_string`；Lua 语法/运行错误由 wasm-bindgen 抛成 JS 异常。
    const bridge = window.__RUSTLUA_LUA_BRIDGE;
    if (!bridge) throw new Error('wasm lua bridge not loaded');
    try {
      bridge.lua_do_string(source, name);
    } catch (error) {
      throw new Error(String((error && error.message) || error));
    }
  }

  // 引擎里存在的宿主全局清单——**从桌面 API 基线派生，不是手抄**。
  //
  // 为什么必须有这份清单：下面的 `hostCompat` 会把名单里缺的全局换成"自报家门"的代理，
  // 而 `missingHostGlobals()` 用**同一份名单**判断"这条路径到底缺哪些 API"。
  // **不在名单里的缺失全局不会得到代理**——它保持 nil，脚本一碰就是
  // `attempt to index a nil value (global 'x')`，而诊断清单里**一个字都不提**它，
  // 于是根因看起来像"游戏脚本写错了"，而不是"这个平台没有这个 API"。
  //
  // `camera2d`（2026-09-23 事故）就是这个坑：它当时不在手抄名单里，于是第一个用到它的脚本
  // 以"取 nil"的姿态炸掉且诊断通道保持沉默（`examples/game_jhyt` 的 `world/camera_view.lua`
  // 在微信形态点「开始新游戏」后启动失败）。名单本身后来又被证实少 4 个真实模块表
  //（`camera3d` / `quad` / `visibility` / `world_text`）。
  //
  // 现在名单来自 wasm 内嵌的桌面基线（`lua_bridge.rs` 的 `host_api_module_names_json`，
  // `include_str!` 的是 `api_surface_snapshot/lua_api_surface.json`——与桌面快照门同一份文件）。
  // **给引擎新增宿主能力后不需要再想这里**：基线一变，web 侧的代理覆盖面自动跟着变。
  let hostGlobalsCache = null;
  function engineHostGlobals() {
    // 静态数据（不碰 Lua 状态），所以 lua_boot 之前可用；取一次就缓存。
    if (hostGlobalsCache) return hostGlobalsCache;
    const bridge = window.__RUSTLUA_LUA_BRIDGE;
    if (!bridge) throw new Error('wasm lua bridge not loaded');
    const raw = bridge.host_api_module_names_json();
    const names = JSON.parse(raw);
    if (!Array.isArray(names) || names.length === 0) {
      throw new Error('宿主全局清单为空（内嵌的桌面 API 基线形态不对）');
    }
    hostGlobalsCache = names;
    return names;
  }

  function missingHostGlobals(hostGlobals) {
    // 经 wasm 桥一次查出缺失清单（bridge 内部遍历全局表）。
    const bridge = window.__RUSTLUA_LUA_BRIDGE;
    if (!bridge) return [];
    try {
      const raw = bridge.lua_missing_globals(JSON.stringify(hostGlobals));
      return JSON.parse(raw || '[]');
    } catch (error) {
      console.warn('[lua bridge] missing globals probe failed:', error);
      return [];
    }
  }

  // 活体 API 表面：运行期真实 `_G` 的「模块表→函数名」，探针与桌面快照门**同一个常量**
  // （`LUA_API_PROBE`）。harness 拿它和提交进仓库的 web 基线对拍，防止 web 侧表面悄悄漂移
  // 或基线腐烂。只在被调用时执行，正常游戏路径不付出代价。
  window.__rustlua_export_api_surface = function () {
    const bridge = window.__RUSTLUA_LUA_BRIDGE;
    if (!bridge) throw new Error('wasm lua bridge not loaded');
    return JSON.parse(bridge.lua_api_surface_json());
  };

  function markBoot(state, detail) {
    const body = document.body || document.documentElement;
    if (!body) return;
    body.setAttribute('data-rustlua-boot', state);
    if (detail) body.setAttribute('data-rustlua-boot-detail', String(detail).slice(0, 400));
  }

  function callGlobal(L, name, args) {
    // WP10 迁移：经 wasm 桥 `lua_call_global` 调用。参数转 JSON 数组（Rust 侧转
    // Lua 值），返回该全局的字符串化结果或 null（全局缺失 / 返回非字符串），
    // 协议与 fengari 版 `lua_isstring ? tostring : null` 逐项一致。
    const bridge = window.__RUSTLUA_LUA_BRIDGE;
    if (!bridge) return null;
    try {
      const value = bridge.lua_call_global(name, JSON.stringify(args || []));
      // wasm-bindgen 对 Ok(None) 返回 undefined；协议层统一归一成 null（fengari 版语义）。
      return value === undefined ? null : value;
    } catch (error) {
      throw new Error(String((error && error.message) || error));
    }
  }

  window.rustluaRunLuaGame = async function (status) {
    if (!window.__RUSTLUA_LUA_BRIDGE) throw new Error('wasm lua bridge not loaded (fengari execution layer removed)');
    const bridge = window.__RUSTLUA_LUA_BRIDGE;
    const project = await fetchText('pack/project.toml');
    const startup = keyValue(project, 'startup_scene', 'scenes/main.scene');
    const mainScript = keyValue(project, 'main_script', 'scripts/main.lua');
    const projectName = keyValue(project, 'name', 'game');
    const sceneText = await fetchText('pack/' + startup);
    // 场景可切换 ⇒ 这两项必须可重新赋值（切图时整体换掉）。
    let sceneState = parseScene(sceneText);
    const canvas = document.getElementById('rustlua-canvas');
    const ctx = canvas.getContext('2d', { willReadFrequently: true });
    if (!ctx) throw new Error('2D canvas context is not available');
    const webInput = createWebInput(canvas);
    let images = await loadSceneImages(sceneState);

    const manifest = await fetchText('pack/manifest.txt');
    // 可切换的场景清单来自 manifest（发布时真实随包/随资源包分发的那批），
    // 而不是"磁盘上看着有"——与引擎按 scenes/<name>.scene 是否存在判定同源。
    const knownScenes = {};
    const knownAudio = new Set();
    const modules = {};
    for (const line of manifest.split(/\r?\n/).slice(1)) {
      const path = line.split('\t')[0];
      if (path && path.startsWith('scenes/') && path.endsWith('.scene')) {
        knownScenes[path.slice('scenes/'.length, -'.scene'.length)] = true;
      }
      // 音频文件名集合给音频后端做"在不在"的同步判据（浏览器没有 accessSync）。
      if (path && path.startsWith('assets/audio/')) knownAudio.add(path);
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
    // WP10 迁移（fengari → mlua-wasm）：Lua 状态由 wasm 侧 `LuaRuntime` 承载
    // （`lua_boot` 建运行时 + vfs provider，provider 读 wasm 预加载资源表）；
    // main.lua 在补丁注入完成后经 `lua_execute_main` 执行（读 vfs + pack 解码，
    // 与桌面/微信压缩包语义一致），这里不再 fetch main 源码。
    // `L` 保留为 null 仅为调用点兼容（doString/callGlobal/missingHostGlobals 已走 bridge）。
    bridge.lua_boot();
    const L = null;
    installLuaAudioBridge(knownAudio);
    try {
      const hostGlobals = engineHostGlobals();
      const hostCompat = hostGlobals.map(name => `if ${name} == nil then ${name} = __rustlua_missing_host_global(${luaQuote(name)}) end`).join('\n');
      // 引擎里有、web 运行时没实现的宿主 API：**不再静默变成空表**。
      // 旧写法（if map == nil then map = {} end）把"未实现的全局"伪装成一张空表，
      // 于是 `map.current()` 报的是 `attempt to call a nil value (field 'current')`——
      // 看上去像引擎缺一个字段，实际是整个 map 表都没实现（实测把人误导到"引擎 API 缺失"）。
      // 现在换成会**自报家门**的代理：每次访问记一条 `全局.字段` 路径并打印，同时返回
      // 可调用/可索引的替身让执行继续——一条用例就能把所有缺口探全，而不是撞一个修一个。
      // `__rustlua_export_missing_api()` 给出清单，供宿主/harness 断言"这条路径不缺 API"。
      const missingApiCompat = `
__rustlua_missing_api = {}
function __rustlua_record_missing(path)
  if not __rustlua_missing_api[path] then
    __rustlua_missing_api[path] = true
    print('[lua missing-host-api] ' .. path)
  end
end
function __rustlua_missing_proxy(prefix)
  return setmetatable({}, {
    __index = function(_, key)
      local child = prefix .. '.' .. tostring(key)
      __rustlua_record_missing(child)
      return __rustlua_missing_proxy(child)
    end,
    __call = function() return __rustlua_missing_proxy(prefix) end,
    __tostring = function() return '<missing host API ' .. prefix .. '>' end
  })
end
-- 整个宿主全局都没实现（如 inv / grid / day）：换成会自报家门的代理。
function __rustlua_missing_host_global(name)
  return __rustlua_missing_proxy(name)
end
-- 宿主表存在但缺方法（如引擎 save 有 clear，shim 里没有）：同样点名。
-- 只在表自身没有元表时安装，避免盖掉上面代理的 __call。
function __rustlua_trap_host_table(name, api)
  if type(api) == 'table' and getmetatable(api) == nil then
    setmetatable(api, {
      __index = function(_, key)
        local child = name .. '.' .. tostring(key)
        __rustlua_record_missing(child)
        return __rustlua_missing_proxy(child)
      end
    })
  end
end
function __rustlua_export_missing_api()
  local rows = {}
  for path in pairs(__rustlua_missing_api) do rows[#rows + 1] = path end
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
-- ==== WP10 批次 B/C 补齐：event/data(JS桥)/asset/tag/light/interact/container/shop ====
-- 以下补丁先于 missing 清单生成执行，表本身可能是 nil（尚未被代理占位）——
-- 因此每个表先 X = X or {} 再挂方法，避免 "global is nil" 或覆盖真实表。
event = event or {}
tag = tag or {}
light = light or {}
interact = interact or {}
container = container or {}
shop = shop or {}
asset = asset or {}
-- 与补丁区既有同族表（inv/grid/map/sprite…）同一套 __rustlua_ 机制；语义逐条对齐桌面
-- core/script.rs。event 回调是 fengari 函数（emit 快照遍历，回调里再 emit/订阅/退订
-- 不影响本次分发）；tag/light 是实体状态（__rustlua_require_alive 同引擎）；container 复用
-- 背包机制（键 "container:<id>"）；shop 目录直接存 Lua 表（price 读 buy/sell）。
__rustlua_events = {}
function event.on(name, callback)
  assert(type(callback) == 'function', 'event.on expects a function')
  local list = __rustlua_events[tostring(name)]
  if not list then list = {} __rustlua_events[tostring(name)] = list end
  list[#list + 1] = callback
end
function event.subscribe(name, callback) return event.on(name, callback) end
function event.emit(name, data)
  local list = __rustlua_events[tostring(name or '')] or {}
  local callbacks = {}
  for index = 1, #list do callbacks[index] = list[index] end
  for index = 1, #callbacks do callbacks[index](data) end
end
function event.unsubscribe(name, callback)
  local key = tostring(name)
  if callback == nil then __rustlua_events[key] = nil return end
  local list = __rustlua_events[key]
  if not list then return end
  for index = #list, 1, -1 do
    if list[index] == callback then table.remove(list, index) end
  end
end
function event.off(name, callback) return event.unsubscribe(name, callback) end

-- 实体标签（引擎 tag.get/set 同构）：id → { key = value }；空 value 删除键、删空袋。
__rustlua_tags = {}
function tag.get(id, key)
  -- 存活校验（引擎 tag.get 的 require_alive）：未知实体是**错误**而不是返回 nil。
  -- 缺了它，读一个不存在的实体会拿到空标签袋，调用方的 pcall 判据整体失真
  -- （L3 契约 tag.get.unknown_entity：PC ok=false / web 曾 ok=true）。
  __rustlua_require_alive(tonumber(id))
  local bag = __rustlua_tags[tonumber(id)]
  if not bag then return nil end
  return bag[tostring(key)]
end
function tag.set(id, key, value)
  id, key = tonumber(id), tostring(key or '')
  if key == '' then error('tag key must not be empty', 2) end
  __rustlua_require_alive(id)
  local bag = __rustlua_tags[id]
  value = tostring(value or '')
  if value == '' then
    if bag then
      bag[key] = nil
      if next(bag) == nil then __rustlua_tags[id] = nil end
    end
  else
    if not bag then bag = {} __rustlua_tags[id] = bag end
    bag[key] = value
  end
end

-- 光照（引擎 light 同构）：实体 → 方向/颜色/强度；set_active 要求该实体已有光。
__rustlua_lights = {}
__rustlua_active_light = nil
function light.set_active(id)
  id = tonumber(id)
  __rustlua_require_alive(id)
  if not __rustlua_lights[id] then error('entity has no light', 2) end
  __rustlua_active_light = id
  return true
end
function light.get(id)
  id = tonumber(id)
  __rustlua_require_alive(id)
  local l = __rustlua_lights[id]
  if not l then error('entity has no light', 2) end
  return { dx = l.dx or 0, dy = l.dy or 0, dz = l.dz or 0, r = l.r or 1, g = l.g or 1, b = l.b or 1, intensity = l.intensity or 1 }
end
function light.set(id, dx, dy, dz, r, g, b, intensity)
  id = tonumber(id)
  __rustlua_require_alive(id)
  intensity = tonumber(intensity) or 1
  if not intensity or intensity ~= intensity or intensity < 0 then
    error('light intensity must be finite and non-negative', 2)
  end
  __rustlua_lights[id] = { dx = dx or 0, dy = dy or 0, dz = dz or 0, r = r or 1, g = g or 1, b = b or 1, intensity = intensity }
end

-- 交互（引擎 interact 同构）：prompt 状态 + query（按距离升序、同距 priority 降序）。
__rustlua_interact_prompt = nil
function interact.prompt(text) __rustlua_interact_prompt = tostring(text or '') end
function interact.clear_prompt() __rustlua_interact_prompt = nil end
function interact.get_prompt() return __rustlua_interact_prompt end
function interact.query(x, y, radius)
  x, y, radius = tonumber(x), tonumber(y), tonumber(radius) or 0
  if not radius or radius ~= radius or radius < 0 then
    error('interact radius must be finite and non-negative', 2)
  end
  local hits = {}
  for id, bag in pairs(__rustlua_tags) do
    if bag.interact or bag.kind then
      local t = __rustlua_transforms[id]
      if t then
        local dx, dy = (t.x or 0) - x, (t.y or 0) - y
        if dx * dx + dy * dy <= radius * radius then
          hits[#hits + 1] = { id = id, kind = bag.kind or bag.interact or '', priority = tonumber(bag.priority) or 0 }
        end
      end
    end
  end
  table.sort(hits, function(a, b)
    local ta, tb = __rustlua_transforms[a.id], __rustlua_transforms[b.id]
    local da = ((ta and ta.x or 0) - x) ^ 2 + ((ta and ta.y or 0) - y) ^ 2
    local db = ((tb and tb.x or 0) - x) ^ 2 + ((tb and tb.y or 0) - y) ^ 2
    if da ~= db then return da < db end
    return -a.priority < -b.priority
  end)
  return hits
end

-- 容器（引擎 container 同构）：把实体关联成第二个背包，键 "container:<id>"。
function container.open(entity_id, capacity)
  local key = 'container:' .. tostring(tonumber(entity_id))
  __rustlua_inventories[key] = __rustlua_inventory_new(capacity or 12)
  return key
end
function container.get(entity_id)
  local key = 'container:' .. tostring(tonumber(entity_id))
  if __rustlua_inventories[key] then return key end
  return nil
end
function container.close(entity_id)
  local key = 'container:' .. tostring(tonumber(entity_id))
  local existed = __rustlua_inventories[key] ~= nil
  __rustlua_inventories[key] = nil
  return existed
end
function container.move(entity_id, from_index, to_id, to_index)
  return inv.move('container:' .. tostring(tonumber(entity_id)), from_index, to_id, to_index, nil)
end

-- 商店目录（引擎 shop 同构：set_catalog/price）。
__rustlua_shops = {}
function shop.set_catalog(name, spec)
  assert(type(spec) == 'table', 'shop.set_catalog expects a table')
  __rustlua_shops[tostring(name)] = spec
  return true
end
function shop.price(name, item_id)
  local catalog = __rustlua_shops[tostring(name)]
  if not catalog then return nil end
  local entry = catalog[tostring(item_id)]
  if not entry then return nil end
  return { buy = tonumber(entry.buy) or 0, sell = tonumber(entry.sell) or 0 }
end

-- 资源句柄（引擎 asset 同构：load 返回 "asset:<path>"、release 注销）。
__rustlua_assets = {}
function asset.load(path)
  local key = tostring(path)
  local token = 'asset:' .. key
  __rustlua_assets[key] = token
  return token
end
function asset.release(token)
  for path, value in pairs(__rustlua_assets) do
    if value == token then __rustlua_assets[path] = nil end
  end
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
-- clock 兜底已**删除**（L3 行为契约实测出来的，见 scripts/web-api-contract.js）：
-- 它写于"微信侧 wasm 桥未接线、clock 是 nil"的年代（WP10 批 A 之前）。今天 wasm VM 里
-- clock 由 api::register_all 注册，与桌面**同一份 Rust 实现**（L1 注册层锁实测 41 表 / 275 函数
-- 逐项相同），于是这段变成了纯粹的**有损重复**：把引擎版覆盖成只有 day/minute/minute_of_day，
-- 少掉 hour/paused/rate（Lua 读到 nil 且不报错），并且 set/add 不做引擎的参数校验与跨天钳位
-- （契约实测：clock.set(0,0) 桌面拒绝、补丁接受；clock.add(-9999) 桌面钳到 day=2/minute=0、
-- 补丁原样留着 minute=30；clock.sleep_to_morning() 桌面 day=6/minute=360、补丁 day=183/minute=1000）。
-- 保留它挡不住任何东西：两边都**没有**自动推进——web 的帧循环从不叫 LuaRuntime::step，
-- 引擎版与补丁版一样只在游戏自己调用时才变。真要补的是那个缺口（见 docs/51「web 时间基」）。
-- calendar/day/app 继续由 missing 代理自报（补丁层没有覆盖它们）。
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
-- 菜单导航连发（引擎 input.action_repeat）：web 端没有"按住重复"的设备时钟，
-- 语义取"一次边沿 = 一步"（与 DPad 一次触摸 = 一步、M20-08 手感一致），等价于
-- action_pressed。必须实现而非缺失——missing 代理 __call 恒返回 truthy 替身，
-- ui_step 里 input.action_repeat or input.action_pressed 会选中代理，方向键永远判定为"上"。
function input.action_repeat(action)
  for _,key in ipairs(bindings[tostring(action)] or {}) do
    if __rustlua_input and __rustlua_input.just and __rustlua_input.just[key] then return true end
  end
  return false
end
function input.bindings(action) local result = {}; for i,k in ipairs(bindings[tostring(action)] or {}) do result[i]=k end; return result end
-- 按住态（引擎 action_is_down）与抬起边沿（引擎 action_just_released）。
-- input.action 此前**整族缺失**（补丁层那张 input = { … } 是整表替换，漏名即 nil），
-- action_released 则是 L3 契约 input.action.no_device 报"缺宿主 API"的那一条。
function input.action(action)
  for _,key in ipairs(bindings[tostring(action)] or {}) do
    if __rustlua_input and __rustlua_input.keys and __rustlua_input.keys[key] then return true end
  end
  return false
end
function input.action_released(action)
  for _,key in ipairs(bindings[tostring(action)] or {}) do
    if __rustlua_input and __rustlua_input.released and __rustlua_input.released[key] then return true end
  end
  return false
end
function input.action_pressed(action)
  for _,key in ipairs(bindings[tostring(action)] or {}) do
    if __rustlua_input and __rustlua_input.just and __rustlua_input.just[key] then return true end
  end
  return false
end
-- 轴 = 正动作绑定值之和 − 负动作绑定值之和，钳到 [-1, 1]（引擎 InputState::action_axis）。
-- **首参是负方向**动作，所以垂直轴写作 action_axis("move_down", "move_up")：
-- 写成 ("move_up","move_down") 会让按键方向整体反号（引擎测试里有这条断言）。
function input.action_axis(negative, positive)
  -- 引擎侧返回 f64（action_axis），Lua 里必须是**浮点**：整数 0 与浮点 0.0 在
  -- tostring 下不同（0 / 0.0），契约按字符串逐行对拍。
  local value = 0.0
  for _,key in ipairs(bindings[tostring(negative)] or {}) do
    if __rustlua_input and __rustlua_input.keys and __rustlua_input.keys[key] then value = value - 1 end
  end
  for _,key in ipairs(bindings[tostring(positive)] or {}) do
    if __rustlua_input and __rustlua_input.keys and __rustlua_input.keys[key] then value = value + 1 end
  end
  if value < -1 then return -1.0 elseif value > 1 then return 1.0 end
  return value
end
function input.actions()
  local names = {}
  for name in pairs(bindings) do names[#names + 1] = name end
  table.sort(names)
  return names
end
-- 面板焦点环（引擎 ui.focus_* 的 web 实现）：Lua 侧状态机。引擎事实：0 基序号、
-- focus_open(id,count[,columns]) 建立/重建环、focus_move 按列网格方向移动、到边界环绕。
-- 此前缺失 → panel_nav.lua 的 api() 探测到非函数 → 全部降级（open 不建环、move 返回
-- nil），于是面板方向导航（方向键/DPad/摇杆）在 web 端全是死的——DPad 触摸后高亮不动的根因。
local focus_rings = {}
local function focus_ring(id)
  local key = tostring(id)
  local ring = focus_rings[key]
  if not ring then
    ring = { count = 0, cols = 1, index = 0 }
    focus_rings[key] = ring
  end
  return ring
end
local function focus_clamp(ring, value)
  if ring.count <= 0 then return 0 end
  if value < 0 then return 0 end
  if value >= ring.count then return ring.count - 1 end
  return value
end
ui.focus_open = function(id, count, columns)
  local ring = focus_ring(id)
  ring.count = math.max(0, tonumber(count) or 0)
  ring.cols = math.max(1, tonumber(columns) or 1)
  ring.index = focus_clamp(ring, ring.index)
  return ring.count
end
ui.focus_select = function(id, index)
  local ring = focus_ring(id)
  ring.index = focus_clamp(ring, math.floor(tonumber(index) or 0))
  return true
end
ui.focus_index = function(id)
  local ring = focus_rings[tostring(id)]
  if not ring then return nil end
  return ring.index
end
ui.focus_count = function(id)
  local ring = focus_rings[tostring(id)]
  if not ring then return 0 end
  return ring.count
end
ui.focus_move = function(id, direction)
  local ring = focus_ring(id)
  if ring.count <= 0 then return 0 end
  local cols = ring.cols
  local rows = math.max(1, math.ceil(ring.count / cols))
  local row = math.floor(ring.index / cols)
  local col = ring.index % cols
  if direction == "up" then
    row = (row - 1 + rows) % rows
  elseif direction == "down" then
    row = (row + 1) % rows
  elseif direction == "left" then
    col = (col - 1 + cols) % cols
  elseif direction == "right" then
    col = (col + 1) % cols
  end
  ring.index = focus_clamp(ring, row * cols + col)
  return ring.index
end
ui.focus_close = function(id)
  focus_rings[tostring(id)] = nil
  return true
end
-- 列表滚动（panel_nav.window 用）：状态与算法都镜像引擎的 ScrollState
-- （rustlua-ui/src/ui_navigation.rs 的 max_offset / scroll_extents / scroll_to_visible）：
-- 偏移钳在 [0, max(0, total − view)]；scroll_to_visible 只在"目标行真在视口外"时才动，
-- 返回**是否真的动了**；scroll_extents 返回钳制后的偏移而不是 true。此前补丁版把 y 直接
-- 当成偏移写进去，于是没有登记尺寸（引擎的 or_default 建出 view=total=0 ⇒ max_offset=0）
-- 时 web 上"滚到可见"会把偏移设成任意值，与桌面分叉（L3 契约 ui.scroll.lifecycle：
-- PC offset=0.0 / web 曾 offset=30）。偏移一律浮点——引擎侧是 f32，契约逐行比字符串。
local scroll_state = {}
local function scroll_new() return { view = 0.0, total = 0.0, offset = 0.0 } end
local function scroll_max(s)
  local max = (s.total or 0.0) - (s.view or 0.0)
  if max < 0.0 then max = 0.0 end
  return max
end
ui.scroll_extents = function(id, view_h, total_h)
  local s = scroll_state[tostring(id)] or scroll_new()
  s.view = (tonumber(view_h) or 0) + 0.0
  s.total = (tonumber(total_h) or 0) + 0.0
  local max = scroll_max(s)
  if s.offset > max then s.offset = max end
  if s.offset < 0.0 then s.offset = 0.0 end
  scroll_state[tostring(id)] = s
  return s.offset
end
ui.scroll_to_visible = function(id, y, h)
  local s = scroll_state[tostring(id)] or scroll_new()
  local item_top = (tonumber(y) or 0) + 0.0
  local item_bottom = item_top + math.max(0, tonumber(h) or 0)
  local view_bottom = s.offset + (s.view or 0.0)
  local target
  if item_top < s.offset then
    target = item_top
  elseif item_bottom > view_bottom then
    target = item_bottom - (s.view or 0.0)
  else
    -- 已经可见：引擎版同样用 entry().or_default() 建/留状态，只是不改偏移。
    scroll_state[tostring(id)] = s
    return false
  end
  local max = scroll_max(s)
  if target < 0.0 then target = 0.0 elseif target > max then target = max end
  local before = s.offset
  s.offset = target
  scroll_state[tostring(id)] = s
  return s.offset ~= before
end
ui.scroll_offset = function(id)
  local s = scroll_state[tostring(id)]
  if not s then return nil end
  return s.offset
end
ui.scroll_close = function(id)
  local key = tostring(id)
  local had = scroll_state[key] ~= nil
  scroll_state[key] = nil
  return had
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
      // 顺序：**先装真实实现**（luaPrelude 的 __rustlua_ 核心 + methodCompat 的方法补丁），
      // **再算缺失清单**并给真正缺失的全局装代理——否则 methodCompat 刚补的表
      //（event/tag/light/interact/container/shop/asset/item）会先被 `__rustlua_missing_globals`
      // 误标成缺失，脚本判能力时得到错误结论。
      doString(L, luaPrelude(sceneState, knownScenes) + '\n' + preload, 'engine-host-prelude');
      doString(L, methodCompat + '\n' + genericCompat, 'engine-host-method-compat');
      const eagerlyMissingGlobals = missingHostGlobals(hostGlobals);
      const missingGlobalTable = '__rustlua_missing_globals = {' +
        eagerlyMissingGlobals.map(name => `[${luaQuote(name)}]=true`).join(',') + '}';
      doString(L, missingApiCompat + '\n' + missingGlobalTable + '\n' + hostCompat, 'engine-host-compat');
      // 在**真正的宿主表**上装缺失方法陷阱（放在 methodCompat 之后：那批默认实现
      // 必须先落地，否则陷阱会让 `if not api.load_settings` 之类的探测误判为"有"）。
      doString(L, hostGlobals.map(name => `__rustlua_trap_host_table(${luaQuote(name)}, ${name})`).join('\n'), 'engine-host-table-traps');
      // WP10 迁移：main.lua 不再由 JS fetch 后注入，改经 wasm 桥读 vfs 预加载表
      // + pack 解码执行（与桌面/微信压缩包语义一致；执行前全部宿主补丁已在位）。
      bridge.lua_execute_main();
    } catch (error) {
      const missing = missingHostGlobals(hostGlobals);
      const detail = 'missing engine host API: ' + missing.join(', ');
      markBoot('failed', detail);
      throw new Error(String((error && error.message) || error) + ' [' + detail + ']');
    }
    callGlobal(L, 'on_create');
    const pendingTextures = new Set();
    // on_create 里就会挂形象/生成 NPC（`sprite.set` + `animation.attach` + `entity.spawn`），
    // 这些是**一次性 op**：不在这里立刻取走，首帧画的就是"还没挂外观"的场景。
    // 传 false = 只取快照、不推进速度与动画帧（首帧不该白走一次积分）。
    syncLuaScene(sceneState, callGlobal(L, '__rustlua_export_state', [false]));
    await loadGameFont();
    refreshSceneTextHidden(L);
    let uiElements = drawScene(ctx, sceneState, images, projectName, callGlobal(L, '__rustlua_export_ui'), pendingTextures);
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
        syncLuaScene(sceneState, callGlobal(L, '__rustlua_export_state', [false]));
        refreshSceneTextHidden(L);
        uiElements = drawScene(ctx, sceneState, images, projectName, callGlobal(L, '__rustlua_export_ui'), pendingTextures);
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
        if (sceneVerify && sceneVerify.status === 'in-flight') {
          sceneVerify.status = 'done';
          markSceneVerify('done', 'scene=' + name + ' entities=' + next.entities.size);
        }
      } catch (error) {
        console.error(error);
        if (status) status.textContent = 'scene load failed: ' + error;
        markBoot('failed', (error && error.message) || error);
        sceneSwitch = 'failed';
        if (sceneVerify && sceneVerify.status === 'in-flight') {
          sceneVerify.status = 'failed';
          markSceneVerify('failed',
            ((error && error.message) || String(error)) + ' scene=' + name);
        }
      }
    }

    // P0-①：URL `?rustlua-drive=auto` 驱动"完整剧本"（标题按确认 → 进玩法地图 → 按住方向键移动），
    // 结果写进 `data-rustlua-drive`（done/failed）供无头浏览器验收断言。驱动走**真实输入路径**
    // （window KeyboardEvent），与微信 harness 的 --drive-new-game/--drive-move 同构——
    // 两个平台共用同一份 web-runtime.js，字段含义一致，可以直接对照。
    let driveAuto = null;
    try {
      const search = (typeof window !== 'undefined' && window.location && window.location.search) || '';
      if (search.indexOf('rustlua-drive=auto') >= 0) {
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

    // P0-②：URL `?rustlua-drive=jhyt` 驱动**完整玩法剧本**——与微信 harness 的
    // --drive-new-game/--drive-move/--bag/--quest 同构，但走真实浏览器输入与真实 wasm。
    // 剧本动线（与 examples/game_jhyt/scripts/data/maps.lua 的 M17-01 动线同步）：
    //   标题确认 → 新游戏(manor) → 移动 → 背包开合 → 任务面板开合 →
    //   山庄→云泽村真实场景切换（走到出口格按 E）→ 实体保持校验 → 回山庄 → 里程碑全落定。
    // 里程碑写进 data-rustlua-drive（done/failed + detail），verify-webgl-bundle.ps1 -DriveJhyt 断言。
    // 出口世界坐标 = (cell+0.5)*TILE（TILE=32），硬编码自 data/maps.lua；改动线必须两边同步。
    let driveJhyt = null;
    try {
      const search = (typeof window !== 'undefined' && window.location && window.location.search) || '';
      if (search.indexOf('rustlua-drive=jhyt') >= 0) {
        driveJhyt = {
          phase: 'confirm',       // confirm → wait-settle → move → bag-open → bag-close →
          frames: 0,              // quest-open → quest-close → nav-exit → interact → wait-map →
          confirmFrames: 2,       // spawn-check → nav-back → interact-back → wait-back → done
          milestones: [],
          uiBase: -1,
          exitKey: null,
          limit: {
            'wait-settle': 500, 'move': 40, 'bag-open': 70, 'bag-close': 70,
            'quest-open': 70, 'quest-close': 70, 'nav-exit': 900, 'wait-map': 600,
            'nav-back': 900, 'wait-back': 600
          }
        };
      }
    } catch (e) { driveJhyt = null; }
    if (driveJhyt) {
      // 与 data/maps.lua 同步的出口表（仅剧本动线用到的两段）。
      const JHYT_EXITS = {
        manor:   { world: [528, 48], to: 'village' },  // maps.manor.exits[1].cell=(16,1) → world(528,48)
        village: { world: [624, 48], to: 'manor'   }   // maps.village.exits[1].cell=(19,1) → world(624,48)
      };
      function jhytReadWorld() {
        const body = document.body || document.documentElement;
        if (!body) return '';
        return body.getAttribute('data-rustlua-world') || '';
      }
      function jhytWorldField(text, name) {
        const match = text.match(new RegExp(name + '=([^\\s]+)'));
        return match ? match[1] : '';
      }
      function jhytPlayerPos(text) {
        const match = text.match(/player=\S+?@(-?\d+),(-?\d+)/);
        return match ? [Number(match[1]), Number(match[2])] : null;
      }
      function jhytUiElements() {
        const body = document.body || document.documentElement;
        if (!body) return -1;
        // 优先读每帧真值 ui-now（剧本判定用）；回退到 30 帧一更的 ui-elements（老字段）。
        // 不用 `??`：微信上传器的解析器实测只到 ES2019，`??` 会让整包上传被拒
        // （见 scripts/check-upload-ecma.mjs 头部）。getAttribute 缺属性时返回 null，语义等价。
        let raw = body.getAttribute('data-rustlua-ui-now');
        if (raw === null) raw = body.getAttribute('data-rustlua-ui-elements');
        return raw === null ? -1 : (Number(raw) || 0);
      }
      function jhytFail(d, detail) {
        markDrivePhase('failed');
        markDrive('failed', 'phase=' + d.phase + ' ' + detail);
        d.phase = 'failed';
      }
      function jhytSucceed(d) {
        markDrivePhase('done');
        markDrive('done', 'milestones=' + d.milestones.join(','));
        d.phase = 'done';
      }
      // 朝向目标世界坐标走一格：只注入一个方向键；到达半格内返回 arrived。
      function jhytNavigate(d, targetX, targetY, worldText) {
        const pos = jhytPlayerPos(worldText);
        if (!pos) return 'no-player';
        const dx = targetX - pos[0], dy = targetY - pos[1];
        if (Math.abs(dx) <= 16 && Math.abs(dy) <= 16) {
          if (d.exitKey) { dispatchDriveKey('keyup', d.exitKey); d.exitKey = null; }
          return 'arrived';
        }
        let key;
        if (Math.abs(dx) >= Math.abs(dy)) key = dx > 0 ? 'd' : 'a';
        else key = dy > 0 ? 'w' : 's';
        if (d.exitKey !== key) {
          if (d.exitKey) dispatchDriveKey('keyup', d.exitKey);
          dispatchDriveKey('keydown', key);
          d.exitKey = key;
        }
        return 'moving';
      }
      function stepDriveJhyt() {
        if (!driveJhyt || driveJhyt.phase === 'done' || driveJhyt.phase === 'failed') return;
        const d = driveJhyt;
        d.frames += 1;
        // 每帧把当前阶段写进 DOM：无头验收 dump 时能直接看到剧本卡在哪个阶段。
        markDrivePhase(d.phase);
        const limit = d.limit[d.phase];
        if (limit && d.frames > limit) {
          jhytFail(d, 'timeout phase=' + d.phase + ' frames=' + d.frames +
            ' world=' + jhytWorldField(jhytReadWorld(), 'map'));
          return;
        }
        const world = jhytReadWorld();
        const map = jhytWorldField(world, 'map');
        const alive = Number(jhytWorldField(world, 'alive') || 0);
        const scene = Number(jhytWorldField(world, 'scene') || 0);
        if (d.phase === 'confirm') {
          if (d.frames <= d.confirmFrames) dispatchDriveKey('keydown', ' ');
          else { dispatchDriveKey('keyup', ' '); d.phase = 'wait-settle'; d.frames = 0; }
          return;
        }
        if (d.phase === 'wait-settle') {
          if (map === 'manor' && alive > 0) {
            d.milestones.push('M1:new-game(map=' + map + ',alive=' + alive + ')');
            d.phase = 'move'; d.frames = 0;
          }
          return;
        }
        if (d.phase === 'move') {
          if (d.frames === 1) {
            d._moveBefore = jhytPlayerPos(world);
            dispatchDriveKey('keydown', 'w');
          }
          if (d.frames >= 12) {
            dispatchDriveKey('keyup', 'w');
            const before = d._moveBefore;
            const after = jhytPlayerPos(world);
            if (before && after && (after[0] !== before[0] || after[1] !== before[1])) {
              d.milestones.push('M2:move(' + before.join(',') + '->' + after.join(',') + ')');
              d.phase = 'bag-open'; d.frames = 0;
            } else {
              jhytFail(d, 'player did not move while holding w (before=' +
                (before && before.join(',')) + ' after=' + (after && after.join(',')) + ')');
            }
          }
          return;
        }
        if (d.phase === 'bag-open' || d.phase === 'bag-close' ||
            d.phase === 'quest-open' || d.phase === 'quest-close') {
          const isBag = d.phase.indexOf('bag') === 0;
          const isOpen = d.phase.indexOf('open') >= 0;
          const key = isBag ? 'i' : 'l';
          if (d.frames === 1) {
            d.uiBase = jhytUiElements();
            dispatchDriveKey('keydown', key);
          }
          if (d.frames === 4) dispatchDriveKey('keyup', key);
          // 面板开关动画期间元素数会跳变：检查点从第 8 帧起、每 6 帧一次（比旧实现更晚、
          // 更稀疏），避免"刚多一个元素就误判已开、紧接着按关闭键被动画吞掉"的时序问题。
          if (d.frames >= 8) {
            const now = jhytUiElements();
            const ok = isOpen ? now > d.uiBase : now < d.uiBase;
            if (ok) {
              const mark = (isBag ? 'M3:bag' : 'M4:quest') + (isOpen ? '-open' : '-close');
              d.milestones.push(mark + '(' + d.uiBase + '->' + now + ')');
              d.phase = isOpen ? (d.phase === 'bag-open' ? 'bag-close' : 'quest-close')
                               : (d.phase === 'bag-close' ? 'quest-open' : 'nav-exit');
              d.frames = 0;
              return;
            }
            // 方向未达成：第一次按键很可能被开关动画吞掉（总门负载下实际踩到 quest-close
            // 卡死）。面板开着时按 l/i 是关闭、关着时按是打开（toggle），重按不会误反方向，
            // 所以每 6 帧重发一次完整的 click（keydown 3 帧后 keyup），直到阶段 limit。
            const mod = (d.frames - 8) % 6;
            if (mod === 0) dispatchDriveKey('keydown', key);
            else if (mod === 3) dispatchDriveKey('keyup', key);
          }
          return;
        }
        if (d.phase === 'nav-exit' || d.phase === 'nav-back') {
          const target = JHYT_EXITS[d.phase === 'nav-exit' ? 'manor' : 'village'];
          const nav = jhytNavigate(d, target.world[0], target.world[1], world);
          if (nav === 'arrived') {
            d.phase = d.phase === 'nav-exit' ? 'interact' : 'interact-back';
            d.frames = 0;
          }
          return;
        }
        if (d.phase === 'interact' || d.phase === 'interact-back') {
          if (d.frames <= 4) dispatchDriveKey('keydown', 'e');
          else {
            dispatchDriveKey('keyup', 'e');
            d.phase = d.phase === 'interact' ? 'wait-map' : 'wait-back';
            d.frames = 0;
          }
          return;
        }
        if (d.phase === 'wait-map' || d.phase === 'wait-back') {
          const expect = d.phase === 'wait-map' ? 'village' : 'manor';
          if (map === expect) {
            d.milestones.push('M' + (d.phase === 'wait-map' ? '5' : '6') +
              ':scene(' + expect + ',alive=' + alive + ',scene=' + scene + ')');
            if (d.phase === 'wait-map') {
              d.phase = 'spawn-check'; d.frames = 0;
            } else {
              // 最后一段（回山庄）命中即收口：milestones 全部落定，写入 done。
              jhytSucceed(d);
            }
          }
          return;
        }
        if (d.phase === 'spawn-check') {
          if (alive > 0 && alive >= scene) {
            d.milestones.push('M5b:entities-preserved(alive=' + alive + '>=scene=' + scene + ')');
            d.phase = 'nav-back'; d.frames = 0;
          } else {
            jhytFail(d, 'entities lost after scene switch (alive=' + alive + ' scene=' + scene + ')');
          }
          return;
        }
      }
    }

    // 场景切换防回归门：`?rustlua-scene-verify=1` 让首帧把**当前**场景重新走一遍
    // applySceneSwitch（fetch scene 文件 → parseScene → 重绑 Lua → 重跑 on_create）。
    // 与游戏剧本解耦（不假设"标题→玩法地图"流程），只验一条：切换路径本身能跑通、
    // 不会在 wasm 上 unreachable（此前宿主闭包返回 Err 时的 raise 会撞 panic=abort 终止垫，
    // 现场只表现为 boot 绿、场景一切就死）。status: queued -> in-flight -> done|failed。
    let sceneVerify = null;
    try {
      const search = (typeof window !== 'undefined' && window.location && window.location.search) || '';
      if (search.indexOf('rustlua-scene-verify=1') >= 0) {
        sceneVerify = { status: 'queued', target: sceneState.name };
      }
    } catch (e) { sceneVerify = null; }

    function markSceneVerify(state, detail) {
      const body = document.body || document.documentElement;
      if (!body) return;
      body.setAttribute('data-rustlua-scene-verify', state);
      if (detail) body.setAttribute('data-rustlua-scene-verify-detail', String(detail).slice(0, 400));
      console.log('[scene-verify] ' + state + ' ' + (detail || ''));
    }
    if (sceneVerify) markSceneVerify('queued', 'target=' + sceneVerify.target);

    function dispatchDriveKey(type, key) {
      try { window.dispatchEvent(new KeyboardEvent(type, { key, bubbles: true })); } catch (e) { }
    }

    function markDrivePhase(phase) {
      const body = document.body || document.documentElement;
      if (!body) return;
      body.setAttribute('data-rustlua-drive-phase', phase);
      if (driveJhyt) {
        // 帧级诊断：headless 下 rAF 可能停调度（历史上踩过 virtual-time-budget 把 f 卡在 6），
        // 验收脚本 dump 时看不到"卡在哪个阶段、已跑多少帧"就无从判断是慢还是死。
        body.setAttribute('data-rustlua-drive-frames', String(driveJhyt.frames));
        body.setAttribute('data-rustlua-drive-frames-seen', String((window.__frameCounter || 0)));
      }
    }

    function markDrive(state, detail) {
      const body = document.body || document.documentElement;
      if (!body) return;
      body.setAttribute('data-rustlua-drive', state);
      if (detail) body.setAttribute('data-rustlua-drive-detail', String(detail).slice(0, 400));
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
        const summary = String(callGlobal(L, '__rustlua_world_summary') || '');
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
        const summary = String(callGlobal(L, '__rustlua_world_summary') || '');
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
          refreshSceneTextHidden(L);
          drawScene(ctx, sceneState, images, projectName, callGlobal(L, '__rustlua_export_ui'), pendingTextures);
          requestAnimationFrame(frame);
          return;
        }
        if (sceneSwitch === 'failed') return;
        // 场景自检在首帧触发一次：把当前场景重新排队走 applySceneSwitch 全流程。
        // 这里刻意**不**通过 Lua 的 scene.load 排队，而是直接调 JS 侧的 applySceneSwitch，
        // 因为 Lua 的 pending 队列是游戏状态的一部分，测试不应污染它。
        if (sceneVerify && sceneVerify.status === 'queued') {
          sceneVerify.status = 'in-flight';
          markSceneVerify('in-flight', 'reloading scene=' + sceneVerify.target);
          sceneSwitch = 'loading';
          applySceneSwitch(sceneVerify.target);
          requestAnimationFrame(frame);
          return;
        }
        // 场景切换是**排队**的（`scene.load` / `map.goto` 只排队、不改名字）；每帧开头
        // 取一次，取走即应用——与引擎"排队 → transition → 重跑 on_create"同构。
        const pendingScene = callGlobal(L, '__rustlua_take_pending_scene');
        if (pendingScene) {
          sceneSwitch = 'loading';
          applySceneSwitch(pendingScene);
          refreshSceneTextHidden(L);
          drawScene(ctx, sceneState, images, projectName, callGlobal(L, '__rustlua_export_ui'), pendingTextures);
          requestAnimationFrame(frame);
          return;
        }
        stepDrive();
        if (typeof window !== 'undefined') window.__frameCounter = (window.__frameCounter || 0) + 1;
        if (driveJhyt && typeof stepDriveJhyt === 'function') stepDriveJhyt();
        doString(L, '__rustlua_input = ' + luaValue(webInput.snapshot()), '__rustlua_input');
        // 快照是**整份替换**的（设备层每帧重新采样），虚拟键必须在替换**之后**并回来——
        // 顺序反了就等于没注入。对应引擎侧 `InputSnapshot::inherit_virtual_keys`。
        doString(L, '__rustlua_apply_virtual_input()', '__rustlua_virtual_input');
        const result = callGlobal(L, 'update');
        syncLuaScene(sceneState, callGlobal(L, '__rustlua_export_state'));
        refreshSceneTextHidden(L);
        uiElements = drawScene(ctx, sceneState, images, projectName, callGlobal(L, '__rustlua_export_ui'), pendingTextures);
        frameCount += 1;
        const body = document.body || document.documentElement;
        if (body) {
          // 世界状态（场景实体数/地图/落点/存活实体数/主角坐标与当前动画）每帧刷新：
          // 宿主诊断覆盖层与无头验收都读它，"游戏在跑但世界是空的"这类问题不必再猜。
          // scene=/drawn= 由 JS 侧补（场景里声明的实体数、渲染器侧的主角位置），其余由 Lua 侧给。
          const drawnPlayer = rendererPlayerState(sceneState);
          const world = 'scene=' + (sceneState.declared || sceneState.entities.size) + ' '
            + String(callGlobal(L, '__rustlua_world_summary') || '')
            + (drawnPlayer ? ' drawn=' + drawnPlayer : '');
          body.setAttribute('data-rustlua-world', world.slice(0, 300));
          // ui-now 每帧写**真实** UI 元素数（诊断/剧本判定用；ui-elements 仍 30 帧一更，
          // 只服务宿主诊断覆盖层的低频采样，剧本靠 ui-now 拿到当帧真值）。
          body.setAttribute('data-rustlua-ui-now', String(uiElements));
          if (frameCount <= 5 || frameCount % 30 === 0) {
            body.setAttribute('data-rustlua-drawn', String(sampleDrawnPixels(ctx)));
            body.setAttribute('data-rustlua-ui-elements', String(uiElements));
            body.setAttribute('data-rustlua-ui-summary', String(LAST_UI_SUMMARY).slice(0, 400));
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
