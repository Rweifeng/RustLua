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

  // `[input]` 段的菜单导航手感：与引擎 `parse_nav_section`
  // （rustlua-gameplay/src/settings.rs:751）逐条同语义 —— 只认 `[input]` 段、同名键取首次出现、
  // 毫秒键优先于旧帧键（帧键按 60Hz 换算）、数值统一按 `NavSettings::new` 钳制。
  // 键名与常量一一对应：nav_threshold / nav_delay_ms / nav_repeat_ms / nav_delay_frames /
  // nav_repeat_frames（settings.rs:71-84）。
  function parseNavSection(text) {
    const DEFAULT = { threshold: 0.5, delayMs: 350, repeatMs: 90 };
    const MAX_MS = 5000;
    const MIN_REPEAT_MS = 16;
    let threshold = null;
    let delayMs = null;
    let repeatMs = null;
    let delayFrames = null;
    let repeatFrames = null;
    let section = '';
    // 与引擎同：整数优先，其次按浮点取整（负值按 0）。返回 null 表示"这一行不是合法数值"。
    const parseNum = (value) => {
      if (/^\d+$/.test(value)) return Number(value);
      const f = Number(value);
      if (!Number.isFinite(f)) return null;
      return Math.trunc(Math.max(0, f));
    };
    for (const raw of String(text || '').split(/\r?\n/)) {
      const line = raw.trim().replace(/^\uFEFF/, '').trim();
      if (!line || line.startsWith('#')) continue;
      if (line.startsWith('[') && line.endsWith(']')) {
        section = line.slice(1, -1).trim().toLowerCase();
        continue;
      }
      if (section !== 'input') continue;
      const eq = line.indexOf('=');
      if (eq < 0) continue;
      const key = line.slice(0, eq).trim().toLowerCase();
      const value = line.slice(eq + 1).trim().replace(/^"|"$/g, '');
      if (key === 'nav_threshold' && threshold === null) {
        const f = Number(value);
        if (Number.isFinite(f)) threshold = f;
      } else if (key === 'nav_delay_ms' && delayMs === null) {
        delayMs = parseNum(value);
      } else if (key === 'nav_repeat_ms' && repeatMs === null) {
        repeatMs = parseNum(value);
      } else if (key === 'nav_delay_frames' && delayFrames === null) {
        delayFrames = parseNum(value);
      } else if (key === 'nav_repeat_frames' && repeatFrames === null) {
        repeatFrames = parseNum(value);
      }
    }
    // 旧帧键 → 毫秒（60Hz），与 NavSettings::frames_to_ms 同式。
    const framesToMs = (frames) => Math.round(Math.max(0, frames) * (1000 / 60));
    const delay = delayMs !== null ? delayMs : (delayFrames !== null ? framesToMs(delayFrames) : DEFAULT.delayMs);
    const repeat = repeatMs !== null ? repeatMs : (repeatFrames !== null ? framesToMs(repeatFrames) : DEFAULT.repeatMs);
    const clampedThreshold = Number.isFinite(threshold) ? Math.min(1, Math.max(0, threshold)) : DEFAULT.threshold;
    return {
      threshold: clampedThreshold,
      delayMs: Math.min(delay, MAX_MS),
      repeatMs: Math.min(Math.max(repeat, MIN_REPEAT_MS), MAX_MS),
    };
  }

  // `[input].map` 的解析：与引擎 `parse_input_section`（rustlua-gameplay/src/settings.rs:347）
  // + `ActionMap::from_text_with_notes`（rustlua-input/src/input.rs:1006）同语义：
  //   * 只认 `[input]` 段、同名键取首次出现；
  //   * map 值是 TOML 基本字符串（`\n` 是转义换行），首行是版本头
  //     （`rustlua-actions-v2`；v1 的键位写作字符名，走同一套换算）；
  //   * 每行 `action = item,item,…`；条目 `key:<物理键名>` / `mouse:…` / `pad:N` / `padaxis:N±`。
  // 返回 { bindings, keyed, padOnly, unmapped, header }：bindings 只含**键盘**可用项
  // （web 没有手柄轮询，pad/mouse 记数但不参与匹配）。
  function parseInputMap(text) {
    let section = '';
    let raw = null;
    for (const line of String(text || '').split(/\r?\n/)) {
      const trimmed = line.trim().replace(/^\uFEFF/, '').trim();
      if (!trimmed || trimmed.startsWith('#')) continue;
      if (trimmed.startsWith('[') && trimmed.endsWith(']')) {
        section = trimmed.slice(1, -1).trim().toLowerCase();
        continue;
      }
      if (section !== 'input' || raw !== null) continue;
      const eq = trimmed.indexOf('=');
      if (eq < 0) continue;
      if (trimmed.slice(0, eq).trim().toLowerCase() !== 'map') continue;
      let value = trimmed.slice(eq + 1).trim();
      if (value.startsWith('"') && value.endsWith('"')) value = value.slice(1, -1);
      // TOML 基本字符串的转义：本文件只需要 \n（map 的多行就靠它）。
      raw = value.replace(/\\n/g, '\n').replace(/\\"/g, '"').replace(/\\\\/g, '\\');
    }
    const out = { bindings: {}, keyed: [], padOnly: [], unmapped: [], aliased: [], header: '' };
    if (raw === null) return out;
    const lines = raw.split('\n');
    out.header = (lines.shift() || '').trim();
    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      const eq = trimmed.indexOf('=');
      if (eq < 0) continue;
      const action = trimmed.slice(0, eq).trim();
      const keys = [];
      let padSeen = false;
      for (const item of trimmed.slice(eq + 1).split(',')) {
        const entry = item.trim();
        if (!entry) continue;
        if (entry.startsWith('key:')) {
          const name = entry.slice(4).trim().toLowerCase();
          const mapped = physicalKeyToName(name);
          if (mapped === null) out.unmapped.push(name);
          else {
            keys.push(mapped);
            // 有损合并要**计数**：工程绑 `key:shiftleft` 时 web 上右 Shift 也会响应，
            // 这是宿主能力所限、不是解析错误 —— 但读数里必须看得见它发生过（第 74 轮）。
            if (physicalKeyIsLossy(name)) out.aliased.push(name);
            // **物理键语义**：引擎的 `key:equal` 认的是物理键，按下 Shift 也命中
            // ⇒ 本侧必须同时接受该物理键的两个字符产出（'=' 与 '+'、'-' 与 '_'）。
            // 硬编码表当年就是成对写的；只取一个会在"按住 Shift 调音量"这类操作上静默失效。
            if (mapped === '=') keys.push('+');
            if (mapped === '-') keys.push('_');
          }
        } else if (entry.startsWith('pad:') || entry.startsWith('padaxis:') || entry.startsWith('mouse:')) {
          padSeen = true;
        } else {
          out.unmapped.push(entry);
        }
      }
      if (keys.length > 0) {
        out.bindings[action] = keys;
        out.keyed.push(action);
      } else if (padSeen) {
        out.padOnly.push(action);
      }
    }
    return out;
  }

  // v2 物理键名（`KeyCode::to_text`，全小写）→ 本侧输入层的键名（`createWebInput` 的
  // `keyName()`，取自 `event.key` 小写 + 'space'/方向名）。对不上的返回 null 让调用方计入
  // unmapped —— 不静默当成"没有这个绑定"。
  // 引擎物理键名 → 本侧键名。**有损**的那些单独列出来（见 LOSSY_PHYSICAL_KEYS）：
  // 本侧键名取自 `event.key`，它分不出左右修饰键、也分不出小键盘与主键盘同形键
  // ⇒ 这些名字只能**合并**。合并本身是唯一可行的做法，但不能默默做：
  // 工程要是绑了 `key:shiftleft`，web 上它同时响应右 Shift —— 这件事必须能被测出来。
  const SIMPLE_KEY_NAMES = {
    // 名字与 `event.key` 小写同形，直接放行。
    space: 'space',
    escape: 'escape',
    enter: 'enter',
    tab: 'tab',
    backspace: 'backspace',
    delete: 'delete',
    home: 'home',
    end: 'end',
    pageup: 'pageup',
    pagedown: 'pagedown',
    insert: 'insert',
    capslock: 'capslock',
    numlock: 'numlock',
    scrolllock: 'scrolllock',
    printscreen: 'printscreen',
    pause: 'pause',
    contextmenu: 'contextmenu',
    // 引擎给的是**物理键名**，浏览器 `event.key` 给的是**该键产出的字符**。
    // 物理键语义（按下 Shift 也算这个键）在这里兑现：等号/减号还要同时接受 '+' 与 '_'，
    // 由 parseInputMap 补（硬编码表当年就是成对写的）。
    equal: '=',
    minus: '-',
    backquote: '`',
    bracketleft: '[',
    bracketright: ']',
    backslash: '\\',
    semicolon: ';',
    quote: "'",
    comma: ',',
    period: '.',
    slash: '/'
  };
  // 左右修饰键合并成一个名字，小键盘与主键盘同形键也合并（`event.key` 给的字符一样）。
  const LOSSY_PHYSICAL_KEYS = {
    shiftleft: 'shift',
    shiftright: 'shift',
    controlleft: 'control',
    controlright: 'control',
    altleft: 'alt',
    altright: 'alt',
    superleft: 'meta',
    superright: 'meta',
    numpadadd: '+',
    numpadsubtract: '-',
    numpadmultiply: '*',
    numpaddivide: '/',
    numpaddecimal: '.',
    numpadequal: '=',
    numpadenter: 'enter'
  };
  function physicalKeyToName(name) {
    if (Object.prototype.hasOwnProperty.call(SIMPLE_KEY_NAMES, name)) return SIMPLE_KEY_NAMES[name];
    if (Object.prototype.hasOwnProperty.call(LOSSY_PHYSICAL_KEYS, name)) return LOSSY_PHYSICAL_KEYS[name];
    const digit = name.match(/^digit([0-9])$/);
    if (digit) return digit[1];
    const numpad = name.match(/^numpad([0-9])$/);
    if (numpad) return numpad[1];
    const arrow = name.match(/^arrow([a-z]+)$/);
    if (arrow) return arrow[1];
    const key = name.match(/^key([a-z])$/);
    if (key) return key[1];
    // 功能键 F1..F24：`event.key` 就是 'f1' 同形，直接放行。
    if (/^f([1-9]|1[0-9]|2[0-4])$/.test(name)) return name;
    // v1 的字符名（`key:e`、`key:1`、`key:up`、`key:+`）与上表同形，走到这里就是单个字符。
    if (/^[a-z0-9+\-_=]$/.test(name)) {
      if (name === '+') return '=';
      return name;
    }
    return null;
  }
  // 该物理键名是否在 web 上**有损合并**（左右修饰键、小键盘同形键、数字小键盘）。
  function physicalKeyIsLossy(name) {
    if (Object.prototype.hasOwnProperty.call(LOSSY_PHYSICAL_KEYS, name)) return true;
    return /^numpad[0-9]$/.test(name);
  }

  // 反向映射：本侧键名 → 引擎绑定文本（`Binding::to_text` 形如 `key:keyj`，物理键名全小写）。
  // 给不出的返回 null，让调用方**报错**而不是造一个假绑定文本（假文本会被游戏侧当真的解析）。
  function keyNameToBindingText(key) {
    const name = String(key || '');
    if (name === 'space') return 'key:space';
    if (name === 'escape') return 'key:escape';
    if (name === 'enter') return 'key:enter';
    if (name === 'tab') return 'key:tab';
    if (name === 'backspace') return 'key:backspace';
    if (name === 'delete') return 'key:delete';
    if (name === 'home' || name === 'end' || name === 'pageup' || name === 'pagedown') return 'key:' + name;
    // 字符 → 物理键是**多对一**：Shift+等号与等号是同一个物理键，引擎侧都写作 key:equal。
    if (name === '=' || name === '+') return 'key:equal';
    if (name === '-' || name === '_') return 'key:minus';
    if (/^[0-9]$/.test(name)) return 'key:digit' + name;
    if (/^[a-z]$/.test(name)) return 'key:key' + name;
    if (name === 'up' || name === 'down' || name === 'left' || name === 'right') return 'key:arrow' + name;
    // 第 74 轮新增的正向词汇必须**在这里也有反查**，否则两边不对称：扫描表里全认，读回来是空。
    // 实测症状：工程写 `move_up=key:shiftleft`，`lua_applied=7` 一切正常，但
    // `input.bindings('move_up')` 返回**空表**（`action_map_text()` 往返同样会把这个动作丢掉）。
    // 反查用同一张表，按"无歧义优先"的顺序：简单名在前、有损合并名在后（合并名取**左**变体，
    // 与 winit 的规范写法一致；右变体在 web 上无法区分，已在 parseInputMap 侧计入 aliased）。
    for (const engine of Object.keys(SIMPLE_KEY_NAMES)) {
      if (SIMPLE_KEY_NAMES[engine] === name && name !== '=' && name !== '-') return 'key:' + engine;
    }
    if (/^f([1-9]|1[0-9]|2[0-4])$/.test(name)) return 'key:' + name;
    for (const engine of Object.keys(LOSSY_PHYSICAL_KEYS)) {
      if (LOSSY_PHYSICAL_KEYS[engine] === name) return 'key:' + engine;
    }
    // WP7-102：`event.code` 给出的**物理名**（shiftleft/shiftright/…）直接放行。
    // 没有这一条，keyName 返回物理名后这里会落到 null ⇒ 反而比合并更糟（绑定整条失效）。
    if (Object.prototype.hasOwnProperty.call(LOSSY_PHYSICAL_KEYS, name)) return 'key:' + name;
    return null;
  }

  // 引擎绑定文本 → 本侧键名集合，返回 [kind, ...keys]（kind: key|pad|mouse|invalid）。
  // pad/mouse 照引擎**算合法**，但本侧没有手柄轮询（鼠标走 mouse_* 一族、不参与动作匹配）
  // ⇒ keys 为空：调用方据此走"文本合法但本侧匹配不到"的分支并登记降级，不假装它可用。
  function bindingTextToKeys(text) {
    const raw = String(text || '').trim();
    const colon = raw.indexOf(':');
    if (colon < 0) return ['invalid'];
    const kind = raw.slice(0, colon).toLowerCase();
    const value = raw.slice(colon + 1).trim().toLowerCase();
    if (kind === 'pad' || kind === 'padaxis' || kind === 'mouse') {
      if (kind === 'pad') return /^[0-9]+$/.test(value) ? ['pad'] : ['invalid'];
      if (kind === 'padaxis') return /^[0-9]+[+-]$/.test(value) ? ['pad'] : ['invalid'];
      if (/^(left|right|middle)$/.test(value) || /^other:[0-9]+$/.test(value)) return ['mouse'];
      return ['invalid'];
    }
    if (kind !== 'key') return ['invalid'];
    const mapped = physicalKeyToName(value);
    if (mapped === null) return ['invalid'];
    const keys = [mapped];
    if (mapped === '=') keys.push('+');
    if (mapped === '-') keys.push('_');
    return ['key'].concat(keys);
  }

  // 把工程键位表编成一小段 Lua 赋值（一次，启动期；不跨帧）。用 luaQuote 保证转义交给唯一实现。
  function luaBindingsSource(bindings) {
    const parts = [];
    for (const action of Object.keys(bindings)) {
      const keys = bindings[action].map(key => luaQuote(key)).join(', ');
      parts.push('[' + luaQuote(action) + '] = {' + keys + '}');
    }
    return '__rustlua_project_bindings = {' + parts.join(', ') + '}';
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

  // 与原生同一个字符串：引擎内建的 1×1 白纹理（`engine/src/host/frame.rs` 的
  // `WHITE_TEXTURE_PATH`），由 Sprite.tint / Image.color 上色。它**不是文件路径**——
  // 原生在 `renderer/gpu_texture.rs` 里就地造白纹理，从不查盘，而 web 侧此前没有这条
  // 分支：`loadImage` 给它拼上 `pack/` 去读一个不存在的文件，于是每个纯色块实体每帧
  // 失败一次（`ensureTexture` 不缓存失败）——实测 10-06 新包 240 帧内 234 张贴图读取
  // 失败、整条微信运行时用例红。
  const WHITE_TEXTURE_PATH = '__white__';
  // 可见层常量（rustlua-render/src/visibility.rs:26 的 ALL_LAYERS = u32::MAX）。
  // **缺 Visibility 组件 ≠ 0 层**：引擎 entity_visible_in 对没有组件的实体按 view_mask != 0 判，
  // 语义是"全可见"，所以这里也用 ALL_LAYERS 表示，绝不能用 0。
  const ALL_LAYERS = 4294967295;

  function isWhiteTexture(path) {
    return path === WHITE_TEXTURE_PATH;
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

  // .prefab 与 .scene 同构（entity/component 行），但带 parent 列（第三列）与 root= 头部。
  // 渲染只需要 UiLayout/Image/Text（offsets 是绝对屏幕坐标，UI 布局不依赖父子边），
  // parent 仅用于 `prefab.set_visible` 的子树显隐语义。
  function parsePrefab(text) {
    const entities = new Map();
    let rootId = null;
    for (const line of text.split(/\r?\n/)) {
      const parts = line.split('\t');
      if (parts[0] === 'entity') {
        const id = Number(parts[1]);
        entities.set(id, {
          id,
          sourceId: id,
          name: parts[2] || '',
          parent: parts[3] !== undefined && parts[3] !== '' ? Number(parts[3]) : null,
          alive: true,
          components: {},
        });
      } else if (parts[0] === 'component') {
        const entity = entities.get(Number(parts[1]));
        if (entity) entity.components[parts[2]] = parts.slice(3).join('\t');
      }
    }
    const rootLine = text.split(/\r?\n/).find(line => line.startsWith('root='));
    if (rootLine) rootId = Number(rootLine.slice(5));
    for (const entity of entities.values()) applyComponents(entity);
    return { entities, rootId };
  }

  // ---- Tilemap 的图集解算（M72-01/02）----
  // 与 `engine/crates/rustlua-ecs/src/tilemap.rs` **逐位一致**：条目布局、4-bit 邻居掩码的
  // 位序、FNV-1a 的参数与盐顺序都抄那一份。任何一边改了没同步另一边，web 与原生就各画各的
  // （M72-12 三宿主目验会把它当"美术问题"查半天）。
  // 掩码位序按 **index** 而不是按方位命名：地图 row 0 在**最底**，图集 row 0 在**最顶**，
  // 同一句"上邻居"在两个坐标系里指向相反方向。
  const AUTOTILE_SLOTS = 16;
  // u64 一律用 **[hi, lo] 两个无符号 32 位**表示，不用 BigInt 字面量：微信上传器的解析器只到
  // ES2019，`1n` 会让**整包上传被拒**（官方报 `Identifier directly after number`，
  // 见 scripts/check-upload-ecma.mjs；此前被 no-modules 胶水守卫的失败掩盖，2026-10-08 才暴露）。
  // 值与 Lua 垫片、Rust 侧逐位一致，由 scripts/check-tile-hash-parity.mjs 与
  // scripts/web-u64-hash-test.js 两侧对拍钉住。
  const VARIANT_HASH_OFFSET = [0xcbf29ce4, 0x84222325];
  const VARIANT_HASH_PRIME = [0x00000100, 0x000001b3];
  const MIX_AXIS_X = [0x9e3779b9, 0x7f4a7c15];
  const MIX_AXIS_Y = [0xbf58476d, 0x1ce4e5b9];
  const DEFAULT_ATLAS_COLS = 6;
  const DEFAULT_ATLAS_ROWS = 1;
  const LEGACY_KIND_LIMIT = 6;

  // 64×64 → 低 64 位。按 16 位分段：每段乘积 < 2^32、累加 < 2^34，全程落在 2^53 内精确。
  // 结果写进调用方给的 2 元数组（`out[0]`=高 32、`out[1]`=低 32）而不是返回新数组：
  // 内层循环每格要乘 8 次，热路径上不该分配（旧实现用 BigInt，每次乘都分配）。
  // 进位**必须**用 `Math.floor(x / 0x10000)`：c1/c2 会超过 2^32，而 `>>>` 先做 ToUint32，
  // 会把高位进位丢掉（2026-10-08 实测：这样写 20 万次随机乘法里 7% 结果偏高/偏低一个字）。
  const U64_MUL_SCRATCH = [0, 0];
  function mulU64(aHi, aLo, bHi, bLo, out) {
    const a0 = aLo & 0xffff, a1 = aLo >>> 16, a2 = aHi & 0xffff, a3 = aHi >>> 16;
    const b0 = bLo & 0xffff, b1 = bLo >>> 16, b2 = bHi & 0xffff, b3 = bHi >>> 16;
    const c0 = a0 * b0;
    const c1 = Math.floor(c0 / 0x10000) + a0 * b1 + a1 * b0;
    const c2 = Math.floor(c1 / 0x10000) + a0 * b2 + a1 * b1 + a2 * b0;
    // 只留低 64 位，所以 a1*b3 / a2*b2 / a3*b1 这些进第 5 个字的项直接丢。
    const c3 = Math.floor(c2 / 0x10000) + a0 * b3 + a1 * b2 + a2 * b1 + a3 * b0;
    out[0] = ((c3 & 0xffff) << 16 | (c2 & 0xffff)) >>> 0;
    out[1] = ((c1 & 0xffff) << 16 | (c0 & 0xffff)) >>> 0;
    return out;
  }

  // 64 位逻辑右移（我们的输入都被掩成无符号，不存在算术右移的分支）。
  function shiftRightU64(hi, lo, shift) {
    if (shift <= 0) return [hi >>> 0, lo >>> 0];
    if (shift < 32) return [hi >>> shift, ((lo >>> shift) | (hi << (32 - shift))) >>> 0];
    return [0, hi >>> (shift - 32)];
  }

  // 把整数拆成低 64 位。x/y/seed 都从 JSON 进来，量级远小于 2^53；负数按补码，
  // 与旧实现 `BigInt(-5) >> 0 & 0xff` 的取字节语义一致（算术右移 + 掩码 = 补码低位）。
  function splitU64(value) {
    if (typeof value !== 'number' || !isFinite(value) || Math.floor(value) !== value) {
      // 旧实现在这里会 `BigInt(1.5)` / `BigInt(undefined)` 抛错：保持"响亮失败"，
      // 不要悄悄算出一个数字——那会让两张地图用同一个哈希而没人知道。
      throw new Error('tile variant hash needs integer inputs, got ' + String(value));
    }
    const negative = value < 0;
    const magnitude = negative ? -value : value;
    const lo = magnitude % 0x100000000;
    const hi = Math.floor(magnitude / 0x100000000) % 0x100000000;
    if (!negative) return [hi, lo];
    return [
      ((~hi >>> 0) + (lo === 0 ? 1 : 0)) >>> 0,
      (-lo) >>> 0,
    ];
  }

  function mixU64(hi, lo, valueHi, valueLo) {
    let mixedHi = hi, mixedLo = lo;
    for (let index = 0; index < 8; index += 1) {
      const byte = index < 4
        ? (valueLo >>> (index * 8)) & 0xff
        : (valueHi >>> ((index - 4) * 8)) & 0xff;
      mixedLo = (mixedLo ^ byte) >>> 0;
      mulU64(mixedHi, mixedLo, VARIANT_HASH_PRIME[0], VARIANT_HASH_PRIME[1], U64_MUL_SCRATCH);
      mixedHi = U64_MUL_SCRATCH[0];
      mixedLo = U64_MUL_SCRATCH[1];
    }
    return [mixedHi, mixedLo];
  }

  // 确定性变体序号：绝不能用 Math.random / 每帧变化的种子，那等于地表每帧抖一次。
  // avalanche（splitmix64 的收尾混洗）不是可选的美化：原始 FNV 的低比特周期等于模数，
  // variant_count=4 时取模结果在第一行就是 3,2,1,0,3,2,1,0 的周期点阵——确定性有、质感没有。
  const AVALANCHE_MUL_A = [0xbf58476d, 0x1ce4e5b9];
  const AVALANCHE_MUL_B = [0x94d049bb, 0x133111eb];

  function avalanche(hi, lo) {
    let shifted = shiftRightU64(hi, lo, 30);
    let stageHi = (hi ^ shifted[0]) >>> 0, stageLo = (lo ^ shifted[1]) >>> 0;
    mulU64(stageHi, stageLo, AVALANCHE_MUL_A[0], AVALANCHE_MUL_A[1], U64_MUL_SCRATCH);
    stageHi = U64_MUL_SCRATCH[0];
    stageLo = U64_MUL_SCRATCH[1];
    shifted = shiftRightU64(stageHi, stageLo, 27);
    stageHi = (stageHi ^ shifted[0]) >>> 0;
    stageLo = (stageLo ^ shifted[1]) >>> 0;
    mulU64(stageHi, stageLo, AVALANCHE_MUL_B[0], AVALANCHE_MUL_B[1], U64_MUL_SCRATCH);
    stageHi = U64_MUL_SCRATCH[0];
    stageLo = U64_MUL_SCRATCH[1];
    shifted = shiftRightU64(stageHi, stageLo, 31);
    return [(stageHi ^ shifted[0]) >>> 0, (stageLo ^ shifted[1]) >>> 0];
  }

  // 64 位对小数取模：从高 16 位起逐段取模，避免 `hi * 2^32` 冲出 2^53。
  function modU64Small(hi, lo, divisor) {
    let mod = 0;
    mod = (mod * 0x10000 + (hi >>> 16)) % divisor;
    mod = (mod * 0x10000 + (hi & 0xffff)) % divisor;
    mod = (mod * 0x10000 + (lo >>> 16)) % divisor;
    mod = (mod * 0x10000 + (lo & 0xffff)) % divisor;
    return mod;
  }

  function variantFor(map, x, y) {
    const count = map.variant_count;
    if (count < 2) return 0;
    const seed = splitU64(map.variant_seed);
    const axisX = splitU64(x);
    const axisY = splitU64(y);
    let hashHi = VARIANT_HASH_OFFSET[0], hashLo = VARIANT_HASH_OFFSET[1];
    for (const value of [axisX, axisY, seed, MIX_AXIS_X, MIX_AXIS_Y]) {
      const mixed = mixU64(hashHi, hashLo, value[0], value[1]);
      hashHi = mixed[0];
      hashLo = mixed[1];
    }
    const mixed = avalanche(hashHi, hashLo);
    return modU64Small(mixed[0], mixed[1], count);
  }

  function autotileMask(map, x, y) {
    const cells = map.cells;
    const own = cells[y * map.width + x] || 0;
    let mask = 0;
    const offsets = [[0, -1], [1, 0], [0, 1], [-1, 0]];
    for (let bit = 0; bit < offsets.length; bit += 1) {
      const nx = x + offsets[bit][0];
      const ny = y + offsets[bit][1];
      // 越界 = 没有邻居（地图边就是暴露的边界，不是内部瓦）。同 kind 才算邻居。
      if (nx < 0 || ny < 0 || nx >= map.width || ny >= map.height) continue;
      const neighbor = cells[ny * map.width + nx] || 0;
      if (neighbor !== 0 && neighbor === own) mask |= 1 << bit;
    }
    return mask;
  }

  function maxTileId(map) {
    if (map.autotile || map.variant_count > 1) return Math.max(1, map.tile_kinds);
    return map.atlas_cols * map.atlas_rows;
  }

  function resolveAtlasIndex(map, x, y) {
    const cell = map.cells[y * map.width + x] || 0;
    if (cell === 0) return 0;
    if (!map.autotile && map.variant_count <= 1) return cell;
    const kind = cell;
    if (kind > map.tile_kinds) return 0;
    const span = Math.max(1, map.variant_count);
    const capacity = map.atlas_cols * map.atlas_rows;
    let index;
    if (map.autotile) {
      index = 1 + map.tile_kinds * span + (kind - 1) * AUTOTILE_SLOTS
        + (autotileMask(map, x, y) % AUTOTILE_SLOTS);
    } else {
      index = 1 + (kind - 1) * span + variantFor(map, x, y);
    }
    return index <= capacity ? index : 0;
  }

  // 解算结果按布局缓存：10 万格档每帧重算 BigInt 哈希会把帧时间吃光，而"布局没变 ⇒ 解算
  // 没变"是这条门的结构事实。`cells` 被改写（op C）或布局被改写（op L）时版本 +1，缓存作废。
  function layoutKey(map) {
    return [
      map.width, map.height, map.atlas_cols, map.atlas_rows, map.tile_kinds,
      map.variant_count, map.autotile ? 1 : 0, map.variant_seed, map.resolveVersion || 0,
    ].join('|');
  }

  function resolvedEntries(map) {
    const key = layoutKey(map);
    if (map.resolved && map.resolved.key === key) return map.resolved.entries;
    const entries = new Array(map.width * map.height);
    for (let y = 0; y < map.height; y += 1) {
      for (let x = 0; x < map.width; x += 1) {
        entries[y * map.width + x] = resolveAtlasIndex(map, x, y);
      }
    }
    map.resolved = { key, entries };
    return entries;
  }

  function tintAt(map, index) {
    const id = (map.tint_ids && map.tint_ids[index]) || 0;
    if (id === 0) return null;
    const color = map.tint_table && map.tint_table[id - 1];
    return Array.isArray(color) && color.length === 4 ? color : null;
  }

  function blankTintIds(length, previous) {
    const ids = new Array(length).fill(0);
    if (Array.isArray(previous)) {
      for (let i = 0; i < Math.min(length, previous.length); i += 1) ids[i] = previous[i] || 0;
    }
    return ids;
  }

  function parseTintTable(raw, previous) {
    if (raw === undefined) return Array.isArray(previous) ? previous.slice() : [];
    const table = [];
    for (const entry of String(raw).split('|')) {
      if (!entry) continue;
      const parts = entry.split(',').map((part) => Number(part));
      // 与 Rust 侧同一条硬规则：不是四段就整串报错，不截断也不补默认值。
      if (parts.length !== 4 || parts.some((part) => !Number.isFinite(part))) {
        throw new Error(`tints_rgb entries must be r,g,b,a: ${entry}`);
      }
      table.push(parts);
    }
    return table;
  }

  function applySparseTints(map, raw) {
    if (raw === undefined) return;
    for (const entry of String(raw).split(',')) {
      if (!entry) continue;
      const [cell, id] = entry.split(':');
      const index = Number(cell);
      const tintId = Number(id);
      if (!Number.isInteger(index) || index < 0 || index >= map.tint_ids.length) {
        throw new Error(`tints cell index ${cell} is out of range`);
      }
      if (!Number.isInteger(tintId) || tintId < 0 || tintId > map.tint_table.length) {
        throw new Error(`tints id ${id} has no entry in tints_rgb`);
      }
      map.tint_ids[index] = tintId;
    }
  }

  // 离屏画布：三个宿主各有一套（浏览器 `document.createElement`、小游戏 `wx.createCanvas`、
  // 部分环境只有 `OffscreenCanvas`）。这里必须**探到能用为止**，因为适配层的
  // `document.createElement('canvas')` 返回的是没有 `getContext` 的桩对象（见
  // `plugins/wechat-minigame/wechat-adapter.js`），只看 `typeof document` 会拿到废画布。
  // 另一条硬约束：`wx.createCanvas()` **首次**调用返回的是上屏画布（适配层已经用它拿主画布，
  // 但不保证命中那条路径），第二次起才是离屏。所以拿到候选后还要和主画布比一次身份——
  // 万一相等，用它等于每格把整屏擦掉。
  function createOffscreenCanvas(width, height, mainCanvas) {
    const candidates = [];
    if (typeof OffscreenCanvas === 'function') candidates.push(() => new OffscreenCanvas(width, height));
    if (typeof document !== 'undefined' && typeof document.createElement === 'function') {
      candidates.push(() => document.createElement('canvas'));
    }
    if (typeof wx !== 'undefined' && typeof wx.createCanvas === 'function') {
      candidates.push(() => wx.createCanvas());
    }
    for (const create of candidates) {
      let canvas = null;
      try {
        canvas = create();
      } catch (error) {
        continue;
      }
      if (!canvas || canvas === mainCanvas) continue;
      canvas.width = width;
      canvas.height = height;
      const context = typeof canvas.getContext === 'function' ? canvas.getContext('2d') : null;
      if (context) return context;
    }
    return null;
  }

  // 逐格染色的画布实现。引擎的语义是「贴图色 × 顶点色」（着色器里的乘法混合），画布 2D
  // 没有逐像素乘法，所以两趟复合：
  //   1. `multiply` 填充：不透明格子得到 Ct×Cm（与引擎一致）；透明格子会被填上一层填充色；
  //   2. `destination-in` 重采样原格子：把 alpha 乘回去，同时把第 1 步漏进透明区的颜色擦掉。
  // 二值 alpha 的图集（本项目所有瓦片图）下与引擎逐像素一致；抗锯齿边缘像素按画布的复合
  // 公式仍有极小偏差，这是画布 2D 的限制而非这里的近似选择，写在这里免得被当成美术问题。
  // 结果按「贴图 + 源格 + 染色色」缓存：一层通常只有几种染色色（季节/阴影），每格新建
  // 画布等于每帧几百次分配。
  const tintedTileCache = new Map();
  const TINTED_TILE_CACHE_LIMIT = 512;
  // 外层预算：外层键是**图集 Image 对象**，而 loadImage 每次都 new Image（没有跨场景的贴图缓存），
  // 于是每换一次场景、每出现一张带染色的图集，就多一条**永远不再被使用**的外层条目——每条
  // 还各自持有最多 TINTED_TILE_CACHE_LIMIT 张离屏画布。此前只有"同一张贴图内部"的上限，
  // 外层没有任何上限，反复切图会一路把离屏画布留在内存里（症状是长跑后内存只涨不落）。
  // 这里按插入序淘汰最旧的图集（Map 保序），再配合换场景时的显式释放。
  // Sprite.tint 也走这座缓存（投影贴片/装饰每件一张小图、不是图集）：单场景的贴片 +
  // 装饰 + 瓦片图集的外层条目轻松过十，8 会每帧淘汰重建（离屏画布每帧重合成），故放宽到 32。
  const TINTED_TILESET_CACHE_LIMIT = 32;

  // 换场景时调用：`images` 整批换成新 loadImage 的结果，旧图集对象不会再被 drawTintedTile
  // 用到，缓存里的离屏画布必须跟着放掉（否则就是上面那条只涨不落的保留链）。
  function releaseTintedTileCache() {
    tintedTileCache.clear();
  }

  function tintedTileContext(tileset, sx, sy, sw, sh, tint, mainCanvas) {
    const key = `${sx},${sy},${sw},${sh}|${tint.slice(0, 3).join(',')}|${tint[3]}`;
    let bySource = tintedTileCache.get(tileset);
    if (!bySource) {
      bySource = new Map();
      tintedTileCache.set(tileset, bySource);
      // 超预算时淘汰最旧的图集条目（连同它名下全部离屏画布一起丢）。
      while (tintedTileCache.size > TINTED_TILESET_CACHE_LIMIT) {
        const oldest = tintedTileCache.keys().next();
        if (oldest.done) break;
        tintedTileCache.delete(oldest.value);
      }
    }
    const cached = bySource.get(key);
    if (cached) return cached;
    const context = createOffscreenCanvas(Math.max(1, Math.round(sw)), Math.max(1, Math.round(sh)), mainCanvas);
    if (!context) return null;
    // 离屏画布最终由一次 drawImage 落到主画布：不盖源贴图的 src，宿主侧绘制序列里
    // 这次绘制就是空串 src——微信层带门按 src 认投影贴片/遮挡物（M72-06），会整个失明。
    context.canvas._src = (tileset && (tileset._src || tileset.src)) || '';
    context.drawImage(tileset, sx, sy, sw, sh, 0, 0, context.canvas.width, context.canvas.height);
    context.globalCompositeOperation = 'multiply';
    // tint 分量必须**逐通道钳到 [0,255]**：农田湿度等染色来自 farm_soil.lua 的
    // `moist_tints_rgb`，蓝通道是 1.12/1.06/1.01（引擎里 tint>1 = 提亮）。原生片元直接
    // 乘采样值、帧缓冲自然钳位，但 canvas2d 的 fillStyle 走 CSS 解析：`rgb(204,224,286)`
    // 因为 286>255 是**非法颜色**，赋值被静默忽略 → fillStyle 保持上一值（新画布默认黑）
    // → multiply 把整格压成黑块（真机 2026-10-11：微信农场里成片深蓝/紫方块）。钳到 255
    // 等价于「该通道不提亮也不压暗」，是 canvas2d 能做到的、最接近引擎语义的近似。
    const tintR = Math.max(0, Math.min(255, Math.round(tint[0] * 255)));
    const tintG = Math.max(0, Math.min(255, Math.round(tint[1] * 255)));
    const tintB = Math.max(0, Math.min(255, Math.round(tint[2] * 255)));
    context.fillStyle = `rgb(${tintR},${tintG},${tintB})`;
    context.fillRect(0, 0, context.canvas.width, context.canvas.height);
    context.globalCompositeOperation = 'destination-in';
    context.drawImage(tileset, sx, sy, sw, sh, 0, 0, context.canvas.width, context.canvas.height);
    if (tint[3] < 1) {
      // 染色的 alpha 也乘进顶点的 alpha（引擎：a = 贴图 a × 顶点 a）。
      context.globalAlpha = Math.max(0, tint[3]);
      context.fillRect(0, 0, context.canvas.width, context.canvas.height);
      context.globalAlpha = 1;
    }
    context.globalCompositeOperation = 'source-over';
    // 上限按「同一张贴图」计：条目数 = 源格数 × 染色色数，正常一张图集几十条就够，
    // 超了说明数据在无限增长（比如把逐帧变化的色送进来），整张贴图的缓存一起丢。
    if (bySource.size >= TINTED_TILE_CACHE_LIMIT) bySource.clear();
    bySource.set(key, context);
    return context;
  }

  function drawTintedTile(ctx, tileset, sx, sy, sw, sh, tint, dx, dy, dw, dh) {
    const tinted = tintedTileContext(tileset, sx, sy, sw, sh, tint, ctx.canvas);
    // 宿主给不出离屏画布时退化成**不染色**直绘：宁可少一层染色，也不能把主画布当离屏用。
    if (!tinted) {
      ctx.drawImage(tileset, sx, sy, sw, sh, dx, dy, dw, dh);
      return;
    }
    ctx.drawImage(tinted.canvas, 0, 0, tinted.canvas.width, tinted.canvas.height, dx, dy, dw, dh);
  }

  // Lua 侧的 tilemap 镜像：只交**数据**字段。`resolved`（解算缓存）与稠密的零值
  // `tint_ids` 不进 Lua 启动串——前者是渲染器每帧重算用的，后者在没有染色时全是 0，
  // 而 Lua 侧读的都是 `m.tint_ids[i] or 0`，稀疏表语义等价、串长短一半以上。
  function luaTilemapMirror(map) {
    const tints = {};
    for (let index = 0; index < map.tint_ids.length; index += 1) {
      if (map.tint_ids[index]) tints[index + 1] = map.tint_ids[index];
    }
    return {
      tile_size: map.tile_size,
      width: map.width,
      height: map.height,
      texture: map.texture,
      cells: map.cells,
      solid: map.solid,
      tiles_format: map.tiles_format,
      atlas_cols: map.atlas_cols,
      atlas_rows: map.atlas_rows,
      tile_kinds: map.tile_kinds,
      variant_count: map.variant_count,
      autotile: map.autotile,
      variant_seed: map.variant_seed,
      tint_ids: tints,
      tint_table: map.tint_table,
    };
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
      const width = Number(p.width || previous.width || 0);
      const height = Number(p.height || previous.height || 0);
      // 两种 tiles 编码（docs/16 §瓦片映射）：
      //   `tiles_format=0`（旧）：一格一个 ASCII 数字，无分隔符，表达不了 >9；
      //   `tiles_format=1`（当前）：逗号分隔十进制图集索引。
      // 存成数字数组而不是字符串：字符串只能一格一个字符，而渲染与 Lua 都要按**值**读写。
      const format = Number(p.tiles_format || 0);
      let cells;
      if (p.tiles === undefined) {
        cells = (previous.cells || []).slice();
      } else if (format >= 1) {
        cells = String(p.tiles).split(',').map((part) => Number(part) || 0);
      } else {
        cells = Array.from(String(p.tiles)).map((ch) => Number(ch) || 0);
      }
      const atlasCols = Number(p.atlas_cols || previous.atlas_cols || DEFAULT_ATLAS_COLS);
      const atlasRows = Number(p.atlas_rows || previous.atlas_rows || DEFAULT_ATLAS_ROWS);
      const variantCount = Math.max(1, Number(p.variant_count || previous.variant_count || 1));
      // 没有 tile_kinds 的串只可能是旧图：旧编码里格子值恒 ≤ 6（一个图集列一个条目）。
      const tileKinds = Number(p.tile_kinds || 0) || LEGACY_KIND_LIMIT;
      entity.tilemap = {
        tile_size: tileSize[0] || 32,
        width,
        height,
        texture: p.texture || previous.texture || '',
        cells,
        solid: String(p.solid !== undefined ? p.solid : (previous.solid || '')),
        tiles_format: format,
        atlas_cols: atlasCols > 0 ? atlasCols : DEFAULT_ATLAS_COLS,
        atlas_rows: atlasRows > 0 ? atlasRows : DEFAULT_ATLAS_ROWS,
        tile_kinds: tileKinds,
        variant_count: variantCount,
        autotile: p.autotile === '1' || p.autotile === 'true' || previous.autotile === true,
        variant_seed: Number(p.variant_seed || previous.variant_seed || 0),
        // per-tile tint：稀疏存储。`tints=<cellIndex>:<tintId>,...` + `tints_rgb=r,g,b,a|...`。
        tint_ids: blankTintIds(width * height, previous.tint_ids),
        tint_table: parseTintTable(p.tints_rgb, previous.tint_table),
        resolveVersion: previous.resolveVersion || 0,
      };
      applySparseTints(entity.tilemap, p.tints);
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
      // **缺字段时必须留空数组，不能对空串 split**：`Number('') === 0`，于是
      // `String(undefined || '').split(',').map(Number)` 得到 `[0]`（不是 NaN），
      // 主角这种不写 pivot 的 Sprite 会被当成 (0, 0.5) ⇒ 贴图整体右移半个身位，
      // 与居中的 Collider 描边错开一半（实机 2026-10-04 截图）。
      const pivot = p.pivot === undefined ? [] : String(p.pivot).split(',').map(Number);
      entity.sprite = {
        texture: p.texture || '',
        size: width,
        size_y: height,
        pivot_x: Number.isFinite(pivot[0]) ? pivot[0] : 0.5,
        pivot_y: Number.isFinite(pivot[1]) ? pivot[1] : 0.5,
        // 排序层与层内次序：原生按 `SpriteSorting::effective_layer` = 层秩×1000 + order 分桶
        // （rustlua-render/src/sprite.rs）。不解析就等于所有实体同层，只能按 id 画。
        layer: String(p.sorting_layer || 'default'),
        order: Number(p.order) || 0,
        // tint（原生 `Sprite.tint: [f32;4]`，缺省 [1,1,1,1]，见 sprite.rs:156）：只有
        // `texture=__white__` 这条路径在 web 上用得到——白图被 tint 乘成纯色块，
        // 不解析就会全画成白色。
        tint: p.tint === undefined ? [1, 1, 1, 1] : parseColor(p.tint, [1, 1, 1, 1]),
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
    // 世界空间文字（场景里的 WorldText 组件 / world_text.attach）：锚在实体的**世界位置**上、
    // 随相机移动，与屏幕空间的 UI Text（drawUi 那两趟）是两条路。引擎侧结构是
    // WorldText { text: UiText, offset: [f32;2], centered: bool }——字体字段与 UI Text 同一套。
    if (entity.components.WorldText) {
      const fields = parseComponentFields(entity.components.WorldText);
      const offset = String(fields.offset || '0,0').split(',').map(Number);
      entity.worldText = {
        value: fields.text || '',
        size: Number(fields.font_size) || 24,
        color: parseColor(fields.color, [1, 1, 1]),
        offsetX: Number.isFinite(offset[0]) ? offset[0] : 0,
        offsetY: Number.isFinite(offset[1]) ? offset[1] : 0,
        centered: fields.centered === 'true' || fields.centered === '1'
      };
    } else {
      // detach 语义：组件真的没了就必须**停止绘制**，而不是留着上一次的副本继续画。
      delete entity.worldText;
    }
    // 可见层组件（visibility，WP7-34）：场景里目前**没有**任何实体写它（typed_scene 的组件表
    // 还没覆盖 Visibility，见 examples/game_api/scenes/main.scene 的注释），因此这条解析
    // 是为运行期 visibility.set 补的组件串回读路径；缺组件时保持"全可见"。
    if (entity.components.Visibility) {
      const fields = parseComponentFields(entity.components.Visibility);
      const layers = Number(fields.layers);
      entity.layers = Number.isFinite(layers) ? (layers >>> 0) : ALL_LAYERS;
    } else if (entity.layers === undefined) {
      entity.layers = ALL_LAYERS;
    }
  }

  function loadImage(path) {
    return new Promise(resolve => {
      if (!path || isWhiteTexture(path)) {
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
      // WP7-102：`event.code` **只**用在"已枚举的有损合并"上（左右修饰键、小键盘同形键），
      // 其余键名继续沿用 `event.key` —— 键名空间的变化因此是**可枚举、可测试**的。
      // 改这里必须**同时**看 keyNameToBindingText（名字换算只有一份权威），否则绑定会静默失效。
      const code = String(event.code || '').toLowerCase();
      if (code && Object.prototype.hasOwnProperty.call(LOSSY_PHYSICAL_KEYS, code)) return code;
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
    // 事件坐标 → 设计坐标（1280×720）。两级：
    //   ① CSS 像素 → 画布像素：rect 是布局尺寸、canvas.width 是后备缓冲，浏览器里
    //      两者必然不等（差 dpr 与窗口比例）。缺这一步等于把 CSS 像素当设计像素用，
    //      实测 530×557 窗口里画布视觉中心算成 (265,279) 而不是 (640,360)，越靠右下
    //      偏得越多——虚拟摇杆/按钮与"点哪块地"整条命中链一起错位。
    //   ② 画布像素 → 设计坐标：与 drawScene 的 contain 变换互逆（(x - ox) / s）。
    // 微信适配层的 getBoundingClientRect 桩直接返回 canvas 物理尺寸，①恒等于 1，
    // 所以这一级对小游戏是安全的空操作。
    const eventToDesign = event => {
      const rect = canvas.getBoundingClientRect();
      const px = (event.clientX - rect.left) * (rect.width > 0 ? canvas.width / rect.width : 1);
      const py = (event.clientY - rect.top) * (rect.height > 0 ? canvas.height / rect.height : 1);
      const fitS = Math.min(canvas.width / 1280, canvas.height / 720);
      const fitOx = (canvas.width - 1280 * fitS) / 2;
      const fitOy = (canvas.height - 720 * fitS) / 2;
      return [(px - fitOx) / fitS, (py - fitOy) / fitS];
    };
    canvas.addEventListener('mousedown', event => {
       const button = event.button === 2 ? 'right' : 'left';
       // 每次 mousedown 都记按下边沿（不只首按）：多指场景里第二根手指按按钮也必须产生
       // 一次边沿，否则"按住摇杆再点按钮"按钮永远收不到按下（微信端虚拟按钮失效的根因③）。
       // 按下位置单独记进 press_cursor：光标可能正跟手（摇杆），虚拟控件按按下位置命中测试。
       state.mouseJust[button] = true;
       state.mouse[button] = true;
       state.pressCursor = eventToDesign(event);
     });
     window.addEventListener('mouseup', event => {
       const button = event.button === 2 ? 'right' : 'left';
       // 有按下过才记边沿：宿主（微信适配层）在触摸链上可能补发 mouseup，
       // 没按过的"抬起"不该产生一次幽灵点击。
       if (state.mouse[button]) state.mouseUp[button] = true;
       state.mouse[button] = false;
     });
    canvas.addEventListener('mousemove', event => {
      state.cursor = eventToDesign(event);
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
  //   L  tilemap.set_atlas / set_kinds      I  tilemap.set_tint    J  tilemap.clear_tint
  //   Z  tilemap.set_solid
  //   D  destroy                            T  Transform
  //   A  动画当前 clip + **帧序号**（不是累计 tick，引擎 current_frame 的算法在 Lua 侧）
  // 结构行（R/S/X/C/U/L/I/J/Z/D）由 op 列表一次性发出，T/A 每帧对存活实体重发。
  function syncLuaScene(sceneState, text) {
    const payload = String(text || '');
    // WP7-60：每帧状态载荷的字节数——这是"状态导出要不要换二进制"这个问题的**基线**，
    // 纯 JS 侧读长度、无额外跨界，所以常驻不心疼（读数只认一个来源）。
    stateBytes += payload.length;
    for (const line of payload.split(/\r?\n/)) {
      if (!line) continue;
      const parts = line.split('\t');
      const id = Number(parts[1]);
      if (parts[0] === 'H') {
        // 隐藏文本集（WP7-59）：跟状态**同一份载荷**到达，帧循环因此不必再单独拉一次
        // `__rustlua_export_scene_text_hidden`（每帧少一次跨界 + 少一次 Set 重建）。
        // **空行同样要覆盖**：脚本把名字恢复可见之后，旧名单必须真的消失。
        // 它不参与实体解析，所以先处理并 `continue`（`parts[1]` 在这里是名字表，不是 id）。
        p2SceneTextHidden = new Set();
        for (const name of (parts[1] ? String(parts[1]).split(',') : [])) {
          if (name) p2SceneTextHidden.add(name);
        }
        sceneTextHiddenFromState = true;
        continue;
      }
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
        // WP7-60：这里收到几行 T，就是 Lua 侧**真的发了**几行（未变的实体不发）。
        // 这个计数是门用来证明"按变化发"确实生效的证据：全量重发是 帧数 × 存活实体数。
        stateTransformLines += 1;
        entity.transform = { x: Number(parts[2] || 0), y: Number(parts[3] || 0), rotation: Number(parts[4] || 0), scale_x: Number(parts[5] || 1), scale_y: Number(parts[6] || 1) };
      } else if (parts[0] === 'A') {
        const previous = entity.animation || { clips: {} };
        entity.animation = { current: parts[2] || 'idle', frame: Number(parts[3] || 0), clips: previous.clips || {} };
      } else if (parts[0] === 'S') {
        entity.components.Sprite = parts.slice(2).join('\t');
        applyComponents(entity);
      } else if (parts[0] === 'Q') {
        // `quad.set`（WP7-30）：与 S 行同构——整串交给同一份 applyComponents，
        // 缺省字段的默认值由渲染侧那份解析决定，Lua 侧不另立一套。
        entity.components.Quad = parts.slice(2).join('\t');
        applyComponents(entity);
      } else if (parts[0] === 'W') {
        // world_text.attach / detach：非空串 = 挂上，空串 = 摘掉（"真的移除了"由 Lua 侧返回，
        // 这里只负责让渲染侧跟着变——空串删组件才会停止绘制）。
        const spec = parts.slice(2).join('\t');
        if (spec) {
          entity.components.WorldText = spec;
        } else {
          delete entity.components.WorldText;
        }
        applyComponents(entity);
      } else if (parts[0] === 'V') {
        // visibility.set：层掩码变了要影响**绘制**——引擎是"不在视图里就不产顶点"，
        // 不是"画成透明"。0 表示在任何视图里都不画。
        entity.layers = Number(parts[2]) >>> 0;
      } else if (parts[0] === 'K') {
        // 行字母选 K：C 已经被「格子值」占用（见下面那个 C 分支），用它会把瓦片格更新遮蔽掉。
        // camera.set_active：本侧只有一个"主动相机"概念，用它决定相机变焦的来源。
        activeCameraId = Number(parts[1]);
      } else if (parts[0] === 'E') {
        // camera2d 的**请求**（行字母 E：上面这套 if/else 链里 E 未被占用，插入前已核对）。
        // `E\t<动词>\t…`：center / zoom / follow / unfollow / shake。请求在这里落成渲染侧状态，
        // 由 computeCamera 消费，再由宿主每帧把结果回填给 Lua ⇒ 脚本 get 到的就是"渲染用的那份"。
        const verb = parts[1];
        if (verb === 'center') {
          camera2dState.center = { x: Number(parts[2]) || 0, y: Number(parts[3]) || 0 };
          camera2dState.scripted = true;
        } else if (verb === 'zoom') {
          const zoom = Number(parts[2]);
          if (Number.isFinite(zoom) && zoom > 0) {
            camera2dState.zoom = zoom;
            camera2dState.scripted = true;
          }
        } else if (verb === 'follow') {
          camera2dState.followId = parts[2] === '-' || parts[2] === undefined ? null : Number(parts[2]);
          camera2dState.offsetX = Number(parts[3]) || 0;
          camera2dState.offsetY = Number(parts[4]) || 0;
          camera2dState.deadX = Number(parts[5]) || 0;
          camera2dState.deadY = Number(parts[6]) || 0;
          camera2dState.smoothing = Number(parts[7]) || 0;
          // follow(nil) 与 unfollow() 在引擎里是同一件事（api/camera.rs:359 `None => None`）：
          // 清掉跟随目标，相机**停在原地**——不是"回落到默认启发式"。默认启发式（跟名字像 player
          // 的实体）只是本侧的既有口径，脚本一旦解除跟随就该停住，否则解除动作看起来毫无效果。
          if (camera2dState.followId === null) freezeCameraHere();
          camera2dState.scripted = true;
        } else if (verb === 'unfollow') {
          camera2dState.followId = null;
          freezeCameraHere();
          camera2dState.scripted = true;
        } else if (verb === 'shake') {
          const amp = Number(parts[2]) || 0;
          const duration = Number(parts[3]) || 0;
          if (amp > 0 && duration > 0) {
            camera2dState.shakeAmp = amp;
            camera2dState.shakeLeft = duration;
            camera2dState.shakeTotal = duration;
          }
        }
      } else if (parts[0] === 'X') {
        entity.components.Animation = parts.slice(2).join('\t');
        applyComponents(entity);
      } else if (parts[0] === 'C') {
        // 格子值 = **图集条目**（默认布局）或**逻辑 kind**（autotile/变体布局），语义由
        // 布局决定，解算在 `resolveAtlasIndex` 里做。这里只写原始格子值。
        const map = entity.tilemap;
        const x = Number(parts[2]);
        const y = Number(parts[3]);
        if (!map || !Number.isFinite(x) || !Number.isFinite(y)) continue;
        const index = y * map.width + x;
        if (index < 0 || index >= map.cells.length) continue;
        const value = Math.max(0, Math.min(255, Math.floor(Number(parts[4]) || 0)));
        if (map.cells[index] === value) continue;
        map.cells[index] = value;
        // 解算缓存按版本作废：一格变，autotile 下邻格的掩码也可能跟着变，整层重算
        // 才是不出错的口径（原生侧是脏块重建 + 掩码外圈，同一件事）。
        map.resolveVersion = (map.resolveVersion || 0) + 1;
      } else if (parts[0] === 'L') {
        // `tilemap.set_atlas` / `tilemap.set_kinds`（M72-01/02 的布局声明）。整行是
        // `L\t<id>\t<cols>\t<rows>\t<kinds>\t<variants>\t<autotile>\t<seed>`。
        const map = entity.tilemap;
        if (!map) continue;
        const cols = Math.floor(Number(parts[2]) || 0);
        const rows = Math.floor(Number(parts[3]) || 0);
        if (cols > 0) map.atlas_cols = cols;
        if (rows > 0) map.atlas_rows = rows;
        map.tile_kinds = Math.max(0, Math.floor(Number(parts[4]) || 0));
        map.variant_count = Math.max(1, Math.floor(Number(parts[5]) || 1));
        map.autotile = parts[6] === '1' || parts[6] === 'true';
        map.variant_seed = Math.floor(Number(parts[7]) || 0);
        map.resolveVersion = (map.resolveVersion || 0) + 1;
      } else if (parts[0] === 'I') {
        // 逐格染色（M72-02）：`I\t<id>\t<x>\t<y>\t<r>\t<g>\t<b>\t<a>`。传**颜色**而不是
        // 调色板序号：序号是存储细节，两侧各自去重即可，交换序号只会把两侧的对齐问题
        // 暴露到画面上（原生 `set_tint` 也是在引擎侧分配的）。
        const map = entity.tilemap;
        const x = Number(parts[2]);
        const y = Number(parts[3]);
        if (!map || !Number.isFinite(x) || !Number.isFinite(y)) continue;
        const index = y * map.width + x;
        if (index < 0 || index >= map.tint_ids.length) continue;
        const color = [Number(parts[4]), Number(parts[5]), Number(parts[6]), Number(parts[7])];
        if (color.some((part) => !Number.isFinite(part))) continue;
        let tintId = map.tint_table.findIndex((existing) => existing.join(',') === color.join(','));
        if (tintId < 0) {
          map.tint_table.push(color);
          tintId = map.tint_table.length - 1;
        }
        map.tint_ids[index] = tintId + 1;
      } else if (parts[0] === 'J') {
        // `tilemap.clear_tint`：`J\t<id>\t<x>\t<y>`（0 = 不画染色，回到贴图原色）。
        const map = entity.tilemap;
        const x = Number(parts[2]);
        const y = Number(parts[3]);
        if (!map || !Number.isFinite(x) || !Number.isFinite(y)) continue;
        const index = y * map.width + x;
        if (index >= 0 && index < map.tint_ids.length) map.tint_ids[index] = 0;
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
    // 小游戏里字体是适配层用 wx.loadFont 注册的（它会把 ctx.font 里的 RustLuaGameFont 换成
    // 宿主返回的真实族名）。这里的 FontFace + document.fonts 那条路在小游戏宿主上会撞上原生
    // FontFaceSet，抛 "parameter 1 is not of type 'FontFace'"，所以认标记、不再自己注册。
    if (typeof window !== 'undefined' && window.__RUSTLUA_FONT_OWNER === 'wechat-adapter') {
      GAME_FONT = 'RustLuaGameFont, sans-serif';
      return;
    }
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
  // 验收/诊断开关（**发布默认关闭**）。
  //
  // 为什么要有这个开关：`data-rustlua-drawn` 是"页面启动了但没画面"的唯一判据，但它靠
  // 全画布 `getImageData` 得来 —— 高 DPR 画布上那是一次 GPU 同步读回 + `width*height*4`
  // 的临时内存，而它在发布包里**没有任何消费方**（玩家不看 DOM 属性，验收脚本才看）。
  // 此前它无条件每 30 帧跑一次，等于把一份只在验收时有用的开销常驻在玩家设备上。
  // 所以：验收入口显式打开（URL `?rustlua-diagnostics=1` 或 `window.__RUSTLUA_DIAGNOSTICS`），
  // 发布包不做像素回读。`__RUSTLUA_DEBUG` 也认——那是既有的"我要看诊断"开关。
  const DIAGNOSTICS_ON = (function () {
    try {
      // 读 globalThis 而不是 window：微信宿主里 window 是适配层补出来的壳，而 harness 是
      // 在 Node 主 realm 里跑 game.js（那里的真全局是 globalThis）；浏览器里两者是同一个对象。
      const scopes = [];
      if (typeof globalThis !== 'undefined') scopes.push(globalThis);
      if (typeof window !== 'undefined' && window !== globalThis) scopes.push(window);
      for (let i = 0; i < scopes.length; i += 1) {
        if (scopes[i].__RUSTLUA_DIAGNOSTICS || scopes[i].__RUSTLUA_DEBUG) return true;
      }
      if (typeof location !== 'undefined' && typeof location.search === 'string'
        && location.search.indexOf('rustlua-diagnostics=1') >= 0) return true;
    } catch (error) { /* 宿主不提供全局/location：按关闭处理 */ }
    return false;
  })();

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

  // 预设文字自报：`{drawn, total}` 只统计**运行期 prefab 实例**里带非空 Text 的实体。
  // `data-rustlua-ui-elements` 数的是 Lua 每帧 `ui.*` 的动态行，屏幕 UI 实体一个都不计入，
  // 所以 prefab 文字整批不画时它读数不变（实测 2026-10-07 修复前后 drawn/ui-elements 逐位
  // 相同）——这道门当时是瞎的。这里单独报一对，无头门就能断言 total>0 且 drawn==total。
  let LAST_PREFAB_TEXT = { drawn: 0, total: 0 };
  // 预设九宫格自报：`{cells, entities}` = 走过 3×3 切片路径的 drawImage 次数 / 声明了
  // `nine_slice` 且边距非全零的场景 UI 贴图实体数（判据与原生 `mesh_ui.rs:729` 同一条）。
  // 门断的是"到底切没切"：整张拉伸时 cells 恒为 entities（每实体 1 次），切片时是它的 2~9 倍。
  let LAST_PREFAB_SLICE = { cells: 0, entities: 0 };

  // 9-slice 绘制（与引擎 nine_slice_cells 对齐）：corners 固定像素、edges 单向拉伸、
  // center 双向拉伸。slice = "l,r,t,b" 纹理像素；目标比边距和还窄时边距等比收缩。
  function drawNineSliceImage(ctx, tex, dx, dy, dw, dh, sliceStr) {
    const tw = tex.naturalWidth || tex.width;
    const th = tex.naturalHeight || tex.height;
    if (!(tw > 0) || !(th > 0)) return 0;
    const parts = String(sliceStr || '').split(',').map(Number);
    if (parts.length !== 4 || parts.some(v => !Number.isFinite(v) || v < 0)) {
      ctx.drawImage(tex, dx, dy, dw, dh);
      return 1;
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
    let made = 0;
    for (const c of cells) {
      if (c[6] <= 0 || c[7] <= 0 || c[2] <= 0 || c[3] <= 0) continue;
      ctx.drawImage(tex, c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]);
      made += 1;
    }
    // 返回实际发的 drawImage 次数：预设九宫格自报（LAST_PREFAB_SLICE）要拿它和"该切的实体数"
    // 对账——退化成整张拉伸时这里是 1，门就能看见。
    return made;
  }

  function drawUi(ctx, uiText, images, pending, sceneUiEntities, suppressSceneText) {
    const ui = parseUiLines(uiText);
    LAST_PREFAB_TEXT = { drawn: 0, total: 0 };
    LAST_PREFAB_SLICE = { cells: 0, entities: 0 };
    LAST_UI_SUMMARY = ui.panels.map(item => (item.shape === 1 ? 'C:' : 'P:') + item.id)
      .concat(ui.images.map(item => 'I:' + item.id + ':' + item.texture), ui.bars.map(item => 'B:' + item.id), ui.texts.map(item => 'X:' + item.id))
      .join(',');
    // 预设骨架趟：**先画**，垫在动态层之下（对齐原生 frame_data.rs 的 push_scene_ui_images
    // → push_ui_* 顺序）。顺序反了时不透明的卡底 `ui_skill_card_9s.png` 会整块埋掉压在它
    // 上面的 24×24 招式图标，两个宿主一起瞎（实机 2026-10-07，PC 侧同样复现）。
    for (const entity of sceneUiEntities || []) {
      drawSceneUiEntity(ctx, entity, images, pending, suppressSceneText, 'image');
    }
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
    // 场景底图趟已挪到本函数开头（骨架在下、动态层在上）。
    for (const text of ui.texts) {
      const size = text.size > 0 ? text.size : 20;
      const scale = text.scale > 0 ? text.scale : 1;
      ctx.font = (size * scale) + 'px ' + GAME_FONT;
      ctx.textBaseline = 'top';
      ctx.fillStyle = '#ffffff';
      ctx.fillText(text.text, text.x, text.y);
    }
    // 场景文字趟：原生的文字管线在场景图之上，故排在场景底图趟之后。
    // 分母只数**该画**的：被 `ui.scene_text_visible(name,false)` 逐名接管给脚本绘制的
    // （title_menu 的三份 Btn*Label，M22 重影修出来的口径）不是缺陷，不能算进"应有"。
    for (const entity of sceneUiEntities || []) {
      const counted = entity.prefabHandle !== undefined && entity.text && entity.text.value
        && !p2SceneTextHidden.has(String(entity.name || ''));
      if (counted) LAST_PREFAB_TEXT.total += 1;
      if (drawSceneUiEntity(ctx, entity, images, pending, suppressSceneText, 'text') && counted) {
        LAST_PREFAB_TEXT.drawn += 1;
      }
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

  // 场景静态 UI 实体（UiLayout + Image/Text）。九宫格走 3×3 切片，与原生
  // `Image.nine_slice` 的解析结果同一份来源。
  // `pass` 拆 'image' / 'text' 两趟，与原生 frame_data.rs 的提交序一一对齐：
  //   预设图（骨架，最下）→ 动态 panel/image/bar → 动态字 → 预设字（最上）。
  // 文字趟留在最后不是绘制偏好：原生 `prewarm_frame_glyphs` 必须是本帧最后一个会
  // 撑大字体图集的人（图集增长只重缩放已缓存字形的 UV，不回改已写进顶点缓冲的 UV），
  // 所以场景字挪不到动态文字之前；web 侧跟同一口径，避免两宿主分层不一致。
  // （2026-10-05 那次"合成一趟会让场景底图盖不住 hover"的前提已随 PC 侧绘制序修正消失。）
  function drawSceneUiEntity(ctx, entity, images, pending, suppressText, pass) {
    const rect = entity.uiRect;
    if (!rect || rect.w <= 0 || rect.h <= 0) return false;
    let drew = false;
    if (pass === 'image' && entity.image && entity.image.texture) {
      if (isWhiteTexture(entity.image.texture)) {
        // 纯色斑：原生把 Image.color 直接写进顶点（第四段就是 alpha），canvas 2D 里对应
        // fillStyle 的 RGBA。`colorCss` 只取前三段，所以第四段必须像这里一样显式交出去，
        // 否则 0.22 的半透明面板会垫成不透明、盖住底下的场景。
        ctx.fillStyle = colorCss(entity.image.color, entity.image.color[3]);
        ctx.fillRect(rect.x, rect.y, rect.w, rect.h);
        drew = true;
      } else {
        const texture = ensureTexture(images, pending, entity.image.texture);
        if (texture) {
          ctx.globalAlpha = entity.image.color[3] === undefined ? 1 : entity.image.color[3];
          // 九宫格：原生在 `mesh_ui.rs:727` 用 `Image.nine_slice` 把目标矩形拆成 3×3 格
          // （角部固定纹理像素、边单向拉伸、中心双向拉伸），`push_scene_ui` 也把这个字段
          // 带给了场景/预设实体（`mesh_ui.rs:515`）。web 侧此前解析完就没人读，整张贴图
          // 拉进目标矩形：96×96 的 `ui_panel_main_9s` 画进 720×640 时 16 px 的边框被吹成
          // 约 120 px，128×64 的招式卡压进方形卡底——面板"边框糊一大片、里面发黑"（实机 2026-10-07）。
          const slice = entity.image.nineSlice;
          if (slice && slice.length === 4 && slice.every(v => Number.isFinite(v) && v >= 0)
            && slice.some(v => v > 0)) {
            LAST_PREFAB_SLICE.entities += 1;
            LAST_PREFAB_SLICE.cells += drawNineSliceImage(ctx, texture, rect.x, rect.y, rect.w, rect.h, slice.join(','));
          } else {
            ctx.drawImage(texture, rect.x, rect.y, rect.w, rect.h);
          }
          ctx.globalAlpha = 1;
          drew = true;
        }
      }
    }
    // 场景自带的 Text 只在**启动场景**里画：原生的屏幕 UI 布局节点表 `ui_scene_nodes`
    // 在 `RuntimeScene::init` 里按启动场景建一次（engine/src/host/runtime_scene/scene.rs:480
    // 是唯一写入点，字段在 mod.rs:187，读取在 mesh_ui.rs:122），而 Lua 的 scene.load /
    // map.goto 重建整个 ECS world 后只紧跟一个相机同步系统（schedule.rs:189 与 :206），
    // 这张表不重建 ⇒ 切图后 HudTitle/HudHint 取不到矩形、被静默跳过。web 侧按同一口径处理，
    // 否则场景文字与 Lua HUD 的文字叠成两层（实机 2026-10-04：左上角"山庄 · 药田垂直切片"
    // 与"目标 …"重影）。
    // 但**只对场景文件声明的实体成立**：`prefab.spawn` 出来的实例带 `prefabHandle`，是 Lua 换
    // 场景后重新生成、每帧 `prefab.set_text` 填字的，快照失效与它无关。整趟挡掉会让 hud 的
    // HudLine1-6 和 inventory/shop/dialogue/pause/quest 六个面板只剩暗底（实机 2026-10-07：
    // 换场景后 `set_text` 返回 true 而画面上没有那行字，同实例的 Image 却照画）。
    if (pass === 'text' && entity.text && entity.text.value && (!suppressText || entity.prefabHandle !== undefined) && !p2SceneTextHidden.has(String(entity.name || ''))) {
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
  // 视图遮罩与"是否在本视图内"。引擎侧遮罩来自 RuntimeScene.view_world_mask
  // （默认 ALL_LAYERS，见 runtime_scene/scene.rs:659），判定见
  // rustlua-render/src/visibility.rs:117 的 entity_visible_in：
  // 有组件 ⇒ layers & view_mask != 0；没组件 ⇒ view_mask != 0。
  // 本侧**没有**设置遮罩的入口，所以恒为 ALL_LAYERS ⇒ 默认情形与引擎逐条一致；
  // 一旦接入遮罩输入，只需改这一个变量（过滤逻辑已经是引擎语义）。
  let viewWorldMask = ALL_LAYERS;
  // 主动相机（「camera.set_active」的 C 行写进来）：null = 脚本没设过，行为与以前完全一致。
  // 已知差异：本侧只让主动相机决定**变焦**；引擎还会用它的 Transform 当视口中心。
  // 改中心会动到既有工程（jhyt）的画面，故留到与 jhyt 重新验收时一起做（已记入计划）。
  let activeCameraId = null;
  // 剧本 update() 返回值的"高水位"（只记非空）：见下面写 data-rustlua-script-result 的地方。
  let lastScriptResult = '';
  // 键位读数的**基础串**（来源/工程动作数/未映射数）：启动期算一次、之后不变；
  // `data-rustlua-bindings` 每帧诊断时由它 + `__rustlua_binding_probe()` 拼出当帧的编辑族计数。
  let bindingReadoutBase = '';
  // 诊断 Lua 通道（rustlua-lua-do）的执行状态：none = 没给这条 URL，ok/error: … = 结果。
  let luaDoStatus = 'none';
  // camera2d：脚本请求的**落点状态**（渲染侧是唯一真源）。
  // 引擎侧 set/set_zoom/follow 是"请求、宿主下一帧消费"（api/camera.rs:240），get/get_zoom 读宿主
  // 回填的真实值 ⇒ 这里同样把请求落成状态、由 computeCamera 消费，再把结果写回 Lua 的
  // __rustlua_cam_x / __rustlua_cam_y / __rustlua_cam_zoom。三处（Lua get、渲染、screen↔world）
  // 因此永远读的是**同一份**数值。
  // 已知差异（诚实登记）：本侧只实现"跟随 + 中心 + 变焦 + 抖动"四种，不实现 dead_zone/smoothing
  // 的逐帧平滑（参数收下并记录，但不改变取景），因为默认取景本来就是硬跟随。
  const camera2dState = {
    center: null,      // 脚本显式 set(x,y) 的中心；优先于 follow 与默认
    followId: null,    // 脚本 follow(id) 的目标实体（JS 世界的实体 id）
    zoom: null,        // 脚本 set_zoom(z)
    offsetX: 0, offsetY: 0, deadX: 0, deadY: 0, smoothing: 0,
    shakeAmp: 0, shakeLeft: 0, shakeTotal: 0,
    scripted: false,   // 脚本是否接管过取景（诊断用：区分"默认取景"与"脚本取景"）
  };
  function entityVisibleInView(entity) {
    if (entity.layers === undefined) return viewWorldMask !== 0;
    return (entity.layers & viewWorldMask) !== 0;
  }

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
        // 主动相机优先；脚本没调过 set_active（null）时沿用原口径（最后一个带相机的实体）。
        if (activeCameraId === null || entity.id === activeCameraId) {
          camZoom = Number(entity.camera.zoom);
        }
      }
      if (!player && entity.transform) {
        const name = String(entity.name || '');
        if (name === 'JianghuPlayer' || name.toLowerCase().indexOf('player') !== -1) {
          player = entity;
        }
      }
    }
    if (!map) return null;
    // 脚本的 camera2d.set_zoom 优先于场景里的 Camera 组件：引擎里两者走同一条相机选择链
    // （api/camera.rs:28-35 的注释），这里让脚本请求排在组件之后、默认之前。
    const scriptZoom = Number(camera2dState.zoom);
    if (camera2dState.zoom !== null && scriptZoom > 0) camZoom = scriptZoom;
    const mapW = map.width * map.tile_size;
    const mapH = map.height * map.tile_size;
    // 半视口按 zoom 缩放（与原生 Camera2D::visible_extent = 视口/2/zoom 一致）：
    // zoom=2 时可见域 = 640×360 世界单位（20×11.25 格），屏幕 tile 64px。
    const halfW = viewportW / 2 / camZoom;
    const halfH = viewportH / 2 / camZoom;
    let cx = mapW / 2;
    let cy = mapH / 2;
    // 取景来源优先级：脚本 camera2d.set 的中心 > 脚本 camera2d.follow 的实体 > 默认口径
    // （名字像 player 的实体）。前两者都是**脚本意图**，所以脚本一接管就不再走默认启发式——
    // 否则 `follow(player)` 与"碰巧名字里有 player"两条路径会各自算出不同的取景。
    const followEntity = camera2dState.followId !== null
      ? sceneState.entities.get(camera2dState.followId)
      : null;
    const scriptTarget = camera2dState.center
      || (followEntity && followEntity.alive !== false && followEntity.transform ? followEntity.transform : null);
    if (scriptTarget) {
      cx = Number(scriptTarget.x) || 0;
      cy = Number(scriptTarget.y) || 0;
      if (followEntity && followEntity.transform) {
        cx += Number(camera2dState.offsetX) || 0;
        cy += Number(camera2dState.offsetY) || 0;
      }
    } else if (player && player.transform) {
      cx = player.transform.x;
      cy = player.transform.y;
    }
    const clampAxis = (value, min, max) => (min > max ? (min + max) / 2 : Math.max(min, Math.min(max, value)));
    cx = clampAxis(cx, halfW, mapW - halfW);
    cy = clampAxis(cy, halfH, mapH - halfH);
    // camera2d.shake：幅度按剩余时间线性衰减、相位用剩余时间推进（引擎的 shake 同样是衰减包络）。
    // 抖动加在**夹取之后**：抖动越界是允许的（相机被"晃出"地图边缘正是震屏该有的样子）。
    if (camera2dState.shakeLeft > 0 && camera2dState.shakeAmp > 0) {
      const envelope = camera2dState.shakeTotal > 0 ? camera2dState.shakeLeft / camera2dState.shakeTotal : 1;
      const phase = camera2dState.shakeLeft * 60;
      cx += Math.sin(phase) * camera2dState.shakeAmp * envelope;
      cy += Math.cos(phase * 1.3) * camera2dState.shakeAmp * envelope;
    }
    return { x: cx, y: cy, zoom: camZoom };
  }

  // 本帧的相机（渲染与脚本共用同一份）。
  // `frameCameraFresh` 是**本帧新鲜**闸门：只有 refreshFrameCamera 刚算过的那一份才允许被
  // drawScene 复用。别的绘制路径（不走剧本的直绘帧）看到的是 false ⇒ 照旧现算，
  // 行为与改动前逐字一致——复用绝不能把"上一帧的相机"画出来。
  let frameCamera = null;
  let frameCameraFresh = false;
  // 每帧在**跑剧本之前**算一次相机，然后：
  //   * 留给 drawScene 复用（同一帧不再算第二遍，也不会有"脚本读到的"与"画出来的"两份）；
  //   * 用 lua_set_number 回填给 Lua（三个数，不编译源码、不传 JSON——热路径上每帧都要写）。
  // 引擎注释（api/camera.rs:240）明说 `get`/`get_zoom` 读的是**宿主每帧回填的真实值**，
  // 所以回填必须发生在 update **之前**：脚本在 update 里问的相机才是这一帧真正用的那份。
  function refreshFrameCamera(sceneState) {
    const cam = computeCamera(sceneState, 1280, 720) || { x: 0, y: 0, zoom: 1 };
    frameCamera = cam;
    frameCameraFresh = true;
    setLuaNumbers([['__rustlua_cam_x', cam.x], ['__rustlua_cam_y', cam.y], ['__rustlua_cam_zoom', cam.zoom]]);
    return cam;
  }
  // drawScene 取相机：本帧新鲜就复用（不重复算第二遍），否则现算（与改动前一致）。
  function cameraForDraw(sceneState) {
    if (frameCameraFresh && frameCamera) return frameCamera;
    return computeCamera(sceneState, 1280, 720);
  }
  // 解除跟随时把取景"钉"在当前这一帧的相机位置上（引擎的 unfollow 就是停住不动）。
  // 用本帧的相机当落点：它已经过地图夹取，所以钉住之后不会又把画面拉出边界。
  function freezeCameraHere() {
    if (frameCamera) camera2dState.center = { x: frameCamera.x, y: frameCamera.y };
  }

  // 场景文字实体隐藏集：title_menu 把按钮标签接管为脚本绘制（ui.scene_text_visible(false)），
  // 场景里的 Text 实体（BtnStartLabel 等）不再画，避免「场景层 + 脚本层」两层文字重叠。
  let p2SceneTextHidden = new Set();
  // WP7-59：这一帧的隐藏文本集是不是**跟着状态载荷**（`H` 行）一起来的。
  // 兼容层如果自己覆盖了 `__rustlua_export_state`，就不会有 `H` 行——那时必须回退到
  // 单独拉一次（老路），而 `sceneTextHiddenPulls` 会把这件事如实记下来：
  // "谁还在走老路"要能一眼看出来，而不是靠猜。
  let sceneTextHiddenFromState = false;
  let sceneTextHiddenPulls = 0;
  // WP7-60：状态载荷的基线读数（见 syncLuaScene 里的说明）。
  let stateBytes = 0;
  let stateTransformLines = 0;
  // WP7-61：状态回写的**调用计数**——把"每帧到底导出几次"从"靠人记"变成"可断言的读数"。
  // 曾经 3 个地方各自直调 `__rustlua_export_state`、5 个地方各自直调 `__rustlua_export_ui`，
  // 于是"结果有没有被消费""隐藏文本集从哪条路来""一帧会不会导出两次"全都没有单一出口。
  let stateExports = 0;        // 总次数（快照 + 帧循环）
  let stateFrameExports = 0;   // 帧循环那次（每帧至多一次 ⇒ 必须 ≤ 帧数）
  let stateSnapshotExports = 0; // 启动/换场景的只读快照（传 false，不推进积分）
  let uiBytes = 0;             // UI 载荷的累计字节数（同一帧里唯一没量过的一块）
  // WP7-62：UI 载荷的形状（按行首类型：P 面板 / I 图片 / B 进度条 / X 文本）。
  let uiPanelBytes = 0; let uiPanelRows = 0;
  let uiImageBytes = 0; let uiImageRows = 0;
  let uiBarBytes = 0; let uiBarRows = 0;
  let uiTextBytes = 0; let uiTextRows = 0;
  let uiOtherBytes = 0; let uiOtherRows = 0;
  // WP7-62：UI 载荷的冗余度（与上一帧逐字节相同的帧数）——决定"变更推送值不值得做"的那个读数。
  let uiSameFrames = 0;
  let uiPayloadSeen = false;
  let uiPayloadLast = '';
  // WP7-64：两个回写导出的**实测开销**（只在诊断开启时累计）。前面几轮量的是规模（字节/行数），
  // 从规模推开销只能估；这里直接测时间。`exportNowMs` 优先 `performance.now()`（亚毫秒分辨率），
  // 退化到 `Date.now()`；`recordExportCost` 只做加法/最大值/慢帧计数，不分配对象。
  let stateExportMs = 0; let stateExportMsMax = 0;
  let uiExportMs = 0; let uiExportMsMax = 0;
  // WP7-66：消费端（解析 + 绘制）单独计时，以及**慢帧分布**——第 96 轮只有一个"单帧最大"，
  // 不知道多常发生。阈值取一帧预算的八分之一。
  let drawMs = 0; let drawMsMax = 0;
  // WP7-74：**跨语言调用计数**——只数真实发生的桥调用（诊断路径不算），把"每帧跨界几次"
  // 从文字台账变成读数。生产帧路径应当是**常数**：call_global×2（export_state/export_ui）
  // + fixed_frame + input_frame + set_number(dt)。每帧均摊 = 本计数 / frame，可直接与
  // "宿主接管后"的同名读数上下对比（见第 105 轮的设备验证限制）。
  let xcCallGlobal = 0; let xcFixed = 0; let xcInput = 0; let xcSetNum = 0;
  // WP7-76：按**名字**拆开这两条通道（只统计谁在每帧重复调用）。只在诊断开启时记录，
  // 生产路径不额外付钱；读数里给一张按次数降序的短表。
  const xcNames = new Map(); let xcNameGate = false;
  const xsnNames = new Map();
  function xcNameText(map) {
    if (!map || map.size === 0) return "-";
    const list = [];
    map.forEach(function (v, k) { list.push(k + ":" + v); });
    list.sort(function (a, b) { return (parseInt(b.split(":").pop(), 10) || 0) - (parseInt(a.split(":").pop(), 10) || 0); });
    return list.join("|");
  }
  // WP7-80：**门面漏名自证**。桥是宿主适配层的显式白名单（`wechat-adapter.js` 与 `index.html` 各一份），
  // 漏一项的表现是"真机启动即失败、报错只有一句 is not a function"。这里在 boot 后第一次拼读数时
  // 把"运行时实际看得见哪些桥导出"记下来 ⇒ 漏名会在读数里直接现形，而不必等到真机。
  let bridgeProbeText = null;
  function bridgeProbe() {
    if (bridgeProbeText !== null) return bridgeProbeText;
    const names = ["lua_set_number", "lua_set_numbers", "lua_input_frame", "lua_fixed_frame",
      "lua_fixed_run_steps", "lua_call_global", "input_frame_layout_json"];
    const br = (typeof window !== "undefined" && window.__RUSTLUA_LUA_BRIDGE) || null;
    const bad = [];
    for (let i = 0; i < names.length; i += 1) {
      if (!br || typeof br[names[i]] === "undefined") bad.push(names[i]);
    }
    bridgeProbeText = bad.length === 0 ? "ok" : ("missing:" + bad.join(","));
    return bridgeProbeText;
  }
  // WP7-69：把 drawScene 拆成 tilemap / 实体收集+排序 两段（其余 = 精灵+文本+覆盖层，用 drawUs 减出来）。
  // 瓦片记"考察/实画"两个计数：考察 = 进到格子循环，实画 = tile ~= 0 真的画了。
  let tileMs = 0; let collectMs = 0;
  let tileCellsSeen = 0; let tileCellsDrawn = 0; let tileCellsCulled = 0;
  const EXPORT_SLOW_MS = 2;
  let stateSlowFrames = 0; let uiSlowFrames = 0; let drawSlowFrames = 0;
  function exportNowMs() {
    return (typeof performance !== 'undefined' && performance && typeof performance.now === 'function')
      ? performance.now() : Date.now();
  }
  function recordExportCost(which, ms) {
    if (!(ms >= 0)) return;
    if (which === 'state') {
      stateExportMs += ms;
      if (ms > stateExportMsMax) stateExportMsMax = ms;
      if (ms > EXPORT_SLOW_MS) stateSlowFrames += 1;
    } else if (which === 'ui') {
      uiExportMs += ms;
      if (ms > uiExportMsMax) uiExportMsMax = ms;
      if (ms > EXPORT_SLOW_MS) uiSlowFrames += 1;
    } else {
      drawMs += ms;
      if (ms > drawMsMax) drawMsMax = ms;
      if (ms > EXPORT_SLOW_MS) drawSlowFrames += 1;
    }
  }
  function refreshSceneTextHidden(L) {
    sceneTextHiddenPulls += 1;
    p2SceneTextHidden = new Set();
    try {
      const raw = String(callGlobal(L, '__rustlua_export_scene_text_hidden') || '');
      if (raw) raw.split(',').forEach(name => { if (name) p2SceneTextHidden.add(name); });
    } catch (e) { /* 兼容层缺失时保持空集 */ }
  }

  // WP7-61：世界状态回写的**唯一所有者**。
  // 只有这一个入口能取走 Lua 侧的 op 队列（`__rustlua_export_state` 一进来就把队列清空），
  // 所以"谁取走了什么"不再取决于调用点的先后顺序——快照与帧循环走同一条路，只是
  // `advance=false` 表示"只看一眼、不推进积分与动画"（首帧/换场景不该白走一次模拟）。
  // 它同时负责隐藏文本集的来源判定（H 行优先、覆盖不了才回退老路）。
  function exportWorldState(L, state, advance) {
    stateExports += 1;
    if (advance) stateFrameExports += 1; else stateSnapshotExports += 1;
    sceneTextHiddenFromState = false;
    // `advance` 为真时**不带参数**调用（与改动前的帧循环逐字一致）；为假时传 `[false]`，
    // 这也是改动前两条快照路径的写法——归一的是调用点，不是语义。
    // WP7-64：实测这一次导出花多少时间（只在诊断开启时）。
    const stateT0 = DIAGNOSTICS_ON ? exportNowMs() : 0;
    const payload = advance
      ? callGlobal(L, '__rustlua_export_state')
      : callGlobal(L, '__rustlua_export_state', [false]);
    if (DIAGNOSTICS_ON) recordExportCost('state', exportNowMs() - stateT0);
    syncLuaScene(state, payload);
    if (!sceneTextHiddenFromState) refreshSceneTextHidden(L);
  }

  // WP7-61：UI 回写的唯一所有者（顺带量出 UI 载荷的字节数——基线一直缺这一块）。
  // 注：这里必须是 `callGlobal` 直调。改动时用全局字符串替换收敛调用点，**定义体自己也被那条
  // 模式命中过**，于是这里一度变成 `const payload = exportUiPayload(L);`——语法检查全绿、
  // 运行时无限递归。教训：全局替换要先框住范围，或者替换完逐处核对定义体。
  function exportUiPayload(L) {
    const uiT0 = DIAGNOSTICS_ON ? exportNowMs() : 0;
    const payload = callGlobal(L, '__rustlua_export_ui');
    const text = String(payload || '');
    uiBytes += text.length;
    // WP7-62：把 UI 载荷**按行首类型**拆开记字节/行数，第一次看清它的形状。
    // 只扫一遍字符串、**不建任何模型**——零语义，不可能重演第 91 轮"影子状态分家"那类事故。
    // 行首：P=80 面板 / I=73 图片 / B=66 进度条 / X=88 文本（与 Lua 侧四个循环一一对应）。
    if (text) {
      let start = 0;
      while (start < text.length) {
        let end = text.indexOf('\n', start);
        if (end < 0) end = text.length;
        const kind = text.charCodeAt(start);
        const len = end - start + 1;
        if (kind === 80) { uiPanelBytes += len; uiPanelRows += 1; }
        else if (kind === 73) { uiImageBytes += len; uiImageRows += 1; }
        else if (kind === 66) { uiBarBytes += len; uiBarRows += 1; }
        else if (kind === 88) { uiTextBytes += len; uiTextRows += 1; }
        else { uiOtherBytes += len; uiOtherRows += 1; }
        start = end + 1;
      }
    }
    // WP7-62：冗余度——这一帧与上一帧**逐字节相同**就记一次（只比较、不缓存模型 ⇒ 零语义）。
    if (uiPayloadSeen && text === uiPayloadLast) uiSameFrames += 1;
    uiPayloadSeen = true;
    uiPayloadLast = text;
    if (DIAGNOSTICS_ON) recordExportCost('ui', exportNowMs() - uiT0);
    return payload;
  }

  // 层名 → 秩，与原生 `sorting_layer_rank`（rustlua-render/src/sprite.rs:105）逐项一致；
  // 未知层名回退 default=0（原生同样回退）。
  const SORT_LAYER_RANK = { background: -2, world: -1, default: 0, actor: 1, foreground: 2, ui: 3 };
  function entitySortKey(entity) {
    const sprite = entity.sprite;
    let rank = 0;
    if (sprite && sprite.layer) {
      const mapped = SORT_LAYER_RANK[String(sprite.layer).trim().toLowerCase()];
      if (mapped !== undefined) rank = mapped;
    }
    const order = sprite ? Math.max(-999, Math.min(999, sprite.order || 0)) : 0;
    return rank * 1000 + order;
  }

  // 碰撞描边自检：把玩家看不见的那一层（实心格 + 实体 Collider）显形，用来直接对图判断
  // "阻挡到底压不压在建筑上"。坐标复用 drawScene 传入的 toCanvas/camZoom，与实体绘制
  // 同一口径 ⇒ 两边一旦错位，错位来自数据，不是这里的换算。
  function drawColliderOverlay(ctx, sceneState, toCanvas, camZoom) {
    let cells = 0;
    let bodies = 0;
    ctx.save();
    ctx.lineWidth = 1;
    for (const entity of sceneState.entities.values()) {
      const map = entity.alive ? entity.tilemap : null;
      if (!map) continue;
      const origin = entity.transform || { x: 0, y: 0 };
      const ts = map.tile_size || 32;
      const side = ts * camZoom;
      ctx.fillStyle = 'rgba(239,68,68,0.30)';
      ctx.strokeStyle = 'rgba(248,113,113,0.85)';
      for (let y = 0; y < map.height; y += 1) {
        for (let x = 0; x < map.width; x += 1) {
          if (map.solid[y * map.width + x] !== '1') continue;
          // 与瓦片绘制同一个锚点约定：单元格上沿 = 世界 (y+1)*ts（画布 y 向下）。
          const cell = toCanvas(origin.x + x * ts, origin.y + (y + 1) * ts);
          ctx.fillRect(cell.x, cell.y, side, side);
          ctx.strokeRect(cell.x, cell.y, side, side);
          cells += 1;
        }
      }
      // 地图矩形外圈：Lua 侧 `__rustlua_cell_solid` 把越界也当实心，所以图外那圈空气墙
      // 必须画出来，否则表现为"边界凭空挡人"。
      const left = toCanvas(origin.x, origin.y + map.height * ts);
      const right = toCanvas(origin.x + map.width * ts, origin.y);
      ctx.strokeStyle = 'rgba(250,204,21,0.9)';
      ctx.strokeRect(left.x, left.y, right.x - left.x, right.y - left.y);
    }

    ctx.font = '11px sans-serif';
    for (const entity of sceneState.entities.values()) {
      const sprite = entity.alive && !entity.uiRect ? entity.sprite : null;
      if (!sprite) continue;
      const pos = entity.transform || { x: 0, y: 0 };
      const sizeY = sprite.size_y ? sprite.size_y : sprite.size;
      const pivotX = sprite.pivot_x !== undefined ? sprite.pivot_x : 0.5;
      const pivotY = sprite.pivot_y !== undefined ? sprite.pivot_y : 0.5;
      const p = toCanvas(pos.x, pos.y);
      ctx.strokeStyle = 'rgba(74,222,128,0.8)';
      ctx.strokeRect(p.x - pivotX * sprite.size * camZoom,
        p.y - (1 - pivotY) * sizeY * camZoom,
        sprite.size * camZoom, sizeY * camZoom);
    }

    for (const entity of sceneState.entities.values()) {
      const col = entity.alive ? entity.collider : null;
      if (!col) continue;
      bodies += 1;
      const pos = entity.transform || { x: 0, y: 0 };
      const c = toCanvas(pos.x + (col.ox || 0), pos.y + (col.oy || 0));
      const hx = (col.hx || 0) * camZoom;
      const hy = (col.hy || 0) * camZoom;
      const sensor = col.sensor === true;
      ctx.strokeStyle = sensor ? 'rgba(168,85,247,0.95)' : 'rgba(34,211,238,0.95)';
      ctx.fillStyle = sensor ? 'rgba(168,85,247,0.18)' : 'rgba(34,211,238,0.18)';
      // 方框 = web 真正会挡的形状（`__rustlua_blocked_at` 只用 hx/hy 做 AABB），
      // 圆 = 原生侧 ball 的形状；两者只在四角差出半格，分开画才看得出差别。
      ctx.fillRect(c.x - hx, c.y - hy, hx * 2, hy * 2);
      ctx.strokeRect(c.x - hx, c.y - hy, hx * 2, hy * 2);
      if (col.shape === 'ball') {
        ctx.beginPath();
        ctx.arc(c.x, c.y, hx, 0, Math.PI * 2);
        ctx.stroke();
      }
      ctx.fillStyle = 'rgba(34,211,238,0.95)';
      ctx.fillText(String(entity.name || ('#' + entity.id)), c.x + hx + 3, c.y - hy - 3);
    }

    // 图例贴在**左上引导文字的下一行**、与它们同一左边缘（x=16、textBaseline=top）：
    // 目标行占 layout.ROW.goals_y=8（17px）、动作反馈行占 feedback_y=36（18px），
    // 行距 28 → 图例取 64，是这两行之下第一条空行。原先画在顶部居中（y=12）与目标行
    // 完全交错，两边都读不出整句。
    ctx.fillStyle = '#d0d7e2';
    ctx.font = '12px sans-serif';
    ctx.textAlign = 'left';
    ctx.textBaseline = 'top';
    ctx.fillText('碰撞描边 实心格=' + cells + ' 阻挡体=' + bodies +
      ' ｜ 红=实心格 黄=地图边界(图外也挡) 青=Collider(ball 另画原生圆) 紫=sensor 绿=Sprite 占位',
      16, 64);
    ctx.restore();
  }

  // 渲染侧本帧实际遍历到的 Tilemap 层数（与 Lua 侧 `__rustlua_tilemap_layers` 配对自报，
  // 见世界摘要里的 `tilemaps=画/声明`）。
  let tileLayersDrawn = 0;

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

    // 复用本帧在跑剧本**之前**算好的那份（refreshFrameCamera）；没有本帧新鲜值时（例如不走剧本的
    // 直绘路径）现算 —— 两条路径都得到同一口径：脚本读到的相机 = 画出来的相机。
    const camera = cameraForDraw(sceneState);
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

    // 原生 `append_tilemap_vertices`（frame_data.rs 的 "Tile maps draw first (layer 0)"）遍历
    // 的是**每一个**挂 Tilemap 的实体，山庄就有两张：GroundMap(1) 夯土打底、FarmGrid(2) 农田
    // overlay。原先只 `.find()` 第一张 ⇒ 翻土/浇水/出苗在 web 上整层不可见（PC 有，实测
    // 2026-10-04）。层序按 entity id 升序 = 场景文件里的出现序，与原生那条 ECS 迭代序在
    // "后声明的层 id 更大"时同序（山庄/云泽村都是这个写法）。
    const tilemapEntities = Array.from(sceneState.entities.values())
      .filter(entity => entity.alive && entity.tilemap && entityVisibleInView(entity))
      .sort((a, b) => a.id - b.id);
    tileLayersDrawn = tilemapEntities.length;
    const tileT0 = DIAGNOSTICS_ON ? exportNowMs() : 0;
    for (const tilemapEntity of tilemapEntities) {
      const map = tilemapEntity.tilemap;
      const origin = tilemapEntity.transform || { x: 0, y: 0 };
      // 地表贴图可能被 Lua 的 tilemap.set_texture 换掉（换季/换图），不能只靠
      // 场景载入时预加载的那一张，走按需加载（首帧未解出时先退化成纯色格）。
      const tileset = ensureTexture(images, pending, map.texture);
      // 图集按**声明的 cols×rows** 等分，不再从贴图宽度反推列数（M72-01 的原生侧同条
      // 规则）：反推出来的列数与场景声明不一致时，画面只是"换个瓦片"，没人会发现数据
      // 错了。解算后的条目（`resolvedEntries`）才是真正要采样的格子。
      const cols = Math.max(1, map.atlas_cols);
      const rows = Math.max(1, map.atlas_rows);
      const entries = resolvedEntries(map);
      const cellWidth = tileset ? tileset.width / cols : map.tile_size;
      const cellHeight = tileset ? tileset.height / rows : map.tile_size;
      // WP7-72：**复用一个坐标对象**。原来每格调一次 `toCanvas`（每次返回新对象），裁剪后仍有
      // 287 次/帧；真机 60 fps 下约 1.7 万次/秒的纯 GC 压力。这里把同一条公式就地算、写进同一个
      // 对象——**裁剪与绘制的调用点一行都不用改**，等价性由"同一公式、同一对象、同一次迭代内
      // 用完即弃"保证（本循环里没有任何人保留这个对象）。
      const cell = { x: 0, y: 0 };
      const camOn = !!camera;
      const camX = camera ? camera.x : 0;
      const camY = camera ? camera.y : 0;
      for (let y = 0; y < map.height; y += 1) {
        for (let x = 0; x < map.width; x += 1) {
          const i = y * map.width + x;
          const tile = entries[i];
          // 瓦片值 0 = **不画**（原生 `build_chunk`：tilemap.rs:40 的 `if cell == 0 { continue }`），
          // 不是画底色。FarmGrid 初值 920 格全 0，靠的正是"空格透出下层夯土"；这里一旦补底色，
          // overlay 会把打底那张整个糊掉。纯色格只留给"瓦片有值但贴图还没解出"的首帧兜底。
          if (DIAGNOSTICS_ON) tileCellsSeen += 1;
          if (tile === 0) continue;
          // 单元格世界矩形 [y*ts, (y+1)*ts]；画布以单元格**上沿**为锚点直绘，无需翻转贴图。
          const cwx = origin.x + x * map.tile_size;
          const cwy = origin.y + (y + 1) * map.tile_size;
          // 与 `toCanvas` 逐字等价：无相机时原样直绘；有相机时以 640/360 为中心、y 反向
          // （camZoom 在无相机时正好是 1，与 `toCanvas` 的 `camera ? camera.zoom : 1` 同源）。
          cell.x = camOn ? 640 + (cwx - camX) * camZoom : cwx;
          cell.y = camOn ? 360 - (cwy - camY) * camZoom : cwy;
          const tileDrawSize = map.tile_size * camZoom;
          // WP7-71：**视野裁剪**。第 101 轮量到 tilemap 占绘制端约 68%、每帧实画 882 格；
          // 画布此刻已在开头那次 contain 的 scale 变换**内**（见本函数注释），坐标系 = 1280×720
          // 设计空间 ⇒ 可见矩形就是 [0,1280]×[0,720]。留**一格余量**：越界不足一格的格子照画，
          // 于是可见像素逐点不变，也不给"缩放/取整差一点"留边界隐患。
          if (cell.x + tileDrawSize < -tileDrawSize || cell.y + tileDrawSize < -tileDrawSize
            || cell.x > 1280 + tileDrawSize || cell.y > 720 + tileDrawSize) {
            if (DIAGNOSTICS_ON) tileCellsCulled += 1;
            continue;
          }
          if (DIAGNOSTICS_ON) tileCellsDrawn += 1;
          if (tileset) {
            const tileIndex = tile - 1;
            const sx = (tileIndex % cols) * cellWidth;
            const sy = Math.floor(tileIndex / cols) * cellHeight;
            const tint = tintAt(map, i);
            if (tint) {
              drawTintedTile(ctx, tileset, sx, sy, cellWidth, cellHeight, tint, cell.x, cell.y, map.tile_size * camZoom, map.tile_size * camZoom);
            } else {
              ctx.drawImage(tileset, sx, sy, cellWidth, cellHeight, cell.x, cell.y, map.tile_size * camZoom, map.tile_size * camZoom);
            }
          } else {
            const solid = map.solid[i] === '1';
            ctx.fillStyle = solid ? '#334155' : '#1f2937';
            ctx.fillRect(cell.x, cell.y, map.tile_size * camZoom - 1, map.tile_size * camZoom - 1);
          }
        }
      }
    }

    if (DIAGNOSTICS_ON) tileMs += exportNowMs() - tileT0;
    // 两趟绘制：世界实体按原生的 (层秩, order) 排，屏幕空间 UI 单独一趟画在最后。
    // 原先是"全部实体按 id 一趟"，于是门楼（world, order=5）被角色（actor）压在下面、
    // 菜单底图（id 小）会被后画的世界贴图盖住——原生侧它们分属不同的 sorting layer。
    const collectT0 = DIAGNOSTICS_ON ? exportNowMs() : 0;
    const worldEntities = [];
    const screenUiEntities = [];
    for (const entity of sceneState.entities.values()) {
      if (!entity.alive || entity.tilemap) continue;
      // 世界实体的可见层过滤：不在视图里的实体**不进入绘制趟**（不是画成透明）。
      // 屏幕空间 UI 走 UI 系统那条路，这里不动它。
      if (!entityVisibleInView(entity)) continue;
      // 预设子树显隐（`prefab.set_visible` 子树语义）：隐藏的节点不进入绘制趟，
      // 与原生侧「被隐藏的整棵子树一个布局项都不建」同一口径。
      if (entity.prefabHidden) continue;
      if (entity.uiRect) screenUiEntities.push(entity);
      else worldEntities.push(entity);
    }
    worldEntities.sort((a, b) => (entitySortKey(a) - entitySortKey(b)) || (a.id - b.id));
    screenUiEntities.sort((a, b) => a.id - b.id);
    if (DIAGNOSTICS_ON) collectMs += exportNowMs() - collectT0;
    for (const entity of worldEntities) {
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
      // Sprite.tint（原生片元 = 贴图色 × tint，alpha 也相乘）：web 侧此前对带贴图的精灵
      // 直接 drawImage 原图，于是「白 blob 贴图 + 半透深色 tint」的投影贴片在 web/微信画成
      // **不透明白块**（实机 2026-10-12）。这里复用瓦片的两趟染色离屏画布（按 图+tint 缓存，
      // 首帧之后只取缓存）；默认 tint 走原直绘快路，不多花一次离屏合成。
      const tint = entity.sprite && entity.sprite.tint ? entity.sprite.tint : null;
      const needsTint = !!tint && (tint[0] !== 1 || tint[1] !== 1 || tint[2] !== 1 || tint[3] !== 1);
      if (sprite && clip) {
        // frame 是**帧序号**（Lua 侧按引擎 current_frame 算好回写），不是累计 tick：
        // 引擎的 current_frame_uv 只取条带首行（frame/cols, 0），所以 sy 恒为 0。
        const index = Math.max(0, Math.floor(Number(entity.animation.frame) || 0));
        const sx = (index % Math.max(1, clip.cols)) * size;
        if (needsTint) drawTintedTile(ctx, sprite, sx, 0, size, size, tint, dx, dy, size * camZoom, sizeY * camZoom);
        else ctx.drawImage(sprite, sx, 0, size, size, dx, dy, size * camZoom, sizeY * camZoom);
      } else if (sprite) {
        if (needsTint) drawTintedTile(ctx, sprite, 0, 0, sprite.width, sprite.height, tint, dx, dy, size * camZoom, sizeY * camZoom);
        else ctx.drawImage(sprite, dx, dy, size * camZoom, sizeY * camZoom);
      } else if (!clip && entity.sprite && isWhiteTexture(entity.sprite.texture)) {
        // 白图 × tint = 纯色方块。原生画的是 tint 乘上 1×1 白纹理的四边形，所以这里
        // 必须是**方**、且用 Sprite.tint；下面那条圆点是"根本没有贴图字段"的兜底，
        // 把白图归到兜底就等于把 NPC 的性别/职业配色全丢掉（且颜色按实体名猜）。
        ctx.fillStyle = colorCss(entity.sprite.tint, entity.sprite.tint[3]);
        ctx.fillRect(dx, dy, size * camZoom, sizeY * camZoom);
      } else {
        ctx.fillStyle = entityColor(entity);
        ctx.beginPath();
        ctx.arc(p.x, p.y, Math.max(6, size / 2) * camZoom, 0, Math.PI * 2);
        ctx.fill();
      }
    }

    // 世界空间文字：锚在实体的**世界位置**上（+ offset），随相机移动——与屏幕空间的
    // UI Text 是两条路。detach 之后组件被删掉，这里自然就不画了。
    for (const entity of sceneState.entities.values()) {
      if (!entity.alive || !entity.worldText || !entity.worldText.value) continue;
      if (!entityVisibleInView(entity)) continue;
      const pos = entity.transform || { x: 0, y: 0 };
      const p = toCanvas(pos.x + entity.worldText.offsetX, pos.y + entity.worldText.offsetY);
      ctx.font = Math.max(1, entity.worldText.size) * camZoom + 'px ' + GAME_FONT;
      ctx.fillStyle = colorCss(entity.worldText.color);
      ctx.textAlign = entity.worldText.centered ? 'center' : 'left';
      ctx.textBaseline = 'middle';
      ctx.fillText(entity.worldText.value, p.x, p.y);
    }
    ctx.textAlign = 'left';
    ctx.textBaseline = 'alphabetic';

    // 屏幕空间 UI（菜单底图/按钮/标签）不再在这里画：它由 drawUi 的两趟按原生提交序
    // 承担（预设图在动态层之下、预设字在动态字之上，见 drawSceneUiEntity 的 pass 说明）。

    if (typeof window !== 'undefined' && window.__RUSTLUA_DEBUG) {
      ctx.fillStyle = '#d0d7e2';
      ctx.font = '14px sans-serif';
      ctx.fillText('Project: ' + projectName + ' | Scene: ' + sceneState.name + ' | Lua update running', 12, 704);
    }
    // 逐帧读全局，所以控制台里 `window.__RUSTLUA_DEBUG_COLLIDERS = true` 立刻生效、
    // 不用刷新（微信侧没有 location，只能走这条）。
    if (typeof window !== 'undefined' && window.__RUSTLUA_DEBUG_COLLIDERS) {
      drawColliderOverlay(ctx, sceneState, toCanvas, camZoom);
    }
    // 动态 UI（面板/按钮/文字）也必须在 contain 变换**内**绘制：它与场景共用 1280×720
    // 设计坐标系。若在 restore 之后画，微信窗口比例≠16:9 时动态 UI 按原始设计坐标铺在
    // 画布左上（未缩放居中），与场景错位分离（1785×832 实机截图）。
    const uiCount = drawUi(ctx, uiText, images, pending, screenUiEntities, sceneState.sceneTextSuppressed === true);
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
  // 问引擎要"这个名字对应的引擎句柄"（Lua 编码 `index << 32 | generation`）。返回 0/undefined 表示
  // 引擎不认识它——多半是"JS 宿主自己切了场景、引擎还停在上一张"（两个世界不同步，见 WP7-30）。
  // 引擎侧没有这个出口时**只告警一次**并放弃翻译：宁可让那条 API 报"无效句柄"，也不要静默用一个
  // 编出来的数字。
  let engineHandleWarned = false;
  function engineEntityHandle(name) {
    if (!name) {
      return 0;
    }
    const bridge = (typeof window !== 'undefined' && window.__RUSTLUA_LUA_BRIDGE) || null;
    if (!bridge || typeof bridge.lua_entity_handle !== 'function') {
      if (!engineHandleWarned) {
        engineHandleWarned = true;
        if (typeof console !== 'undefined' && console.warn) {
          console.warn('web-runtime: wasm 侧没有 lua_entity_handle 导出（产物与运行时版本不一致），'
            + '实体 id 无法翻译成引擎句柄；引擎侧 API（quad/mesh/light/world_text）会拿到无效句柄');
        }
      }
      return 0;
    }
    const handle = bridge.lua_entity_handle(name);
    return typeof handle === 'number' && handle > 0 ? handle : 0;
  }
  function sceneBindings(sceneState) {
    const names = {};
    const transforms = {};
    const tilemaps = {};
    const animations = {};
    const colliders = {};
    // 一等矩形的组件串镜像（quad.get/set 的读侧；Q 行是写侧）。
    const quads = {};
    // 世界文字的组件串镜像（world_text.detach 的读侧；W 行是写侧）。
    const worldTexts = {};
    // 可见层镜像（visibility.get 的读侧；V 行是写侧）。
    const layers = {};
    // 2D 相机镜像：camera.set_active 要用它判断"这个实体到底有没有相机"
    // （引擎那句 "entity has no camera" 正是这个意思）。
    const cameras = {};
    // **实体 id 的两个空间**（第 60 轮实测结论，详见计划 WP7-30）：
    // 这套 JS 世界用 `.scene` 文件里的**作者 id** 当实体 id（`entity\t13\tScaleBar` ⇒ 13），
    // 而引擎把 Lua 侧的 i64 按 `index << 32 | generation` 解包 ⇒ 作者 id 13 变成
    // `EntityId(0:g13)` ⇒ `invalid entity id`。本文件覆盖了 `entity`/`sprite`/`transform`/
    // `tilemap`/`animation`，所以这些调用走不到引擎；**没有**覆盖的 `quad`/`mesh`/`light`/
    // `world_text` 会落到引擎上，于是两个 id 空间在这里撞车（第 58/59 轮那条 `:215` 的报错）。
    // 处理办法：本世界的表仍按作者 id 索引（它自己的 API 要用），另外给出一张
    // **作者 id → 引擎句柄**的翻译表，供那几个引擎侧 API 在下发前换算。
    const engineIds = {};
    for (const entity of sceneState.entities.values()) {
      names[entity.name] = entity.id;
      const handle = engineEntityHandle(entity.name);
      if (handle) {
        engineIds[entity.id] = handle;
      }
      if (entity.transform) transforms[entity.id] = entity.transform;
      if (entity.tilemap) tilemaps[entity.id] = luaTilemapMirror(entity.tilemap);
      if (entity.animation) animations[entity.id] = entity.animation;
      if (entity.collider) colliders[entity.id] = entity.collider;
      if (entity.camera) cameras[entity.id] = entity.camera;
      if (entity.components && entity.components.Quad) quads[entity.id] = entity.components.Quad;
      if (entity.components && entity.components.WorldText) worldTexts[entity.id] = entity.components.WorldText;
      if (entity.components && entity.components.Visibility) {
        const spec = parseComponentFields(entity.components.Visibility);
        const value = Number(spec.layers);
        if (Number.isFinite(value)) layers[entity.id] = value >>> 0;
      }
    }
    // 诊断（WP7-30 的一次性读数）：映射命中数说明"引擎是否认识这些名字"，出口有无说明产物是否配套。
    if (typeof console !== 'undefined' && console.log) {
      const diagBridge = (typeof window !== 'undefined' && window.__RUSTLUA_LUA_BRIDGE) || null;
      const diagMapped = Object.keys(engineIds).length;
      console.log('web-runtime: 引擎句柄映射 ' + diagMapped + '/' + sceneState.entities.size
        + '（场景 ' + sceneState.name + '；出口='
        + (diagBridge && typeof diagBridge.lua_entity_handle === 'function' ? '有' : '无') + '）');
    }
    return `
__rustlua_scene_name = ${luaQuote(sceneState.name)}
__rustlua_names = ${luaValue(names)}
__rustlua_engine_ids = ${luaValue(engineIds)}
__rustlua_quads = ${luaValue(quads)}
__rustlua_world_texts = ${luaValue(worldTexts)}
__rustlua_layers = ${luaValue(layers)}
__rustlua_cameras = ${luaValue(cameras)}
__rustlua_all_layers = 4294967295
__rustlua_white_texture = ${luaQuote(WHITE_TEXTURE_PATH)}
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
-- prefab 七 API 的 web 模拟：Rust 侧的 prefab.spawn 把实体建进引擎 ECS，而 web 渲染
-- 只认 Lua 模拟侧（__rustlua_* 表 + sceneState），两条链不通，预设 UI 就整屏不画。
-- 这里用 JS 桥（__rustlua_prefab_*，注册见 rustluaRunLuaGame）在渲染侧建实体。
-- 语义与引擎一致：spawn 返回句柄；root/node 返回实体 id（无效句柄返回 nil，探活用
-- pcall(root ~= nil) 判死；引擎版对死句柄抛错，pcall 同样兜底）；set_visible 子树显隐。
prefab = {
  spawn = function(path)
    local h = __rustlua_prefab_spawn(tostring(path))
    if h == nil then return nil end
    return h
  end,
  despawn = function(h) return __rustlua_prefab_despawn(h) == true end,
  root = function(h) return __rustlua_prefab_root(h) end,
  node = function(h, name) return __rustlua_prefab_node(h, tostring(name)) end,
  set_text = function(h, name, text) return __rustlua_prefab_set_text(h, tostring(name), tostring(text or '')) == true end,
  set_visible = function(h, name, visible) return __rustlua_prefab_set_visible(h, tostring(name), visible ~= false) == true end,
  set_rect = function(h, name, l, t, r, b) return __rustlua_prefab_set_rect(h, tostring(name), tonumber(l), tonumber(t), tonumber(r), tonumber(b)) == true end,
}
-- 虚拟键（触屏摇杆/虚拟按钮注入）是**跨帧保持**的按住态：引擎 InputSnapshot::
-- inherit_virtual_keys 每帧把上一份快照的虚拟键集合并入新快照。宿主每帧都整份替换
-- __rustlua_input（设备层重新采样），不并回来就会退化成「摇杆能动、角色不动」——
-- 同帧注入、下一帧被清空，input.action_axis 恒为 0（引擎侧踩过同一个坑）。
__rustlua_virtual_keys = {}
__rustlua_virtual_just = {}
__rustlua_input = { axis_x = 0, axis_y = 0, keys = {}, just = {}, released = {}, mouse = {}, mouse_just = {}, mouse_just_released = {}, cursor = {0, 0}, press_cursor = {0, 0}, cursor_delta = {0, 0}, scroll = {0, 0} }
log ={ info = function(v) print('[lua] ' .. tostring(v)) end, warn = function(v) print('[lua warn] ' .. tostring(v)) end, error = function(v) print('[lua error] ' .. tostring(v)) end, debug = function(v) print('[lua debug] ' .. tostring(v)) end }
__rustlua_dt = 1 / 60
time = { delta = function() return __rustlua_dt end }
-- 引擎物理键名 → web 键名：native 引擎的动作表用 keyw/keya/keyj...（core/input.rs 的
-- ActionMap），web 的动作层（下方 bindings 表）用 w/a/j...。虚拟控件
-- （ui/virtual_pad.lua）注入的是引擎名，不翻译就永远命中不了任何动作（微信实测：
-- 可见摇杆/按钮全部"按了没反应"）。
-- 引擎物理键名 → 本侧输入层键名。**查询与注入共用这一条**：
--   * 查询入口 is_down/just_pressed/just_released/pressed/released 收的是引擎物理键名
--     （「game_api/scripts/demo_input.lua」 就在用 「keyw」/「arrowleft」/「keyp」），
--   * 虚拟控件注入的也是引擎名（「ui/virtual_pad.lua」）。
-- 名字换算只有一份权威实现（宿主 physicalKeyToName ↔ keyNameToBindingText，那份映射同时负责
-- 解析工程文件的 「[input].map」），Lua 侧问宿主、不自己列表。宿主没给回调（或认不出这个名字）
-- 就退回小写原名 —— **认不出要透传**，因为补丁层自己的旧调用方传的是裸名（'w'/'space'）。
local function __rustlua_virtual_name(key)
  local name = tostring(key):lower()
  local mapper = __rustlua_physical_to_web
  if type(mapper) == 'function' then
    local mapped = mapper(name)
    if type(mapped) == 'string' and mapped ~= '' then return mapped end
  end
  local letter = name:match('^key([a-z])$')
  if letter then return letter end
  -- native 引擎把"关闭"作为独立物理键 cancel；web 键盘关闭键是 Escape，
  -- bindings.cancel 绑在 'escape' 上。虚拟控件注入 "cancel" 时须翻成 'escape'，
  -- 否则 just["cancel"] 永远不被 action_pressed("cancel") 读到（关闭按钮点了没反应）。
  -- （宿主映射表里**没有** cancel，所以这一条必须留在本地。）
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
input = { axis = function(name) name = tostring(name or 'x'):lower(); if name == 'x' or name == 'horizontal' then return (__rustlua_input.axis_x or 0) + 0.0 end; if name == 'y' or name == 'vertical' then return (__rustlua_input.axis_y or 0) + 0.0 end; return 0.0 end, is_down = function(key) return __rustlua_input.keys[__rustlua_virtual_name(key)] == true end, just_pressed = function(key) return __rustlua_input.just[__rustlua_virtual_name(key)] == true end, just_released = function(key) return __rustlua_input.released[__rustlua_virtual_name(key)] == true end, pressed = function(key) return __rustlua_input.just[__rustlua_virtual_name(key)] == true end, released = function(key) return __rustlua_input.released[__rustlua_virtual_name(key)] == true end, mouse_is_down = function(button) return __rustlua_input.mouse[tostring(button or 0)] == true end, mouse_just_pressed = function(button) return __rustlua_input.mouse_just[tostring(button or 0)] == true end, cursor_position = function() return __rustlua_input.cursor or {0, 0} end, press_position = function() return __rustlua_input.press_cursor or __rustlua_input.cursor or {0, 0} end, cursor_delta = function() return __rustlua_input.cursor_delta or {0, 0} end, scroll_delta = function() local s = __rustlua_input.scroll; return { s[1] + 0.0, s[2] + 0.0 } end, mouse_just_released = function(button) return __rustlua_input.mouse_just_released and __rustlua_input.mouse_just_released[tostring(button or 0)] == true end, virtual_key = function(key, down) local name = __rustlua_virtual_name(key); local was_down = __rustlua_input.keys and __rustlua_input.keys[name] == true; if down then __rustlua_virtual_keys[name] = true else __rustlua_virtual_keys[name] = nil end; if __rustlua_input.keys then __rustlua_input.keys[name] = down and true or false end; if not down and was_down then __rustlua_input.released[name] = true end end, virtual_pressed = function(key) local name = __rustlua_virtual_name(key); __rustlua_virtual_keys[name] = true; __rustlua_virtual_just[name] = true; if __rustlua_input.keys then __rustlua_input.keys[name] = true end; if __rustlua_input.just then __rustlua_input.just[name] = true end end }
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
-- 一个 rAF 回调可能推进**多个**固定步（追帧）。一次性边沿（just/released/鼠标边沿/
-- 光标与滚轮增量）只能在首个固定步里可见一次——否则同一帧的补步会把 just_pressed 类动作
-- 重复触发（引擎侧同一条纪律：run_pipelines 在 step == 0 时调 finish_frame，注释原话是
-- "catch-up steps in the same frame don't re-fire just_pressed actions"）。
function __rustlua_finish_step_input()
  local inp = __rustlua_input
  if type(inp) ~= 'table' then return end
  inp.just = {}
  inp.released = {}
  inp.mouse_just = {}
  inp.mouse_just_released = {}
  inp.cursor_delta = { 0, 0 }
  inp.scroll = { 0, 0 }
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

-- 引擎侧 API 的 id 翻译（第 60 轮）：quad / mesh / light / world_text 是本文件**没有**覆盖的
-- 引擎全局，调用会落到引擎上；而本世界的实体 id 是 .scene 的**作者 id**，被引擎按
-- index<<32|generation 解包后就成了 EntityId(0:g13) 这种不存在的句柄。这里用元表把
-- 这些表的**第一个参数**（实体 id）从作者 id 翻成引擎句柄（__rustlua_engine_ids，由场景绑定注入），
-- 引擎不认识的名字保持原样——让它在引擎里明确报错，而不是被静默翻译成别的实体。
local __rustlua_host_entity_api = {}
for _, __api_name in ipairs({ 'quad', 'mesh', 'light', 'world_text' }) do
  local __host = _G[__api_name]
  if __host ~= nil then
    __rustlua_host_entity_api[__api_name] = __host
    _G[__api_name] = setmetatable({}, {
      __index = function(_, key)
        local fn = __host[key]
        if type(fn) ~= 'function' then return fn end
        return function(id, ...)
          if type(id) == 'number' and __rustlua_engine_ids ~= nil then
            local mapped = __rustlua_engine_ids[id]
            if mapped ~= nil then id = mapped end
          end
          return fn(id, ...)
        end
      end,
    })
  end
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
-- ---- Tilemap 图集解算的 Lua 侧镜像（M72-01/02）----
-- 与引擎的 rustlua-ecs/src/tilemap.rs 同一套：条目布局、掩码位序、FNV 常数。
-- 常数写成**带符号十进制**而不是 0xcbf2…：那个字面量超出 i64，写法在不同 Lua 版本上
-- 的宽容度不一样，而这里要的只是同一串位（u64 与 i64 是同一 64 位，只是解释不同）。
local AUTOTILE_SLOTS = 16
local VARIANT_HASH_OFFSET = -3750763034362895579
local VARIANT_HASH_PRIME = 1099511628211
local MIX_AXIS_X = -7046029254386353131
local MIX_AXIS_Y = -4658895280553007687
local AVALANCHE_MUL_A = -4658895280553007687
local AVALANCHE_MUL_B = -7723592293110705685
local TWO64_MOD = {}
-- **无符号**取模。Lua 的 % 是带符号的下取模，而 Rust 侧是 u64 取模：variant_count
-- 不是 2 的幂时两者会给出不同结果（-1 对 3：Lua 得 2、u64 得 0），于是 web 与原生
-- 各选各的变体——同一张图在两个宿主上长得不一样，还全都"自洽"。
local function u64_mod(value, n)
  if n <= 0 then return 0 end
  if value >= 0 then return value % n end
  local shift = TWO64_MOD[n]
  if not shift then
    local p = 1 % n
    for _ = 1, 64 do p = (p * 2) % n end
    TWO64_MOD[n] = p
    shift = p
  end
  return (value % n + shift) % n
end
local function as_int(value)
  -- 位运算要求整数：Lua 侧传进来的坐标/种子经常是 float（3.0 而不是 3），
  -- 3.0 >> 8 会直接报错，而 JS 那边所有值天生是 double。
  return math.floor(tonumber(value) or 0)
end
-- **逻辑**右移：Lua 的 >> 给负数补符号位，而 Rust 侧是 u64 的零填充右移。同一个位串
-- 用算术右移会多出一串 1，avalanche 混完就分家——web 与原生各画各的地表。
local function lshr(value, bits)
  return (value >> bits) & ((1 << (64 - bits)) - 1)
end
-- 雪崩（splitmix64 的最后一步）。不混直接取模的话，count 是 2 的幂时 FNV 低位是周期的，
-- 变体会排成整齐的点阵（count=4 第一行 3,2,1,0,3,2,1,0），看着就是假的地表。
local function avalanche(value)
  local stage = value ~ lshr(value, 30)
  stage = stage * AVALANCHE_MUL_A
  stage = stage ~ lshr(stage, 27)
  stage = stage * AVALANCHE_MUL_B
  return stage ~ lshr(stage, 31)
end
__rustlua_variant_for = function(m, x, y)
  local count = m.variant_count or 1
  if count < 2 then return 0 end
  local hash = VARIANT_HASH_OFFSET
  for _, value in ipairs({ as_int(x), as_int(y), as_int(m.variant_seed), MIX_AXIS_X, MIX_AXIS_Y }) do
    for shift = 0, 56, 8 do
      hash = (hash ~ ((value >> shift) & 0xff)) * VARIANT_HASH_PRIME
    end
  end
  return u64_mod(avalanche(hash), count)
end
__rustlua_autotile_mask = function(m, x, y)
  x, y = as_int(x), as_int(y)
  local own = (m.cells and m.cells[y * m.width + x + 1]) or 0
  local mask = 0
  -- 位序按 **index**：地图 row 0 在最底、图集 row 0 在最顶，"上邻居"在两个坐标系里
  -- 指向相反方向，所以这里写的是 bit0=y-1、bit1=x+1、bit2=y+1、bit3=x-1。
  local offsets = { { 0, -1 }, { 1, 0 }, { 0, 1 }, { -1, 0 } }
  for bit = 1, 4 do
    local nx, ny = x + offsets[bit][1], y + offsets[bit][2]
    if nx >= 0 and ny >= 0 and nx < m.width and ny < m.height then
      local neighbor = (m.cells and m.cells[ny * m.width + nx + 1]) or 0
      -- 越界 = 没有邻居；同 kind 才算邻居（引擎里 own==0 时掩码恒 0）。
      if neighbor ~= 0 and neighbor == own then mask = mask | (1 << (bit - 1)) end
    end
  end
  return mask
end
__rustlua_max_tile_id = function(m)
  if m.autotile or (m.variant_count or 1) > 1 then
    return math.max(1, m.tile_kinds or 0)
  end
  return (m.atlas_cols or 6) * (m.atlas_rows or 1)
end
__rustlua_atlas_index = function(m, x, y)
  local id = (m.cells and m.cells[y * m.width + x + 1]) or 0
  if id == 0 then return 0 end
  if not m.autotile and (m.variant_count or 1) <= 1 then return id end
  local kind = id
  if kind > (m.tile_kinds or 0) then return 0 end
  local span = math.max(1, m.variant_count or 1)
  local capacity = (m.atlas_cols or 6) * (m.atlas_rows or 1)
  local index
  if m.autotile then
    index = 1 + (m.tile_kinds or 0) * span + (kind - 1) * AUTOTILE_SLOTS
      + (__rustlua_autotile_mask(m, x, y) % AUTOTILE_SLOTS)
  else
    index = 1 + (kind - 1) * span + __rustlua_variant_for(m, x, y)
  end
  return index <= capacity and index or 0
end
-- 布局整行重发（set_atlas / set_kinds 都改的是同一份布局）：一行带全部七个字段，
-- 比"哪个字段变了发哪几个"少一类两侧对齐问题。
function __rustlua_emit_layout(m, id)
  __rustlua_emit('L\t' .. id .. '\t' .. (m.atlas_cols or 6) .. '\t' .. (m.atlas_rows or 1)
    .. '\t' .. (m.tile_kinds or 0) .. '\t' .. (m.variant_count or 1)
    .. '\t' .. (m.autotile and 1 or 0) .. '\t' .. (m.variant_seed or 0))
end
__rustlua_tint_at = function(m, x, y)
  local i = __rustlua_cell_index(m, x, y)
  local tint_id = i and m.tint_ids and m.tint_ids[i] or 0
  local color = tint_id > 0 and m.tint_table and m.tint_table[tint_id] or nil
  -- 未染色的格子读回**白**（引擎 tint_at 的 [1,1,1,1]：乘法下白 = 不变）。
  return { r = color and color[1] or 1, g = color and color[2] or 1,
    b = color and color[3] or 1, a = color and color[4] or 1 }
end
tilemap = {
  -- 引擎的 tilemap.get 带 texture（season_visual 靠它判断"是否已是目标地表"，
  -- 少了这个字段就会每帧重复写组件、网格反复重建）。图集布局（M72-01/02）也在同一张
  -- 表里读回：数据驱动的美术脚本要能检查"格子按几列几行寻址"，而不只是知道用了哪张贴图。
  get = function(id)
    local m = __rustlua_tilemap_of(id)
    if not m then return nil end
    return {
      width = m.width, height = m.height, tile_size = m.tile_size, texture = m.texture,
      tiles_format = m.tiles_format or 0,
      atlas_cols = m.atlas_cols or 6, atlas_rows = m.atlas_rows or 1,
      tile_kinds = m.tile_kinds or 0, variant_count = m.variant_count or 1,
      autotile = m.autotile == true, variant_seed = m.variant_seed or 0,
      max_tile_id = __rustlua_max_tile_id(m),
    }
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
    return m.cells[i] or 0
  end,
  set_cell = function(id, x, y, value)
    local m = __rustlua_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local cx, cy = math.floor(tonumber(x) or -1), math.floor(tonumber(y) or -1)
    local i = __rustlua_cell_index(m, cx, cy)
    if not i then error('tilemap cell (' .. tostring(x) .. ', ' .. tostring(y) .. ') is out of bounds', 2) end
    -- 越界的格子值当场报错（引擎 Tilemap::set_cell 的 InvalidCellIndex）：写 7 进一张
    -- 6 列 1 行的图集，下一格读回来是 7、画出来是空的，静默写进去等于把问题留给画面。
    local code = math.floor(tonumber(value) or 0)
    if code < 0 or code > __rustlua_max_tile_id(m) then
      error('tile cell index ' .. code .. ' is out of atlas range', 2)
    end
    -- Lua 侧自身也要跟着变：tilemap.get_cell 必须读到刚写进去的值（farm 依赖它
    -- 判断「这块地已经锄过了」，只发渲染 op 会让两边数据分叉）。
    m.cells[i] = code
    __rustlua_emit('C\t' .. id .. '\t' .. cx .. '\t' .. cy .. '\t' .. code)
    return true
  end,
  -- 图集布局声明（M72-01）：tilemap.set_atlas(id, cols, rows)。显式声明，不从贴图
  -- 尺寸反推——贴图被重新缩放时反推会静默重排每一个格子。
  set_atlas = function(id, cols, rows)
    local m = __rustlua_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local c, r = math.floor(tonumber(cols) or 0), math.floor(tonumber(rows) or 0)
    if c < 1 or r < 1 then error('tileset atlas must have at least one column and one row', 2) end
    m.atlas_cols, m.atlas_rows = c, r
    __rustlua_emit_layout(m, id)
    return true
  end,
  -- 逻辑 kind 布局（M72-02）：tilemap.set_kinds(id, kinds, variants, autotile?, seed?)。
  -- 格子值的意思随布局改变（1..kinds 的逻辑 kind ↔ 图集条目），值本身不动。
  set_kinds = function(id, kinds, variants, autotile, seed)
    local m = __rustlua_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local k = math.floor(tonumber(kinds) or 0)
    local v = math.max(1, math.floor(tonumber(variants) or 1))
    if k < 0 then error('tile_kinds must not be negative', 2) end
    m.tile_kinds, m.variant_count = k, v
    m.autotile = autotile == true
    if seed ~= nil then m.variant_seed = math.floor(tonumber(seed) or 0) end
    -- 与引擎 validate_layout 同一条硬规则：块放不下图集就报错，不画"少几格"的画面。
    local needed = k * (math.max(1, v) + 16 * (m.autotile and 1 or 0))
    if needed > m.atlas_cols * m.atlas_rows then
      error('tileset atlas is too small: layout needs ' .. needed .. ' entries but has '
        .. (m.atlas_cols * m.atlas_rows), 2)
    end
    __rustlua_emit_layout(m, id)
    return true
  end,
  -- 本格这帧实际采样哪个图集条目（0 = 不画）。只读解算结果，供门与脚本断言确定性。
  atlas_index = function(id, x, y)
    local m = __rustlua_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local cx, cy = math.floor(tonumber(x) or -1), math.floor(tonumber(y) or -1)
    if not __rustlua_cell_index(m, cx, cy) then return 0 end
    return __rustlua_atlas_index(m, cx, cy)
  end,
  autotile_mask = function(id, x, y)
    local m = __rustlua_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local cx, cy = math.floor(tonumber(x) or -1), math.floor(tonumber(y) or -1)
    if not __rustlua_cell_index(m, cx, cy) then return 0 end
    return __rustlua_autotile_mask(m, cx, cy)
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
  -- 逐格染色（M72-02 的四季/阴湿地表与阴影压暗）：语义是**乘法**（顶点色 × 贴图色），
  -- 不是半透明叠加。相同颜色复用同一个序号（引擎 register_tint 的幂等），否则一层
  -- 季节色会往调色板里塞几百份重复项。
  set_tint = function(id, x, y, r, g, b, a)
    local m = __rustlua_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local cx, cy = math.floor(tonumber(x) or -1), math.floor(tonumber(y) or -1)
    local i = __rustlua_cell_index(m, cx, cy)
    if not i then error('tilemap cell (' .. tostring(x) .. ', ' .. tostring(y) .. ') is out of bounds', 2) end
    local color = { tonumber(r) or 1, tonumber(g) or 1, tonumber(b) or 1, tonumber(a) or 1 }
    m.tint_table = m.tint_table or {}
    m.tint_ids = m.tint_ids or {}
    local tint_id = 0
    for k = 1, #m.tint_table do
      local c = m.tint_table[k]
      if c[1] == color[1] and c[2] == color[2] and c[3] == color[3] and c[4] == color[4] then
        tint_id = k
        break
      end
    end
    if tint_id == 0 then
      tint_id = #m.tint_table + 1
      m.tint_table[tint_id] = color
    end
    m.tint_ids[i] = tint_id
    __rustlua_emit('I\t' .. id .. '\t' .. cx .. '\t' .. cy .. '\t'
      .. color[1] .. '\t' .. color[2] .. '\t' .. color[3] .. '\t' .. color[4])
    return true
  end,
  clear_tint = function(id, x, y)
    local m = __rustlua_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    local cx, cy = math.floor(tonumber(x) or -1), math.floor(tonumber(y) or -1)
    local i = __rustlua_cell_index(m, cx, cy)
    if not i then error('tilemap cell (' .. tostring(x) .. ', ' .. tostring(y) .. ') is out of bounds', 2) end
    if m.tint_ids then m.tint_ids[i] = nil end
    __rustlua_emit('J\t' .. id .. '\t' .. cx .. '\t' .. cy)
    return true
  end,
  tint = function(id, x, y)
    local m = __rustlua_tilemap_of(id)
    if not m then error('entity has no tilemap', 2) end
    return __rustlua_tint_at(m, math.floor(tonumber(x) or -1), math.floor(tonumber(y) or -1))
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
-- 碰撞体几何（overlap / raycast，WP7-35）。本侧没有物理世界，但 Collider 组件的字段
-- （shape / ox,oy / hx,hy，解析处见本文件 applyComponents 的 Collider 分支）与引擎的
-- ColliderDesc::from_component_str 一致，所以这两个查询可以在**同一份数据**上做几何真判：
--   ball   ⇒ 圆，hx 是半径
--   cuboid ⇒ 轴对齐矩形，hx/hy 是半长，中心 = transform + offset
-- sensor **参与**查询（引擎里 sensor 只影响接触解算，不影响查询命中）。
function __rustlua_collider_center(col, pos)
  return (pos.x or 0) + (col.ox or 0), (pos.y or 0) + (col.oy or 0)
end

function __rustlua_circle_hits_collider(cx, cy, r, col, pos)
  local ex, ey = __rustlua_collider_center(col, pos)
  local hx, hy = col.hx or 0, col.hy or 0
  if (col.shape or 'ball') ~= 'cuboid' then
    local dx, dy, rr = cx - ex, cy - ey, r + hx
    return dx * dx + dy * dy <= rr * rr
  end
  local nx = math.max(ex - hx, math.min(cx, ex + hx))
  local ny = math.max(ey - hy, math.min(cy, ey + hy))
  local dx, dy = cx - nx, cy - ny
  return dx * dx + dy * dy <= r * r
end

-- 命中返回**最近的正向距离**，未命中返回 nil；起点落在形状内部 ⇒ 0。
function __rustlua_ray_hits_collider(ox, oy, vx, vy, maxd, col, pos)
  local ex, ey = __rustlua_collider_center(col, pos)
  local hx, hy = col.hx or 0, col.hy or 0
  if (col.shape or 'ball') ~= 'cuboid' then
    local mx, my = ox - ex, oy - ey
    local b = mx * vx + my * vy
    local c = mx * mx + my * my - hx * hx
    if c <= 0 then
      return 0
    end
    local disc = b * b - c
    if disc < 0 then
      return nil
    end
    local t = -b - math.sqrt(disc)
    if t < 0 or t > maxd then
      return nil
    end
    return t
  end
  local tmin, tmax = 0, maxd
  local function slab(origin, dir, lo, hi)
    if dir == 0 then
      return origin >= lo and origin <= hi
    end
    local t1, t2 = (lo - origin) / dir, (hi - origin) / dir
    if t1 > t2 then
      t1, t2 = t2, t1
    end
    if t1 > tmin then
      tmin = t1
    end
    if t2 < tmax then
      tmax = t2
    end
    return tmin <= tmax
  end
  if not slab(ox, vx, ex - hx, ex + hx) then
    return nil
  end
  if not slab(oy, vy, ey - hy, ey + hy) then
    return nil
  end
  return tmin
end

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
  -- 碰撞体查询（引擎走 Rapier，见 api/physics.rs:161）：形状/offset/sensor 都参与。
  -- 与上面的 positions_in_radius **不是**同一件事：那个只看 Transform 位置，
  -- 没有碰撞体的实体也会被查到；这个只查碰撞体。
  overlap = function(x, y, radius, opts)
    local cx, cy, r = tonumber(x), tonumber(y), tonumber(radius)
    local finite = function(v) return v ~= nil and v == v and v ~= math.huge and v ~= -math.huge end
    if not finite(r) or r < 0 then
      error('overlap radius must be finite and non-negative', 2)
    end
    if not finite(cx) or not finite(cy) then
      error('overlap position must be finite', 2)
    end
    -- opts 里的碰撞组（groups）本侧没有对应概念 ⇒ **忽略**（已记录的差异，不是"支持了"）。
    local result = {}
    local n = 1
    for id, col in pairs(__rustlua_colliders or {}) do
      local pos = __rustlua_transforms[id]
      if pos and __rustlua_alive[id] and __rustlua_circle_hits_collider(cx, cy, r, col, pos) then
        result[n] = id
        n = n + 1
      end
    end
    return result
  end,
  -- 射线查询：返回**最近**命中的碰撞体实体，未命中返回 nil（与引擎同形）。
  -- 方向先归一化；max_distance 是**距离**（与引擎的参数名同义）。
  raycast = function(x, y, dx, dy, max_distance, opts)
    local ox, oy, vx, vy, maxd = tonumber(x), tonumber(y), tonumber(dx), tonumber(dy), tonumber(max_distance)
    local finite = function(v) return v ~= nil and v == v and v ~= math.huge and v ~= -math.huge end
    if not finite(maxd) or maxd < 0 then
      error('raycast distance must be finite and non-negative', 2)
    end
    if not finite(ox) or not finite(oy) or not finite(vx) or not finite(vy) then
      error('raycast arguments must be finite', 2)
    end
    local len = math.sqrt(vx * vx + vy * vy)
    if len <= 0 then
      return nil
    end
    vx, vy = vx / len, vy / len
    local best_id, best_t = nil, nil
    for id, col in pairs(__rustlua_colliders or {}) do
      local pos = __rustlua_transforms[id]
      if pos and __rustlua_alive[id] then
        local t = __rustlua_ray_hits_collider(ox, oy, vx, vy, maxd, col, pos)
        if t and (best_t == nil or t < best_t) then
          best_id, best_t = id, t
        end
      end
    end
    return best_id
  end,
  -- 连续碰撞检测：本侧没有刚体、也没有 CCD ⇒ **只记录开关**（get_ccd 读得回来），
  -- 不改变碰撞行为。这是已记录的差异，不能当成"CCD 已支持"。
  set_ccd = function(id, enabled)
    __rustlua_require_alive(id)
    __rustlua_record_degraded('physics.set_ccd')
    __rustlua_ccd = __rustlua_ccd or {}
    __rustlua_ccd[id] = enabled and true or false
    return __rustlua_ccd[id]
  end,
  get_ccd = function(id)
    __rustlua_require_alive(id)
    __rustlua_record_degraded('physics.get_ccd')
    __rustlua_ccd = __rustlua_ccd or {}
    return __rustlua_ccd[id] == true
  end,
  -- 调试绘制：本侧没有物理调试层 ⇒ 记录开关并**一次性告警**（不假装画出来了）。
  debug_draw = function(enabled)
    local value = enabled and true or false
    __rustlua_record_degraded('physics.debug_draw')
    if __rustlua_debug_draw_warned ~= true then
      __rustlua_debug_draw_warned = true
      if log and log.warn then
        log.warn('web 世界没有物理调试层：physics.debug_draw 只记录开关，不画碰撞体描边')
      end
    end
    return value
  end,
  -- 速度读回：与引擎同形（数值下标 [1]/[2] 与命名键 x/y 都在）。
  get_velocity = function(id)
    __rustlua_require_alive(id)
    local v = __rustlua_velocity[id] or { x = 0, y = 0 }
    local x, y = v.x or 0, v.y or 0
    return { x, y, x = x, y = y }
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
  -- 帧碰撞事件：本侧没有接触解算 ⇒ 恒空表（形状与引擎一致，差异明写，不返回 nil）。
  -- 注：这里原先另有一份同名的 events 存根写在表尾，会**覆盖**上面这一份（Lua 同名键取最后），
  -- 于是实现变成死代码。第 66 轮合并成这一份，并挂上降级标记。
  events = function()
    __rustlua_record_degraded('physics.events')
    return {}
  end
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
      size_x = tonumber(w) or 32, size_y = tonumber(h) or tonumber(w) or 32,
      -- 排序层回读与原生 sprite.get 同名同默认值（layer 'default' / order 0）：
      -- 遮挡跟着脚位 Y 走之后，验收要能原样读回自己写下去的那两个键（M35）。
      sorting_layer = spec:match('sorting_layer=([^;]+)') or 'default',
      order = tonumber(spec:match('order=([^;]+)')) or 0
    }
  end
}
-- 一等矩形：quad.get / quad.set，与引擎同名 API（rustlua-scripting/src/script/api/renderable.rs）
-- 以及 rustlua-render/src/quad.rs 的 from_component_str **同规则**：默认 size=64,64 /
-- pivot=0.5,0.5 / color=1,1,1,1 / z=0、缺省贴图给 __white__ 哨兵、size 必填、未知字段报错。
--
-- 为什么要在这里补：本文件此前**没有**覆盖 quad，于是 quad.* 落到引擎上；而本世界的实体 id 是
-- .scene 的**作者 id**，引擎按 index<<32|generation 解包 ⇒ 作者 id 13 变成 EntityId(0:g13)
-- ⇒ invalid entity id（第 60 轮实测，正好是 demo_main.lua:215 那条报错）。
-- 改动经 Q 行交给渲染侧同一份 applyComponents（与 sprite.set 的 S 行同构）。
function __rustlua_parse_pair(raw, what)
  local a, b = raw:match('^%s*(%-?[%d%.]+)%s*,%s*(%-?[%d%.]+)%s*$')
  if not a then error('invalid quad ' .. what .. ': ' .. raw, 3) end
  return tonumber(a), tonumber(b)
end

function __rustlua_parse_quad(spec)
  local out = {
    size_x = 64, size_y = 64,
    pivot_x = 0.5, pivot_y = 0.5,
    color_r = 1, color_g = 1, color_b = 1, color_a = 1,
    z = 0, texture = nil, layer = 'default', order = 0
  }
  local seen, has_size = false, false
  for field in tostring(spec):gmatch('[^;]+') do
    local key, raw = field:match('^%s*([%w_]+)%s*=%s*(.-)%s*$')
    if key == nil then error('malformed quad field: ' .. field, 3) end
    seen = true
    if key == 'size' then
      out.size_x, out.size_y = __rustlua_parse_pair(raw, 'size')
      has_size = true
    elseif key == 'pivot' then
      out.pivot_x, out.pivot_y = __rustlua_parse_pair(raw, 'pivot')
    elseif key == 'color' then
      local parts = {}
      for value in raw:gmatch('[^,]+') do parts[#parts + 1] = tonumber(value) end
      if #parts ~= 4 then error('invalid quad color: ' .. raw, 3) end
      out.color_r, out.color_g, out.color_b, out.color_a = parts[1], parts[2], parts[3], parts[4]
    elseif key == 'texture' then
      if raw == '' then error('quad texture must not be empty', 3) end
      out.texture = raw
    elseif key == 'z' then
      local z = tonumber(raw)
      if z == nil then error('invalid quad z: ' .. raw, 3) end
      out.z = z
    elseif key == 'layer' then
      if raw == '' then error('invalid quad layer: ' .. raw, 3) end
      out.layer = raw
    elseif key == 'order' then
      local order = tonumber(raw)
      if order == nil or order ~= math.floor(order) then error('invalid quad order: ' .. raw, 3) end
      out.order = order
    elseif key == 'flip_x' or key == 'flip_y' then
      out[key] = (raw == '1' or raw == 'true')
    else
      error('unknown quad field: ' .. key, 3)
    end
  end
  if not seen then error('quad component is empty', 3) end
  if not has_size then error('quad component is missing field: size', 3) end
  return out
end

quad = {
  set = function(id, component)
    __rustlua_require_alive(id)
    local spec = tostring(component or '')
    -- 先按引擎同一份规则校验：不合格的组件串在**改动生效前**就报错，而不是画到一半才失败。
    __rustlua_parse_quad(spec)
    __rustlua_quads[id] = spec
    __rustlua_emit('Q\t' .. id .. '\t' .. spec)
    return true
  end,
  get = function(id)
    __rustlua_require_alive(id)
    local spec = __rustlua_quads[id]
    if not spec then error('entity has no quad', 2) end
    local value = __rustlua_parse_quad(spec)
    -- 与引擎一致：没写贴图时给出**实际会采样到**的路径（内置白纹理哨兵）。
    value.texture = value.texture or __rustlua_white_texture
    value.component = spec
    return value
  end
}
-- 世界空间文字：world_text.attach / world_text.detach，与引擎同语义
-- （rustlua-scripting/src/script/api/renderable.rs 的 attach 恒返回 true、detach 返回"是否真的移除了"）。
-- 本文件此前**没有**覆盖 world_text ⇒ 它落到引擎上、本世界的作者 id 被当成引擎句柄，
-- 于是 invalid entity id: EntityId(0:g10)（第 62 轮实测，demo_main.lua:330）。
-- 组件串就是 UiText 的字段（text / font_size / color）加上 offset 与 centered。
world_text = {
  attach = function(id, component)
    __rustlua_require_alive(id)
    local spec = tostring(component or '')
    if spec:match('^%s*$') then error('world_text component must not be empty', 2) end
    if not spec:match('text=') then error('world_text component is missing field: text', 2) end
    __rustlua_world_texts[id] = spec
    __rustlua_emit('W\t' .. id .. '\t' .. spec)
    return true
  end,
  detach = function(id)
    __rustlua_require_alive(id)
    local had = __rustlua_world_texts[id] ~= nil
    __rustlua_world_texts[id] = nil
    -- 没挂过就不发 op：与引擎一致（remove 没东西可删时什么都不做），也免得每帧白刷一行。
    if had then __rustlua_emit('W\t' .. id .. '\t') end
    return had
  end
}
-- 可见层：visibility.get / set / all，与引擎逐条同语义（api/entity.rs:304）：
--   set 把 layers 当 u32 写入（越界报错）并**返回写入的值**；
--   get 对**没有**该组件的实体返回 ALL_LAYERS（不是 0）；
--   all 是常量 u32::MAX（"恢复默认"）。
-- 本文件此前没有覆盖它 ⇒ 它落到引擎上、本世界的作者 id 被当引擎句柄
-- （第 63 轮实测 demo_main.lua:356 的 invalid entity id: EntityId(0:g11)）。
-- 视图遮罩（view_mask）在本侧恒为 ALL_LAYERS（没有设置入口），**默认情形与引擎一致**；
-- 过滤逻辑已按引擎语义写在渲染侧，将来接入遮罩输入只需改那一个变量。
visibility = {
  all = function()
    return __rustlua_all_layers
  end,
  get = function(id)
    __rustlua_require_alive(id)
    local value = __rustlua_layers[id]
    if value == nil then
      return __rustlua_all_layers
    end
    return value
  end,
  set = function(id, layers)
    __rustlua_require_alive(id)
    local value = tonumber(layers)
    if value == nil or value < 0 or value > __rustlua_all_layers or value ~= math.floor(value) then
      error('visibility.set layers must be an integer in 0..=4294967295', 2)
    end
    __rustlua_layers[id] = value
    __rustlua_emit('V\t' .. id .. '\t' .. string.format('%d', value))
    return value
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
  -- 片段名清单（1 基数组）。引擎按**片段声明顺序**给（machine.clips 的插入序）；Lua 表无序，
  -- 这里排序后给出确定结果——顺序可能与引擎不同，但每次相同、可断言（差异已记录）。
  clips = function(id)
    __rustlua_require_alive(id)
    local a = __rustlua_animations[id]
    if not a then error('entity has no animation', 2) end
    local names = {}
    for name in pairs(a.clips or {}) do names[#names + 1] = name end
    table.sort(names)
    return names
  end,
  -- 帧事件：引擎由宿主在推进动画时收集、**取走即清空**（「animation.rs:124」）。
  -- web 世界不收集帧事件 ⇒ 恒为空表：**形状与引擎一致**（1 基数组），
  -- 差异明写在这里与 CHANGELOG，而不是静默返回 nil 让调用方崩在索引上。
  -- 帧事件：本侧不收集 ⇒ 恒空表（形状与引擎一致，差异明写，不返回 nil）。
  events = function()
    __rustlua_record_degraded('animation.events')
    return {}
  end,
  -- 当前片段播放时刻（秒），与 animation3d.time 同名同义（「animation.rs:144」）。
  time = function(id)
    __rustlua_require_alive(id)
    local a = __rustlua_animations[id]
    if not a then error('entity has no animation', 2) end
    return a.time or 0
  end,
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
    -- 与引擎 AnimationStateMachine::set_state 同构（rustlua-animation/src/animation.rs:266）：
    -- **片段同名只返回 false，不动 time/ended/frame**；判同名的顺序也在判未知片段之前。
    -- 游戏侧是"每帧幂等设一次当前片段"的写法（scripts/player/player.lua:285 依赖这条语义，
    -- 注释里写的"同状态时引擎返回 false"就是它），这里无条件重置时钟等于把动画钉在第 0 帧：
    -- web 上行走与待机全部定格、贴图照常画，所以既不报错也看不出宿主哪里坏了。
    if name == a.current then
      return false
    end
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
-- 引擎 ui.text_get(id)（「api/ui.rs:171」）：读回 ui.text / ui.text_at 建的动态文本，
-- 返回 {text, x, y, scale, size, visible}；不存在的 id 返回 nil。
-- 「size」 是**渲染口径**（不是"调用时传了什么"）：未显式给字号时用 「scale × EM_PX」，
-- 引擎的 EM_PX = 12.0（rustlua_math::text_metrics::TEXT_BASE_PX → rustlua_render::text::EM_PX，
-- 「text.rs:183/369」）。本侧 ui.text_at(id,x,y,text,scale,size) 存的 size 可能为 nil，
-- 所以这里必须自己补上乘出来的那个数。
-- 另外要返回**新表**而不是原表：引擎是新建一张只含这六个字段的表，原表多带字段
-- （visible=true 之外的内部字段）会让调用方的逐字段比较/序列化出现差异。
function ui.text_get(id)
  local t = __rustlua_ui_texts[tostring(id)]
  if not t then return nil end
  local scale = t.scale or 1
  return {
    text = t.text or '',
    x = t.x or 0,
    y = t.y or 0,
    scale = scale,
    size = t.size or (scale * 12.0),
    visible = t.visible and true or false,
  }
end
function ui.grid(...) return true end
-- 宽度实测走 JS 的 measureText（注册见 __rustlua_text_js_measure），与原生同一支字体。
-- 每帧几十次跨 wasm 回调不划算，按「字号|文本」缓存；HUD 文案每帧都变（时间/坐标），
-- 所以封顶 512 条就整表丢弃——宁可重测也不无界涨。估算结果**不进缓存**：字体是异步
-- 加载的，把「字体没就绪时量出的 0」钉住会让所有居中永久偏掉。
__rustlua_text_measure_cache = {}
__rustlua_text_measure_count = 0
function ui.measure_text(text, size)
  local s = tostring(text or '')
  local px = tonumber(size) or 16
  local hit = __rustlua_text_measure_cache[px .. '|' .. s]
  if hit then return { width = hit, height = px } end
  local width = nil
  if __rustlua_text_js_measure then
    -- pcall 兜底：JS 回调要跨 wasm 边界，宿主缺 measureText 时宁可退回估算，也不能让
    -- 这条所有 UI 文字居中/截断都经过的函数抛错。
    local ok, measured = pcall(__rustlua_text_js_measure, s, px)
    if ok then width = tonumber(measured) end
  end
  if width and width > 0 then
    if __rustlua_text_measure_count > 512 then
      __rustlua_text_measure_cache = {}
      __rustlua_text_measure_count = 0
    end
    __rustlua_text_measure_cache[px .. '|' .. s] = width
    __rustlua_text_measure_count = __rustlua_text_measure_count + 1
    return { width = width, height = px }
  end
  local count = 0
  for _ in s:gmatch('[^\\128-\\191]') do count = count + 1 end
  return { width = count * px * 0.6, height = px }
end
-- 层序契约：**同类型内按 id 字典序**绘制，与原生引擎逐项一致。Rust 侧 UiState 的
-- texts/bars/panels/images 四个 store 都是 BTreeMap<String, _>（script/mod.rs），
-- push_ui_images 直接对 images.values() 顺序遍历（mesh_ui.rs），所以 PC/编辑器的
-- 层序就是 id 升序。Lua 的 pairs 是哈希序，不排序时 web 每帧都可能换层（表现为
-- 背包底图盖住图标、摇杆关闭盘随机消失）。两侧字符串同为 UTF-8 字节序，等价。
local function sorted_ids(store)
  local ids = {}
  for id in pairs(store) do ids[#ids + 1] = id end
  table.sort(ids)
  return ids
end
-- 具名 UI **文本**的导出（本侧脚本画的字，宿主/验收要能读）。
-- 动机（第 67 轮）：剧本把「每个 API 模块这一帧干了什么」聚合成一行状态文本，经 ui.text_at 画在
-- canvas 上——无头验收**读不到**：既不在 DOM 里，也不是宿主能查的 API。于是「剧本跑到第几节、
-- 哪一节报了错」只能靠「崩在哪一行」反推。这里把具名文本原样导出，验收一次就能读到全部小节的结果。
-- 换行转义成反斜杠加 n 两个字符：DOM 属性值是单行的，真换行会把读数切碎。
function __rustlua_export_ui_texts()
  local rows = {}
  for _, id in ipairs(sorted_ids(__rustlua_ui_texts)) do
    local t = __rustlua_ui_texts[id]
    if t.visible and t.text ~= '' then
      rows[#rows + 1] = tostring(id) .. '=' .. string.gsub(t.text, '[\\r\\n]+', '\\\\n')
    end
    if #rows >= 24 then break end
  end
  return table.concat(rows, '\\n')
end
function __rustlua_export_ui()
  local function num(v) return tostring(v or 0) end
  local function flat(v) return string.gsub(tostring(v or ''), '[\\t\\r\\n]', ' ') end
  -- WP7-65：**变更推送**（第 96 轮量到这条路径进图约 323 µs/帧、标题画面约 435 µs/帧，
  -- 而进图后约 96% 的帧 UI 载荷逐字节相同——平均下来绝大部分是白做）。
  --
  -- 做法：先走一遍**便宜的签名遍历**（不 tostring、不拼串，只把原值塞进复用表），
  -- 与上一帧的签名逐项比较；相同就把上一帧那份串**原样**交回去（零分配）。
  -- 正确性**靠构造保证**：签名覆盖的字段与下面拼串用的字段逐项同序、同默认值
  -- （P 10 项 / I 10 项 / B 9 项 / X 6 项），所以"签名相同 ⇒ 载荷必然逐字节相同"；
  -- 反过来"签名不同但载荷相同"只会多拼一次串，**不会漏发**。
  --
  -- 与第 91 轮那次被真实回归打回的"按变化发"的区别：那里缓存的是**渲染器持有什么**的推测
  -- （影子状态，换场景后会与 JS 侧实际值分家，差值永远不会被发出）；这里缓存的**就是载荷本身**，
  -- 渲染器每帧拿到的仍是完整的一份。所以这条路径不可能出现"少发了一个变化"。
  local sig = __rustlua_ui_sig
  if sig == nil then sig = {}; __rustlua_ui_sig = sig end
  local n = 0
  local function put(v) n = n + 1; sig[n] = v end
  for _, id in ipairs(sorted_ids(__rustlua_ui_panels)) do
    local p = __rustlua_ui_panels[id]
    if p.visible then
      put('P'); put(id); put(p.x); put(p.y); put(p.w); put(p.h)
      put(p.r); put(p.g); put(p.b); put(p.a); put(p.shape or 0)
    end
  end
  for _, id in ipairs(sorted_ids(__rustlua_ui_images)) do
    local i = __rustlua_ui_images[id]
    if i.visible then
      put('I'); put(id); put(i.x); put(i.y); put(i.w); put(i.h)
      put(i.texture); put(i.r); put(i.g); put(i.b); put(i.slice or '')
    end
  end
  for _, id in ipairs(sorted_ids(__rustlua_ui_bars)) do
    local b = __rustlua_ui_bars[id]
    if b.visible then
      put('B'); put(id); put(b.x); put(b.y); put(b.w); put(b.h)
      put(b.ratio); put(b.r); put(b.g); put(b.b)
    end
  end
  for _, id in ipairs(sorted_ids(__rustlua_ui_texts)) do
    local t = __rustlua_ui_texts[id]
    if t.visible then
      put('X'); put(id); put(t.x); put(t.y); put(t.size); put(t.scale); put(t.text)
    end
  end
  local cached = __rustlua_ui_payload
  local prev = __rustlua_ui_sig_prev
  if cached ~= nil and prev ~= nil and __rustlua_ui_sig_prev_len == n then
    local same = true
    for k = 1, n do
      if sig[k] ~= prev[k] then same = false; break end
    end
    if same then return cached end
  end
  -- 交换两张签名表（复用、不拷贝）：本帧的新签名成为"上一帧"，旧的那张留作下帧的草稿。
  __rustlua_ui_sig_prev = sig
  __rustlua_ui_sig_prev_len = n
  __rustlua_ui_sig = prev or {}
  local rows = {}
  for _, id in ipairs(sorted_ids(__rustlua_ui_panels)) do
    local p = __rustlua_ui_panels[id]
    if p.visible then
      rows[#rows + 1] = 'P\\t' .. flat(id) .. '\\t' .. num(p.x) .. '\\t' .. num(p.y) .. '\\t' .. num(p.w) .. '\\t' .. num(p.h) .. '\\t' .. num(p.r) .. '\\t' .. num(p.g) .. '\\t' .. num(p.b) .. '\\t' .. num(p.a) .. '\\t' .. num(p.shape or 0)
    end
  end
  for _, id in ipairs(sorted_ids(__rustlua_ui_images)) do
    local i = __rustlua_ui_images[id]
    if i.visible then
      rows[#rows + 1] = 'I\\t' .. flat(id) .. '\\t' .. num(i.x) .. '\\t' .. num(i.y) .. '\\t' .. num(i.w) .. '\\t' .. num(i.h) .. '\\t' .. flat(i.texture) .. '\\t' .. num(i.r) .. '\\t' .. num(i.g) .. '\\t' .. num(i.b) .. '\\t' .. flat(i.slice or '')
    end
  end
  for _, id in ipairs(sorted_ids(__rustlua_ui_bars)) do
    local b = __rustlua_ui_bars[id]
    if b.visible then
      rows[#rows + 1] = 'B\\t' .. flat(id) .. '\\t' .. num(b.x) .. '\\t' .. num(b.y) .. '\\t' .. num(b.w) .. '\\t' .. num(b.h) .. '\\t' .. num(b.ratio) .. '\\t' .. num(b.r) .. '\\t' .. num(b.g) .. '\\t' .. num(b.b)
    end
  end
  for _, id in ipairs(sorted_ids(__rustlua_ui_texts)) do
    local t = __rustlua_ui_texts[id]
    if t.visible then
      rows[#rows + 1] = 'X\\t' .. flat(id) .. '\\t' .. num(t.x) .. '\\t' .. num(t.y) .. '\\t' .. num(t.size) .. '\\t' .. num(t.scale) .. '\\t' .. flat(t.text)
    end
  end
  -- WP7-65：拼出来的这份串就是下一帧"没变就原样交回"的那一份。
  local out = table.concat(rows, '\\n')
  __rustlua_ui_payload = out
  return out
end
-- ---------------------------------------------------------------------------
-- 存档（web / 微信宿主）：宿主没有文件系统，落盘出口只有一个字符串槽
-- （浏览器 localStorage；微信侧由 wechat-adapter 把 localStorage 映射到 wx.setStorageSync）。
-- 引擎版的 save.commit/load 走 std::fs，wasm 里没有写通道，所以这一层必须自带
-- **序列化 + 读写通道**，不能像以前那样只留 commit=return true 的占位：占位会让
-- save_manager.save() 打出"[save] committed"，而玩家重开什么也没回来（缺的
-- blob_write/blob_read 还会被 hostCompat 的缺失代理换成"可调用且返回真值"的替身，
-- 于是连报错都没有）。
--
-- 落进那个槽的文本是三格套娃，每格只管一件事：
--   ① 行层：save_version=2 头 + 若干「key TAB value」行，键值逐字符转义。与引擎
--      rustlua-gameplay/src/save.rs 的 v2 编码同形（反斜杠、换行、回车、制表符以及
--      其它控制字符写成两字符十六进制转义）。同形买不到"互读文件"（web 拿不到 PC 磁盘），买到的是
--      **坏档判据一致**：缺版本头、版本过高、伪行、未知转义一律整档拒绝，绝不半读。
--   ② 值层：blob 与 map.state 的值是任意 Lua 表（引擎侧走 serde_json）。这里用长度
--      前缀记法而不是 JSON：字符串按字节数取，不必二次转义，少一层就少一类事故。
--      数字分 i/f 两型，浮点用 %.17g 并强制留小数点以保住浮点型，逐位往返；
--      nan/inf 直接 error——写进去就读不回来，那是"看起来成功"的又一副面孔。
--   ③ 表键层：整数键原样回来。引擎的 JSON 会把稀疏整数键退化成字符串键（docs/34 记过），
--      也就是说 web 比 PC 更保真，游戏侧不必为此写分支。
-- 反斜杠一律用 string.char(92) 现取：这段 Lua 整段住在 JS 模板字面量里，想让 Lua 源码
-- 里出现一个反斜杠得在文件里写四遍，改的人必错。
-- ---------------------------------------------------------------------------
local __SAVE_BS = string.char(92)
local __SAVE_TAB = string.char(9)
local __SAVE_LF = string.char(10)
local __SAVE_CR = string.char(13)
local __SAVE_VERSION = 2
local __SAVE_HEADER = 'save_version='
local __SAVE_STORAGE_PREFIX = 'rustlua:save'
local __SAVE_BLOB_PREFIX = '__blob:'
local __SAVE_MAP_KEY = '__map'
local __SAVE_MAP_SPAWN_X = '__map:spawn:x'
local __SAVE_MAP_SPAWN_Y = '__map:spawn:y'
local __SAVE_MAP_STATE_PREFIX = '__mapstate:'
__rustlua_save_slot = nil

local function __save_escape(text)
  return (string.gsub(tostring(text), '[' .. __SAVE_BS .. '%c]', function(ch)
    if ch == __SAVE_BS then return __SAVE_BS .. __SAVE_BS end
    if ch == __SAVE_LF then return __SAVE_BS .. 'n' end
    if ch == __SAVE_CR then return __SAVE_BS .. 'r' end
    if ch == __SAVE_TAB then return __SAVE_BS .. 't' end
    return string.format(__SAVE_BS .. __SAVE_BS .. 'x%02x', string.byte(ch))
  end))
end

-- 与引擎的 unescape 同判据：未知转义与截断的十六进制转义是**错误**，不是"当字面量收下"。
-- 静默收下会让写的人以为自己转义过了，读回来却是另一串字节。
local function __save_unescape(text, line)
  local function fail(reason)
    return nil, 'malformed save file at line ' .. line .. ': ' .. reason
  end
  if not string.find(text, __SAVE_BS, 1, true) then return text end
  local parts = {}
  local i = 1
  while i <= #text do
    local ch = string.sub(text, i, i)
    if ch ~= __SAVE_BS then
      parts[#parts + 1] = ch
      i = i + 1
    else
      local nxt = string.sub(text, i + 1, i + 1)
      if nxt == __SAVE_BS then
        parts[#parts + 1] = __SAVE_BS
        i = i + 2
      elseif nxt == 'n' then
        parts[#parts + 1] = __SAVE_LF
        i = i + 2
      elseif nxt == 'r' then
        parts[#parts + 1] = __SAVE_CR
        i = i + 2
      elseif nxt == 't' then
        parts[#parts + 1] = __SAVE_TAB
        i = i + 2
      elseif nxt == 'x' then
        local hex = string.sub(text, i + 2, i + 3)
        if not string.match(hex, '^[0-9a-fA-F][0-9a-fA-F]$') then
          return fail('truncated or invalid hex escape')
        end
        parts[#parts + 1] = string.char(tonumber(hex, 16))
        i = i + 4
      elseif nxt == '' then
        return fail('trailing backslash in escaped field')
      else
        return fail('unknown escape sequence')
      end
    end
  end
  return table.concat(parts)
end

local function __save_encode_rows(rows)
  local keys = {}
  for key in pairs(rows) do keys[#keys + 1] = key end
  -- 引擎侧是 BTreeMap，行序天然稳定；这里排一次，同一份状态两次 commit 才逐字节相同
  -- （harness 与人工对比 payload 都靠它）。
  table.sort(keys)
  local parts = { __SAVE_HEADER .. __SAVE_VERSION .. __SAVE_LF }
  for _, key in ipairs(keys) do
    parts[#parts + 1] = __save_escape(key) .. __SAVE_TAB .. __save_escape(rows[key]) .. __SAVE_LF
  end
  return table.concat(parts)
end

-- 返回 (rows) 或 (nil, reason)。reason 直接进 save.load 的 "corrupt: " 前缀。
local function __save_decode_rows(text)
  local pending = {}
  local version
  local line_no = 0
  for line in string.gmatch(text .. __SAVE_LF, '([^' .. __SAVE_CR .. __SAVE_LF .. ']*)' .. __SAVE_LF) do
    line_no = line_no + 1
    if line ~= '' and not string.match(line, '^%s+$') then
      local tab = string.find(line, __SAVE_TAB, 1, true)
      if not tab and string.sub(line, 1, #__SAVE_HEADER) == __SAVE_HEADER then
        local body = string.sub(line, #__SAVE_HEADER + 1)
        if not string.match(body, '^%d+$') then
          return nil, 'malformed save file at line ' .. line_no .. ': version is not an integer'
        end
        version = tonumber(body)
      elseif not tab then
        return nil, 'malformed save file at line ' .. line_no .. ': row is missing its separator'
      else
        pending[#pending + 1] = { line_no, string.sub(line, 1, tab - 1), string.sub(line, tab + 1) }
      end
    end
  end
  if not version then return nil, 'save file is missing its version header' end
  if version == 0 or version > __SAVE_VERSION then
    return nil, 'unsupported save version: ' .. version
  end
  local rows = {}
  for _, entry in ipairs(pending) do
    local key, value = entry[2], entry[3]
    if version >= 2 then
      local unescaped_key, key_reason = __save_unescape(entry[2], entry[1])
      if not unescaped_key then return nil, key_reason end
      local unescaped_value, value_reason = __save_unescape(entry[3], entry[1])
      if not unescaped_value then return nil, value_reason end
      key, value = unescaped_key, unescaped_value
    end
    if key == '' then
      return nil, 'malformed save file at line ' .. entry[1] .. ': empty key'
    end
    rows[key] = value
  end
  return rows
end

local function __save_encode_value(value, path, depth)
  if depth > 64 then return error('save payload nests deeper than 64 levels at ' .. path, 0) end
  if value == nil then return 'n' end
  local kind = type(value)
  if kind == 'boolean' then return value and 'b1' or 'b0' end
  if kind == 'number' then
    if value ~= value or value == math.huge or value == -math.huge then
      error('save payload at ' .. path .. ' is not a finite number', 0)
    end
    if math.type(value) == 'integer' then return 'i' .. tostring(value) .. '#' end
    local repr = string.format('%.17g', value)
    if not string.find(repr, '[.eE]') then repr = repr .. '.0' end
    return 'f' .. repr .. '#'
  end
  if kind == 'string' then return 's' .. #value .. ':' .. value end
  if kind == 'table' then
    local fields = {}
    local count = 0
    for key, item in pairs(value) do
      local key_kind = type(key)
      if key_kind ~= 'string' and key_kind ~= 'number' then
        error('save payload table key at ' .. path .. ' is a ' .. key_kind, 0)
      end
      count = count + 1
      fields[count] = __save_encode_value(key, path .. '/(key)', depth + 1) ..
        __save_encode_value(item, path .. '/' .. tostring(key), depth + 1)
    end
    return 't' .. count .. ':' .. table.concat(fields)
  end
  error('save payload at ' .. path .. ' holds a ' .. kind, 0)
end

local function __save_decode_count(text, i, sep)
  local j = string.find(text, sep, i, true)
  if not j then return nil, i, 'missing length terminator' end
  local body = string.sub(text, i, j - 1)
  if not string.match(body, '^%d+$') then return nil, i, 'bad length: ' .. body end
  return tonumber(body), j + 1
end

-- 约定：成功返回 (value, 下一位)，失败返回 (nil, nil, reason)。所以"是否成功"只看
-- 第二个返回值——根值是 nil（tag n）也是成功，别拿第一个值当错误位。
local function __save_decode_value(text, i, depth)
  if depth > 64 then return nil, nil, 'payload nests too deep' end
  local tag = string.sub(text, i, i)
  i = i + 1
  if tag == '' then return nil, nil, 'payload ended early' end
  if tag == 'n' then return nil, i end
  if tag == 'b' then
    local bit = string.sub(text, i, i)
    if bit ~= '0' and bit ~= '1' then return nil, nil, 'bad boolean' end
    return bit == '1', i + 1
  end
  if tag == 'i' or tag == 'f' then
    local j = string.find(text, '#', i, true)
    if not j then return nil, nil, 'unterminated number' end
    local num = tonumber(string.sub(text, i, j - 1))
    if num == nil then return nil, nil, 'bad number' end
    if tag == 'i' and math.type(num) ~= 'integer' then return nil, nil, 'not an integer literal' end
    if tag == 'f' and math.type(num) ~= 'float' then return nil, nil, 'not a float literal' end
    return num, j + 1
  end
  if tag == 's' then
    local len, at, reason = __save_decode_count(text, i, ':')
    if not len then return nil, nil, reason end
    if at + len - 1 > #text then return nil, nil, 'string length runs past the payload' end
    return string.sub(text, at, at + len - 1), at + len
  end
  if tag == 't' then
    local count, at, reason = __save_decode_count(text, i, ':')
    if not count then return nil, nil, reason end
    local out = {}
    i = at
    for _ = 1, count do
      local key, after, key_reason = __save_decode_value(text, i, depth + 1)
      if not after then return nil, nil, key_reason end
      if key == nil then return nil, nil, 'nil table key' end
      local value, next_at, value_reason = __save_decode_value(text, after, depth + 1)
      if not next_at then return nil, nil, value_reason end
      out[key] = value
      i = next_at
    end
    return out, i
  end
  return nil, nil, 'unknown value tag'
end

local function __save_read_value(text)
  local value, at, reason = __save_decode_value(text, 1, 0)
  if not at then return nil, reason or 'unterminated payload' end
  if at <= #text then return nil, 'trailing bytes after the payload' end
  return value, nil
end

-- 槽位 → 存储键，对齐引擎的 save_path_for_slot（默认档不带后缀，具名档把非法字符
-- 换成下划线并挂在同一个前缀后面）。**导出成全局**：宿主侧（harness / 诊断）要按同一
-- 条规则算出键名才能读到 payload 做断言，在 JS 里再抄一遍 sanitizer 迟早分叉。
function __rustlua_save_storage_key()
  local slot = __rustlua_save_slot
  if slot == nil then return __SAVE_STORAGE_PREFIX end
  local safe = (string.gsub(slot, '[^%w%-_]', '_'))
  if safe == '' then safe = 'default' end
  return __SAVE_STORAGE_PREFIX .. '_' .. safe
end

save = {
  write = function(key, value)
    key = tostring(key)
    -- 空键会让整份存档在下次 load 时无法解析（引擎 decode 报 empty key），必须在写入点就拒绝。
    if key == '' then error('save.write key must not be empty', 0) end
    __rustlua_save[key] = tostring(value)
  end,
  read = function(key) return __rustlua_save[tostring(key)] end,
  -- blob 与 map.state 都是"值层套进行层"：__rustlua_save 里的一行文本而已，
  -- commit/load 不必为它们单独开路。
  blob_write = function(key, value)
    key = tostring(key)
    if key == '' then error('save.blob_write key must not be empty', 0) end
    __rustlua_save[__SAVE_BLOB_PREFIX .. key] = __save_encode_value(value, key, 0)
  end,
  blob_read = function(key)
    local encoded = __rustlua_save[__SAVE_BLOB_PREFIX .. tostring(key)]
    if encoded == nil then return nil end
    local decoded, reason = __save_read_value(encoded)
    if reason then
      error('save.blob_read ' .. tostring(key) .. ' is corrupt: ' .. reason, 0)
    end
    return decoded
  end,
  slot = function(name)
    if name == nil then
      __rustlua_save_slot = nil
      return
    end
    name = tostring(name)
    __rustlua_save_slot = (name ~= '') and name or nil
  end,
  slot_get = function() return __rustlua_save_slot end,
  commit = function()
    if type(__rustlua_storage_js_write) ~= 'function' then
      log.warn('[save] 宿主存储回调缺失，commit 已放弃')
      return false, 'no_save_path'
    end
    local rows = {}
    for key, value in pairs(__rustlua_save) do rows[tostring(key)] = tostring(value) end
    -- 地图状态在 web 侧住在自己的活表里（引擎侧它就躺在 save.data 里）。落盘时按**引擎
    -- 同名键**写出，读回时按同名键拆回活表，两边各只有一份事实源。
    if __rustlua_map_current ~= nil then rows[__SAVE_MAP_KEY] = tostring(__rustlua_map_current) end
    local spawn = __rustlua_map_spawn
    if type(spawn) == 'table' then
      local x, y = tonumber(spawn.x), tonumber(spawn.y)
      if x and y then
        rows[__SAVE_MAP_SPAWN_X] = string.format('%.17g', x)
        rows[__SAVE_MAP_SPAWN_Y] = string.format('%.17g', y)
      end
    end
    for name, value in pairs(__rustlua_map_state) do
      local key = tostring(name)
      rows[__SAVE_MAP_STATE_PREFIX .. key] =
        __save_encode_value(value, 'map.state ' .. key, 0)
    end
    local payload = __save_encode_rows(rows)
    local ok, reason = __rustlua_storage_js_write(__rustlua_save_storage_key(), payload)
    if not ok then
      log.error('[save] 落盘失败：' .. tostring(reason or 'storage write failed'))
      return false, reason or 'storage_write_failed'
    end
    log.info('[save] committed ' .. __rustlua_save_storage_key() .. ' (' .. #payload .. ' bytes)')
    return true
  end,
  load = function()
    if type(__rustlua_storage_js_read) ~= 'function' then
      log.warn('[save] 宿主存储回调缺失，load 已放弃')
      return false, 'no_save_path'
    end
    local text = __rustlua_storage_js_read(__rustlua_save_storage_key())
    -- 空串是"这个槽里没有档"的唯一形状：payload 再小也带着版本头，不可能为空。
    if type(text) ~= 'string' or text == '' then return false, 'missing' end
    local rows, reason = __save_decode_rows(text)
    if not rows then return false, 'corrupt: ' .. reason end
    -- 先整档解一遍，再一次性换掉三张活表：任何一行坏掉都整档拒绝，
    -- 绝不留下一半内存态来自宿主、一半来自上一帧。
    local next_save, next_state = {}, {}
    for key, value in pairs(rows) do
      if key == __SAVE_MAP_KEY or key == __SAVE_MAP_SPAWN_X or key == __SAVE_MAP_SPAWN_Y then
        -- 拆到下面的三张活表里，不进 __rustlua_save
      elseif string.sub(key, 1, #__SAVE_MAP_STATE_PREFIX) == __SAVE_MAP_STATE_PREFIX then
        local name = string.sub(key, #__SAVE_MAP_STATE_PREFIX + 1)
        local decoded, decode_reason = __save_read_value(value)
        if decode_reason then
          return false, 'corrupt: map.state ' .. name .. ' ' .. decode_reason
        end
        next_state[name] = decoded
      else
        next_save[key] = value
      end
    end
    local spawn_x, spawn_y = tonumber(rows[__SAVE_MAP_SPAWN_X]), tonumber(rows[__SAVE_MAP_SPAWN_Y])
    __rustlua_save = next_save
    __rustlua_map_state = next_state
    __rustlua_map_current = rows[__SAVE_MAP_KEY]
    __rustlua_map_spawn = (spawn_x and spawn_y) and { x = spawn_x, y = spawn_y } or nil
    log.info('[save] loaded ' .. __rustlua_save_storage_key())
    return true
  end,
  -- 「开始新游戏」必须能丢弃引擎侧持久键：__map / __map:spawn:* / __mapstate:*。
  -- 游戏侧 state.reset() 只清 Lua 表，残留会让新档读回上一局的地图与地图状态
  -- （同进程内「继续 → 玩 → 返回标题 → 开始新游戏」即可复现，main.lua 的注释写着
  -- 顺序固定：先清引擎键，再让各模块写自己的空快照）。返回清掉的键数，与引擎一致：
  -- **只清内存**，槽里的旧 payload 由下一次 commit 整体替换。
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
particles = {
  -- 引擎侧 particles 是**真模拟**（「physics.rs:468」的 burst_with + 「count」返回 len）。
  -- **web 世界没有粒子模拟**：burst 只登记次数、count 恒返回 0。这条差异是**明写**的
  -- （不是"恰好 0"）：脚本判能力请看「__rustlua_missing_globals」与本文档，
  -- 将来补真实模拟时改这里并同步本注释——绝不要让它继续以"返回一张表"的姿态骗过调用方。
  burst = function(...)
    __rustlua_particle_bursts = (__rustlua_particle_bursts or 0) + 1
    __rustlua_record_degraded('particles.burst')
    if __rustlua_particle_warned ~= true then
      __rustlua_particle_warned = true
      if log and log.warn then
        log.warn('web 世界没有粒子模拟：particles.burst 只登记次数，particles.count() 恒为 0')
      end
    end
    return true
  end,
  count = function()
    __rustlua_record_degraded('particles.count')
    return 0
  end
}
-- 相机（引擎侧 camera 是 **3D** 相机：set_active/active 认 Camera 组件，get/set 只认 camera3d）。
-- 原先是 { set = function(...) return true end, set_active = ... }——一对**静默空实现**：
-- 返回 true 却什么都不做，正是本项目最忌的形态（调用方以为相机换了，画面没换）。
-- 现在按引擎逐条对齐：set_active 要求实体真有 Camera 组件（否则报 "entity has no camera"），
-- 本侧是 2D 世界、没有 camera3d ⇒ get/set 与引擎同一条报错，而不是假装成功。
camera = {
  -- 读取器在引擎里的注册名是 get_active（api/camera.rs:50），返回实体号或 nil；没有 active 这个名字。
  get_active = function()
    return __rustlua_active_camera
  end,
  set_active = function(id)
    __rustlua_require_alive(id)
    if __rustlua_cameras[id] == nil then
      error('entity has no camera', 2)
    end
    __rustlua_active_camera = id
    __rustlua_emit('K\t' .. id)
    return true
  end,
  get = function(id)
    __rustlua_require_alive(id)
    if __rustlua_camera3d[id] == nil then
      error('entity has no camera3d', 2)
    end
    return __rustlua_camera3d[id]
  end,
  set = function(id, ...)
    __rustlua_require_alive(id)
    if __rustlua_camera3d[id] == nil then
      error('entity has no camera3d', 2)
    end
    return true
  end
}
-- 2D 相机（camera2d）：与引擎逐条同语义（api/camera.rs:242-397）。
--   * set / set_zoom / follow / unfollow / shake 都只是**请求**，宿主下一帧消费；
--   * get / get_zoom 读**宿主每帧回填的真实值**（引擎注释明写这条：读的不是脚本自述）。
--     宿主用 lua_set_number 每帧写 __rustlua_cam_x / __rustlua_cam_y / __rustlua_cam_zoom——
--     不编译源码、也不跨界传 JSON（热路径上每帧都要写）。
--   * world_to_screen / screen_to_world / mouse_world 用**渲染侧同一份**换算（1280×720 设计空间，
--     x = 640 + (wx - camx) * zoom、y = 360 - (wy - camy) * zoom）。写两遍就会分叉，所以这里
--     只做一次算术，口径与 drawScene 的 toCanvas 逐字一致。
-- 本轮之前本侧**没有** camera2d：脚本问 get() 会落到引擎那一侧（空场景）⇒ 永远 (0,0)/1.0，
-- 而画面是 JS 世界渲染的真实场景——不崩、不报错、也不进缺口清单，是典型的"看起来没事"。
local function __rustlua_cam_finite(value, message)
  if type(value) ~= 'number' or value ~= value or value == math.huge or value == -math.huge then
    error(message, 3)
  end
  return value
end
camera2d = {
  set = function(x, y)
    local nx = __rustlua_cam_finite(x, 'camera2d.set must be finite')
    local ny = __rustlua_cam_finite(y, 'camera2d.set must be finite')
    __rustlua_emit('E\tcenter\t' .. nx .. '\t' .. ny)
    return true
  end,
  get = function()
    return { x = __rustlua_cam_x or 0, y = __rustlua_cam_y or 0 }
  end,
  set_zoom = function(zoom)
    local z = __rustlua_cam_finite(zoom, 'camera2d.set_zoom must be > 0')
    if z <= 0 then error('camera2d.set_zoom must be > 0', 2) end
    __rustlua_emit('E\tzoom\t' .. z)
    return true
  end,
  get_zoom = function()
    return __rustlua_cam_zoom or 1
  end,
  world_to_screen = function(x, y)
    local wx = __rustlua_cam_finite(x, 'camera2d.world_to_screen must be finite')
    local wy = __rustlua_cam_finite(y, 'camera2d.world_to_screen must be finite')
    local z = __rustlua_cam_zoom or 1
    return { x = 640 + (wx - (__rustlua_cam_x or 0)) * z, y = 360 - (wy - (__rustlua_cam_y or 0)) * z }
  end,
  screen_to_world = function(x, y)
    local sx = __rustlua_cam_finite(x, 'camera2d.screen_to_world must be finite')
    local sy = __rustlua_cam_finite(y, 'camera2d.screen_to_world must be finite')
    local z = __rustlua_cam_zoom or 1
    if z == 0 then z = 1 end
    return { x = (__rustlua_cam_x or 0) + (sx - 640) / z, y = (__rustlua_cam_y or 0) - (sy - 360) / z }
  end,
  mouse_world = function()
    local c = (__rustlua_input and __rustlua_input.cursor) or { 0, 0 }
    local z = __rustlua_cam_zoom or 1
    if z == 0 then z = 1 end
    return { x = (__rustlua_cam_x or 0) + ((c[1] or 0) - 640) / z, y = (__rustlua_cam_y or 0) - ((c[2] or 0) - 360) / z }
  end,
  shake = function(amplitude, duration)
    local a = __rustlua_cam_finite(amplitude, 'camera2d.shake needs a finite amplitude and duration > 0')
    local d = __rustlua_cam_finite(duration, 'camera2d.shake needs a finite amplitude and duration > 0')
    if d <= 0 then error('camera2d.shake needs a finite amplitude and duration > 0', 2) end
    __rustlua_emit('E\tshake\t' .. a .. '\t' .. d)
    return true
  end,
  follow = function(id, options)
    local target = '-'
    if id ~= nil then
      __rustlua_require_alive(id)
      target = tostring(id)
    end
    local ox, oy, dx, dy, sm = 0, 0, 0, 0, 0
    if type(options) == 'table' then
      local function num(key)
        local v = options[key]
        if v == nil then return 0 end
        local message = 'camera2d.follow options must be finite'
        if type(v) ~= 'number' or v ~= v or v == math.huge or v == -math.huge then
          error(message, 3)
        end
        return v
      end
      ox, oy = num('offset_x'), num('offset_y')
      dx, dy = num('dead_zone_x'), num('dead_zone_y')
      sm = num('smoothing')
      if dx < 0 or dy < 0 then error('camera2d.follow dead zone must be >= 0', 2) end
    end
    __rustlua_emit('E\tfollow\t' .. target .. '\t' .. ox .. '\t' .. oy .. '\t' .. dx .. '\t' .. dy .. '\t' .. sm)
    return true
  end,
  unfollow = function()
    __rustlua_emit('E\tunfollow')
    return true
  end,
}
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

-- 穿透恢复（原生由 Rapier 负责，web 侧必须自己补上，否则缺这一半就不算"同一套手感"）。
-- 为什么需要：击退/落点判定用的是**中心点所在格**（combat.lua 的 cell_blocked →
-- tilemap.is_solid），而这里的移动积分用的是完整 AABB。于是"中心格空、但 24x24 的箱子
-- 压进邻格"的落点会把实体嵌进实心格；嵌进去以后每个候选位置都判阻挡 ⇒ 一步也动不了
-- （实机 2026-10-04：被怪物打一下，退进药田边的墙里卡死出不来）。
-- 做法：八方向由近及远找第一个 AABB 不阻挡的位置，最多外推 64 世界单位（两个格）。
-- 只在"当前确实嵌着"时才有搜索开销，出来即停。
function __rustlua_unstick(id)
  local p = __rustlua_transforms[id]
  if not p then return false end
  local x, y = p.x or 0, p.y or 0
  if not __rustlua_blocked_at(id, x, y) then return false end
  for step = 2, 64, 2 do
    local tries = {
      { x + step, y }, { x - step, y }, { x, y + step }, { x, y - step },
      { x + step, y + step }, { x + step, y - step }, { x - step, y + step }, { x - step, y - step },
    }
    for _, t in ipairs(tries) do
      if not __rustlua_blocked_at(id, t[1], t[2]) then
        p.x, p.y = t[1], t[2]
        __rustlua_transforms[id] = p
        return true
      end
    end
  end
  return false
end

-- 每帧把 Lua 侧的场景状态交给渲染器（syncLuaScene 解析）。
--   step=false 用于 on_create 之后的"只看不改"快照：不推进速度积分与动画帧，
--   否则一次切换会白白多走一帧动画。
-- 行格式：R=spawn（建实体） S=sprite.set（Sprite 组件串） X=animation.attach
--        C=set_cell  Z=set_solid  U=set_texture  D=destroy  T=Transform  A=动画当前 clip + 帧序号
--        Q=quad.set（Quad 组件串）
--        W=world_text.attach/detach（WorldText 组件串；空串=摘掉）
-- R/D 这类结构变更只发一次（op 列表取走即清）；T/A 每帧对**存活**实体重新发。
function __rustlua_export_state(step)
  local rows = __rustlua_ops
  __rustlua_ops = {}
  local advance = step ~= false
  if advance then
    -- 穿透恢复排在位移**之前**：嵌在格里的实体先出来，这一帧的 WASD 才有效。
    for id in pairs(__rustlua_colliders or {}) do
      if __rustlua_alive[id] == true then __rustlua_unstick(id) end
    end
    for id, v in pairs(__rustlua_velocity) do
      if __rustlua_alive[id] == true then
        __rustlua_move_with_collision(id, (v.x or 0) * time.delta(), (v.y or 0) * time.delta())
      end
    end
    for _, a in pairs(__rustlua_animations) do __rustlua_advance_animation(a, time.delta()) end
  end
  for id, p in pairs(__rustlua_transforms) do
    if __rustlua_alive[id] == true then
      -- WP7-60：**这里曾经试过"只在变换变了时才发"（Lua 侧影子表），已被撤掉**，原因记在
      -- docs 与 CHANGELOG 里，一句话：那条路假设了"我们上次发出去的值 == 渲染器手上持有的值"，
      -- 而换场景/延迟挂包这类路径会让渲染器的实体被**重新装载**（JS 侧回到场景文件里的初值），
      -- 影子表却还记着旧值 ⇒ 差一点就永远发不出去。门里的驱动用例当场抓到：
      -- 「Lua 世界里的主角在 (528,159)，渲染器侧却在 (528,112)（回写通道的 T 行没把位置同步给渲染器）」。
      -- 结论：**要做这种削减，先把状态回写的所有者收敛成一个**（每帧恰好一次、且结果一定被消费），
      -- 否则任何"增量发"都是在赌消费者与生产者对同一份状态的理解永不分离。
      rows[#rows + 1] = 'T\t' .. id .. '\t' .. tostring(p.x or 0) .. '\t' .. tostring(p.y or 0) .. '\t' .. tostring(p.rotation or 0) .. '\t' .. tostring(p.scale_x or 1) .. '\t' .. tostring(p.scale_y or 1)
    end
  end
  for id, a in pairs(__rustlua_animations) do
    if __rustlua_alive[id] == true then
      rows[#rows + 1] = 'A\t' .. id .. '\t' .. tostring(a.current or 'idle') .. '\t' .. tostring(a.frame or 0)
    end
  end
  -- WP7-59：隐藏文本集**并进同一份每帧状态**（同一个 Lua 调用、零额外跨界）。
  -- 它只在 ui.scene_text_visible(name, false) 时变，但以前是每帧**单独**拉一次
  -- （每帧 5 次跨界里的第 5 次，而且每次都要重建一个 Set）。
  -- **空集也要发这一行**：消费侧靠它覆盖旧集合——少发一行，上一次的隐藏名单就会永久
  -- 留在渲染器里（脚本把文字恢复可见了，屏幕上却还空着）。
  -- 注：本段在 JS 模板字符串里，注释中不得出现反引号（会截断模板）。
  local hidden = {}
  for k in pairs(__rustlua_scene_text_hidden or {}) do hidden[#hidden + 1] = k end
  rows[#rows + 1] = 'H\t' .. table.concat(hidden, ',')
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

-- Tilemap 层数在 **Lua 侧**的读数。渲染侧每帧另报它实际遍历了几层，两个一比就看得出
-- "场景声明了 overlay 层、渲染器却整层没画"——web 的 FarmGrid 种田层就被只取第一张的
-- 老写法静默丢掉过（PC 有、web 无，实机 2026-10-04）。两边必须分开取：同一个循环既画
-- 又计数的话，这个数永远自洽。
function __rustlua_tilemap_layers()
  local n = 0
  for id in pairs(__rustlua_tilemaps or {}) do
    -- 只数活着的：渲染侧对 entity.destroy 掉的层不再遍历，两边口径必须一致。
    if __rustlua_alive[id] == true then
      n = n + 1
    end
  end
  -- 桥的 lua_call_global 只字符串化**返回值是字符串**的调用（见上面 callGlobal 的协议注释），
  -- 所以这里交出去的是 tostring 后的数，不是裸数字。
  return tostring(n)
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

    // 候选 src（微信）：**优先问适配层**（wechat-adapter 的 assetSrcFor）。
    //
    // 为什么不能自己拼 'pack/...'：分包形态下首屏音频在构建期被**搬**进 mainpack/（搬运不是
    // 复制），远端形态下整个资源目录都不在包里——两种情况下 'pack/assets/audio/x.mp3' 都指向
    // 一个不存在的文件，症状是"画面正常但没声音"，除了 onError 之外没有任何线索。
    // 适配层同时知道主体路由（mainpack/pack）、代码包改名（hostFileFor）与远端配置，
    // 是这三件事唯一的判据来源；这里再拼一遍等于把同一套规则实现两遍、迟早漂移。
    //
    // 适配层缺席（老产物）时才回落到历史上的三种包内写法。
    function wechatAudioCandidates(name) {
      const logical = 'assets/audio/' + name + '.mp3';
      const adapter = (typeof globalThis !== 'undefined') ? globalThis.__RustLuaWechat : null;
      if (adapter && typeof adapter.assetSrcFor === 'function') {
        try {
          const routed = adapter.assetSrcFor('pack/' + logical);
          if (routed && routed.src) {
            return { list: [routed.src], source: routed.source || 'adapter', pending: !!routed.pending };
          }
        } catch (error) {
          // 适配层探测抛错：回落到包内写法——多试三条路径比"直接判定没有这个音效"更容易查。
        }
      }
      return { list: ['pack/' + logical, logical, './pack/' + logical], source: 'legacy', pending: false };
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
    // 解析结果按音效名缓存：适配层的 assetSrcFor 每次都要做一次 accessSync 探测，
    // 而同一个音效在整局里会被反复播放（走路声、点击声），不缓存就是每播一次探一次。
    const wechatAudioSrcCache = new Map();
    function resolveSrc(name) {
      if (!isWechat) return null;
      const cached = wechatAudioSrcCache.get(name);
      if (cached) return cached;
      const routed = wechatAudioCandidates(name);
      for (let i = 0; i < routed.list.length; i += 1) {
        // 适配层给的路径已经过路由与存在性探测（远端形态下更是个 URL，问不了 accessSync），
        // 只有回落路径才需要在这里自己探。
        if (routed.source !== 'legacy' || fileExists(routed.list[i])) {
          const hit = { src: routed.list[i], source: routed.source, pending: routed.pending };
          wechatAudioSrcCache.set(name, hit);
          return hit;
        }
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
    // 返回 `{ src, source, pending }` 或 null：src 是交给宿主的路径，source/pending 只服务
    // 诊断（"到底走的缓存、包内还是远端"是排查音频缺失的第一个问题）。
    function resolveAnySrc(name) {
      if (missing.has(name)) return null;
      if (!isWechat) return isBrowser ? { src: browserSrc(name), source: 'browser', pending: false } : null;
      return resolveSrc(name);
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
      const resolved = resolveAnySrc(name);
      const src = resolved && resolved.src;
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
      const resolved = resolveAnySrc(name);
      const src = resolved && resolved.src;
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

  // 直接写 Lua 全局数值（不编译源码）。热路径每帧、每个固定步都要改 `__rustlua_dt`，
  // 走 doString 等于每次都编译一小段源码——见 lua_bridge.rs 的 `lua_set_number`。
  function setLuaNumber(name, value) {
    const bridge = window.__RUSTLUA_LUA_BRIDGE;
    if (!bridge) throw new Error('wasm lua bridge not loaded');
    // 导出缺席时**说清楚是哪一边旧**。运行时与 wasm 由同一次打包一起产出，正常不会错配；
    // 真错配了（宿主缓存了旧 web-runtime.js、或产物换了一半）原本只会得到一句
    // "lua_set_number is not a function"，而这句话指不出"该重发哪一半"。
    // 不做静默兜底：那会把"产物换了一半"这种发布故障降级成"帧率有点怪"。
    if (typeof bridge.lua_set_number !== 'function') {
      throw new Error('wasm 缺 lua_set_number 导出：web-runtime.js 比同目录的 wasm 新，'
        + '两者必须来自同一次打包（检查宿主是否缓存了旧运行时，或产物只换了一半）');
    }
    xcSetNum += 1;
    if (DIAGNOSTICS_ON) xsnNames.set(name, (xsnNames.get(name) || 0) + 1);
    bridge.lua_set_number(name, value);

  }
// WP7-77：**批量数值入口**（一次跨界写多个全局）。
//   * 与 `setLuaNumber` 同一口径：导出缺席**不静默兜底**，直接抛——那说明"产物换了一半"；
//   * 用法限制写在调用处：**只允许合并同一时间点的写入**。相机三个数属于同一瞬间，
//     而 `dt` 必须在固定步之前写、脚本在推进前读它 ⇒ 它留在自己的时间点，不并进来。
function setLuaNumbers(pairs) {
  const bridge = window.__RUSTLUA_LUA_BRIDGE;
  if (!bridge || typeof bridge.lua_set_numbers !== "function") {
    throw new Error("wasm 缺 lua_set_numbers 导出：web-runtime.js 比同目录的 wasm 新，"
      + "请重新打包（scripts/package.ps1）后重试");
  }
  // 批量也是一次跨界 ⇒ 计入同一分量（否则读数会"看起来变少"，实则多了一次未计数的调用）。
  xcSetNum += 1;
  return bridge.lua_set_numbers(JSON.stringify(pairs));
}

  // 每步的输入钩子（`__rustlua_apply_virtual_input` / `__rustlua_finish_step_input`）自 WP7-56 起
  // 由 wasm 的固定步实现直接调用（`lua_bridge.rs::lua_fixed_step`：先钩子、再写 `__rustlua_dt`、
  // 最后 `update()`）——这里原来那个 `callLuaHook` 包装函数因此整份删除：它的存在理由就是
  // "别为调一个无参钩子每步编译一段源码"，而那段顺序现在已经搬进 Rust，留着它只会是第二份入口。

  // ---- 每帧输入的批量线格式（`RLFI`）：打包器 ----
  //
  // 替代"每帧把快照拼成一段 Lua 源码、再 `lua_do_string` 编译执行"（`__rustlua_input = {…}`）。
  // 那条路在 wasm 上实测 **10.4 µs + 2,648 B Lua 垃圾/帧**，其中 87–88% 花在"编译 + 跨界编组"，
  // 真正建表只占 1.32 µs——每帧让 Lua 解析一段它早就会解析的字面量是纯浪费。
  //
  // 布局与键表**由 wasm 给出**（`input_frame_layout_json`），这里不手写任何偏移：
  // 偏移写错一个字在 JS 里不会报错，只会让某个字段读到另一个字段的值（"相机偶尔跳一下"）。
  //
  // 与 `setLuaNumber` 同一口径：**不做静默兜底**。导出缺席只可能是"产物换了一半"
  // （宿主缓存了旧运行时 / 只重发了一半），那属于发布故障；顺手退回旧协议会把它降级成
  // "手感有点怪"，恰好是最难查的一类。
  let inputFramePacker = null;
  function getInputFramePacker() {
    if (inputFramePacker) return inputFramePacker;
    const bridge = window.__RUSTLUA_LUA_BRIDGE;
    if (!bridge) throw new Error('wasm lua bridge not loaded');
    if (typeof bridge.input_frame_layout_json !== 'function'
      || typeof bridge.lua_input_frame !== 'function') {
      throw new Error('wasm 缺输入批量导出（input_frame_layout_json / lua_input_frame）：'
        + 'web-runtime.js 与 wasm 必须来自同一次打包（检查宿主是否缓存了旧运行时）');
    }
    const layout = JSON.parse(bridge.input_frame_layout_json());
    const off = layout.offsets;
    const keySlots = new Map();
    layout.key_names.forEach((name, index) => keySlots.set(name, index));
    const mouseSlots = new Map();
    layout.mouse_names.forEach((name, index) => mouseSlots.set(name, index));
    // 复用同一块缓冲：每帧零分配是这次改造的目的之一，逐帧 new Uint8Array 会把省下的又还回去。
    const buffer = new Uint8Array(layout.wire_len);
    const view = new DataView(buffer.buffer);
    // 头（magic/版本/flags）建的时候写一次就够，逐帧重写只是白花时间。
    for (let index = 0; index < layout.magic.length; index += 1) {
      view.setUint8(off.magic + index, layout.magic.charCodeAt(index));
    }
    view.setUint16(off.version, layout.version, true);
    view.setUint16(off.flags, 0, true);
    let frameIndex = 0;
    let appliedFrames = 0;
    let appliedBits = 0;
    // 落不进键表的键名：**统计而不是静默丢弃**。"某个键按了没反应"必须能一眼看出
    // 是"宿主没送到"还是"送到的名字线格式不认"。
    const unmappedKeys = new Set();
    const words = [0, 0, 0, 0];
    // 128 位位图拆成 4 个 u32 小端字写：**不能用 `setBigUint64`**——它要 BigInt（ES2020），
    // 而微信上传器只到 ES2019（M73-00 就因为 BigInt 字面量整包被拒过一次）。
    function writeBitset(base, slots, source, wordCount) {
      for (let index = 0; index < wordCount; index += 1) words[index] = 0;
      for (const name in source) {
        if (source[name] !== true) continue;
        const slot = slots.get(name);
        if (slot === undefined) {
          if (slots === keySlots) unmappedKeys.add(name);
          continue;
        }
        words[slot >>> 5] |= (1 << (slot & 31)) >>> 0;
      }
      for (let index = 0; index < wordCount; index += 1) {
        view.setUint32(base + index * 4, words[index] >>> 0, true);
      }
    }
    function writePair(base, pair) {
      view.setFloat64(base, Array.isArray(pair) ? Number(pair[0]) || 0 : 0, true);
      view.setFloat64(base + 8, Array.isArray(pair) ? Number(pair[1]) || 0 : 0, true);
    }
    inputFramePacker = {
      pack(snapshot) {
        view.setUint32(off.frame, frameIndex >>> 0, true);
        frameIndex = (frameIndex + 1) >>> 0;
        view.setFloat64(off.axis, Number(snapshot.axis_x) || 0, true);
        view.setFloat64(off.axis + 8, Number(snapshot.axis_y) || 0, true);
        writePair(off.cursor, snapshot.cursor);
        writePair(off.press_cursor, snapshot.press_cursor);
        writePair(off.cursor_delta, snapshot.cursor_delta);
        writePair(off.scroll, snapshot.scroll);
        writeBitset(off.keys_held, keySlots, snapshot.keys, 4);
        writeBitset(off.keys_pressed, keySlots, snapshot.just, 4);
        writeBitset(off.keys_released, keySlots, snapshot.released, 4);
        writeBitset(off.mouse_held, mouseSlots, snapshot.mouse, 2);
        writeBitset(off.mouse_just, mouseSlots, snapshot.mouse_just, 2);
        writeBitset(off.mouse_just_released, mouseSlots, snapshot.mouse_just_released, 2);
        return buffer;
      },
      // 一次跨界搬完整帧。返回值是 wasm 侧真的改了几位——恒为 0 说明两边对
      // "这一帧该变哪些位"的理解不一致（不同源的第一手线索）。
      apply() {
        xcInput += 1;
        const touched = bridge.lua_input_frame(buffer);
        appliedFrames += 1;
        appliedBits += touched >>> 0;
        return touched >>> 0;
      },
      stats() {
        return 'frames=' + appliedFrames + ' bits=' + appliedBits
          + ' unmapped=' + unmappedKeys.size
          + (unmappedKeys.size > 0 ? '(' + Array.from(unmappedKeys).slice(0, 8).join(',') + ')' : '');
      }
    };
    return inputFramePacker;
  }

  // 把一帧输入送进 Lua（一次跨界）。调用点只有一个：固定步循环之前。
  function applyInputFrame(snapshot) {
    const packer = getInputFramePacker();
    packer.pack(snapshot);
    return packer.apply();
  }

  // 输入批量读数的常驻项（见调用点的说明）。打包器没建 = 本帧还没跑过固定步。
  function inputStatsReadout() {
    return inputFramePacker ? inputFramePacker.stats() : 'off';
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

  // 宿主缺失 API 清单（方法级，去重有序）：陷阱代理每碰到一个缺失方法就记一笔
  // （`__rustlua_record_missing`），这里只把它读出来。此前那个导出定义了却**没有任何调用点**，
  // 于是"缺哪个 API"只能靠"跑到哪儿炸"逐个发现——每个缺口一次浏览器往返。
  // 现在 boot 读数里直接给出清单，`missing_api_paths=` 一次说清。
  function missingApiPaths() {
    const bridge = window.__RUSTLUA_LUA_BRIDGE;
    if (!bridge) return ['__bridge_absent__'];
    try {
      // **通道很关键**：`lua_do_string(source, chunkname)` 返回 `Result<(), JsValue>`——它**拿不到
      // Lua 返回值**（先试过用它，于是"永远读到空表"，把"清单为空"和"根本读不出来"混成同一种读数，
      // 白跑一轮）。读 Lua 的值要走 `lua_call_global(name, args_json)`，它返回 `Option<String>`。
      const raw = bridge.lua_call_global('__rustlua_export_missing_api', '');
      if (!raw) return [];
      return String(raw).split(/\r?\n/).map(line => line.trim()).filter(Boolean);
    } catch (error) {
      // 失败要给**哨兵值**而不是空表：空表是"确认没有缺口"，读失败是"没测出来"，两者不能同形。
      return ['__probe_error__:' + String((error && error.message) || error).slice(0, 120)];
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

  // 宿主**降级**清单：与上面的"缺口"是两件事——缺口是宿主没有这个方法（陷阱代理顶成假表），
// 降级是我们有实现但语义弱于引擎（只记录开关、恒空表…），且已在代码里明写。
// 走**同一条**可断言通道（DOM 属性 + console.log），因为 log.warn **不进** harness 日志：
// 第 65 轮写了告警却一条都没出现，"没触发"与"没抓到"因此分不开。
// 具名 UI 文本的当帧读数（脚本用 `ui.text_at`/`ui.text` 画在 canvas 上的字）。
// **为什么必须有一条这样的通道**：剧本把"每个 API 模块这一帧干了什么"聚合成一行文本，
// 画在 canvas 上——DOM 里没有、宿主 API 也查不到。于是无头验收只能靠"崩在哪一行"反推
// 剧本跑到第几节、哪一节报错（第 62~66 轮就是这么一轮一个缺口地往前挪的）。
// 走 Lua 侧导出（`__rustlua_export_ui_texts`），与缺口/降级读数同一套"可断言"纪律。
function uiTexts(L) {
  try {
    // 值通道：`lua_do_string` 拿不到返回值，读 Lua 的值必须走 `callGlobal`（它经 bridge
    // 的 `lua_call_global` 返回字符串）。第 62 轮在这上面白跑过一轮。
    return String(callGlobal(L, '__rustlua_export_ui_texts') || '');
  } catch (error) {
    return '__probe_error__:' + String((error && error.message) || error).slice(0, 80);
  }
}

function degradedApiPaths() {
  const bridge = window.__RUSTLUA_LUA_BRIDGE;
  if (!bridge) return ['__bridge_absent__'];
  try {
    const raw = bridge.lua_call_global('__rustlua_export_degraded_api', '');
    if (!raw) return [];
    return String(raw).split(/\r?\n/).map(line => line.trim()).filter(Boolean);
  } catch (error) {
    return ['__probe_error__:' + String((error && error.message) || error).slice(0, 120)];
  }
}

// 「null」（而不是空串）作初值：**第一次 markBoot 必须打一行**，哪怕是"0 项"——
  // 空清单与"探针没跑"在日志里必须长得不一样。
  let lastMissingApiReport = null;
let lastDegradedReport = null;

  function markBoot(state, detail) {
    const body = document.body || document.documentElement;
    if (!body) return;
    body.setAttribute('data-rustlua-boot', state);
    if (detail) body.setAttribute('data-rustlua-boot-detail', String(detail).slice(0, 400));
    // 每次 boot 状态变化顺带记一次宿主缺失 API 清单：失败时这份读数**直接指出缺哪个方法**，
    // 而不是只留下"最后崩在哪一行"（`:316` 那条 `%d` 收到表，症状离原因就很远：
    // 陷阱代理会把缺失方法顶成"可调用且返回一张表"，看起来完全不像"缺一个函数"）。
    const missingPaths = missingApiPaths();
    body.setAttribute('data-rustlua-missing-api', missingPaths.join(','));
    const degradedPaths = degradedApiPaths();
    body.setAttribute('data-rustlua-degraded', degradedPaths.join(','));
    const joined = missingPaths.join(', ');
    // 用 `console.log`（harness 抓得到）且**每次 markBoot 都打**：清单为空也要留下读数，
    // 否则"这条探针跑过没有"无法从日志自证——这正是上一轮把它写成 `lua_do_string` 后
    // 白跑一轮的原因。已打印过的内容不重复刷屏。
    if (joined !== lastMissingApiReport) {
      lastMissingApiReport = joined;
      console.log('web-runtime: 宿主缺失 API ' + missingPaths.length + ' 项' + (joined ? '：' + joined : ''));
    }
    // 降级清单与缺口清单**同一套自证纪律**：初值 null ⇒ 第一次 markBoot 必定打一行（哪怕 0 项），
    // 于是"没降级"与"探针没跑"在日志里长得不一样。
    const degradedJoined = degradedPaths.join(', ');
    if (degradedJoined !== lastDegradedReport) {
      lastDegradedReport = degradedJoined;
      console.log('web-runtime: 宿主降级 API ' + degradedPaths.length + ' 项' + (degradedJoined ? '：' + degradedJoined : ''));
    }
  }

  function callGlobal(L, name, args) {
    // WP10 迁移：经 wasm 桥 `lua_call_global` 调用。参数转 JSON 数组（Rust 侧转
    // Lua 值），返回该全局的字符串化结果或 null（全局缺失 / 返回非字符串），
    // 协议与 fengari 版 `lua_isstring ? tostring : null` 逐项一致。
    const bridge = window.__RUSTLUA_LUA_BRIDGE;
    if (!bridge) return null;
    // 无参调用（`update`、`__rustlua_export_ui`…）传**空串**而不是 `'[]'`：Rust 侧把空串
    // 直接当成"没有参数"，省掉一次 JSON 解析——热路径上每帧有 4~5 次这样的调用，
    // 而解析既花时间又在每次调用上产生一份小垃圾。`[]` 仍然接受（见 lua_bridge.rs）。
    const noArgs = args === undefined || args === null || args.length === 0;
    try {
      xcCallGlobal += 1;
      if (DIAGNOSTICS_ON) xcNames.set(name, (xcNames.get(name) || 0) + 1);
      const value = bridge.lua_call_global(name, noArgs ? '' : JSON.stringify(args));
      // wasm-bindgen 对 Ok(None) 返回 undefined；协议层统一归一成 null（fengari 版语义）。
      return value === undefined ? null : value;
    } catch (error) {
      throw new Error(String((error && error.message) || error));
    }
  }

  window.rustluaRunLuaGame = async function (status) {
    if (!window.__RUSTLUA_LUA_BRIDGE) throw new Error('wasm lua bridge not loaded (fengari execution layer removed)');
    // prefab 模拟的 JS 侧状态：`.prefab` 文本缓存（manifest 预载）+ 句柄 → 实例表。
    // 实例实体直接进 sceneState（UiLayout/Image/Text 同场景实体一条渲染路径），
    // 换场景时整表清空（引擎 world 重建，预设实例随之销毁）。
    const prefabCache = {};
    const prefabInstances = new Map();
    let nextPrefabHandle = 1;
    const PREFAB_SLOT = 1000;
    const bridge = window.__RUSTLUA_LUA_BRIDGE;
    const project = await fetchText('pack/project.toml');
    const startup = keyValue(project, 'startup_scene', 'scenes/main.scene');
    const mainScript = keyValue(project, 'main_script', 'scripts/main.lua');
    const projectName = keyValue(project, 'name', 'game');
    const sceneText = await fetchText('pack/' + startup);
    // 场景可切换 ⇒ 这两项必须可重新赋值（切图时整体换掉）。
    let sceneState = parseScene(sceneText);
    const canvas = document.getElementById('rustlua-canvas');
    // `willReadFrequently` 只在**真的会反复读像素**时开（即诊断/验收）。
    // 它对浏览器是一句明确承诺"这块画布会被反复 getImageData"，代价是把画布退化成 CPU 后备
    // （跳过 GPU 加速）——发布包里唯一的读像素就是画面自检，而那已被 DIAGNOSTICS_ON 关掉，
    // 于是无条件打开等于让每一帧的绘制都走软件光栅化，换不到任何发布期收益。
    // 全文件只有 sampleDrawnPixels 一处 getImageData，所以这里与那个开关同源即可。
    const ctx = canvas.getContext('2d', DIAGNOSTICS_ON ? { willReadFrequently: true } : undefined);
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
      // 预设文件整包预载：`prefab.spawn` 是同步调用（Lua 侧没有 await），不能运行期
      // 再 fetch——启动时按 manifest 一次性读进缓存，spawn 只查表。
      if (path && path.startsWith('assets/prefabs/') && path.endsWith('.prefab')) {
        prefabCache[path] = await fetchText('pack/' + path);
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
    // WP10 迁移（fengari → mlua-wasm）：Lua 状态由 wasm 侧 `LuaRuntime` 承载
    // （`lua_boot` 建运行时 + vfs provider，provider 读 wasm 预加载资源表）；
    // main.lua 在补丁注入完成后经 `lua_execute_main` 执行（读 vfs + pack 解码，
    // 与桌面/微信压缩包语义一致），这里不再 fetch main 源码。
    // `L` 保留为 null 仅为调用点兼容（doString/callGlobal/missingHostGlobals 已走 bridge）。
    bridge.lua_boot();
    const L = null;
    installLuaAudioBridge(knownAudio);
    // 绑定文本 ↔ 本侧键名的双向桥：`input.bindings` 的对外契约是引擎的 `key:keyj` 形式，
    // 而内部匹配用的是 `keyName()` 的裸名（`j`）。两个方向都做成 JS 回调，映射表只有一份
    // （`physicalKeyToName` / `keyNameToBindingText`），Lua 侧不做字符串换算。
    // 第一个返回值是**种类**（key|pad|mouse|invalid）：pad/mouse 文本合法但本侧匹配不到，
    // Lua 侧据此登记降级 —— 不给种类的话，"手柄绑定"与"拼错的绑定"在 Lua 侧长得一模一样。
    bridge.lua_register_js_callback('__rustlua_keys_from_binding', function (text) {
      return bindingTextToKeys(text);
    });
    bridge.lua_register_js_callback('__rustlua_binding_text', function (key) {
      return [keyNameToBindingText(key)];
    });
    // 物理键名 → 本侧输入层键名（`keyw`→`w`、`digit7`→`7`、`arrowleft`→`left`、`space`→`space`）。
    // **同一份映射**（physicalKeyToName ↔ keyNameToBindingText）已经负责解析工程文件的 `[input].map`；
    // 查询入口（is_down/just_pressed/just_released/pressed/released）与注入入口（virtual_key/
    // virtual_pressed）此前各自用 `tostring(key):lower()`，于是引擎名一律落空 ——
    // game_api 的 `demo_input.lua` 用 `input.is_down("keyw")`/`input.is_down("arrowleft")`/
    // `input.just_pressed("keyp")` 这些**物理键名**，在浏览器里全部静默为 false（不报错、不缺 API）。
    // 认不出的名字返回 null，Lua 侧负责退回原名（透传，不误伤裸名调用方）。
    bridge.lua_register_js_callback('__rustlua_physical_to_web', function (name) {
      return physicalKeyToName(String(name).toLowerCase());
    });
    // prefab 七 API 的 JS 桥：Lua 侧 prefab 表（luaPrelude 里的模拟）调这些回调，
    // 在渲染侧（sceneState）直接建/改预设实体。回调返回数组 = Lua 多返回值。
    bridge.lua_register_js_callback('__rustlua_prefab_spawn', function (path) {
      path = String(path || '');
      const text = prefabCache[path] || prefabCache['assets/prefabs/' + path.replace(/^.*prefabs\//, '')];
      if (!text) return [null];
      const parsed = parsePrefab(text);
      const handle = nextPrefabHandle++;
      const base = handle * PREFAB_SLOT;
      const entities = new Map();
      const names = new Map();
      let rootWebId = null;
      for (const [srcId, src] of parsed.entities) {
        const webId = base + srcId;
        const entity = {
          id: webId,
          sourceId: srcId,
          name: src.name,
          parent: src.parent === null ? null : base + src.parent,
          alive: true,
          components: src.components,
          prefabHandle: handle,
          prefabHidden: false,
        };
        applyComponents(entity);
        sceneState.entities.set(webId, entity);
        entities.set(webId, entity);
        if (src.name) names.set(src.name, webId);
        if (srcId === parsed.rootId) rootWebId = webId;
      }
      prefabInstances.set(handle, { entities, names, rootWebId });
      return [handle];
    });
    bridge.lua_register_js_callback('__rustlua_prefab_despawn', function (handle) {
      const inst = prefabInstances.get(Number(handle));
      if (!inst) return [false];
      for (const webId of inst.entities.keys()) sceneState.entities.delete(webId);
      prefabInstances.delete(Number(handle));
      return [true];
    });
    bridge.lua_register_js_callback('__rustlua_prefab_root', function (handle) {
      const inst = prefabInstances.get(Number(handle));
      return inst ? [inst.rootWebId] : [null];
    });
    bridge.lua_register_js_callback('__rustlua_prefab_node', function (handle, name) {
      const inst = prefabInstances.get(Number(handle));
      if (!inst) return [null];
      const webId = inst.names.get(String(name));
      return webId ? [webId] : [null];
    });
    bridge.lua_register_js_callback('__rustlua_prefab_set_text', function (handle, name, text) {
      const inst = prefabInstances.get(Number(handle));
      if (!inst) return [false];
      const webId = inst.names.get(String(name));
      if (!webId) return [false];
      const entity = sceneState.entities.get(webId);
      if (!entity) return [false];
      const value = String(text == null ? '' : text);
      const old = String(entity.components.Text || '');
      entity.components.Text = old.replace(/text=[^;]*/, 'text=' + value);
      if (!entity.text) {
        applyComponents(entity);
      } else {
        entity.text.value = value;
      }
      return [true];
    });
    bridge.lua_register_js_callback('__rustlua_prefab_set_visible', function (handle, name, visible) {
      const inst = prefabInstances.get(Number(handle));
      if (!inst) return [false];
      const webId = inst.names.get(String(name));
      if (!webId) return [false];
      const v = visible !== false;
      const stack = [webId];
      while (stack.length) {
        const cur = stack.pop();
        const entity = sceneState.entities.get(cur);
        if (!entity) continue;
        entity.prefabHidden = !v;
        for (const [id, e] of inst.entities) {
          if (e.parent === cur) stack.push(id);
        }
      }
      return [true];
    });
    bridge.lua_register_js_callback('__rustlua_prefab_set_rect', function (handle, name, l, t, r, b) {
      const inst = prefabInstances.get(Number(handle));
      if (!inst) return [false];
      const webId = inst.names.get(String(name));
      if (!webId) return [false];
      const entity = sceneState.entities.get(webId);
      if (!entity) return [false];
      const left = Number(l), top = Number(t), right = Number(r), bottom = Number(b);
      entity.uiRect = { x: left, y: top, w: right - left, h: bottom - top };
      if (entity.components.UiLayout) {
        entity.components.UiLayout = String(entity.components.UiLayout).replace(/offsets=[^;]*/, 'offsets=' + left + ',' + top + ',' + right + ',' + bottom);
      }
      return [true];
    });
    // `ui.measure_text` 用**真正绘制文字的那支字体、同一个 2D 上下文**量宽，与原生同源
    // （api/ui.rs → render::text::measure_text 用项目字体度量）。原先 Lua 侧按
    // 「0.6 × 字号 × 字数」估算，而全角中文实际占 1 em ⇒ 估出的是真宽的 60%，凡是
    // 「用宽度反推左缘」的绘制都会偏：虚拟按钮标签（x = cx - half_width）实测右偏 7px
    // （18px 两字，36 真宽 vs 21.6 估宽）。量不到时（回调未装/字体未就绪返回 0）
    // Lua 侧仍退回估算，行为不比原来差。
    bridge.lua_register_js_callback('__rustlua_text_js_measure', function (text, px) {
      const size = Number(px);
      if (!Number.isFinite(size) || size <= 0) return [0];
      // 量不出来只能返回 0 让 Lua 退回估算，**不能抛**：ui.measure_text 被所有居中/截断
      // 用着，一处宿主缺口会变成满屏文字错位。真机 2D 上下文不保证有 measureText
      // （微信真机连 TextEncoder/WebAssembly 都缺过，工具绿不等于真机绿）。
      try {
        ctx.font = size + 'px ' + GAME_FONT;
        const width = ctx.measureText(String(text == null ? '' : text)).width;
        return [Number.isFinite(width) ? width : 0];
      } catch (error) {
        return [0];
      }
    });
    // 存档落盘出口。宿主侧只有一个字符串槽：浏览器是 localStorage，微信由
    // wechat-adapter 把同名全局映射到 wx.setStorageSync。序列化全在 Lua 侧做完，
    // 这里只搬字符串——wasm 桥的回调只认基本值（见上面 registerLuaFunction 的注释），
    // 传不了表。
    // 读失败一律折算成空串（=「这个槽里没档」），写失败必须把原因带回去：
    // commit 静默成功就是玩家丢档，lua_api_overrides.json 里那条"存档写回静默失败"
    // 的分叉代价就是这么来的。
    const storageSlot = function () {
      try {
        return (typeof localStorage === 'object' && localStorage) ? localStorage : null;
      } catch (error) {
        // 隐私模式下的 Safari 访问 localStorage 直接抛 SecurityError。
        return null;
      }
    };
    bridge.lua_register_js_callback('__rustlua_storage_js_write', function (key, text) {
      const store = storageSlot();
      if (!store) return [false, 'no_storage'];
      try {
        // 微信适配层的 localStorage 是 shim：它把 `wx.setStorageSync` 的异常 warn 之后**吞掉**，
        // 所以浏览器会抛的配额错误在微信上根本抛不出来。判据改成看返回值——shim 明确返回
        // false 才算失败（真浏览器的 setItem 返回 undefined，不能当成失败）。
        if (store.setItem(String(key), String(text)) === false) {
          return [false, 'storage write rejected by host shim'];
        }
        return [true];
      } catch (error) {
        const reason = String((error && error.message) || error);
        // 配额满（QuotaExceededError）与"存储被禁用"是两类故障，日志里必须分得开：
        // 前者要清档，后者要换宿主通道。
        return [false, 'storage write failed: ' + reason];
      }
    });
    bridge.lua_register_js_callback('__rustlua_storage_js_read', function (key) {
      const store = storageSlot();
      if (!store) return [''];
      try {
        const value = store.getItem(String(key));
        return [typeof value === 'string' ? value : ''];
      } catch (error) {
        return [''];
      }
    });
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
-- **降级**（与上面的"缺口"是两件事，绝不能混）：
--   缺口 = 宿主**没有**这个方法，陷阱代理把它顶成"可调用且返回一张表"（症状离原因很远）；
--   降级 = 我们**有**实现，但语义比引擎弱（例如"只记录开关、不改变行为"），且已在代码里明写。
-- 降级也必须**可断言**：只靠 log.warn 是不够的——harness 抓的是 console.log，warn 根本不进日志
-- （第 65 轮实测：写了告警，日志里一条都没有，于是"没触发"与"没抓到"分不开）。
__rustlua_degraded_api = {}
function __rustlua_record_degraded(path)
  if not __rustlua_degraded_api[path] then
    __rustlua_degraded_api[path] = true
  end
end

function __rustlua_export_degraded_api()
  local rows = {}
  for path in pairs(__rustlua_degraded_api) do
    rows[#rows + 1] = path
  end
  table.sort(rows)
  return table.concat(rows, '\\n')
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
-- 键位权威：工程 project.toml 的 [input].map 是**权威**，上面这张硬编码表只是"工程没给键盘键位
-- 时的回退"。引擎侧键位由 ActionMap 决定，Lua 只读；本侧则有两处真相（工程文件 + 这张表），
-- 必须明确选一处 —— 否则同一个工程在桌面与 web 上会有两套键位。
-- 宿主在启动时把 map 解析好（parseInputMap）编成 __rustlua_project_bindings 送进来，这里合并。
-- **逐动作**合并：工程给了键盘键位就用工程的；工程只给了手柄（pad/padaxis）或没提这个动作，
-- 就保留硬编码键位 —— web 没有手柄轮询，不保留的话那些动作会直接死掉（jhyt 的 move_* 正是
-- 只绑手柄轴，靠这条回退才能用 WASD 跑起来）。
function __rustlua_apply_project_bindings()
  local project = __rustlua_project_bindings
  if type(project) ~= 'table' then return false end
  local applied = 0
  for action, keys in pairs(project) do
    if type(keys) == 'table' and #keys > 0 then
      bindings[action] = keys
      applied = applied + 1
    end
  end
  __rustlua_binding_applied = applied
  return true
end
-- 合并结果的**Lua 侧**读数（宿主启动后读一次）：证明"工程键位真的覆盖进了这张表"，
-- 而不是只证明了"我们解析了文件"。0 = 完全没覆盖（例如工程没有 [input].map）。
function __rustlua_binding_applied_probe()
  return tostring(__rustlua_binding_applied or 0)
end
-- 绑定编辑族：与引擎 「api/input.rs:236-295」 逐条对齐。上面那张表是**生效表**（工程 map 覆盖 +
-- 无手柄回退之后的结果），下面这些函数改的就是它。
--   * rebind(name, binding) → (ok, reason)。语义是**单一主绑定**：先清空该动作的全部旧绑定，再绑
--     这一条（要一键多绑定请改工程文件的 map）。解析失败返回 false + 原因且不改动；而"先清空后
--     绑定"意味着绑定失败时该动作会**空掉** —— 这是引擎的行为，这里照抄（替它「修好」会让两边
--     又不一致，而这类不一致最贵：同一个脚本在两个宿主上得到不同结论）。
--   * unbind(name, binding) → bool（解析失败或本来没绑 = false）。
--   * reset_bindings() → 回到**默认表**（不是工程文件的 map）：本侧默认表就是上面那张硬编码表，
--     与引擎 ActionMap::with_defaults 的动作与键位一一对应（move_up=w、interact=e …）。
--   * action_map_text()/set_action_map(text)：v2 文本的无损往返；版本头不符返回 false + 原因且不改动。
-- pad/mouse 绑定照引擎**算合法**（文本能存、往返无损），但本侧没有手柄轮询 ⇒ 匹配不到，
-- 登记为降级而不是假装可用。
__rustlua_binding_defaults = {}
for __action, __keys in pairs(bindings) do
  local __copy = {}
  for i, k in ipairs(__keys) do __copy[i] = k end
  __rustlua_binding_defaults[__action] = __copy
end
__rustlua_binding_overrides = {}
__rustlua_binding_rebound, __rustlua_binding_unbound, __rustlua_binding_resets = 0, 0, 0
local function __rustlua_bind_pack(...) return { ... }, select('#', ...) end
local function __rustlua_binding_parse(text)
  local packed, count = __rustlua_bind_pack(__rustlua_keys_from_binding(tostring(text or '')))
  local kind = packed[1]
  local keys = {}
  for i = 2, count do keys[#keys + 1] = packed[i] end
  return kind, keys
end
local function __rustlua_binding_texts(action)
  local out = {}
  for _, k in ipairs(bindings[tostring(action)] or {}) do
    local text = __rustlua_binding_text(k)
    if text then
      local seen = false
      for _, existing in ipairs(out) do if existing == text then seen = true; break end end
      if not seen then out[#out + 1] = text end
    end
  end
  return out
end
function input.rebind(action, binding)
  action = tostring(action or '')
  binding = tostring(binding or '')
  if action == '' then return false, 'action name must not be empty' end
  local kind, keys = __rustlua_binding_parse(binding)
  if kind == 'invalid' then return false, 'invalid binding: ' .. binding end
  bindings[action] = nil
  __rustlua_binding_overrides[action] = binding
  __rustlua_binding_rebound = __rustlua_binding_rebound + 1
  if kind == 'pad' or kind == 'mouse' then
    __rustlua_record_degraded('input.rebind(' .. kind .. ')')
    return true, ''
  end
  if #keys == 0 then return false, 'invalid binding: ' .. binding end
  bindings[action] = keys
  return true, ''
end
function input.unbind(action, binding)
  action = tostring(action or '')
  binding = tostring(binding or '')
  local kind, keys = __rustlua_binding_parse(binding)
  if kind == 'invalid' then return false end
  if kind == 'pad' or kind == 'mouse' then
    -- 这类绑定在本侧不落到 bindings 里，没有可解绑的键；清掉覆盖记录即算解绑成功。
    if __rustlua_binding_overrides[action] == binding then
      __rustlua_binding_overrides[action] = nil
      __rustlua_binding_unbound = __rustlua_binding_unbound + 1
      return true
    end
    return false
  end
  local kept, removed = {}, false
  for _, k in ipairs(bindings[action] or {}) do
    local hit = false
    for _, want in ipairs(keys) do if k == want then hit = true; break end end
    if hit then removed = true else kept[#kept + 1] = k end
  end
  if not removed then return false end
  bindings[action] = (#kept > 0) and kept or nil
  __rustlua_binding_unbound = __rustlua_binding_unbound + 1
  return true
end
function input.reset_bindings()
  for action in pairs(bindings) do bindings[action] = nil end
  for action, keys in pairs(__rustlua_binding_defaults) do
    local copy = {}
    for i, k in ipairs(keys) do copy[i] = k end
    bindings[action] = copy
  end
  __rustlua_binding_overrides = {}
  __rustlua_binding_resets = __rustlua_binding_resets + 1
end
function input.action_map_text()
  local names = {}
  for action in pairs(bindings) do names[#names + 1] = action end
  table.sort(names)
  local lines = { 'rustlua-actions-v2' }
  for _, action in ipairs(names) do
    local texts = __rustlua_binding_texts(action)
    if #texts > 0 then lines[#lines + 1] = action .. '=' .. table.concat(texts, ',') end
  end
  return table.concat(lines, '\\n')
end
function input.set_action_map(text)
  text = tostring(text or '')
  local lines = {}
  for line in (text .. '\\n'):gmatch('([^\\n]*)\\n') do lines[#lines + 1] = line end
  local header = (lines[1] or ''):gsub('^' .. string.char(239, 187, 191), '')
  header = header:match('^%s*(.-)%s*$') or ''
  if header ~= 'rustlua-actions-v2' and header ~= 'rustlua-actions-v1' then
    return false, 'unsupported action map version: ' .. header
  end
  -- 先解析到暂存表，出错就**整体不动**（引擎同：版本头/绑定非法时不改当前映射）。
  local staged = {}
  for index = 2, #lines do
    local line = lines[index]:match('^%s*(.-)%s*$')
    if line ~= '' then
      local eq = line:find('=', 1, true)
      if not eq then return false, 'invalid binding: ' .. line end
      local action = line:sub(1, eq - 1):match('^%s*(.-)%s*$')
      local list = {}
      for item in line:sub(eq + 1):gmatch('[^,]+') do
        local entry = item:match('^%s*(.-)%s*$')
        if entry ~= '' then
          local kind, keys = __rustlua_binding_parse(entry)
          if kind == 'invalid' then return false, 'invalid binding: ' .. entry end
          for _, k in ipairs(keys) do list[#list + 1] = k end
        end
      end
      staged[action] = list
    end
  end
  for action in pairs(bindings) do bindings[action] = nil end
  for action, keys in pairs(staged) do bindings[action] = keys end
  __rustlua_binding_overrides = {}
  __rustlua_binding_resets = __rustlua_binding_resets + 1
  return true, ''
end
-- 绑定编辑族的读数（宿主每帧诊断时读一次）：证明显式改动**真的落到了生效表**上。
function __rustlua_binding_probe()
  local bound, overrides = 0, 0
  for _ in pairs(bindings) do bound = bound + 1 end
  for _ in pairs(__rustlua_binding_overrides) do overrides = overrides + 1 end
  -- 可选尾注：诊断用的 Lua 执行通道（URL rustlua-lua-do=）把断言串写进 __rustlua_binding_do_note，
  -- 由这里带出去 —— 否则"通道跑了没有、跑出什么"在页面上看不见。
  local note = __rustlua_binding_do_note
  return 'bound=' .. bound .. ' rebound=' .. __rustlua_binding_rebound
    .. ' unbound=' .. __rustlua_binding_unbound .. ' resets=' .. __rustlua_binding_resets
    .. ' overrides=' .. overrides .. (note and (' | ' .. tostring(note)) or '')
end
-- 最近一次产生真实输入的设备类别（引擎 input.last_device ⇒ "keyboard"/"gamepad"）。
-- 本侧设备层只有键鼠与触摸，**没有手柄轮询**（见上面 bindings 的注释：GamepadAxis/GamepadButton
-- 在 web 侧无对应输入）⇒ 真值永远是 "keyboard"。这里照实返回并**登记为降级**：
-- 假装能报 gamepad 会让 UI 提示在手柄接入时永远不切——那比明确登记"做不到"更糟。
function input.last_device()
  __rustlua_record_degraded('input.last_device')
  return __rustlua_last_device or 'keyboard'
end
-- 手柄三 API：本侧设备层只有键鼠与触摸，**没有手柄轮询** ⇒ 照引擎的**取值类型**返回"没有手柄"
-- 的取值（bool / f32 / 静默无效），并登记为降级。返回假数据会让"手柄接上了却没反应"变成一个
-- 查不出的谜——那比明确登记"做不到"更糟。索引同引擎（钳到 u8 范围）。
function input.pad_button(index)
  __rustlua_record_degraded('input.pad_button')
  return false
end
function input.pad_axis(index)
  __rustlua_record_degraded('input.pad_axis')
  return 0.0
end
-- 引擎的 rumble(low, high, ms) 只登记请求、无手柄时**静默无效**（不报错、不返回）⇒ 本侧同形：
-- 不报错、不返回，登记降级让"震动请求发过没有"可断言。
function input.rumble(low, high, ms)
  __rustlua_record_degraded('input.rumble')
end
-- diagnostics()：引擎返回"出错了"的消息数组。本侧最真实的诊断信息就是**已登记的降级路径**
-- （这台宿主上哪些 API 用的是弱实现）。保持同一形状（字符串数组），游戏侧不需要分支。
function input.diagnostics()
  local rows = {}
  for path in pairs(__rustlua_degraded_api) do
    rows[#rows + 1] = 'web host: ' .. tostring(path) .. ' uses a reduced implementation'
  end
  table.sort(rows)
  return rows
end
-- migration_notes()：引擎的 v1→v2 迁移说明（逐条有损换算），启动时打印一次即可。
-- 本侧的 map 同样来自工程文件，v1 头时给出引擎那句原文（input.rs 的 from_text_with_notes）。
function input.migration_notes()
  if __rustlua_map_header == 'rustlua-actions-v1' then
    return { '输入映射已从 v1 升级到 v2：键位改为物理键名，写盘后即输出 v2 格式' }
  end
  return {}
end
-- settings_path()：引擎返回"输入设置所在文件"的解析路径（没有设置根时 nil）。本侧那份设置就在
-- 打包产物里的 pack/project.toml（启动时已读过，map 与 nav 都从它来）⇒ 如实给出它在包内的路径。
function input.settings_path()
  return __rustlua_settings_path
end
function input.load_settings(path)
  if path ~= nil and path ~= '' then
    __rustlua_record_degraded('input.load_settings(path)')
    return false, 'read_only_bundle'
  end
  -- 启动时已经从 pack/project.toml 应用过绑定与导航手感 ⇒ "效果在位"是如实的；
  -- 但本侧**不能重新读盘**（浏览器与微信都没有同步文件系统）⇒ 登记降级：
  -- 运行期换掉那份文件不会重新生效。
  __rustlua_record_degraded('input.load_settings')
  return true, 'loaded'
end
function input.save_settings(path)
  -- 打包产物只读、宿主没有同步落盘 ⇒ **不假装 saved**（那会让"改键保存了没有"变成查不出的谎）。
  -- 形状与引擎一致 (ok, reason)；reason 用 read_only_bundle —— 引擎没有这个词，是**刻意扩展**：
  -- 宁可给出一个可判定的新原因，也不谎报 saved。
  __rustlua_record_degraded('input.save_settings')
  return false, 'read_only_bundle'
end
-- 菜单导航手感（引擎 input.nav_settings）：读「<project>/project.toml」的「[input]」段。
-- 引擎是**同步读盘**，浏览器没有同步文件系统 ⇒ 宿主在启动时就把已经取到的 project.toml
-- 解析好写进 __rustlua_nav_* 三个全局（见 parseNavSection：段/键名/毫秒优先/钳制与引擎逐条一致），
-- 这里只是把它读出来 ⇒ 是**真读文件**，不是记一个默认值。
-- 显式传 path 时返回 nil：本侧不能按任意路径读盘，与引擎"路径解析不出来就返回 nil"同语义。
function input.nav_settings(path)
  if path ~= nil and path ~= '' then return nil end
  return {
    threshold = __rustlua_nav_threshold or 0.5,
    delay_ms = __rustlua_nav_delay_ms or 350,
    repeat_ms = __rustlua_nav_repeat_ms or 90,
  }
end
-- 覆盖导航手感（引擎 input.set_nav_settings）：三个参数都按 NavSettings::new 钳制，
-- 并**回返钳制后的三元组**（引擎返回 tuple，脚本据此知道真实生效的值）。
--   * 阈值：非有限 → 默认 0.5，否则钳到 0..1；
--   * 延迟：上限 5000ms；重复间隔：钳到 16..5000ms（下限约一个 60Hz 逻辑帧）。
-- 本侧没有手柄模拟量绑定，所以阈值/节奏没有"设备时钟"可驱动是**结构事实**而非遗漏：
-- 引擎自己也只在模拟量绑定上连发（数字键保持一次一步），见 action_repeat 的实现注记。
function input.set_nav_settings(threshold, delay_ms, repeat_ms)
  local t = tonumber(threshold)
  local d = tonumber(delay_ms)
  local r = tonumber(repeat_ms)
  if t == nil or d == nil or r == nil then
    error('input.set_nav_settings needs three numbers', 2)
  end
  local MAX_MS, MIN_REPEAT_MS = 5000, 16
  if t ~= t or t == math.huge or t == -math.huge then t = 0.5 else
    if t < 0 then t = 0 elseif t > 1 then t = 1 end
  end
  d = math.floor(d); r = math.floor(r)
  if d < 0 then d = 0 end
  if d > MAX_MS then d = MAX_MS end
  if r < MIN_REPEAT_MS then r = MIN_REPEAT_MS elseif r > MAX_MS then r = MAX_MS end
  __rustlua_nav_active = { threshold = t, delay_ms = d, repeat_ms = r }
  return t, d, r
end
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
-- 对外文本必须是**引擎的物理键名**（native 的 Binding::to_text 形如 key:keyj、key:digit1、
-- key:arrowup）。游戏侧按这个格式解析：jhyt 「ui/hints.lua」 的 KEYBOARD_KEY_NAMES 表**按物理键名
-- 索引**（它自己的注释就写着「键位文本是物理键名…所以这张表按物理键名索引」），
-- ui/virtual_pad.lua 剥掉 key: 前缀后喂给 input.virtual_key（那个入口同样翻译物理键名，
-- 见 __rustlua_virtual_name）。此前这里手工拼 'key:' .. k，于是吐出的是 key:e / key:7 ——
-- 前缀对了、**名字不对**：virtual_pad 碰巧还能用（e 经翻译仍是 e），但 hints 那张物理键名表
-- **一条都命中不了**，按键说明静默变空（不报错、不缺 API，只表现为「面板上没有按键提示」）。
-- 名字换算只留一份实现（宿主回调 __rustlua_binding_text ↔ keyNameToBindingText），Lua 侧不猜。
function input.bindings(action)
  local result, seen = {}, {}
  for _, k in ipairs(bindings[tostring(action)] or {}) do
    local text = __rustlua_binding_text(k)
    if text and not seen[text] then seen[text] = true; result[#result + 1] = text end
  end
  return result
end
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
-- 引擎 ui.scroll_open(id, viewport, content)（「ui_nav.rs:144」）：**登记**一个滚动区、返回 void。
-- 与 scroll_extents 的区别：那个改尺寸并保留偏移，这个覆盖式重建（引擎是 insert + ScrollState::new，
-- 偏移归零）。此前 web 侧缺它，于是脚本"开了滚动区"其实什么都没发生（陷阱代理还把调用变成"成功"）。
ui.scroll_open = function(id, viewport, content)
  local s = scroll_new()
  s.view = (tonumber(viewport) or 0) + 0.0
  s.total = (tonumber(content) or 0) + 0.0
  scroll_state[tostring(id)] = s
end
ui.scroll_close = function(id)
  local key = tostring(id)
  local had = scroll_state[key] ~= nil
  scroll_state[key] = nil
  return had
end
-- 引擎 ui.scroll_max(id)（「api/ui_nav.rs:187」）：max_offset = max(content - viewport, 0)；
-- **没有这个滚动区时返回 nil**（引擎写的是 「.get(&id).map(ScrollState::max_offset)」），不是 0。
-- 这个区别不是洁癖：game_api 的 demo_index.lua 写 「ui.scroll_max("api_list") or 0」，
-- 而此前本侧**没有**这个函数 ⇒ 缺失陷阱把它代理成"可调用且返回一张表"，既不是 nil 也不是
-- 数字 ⇒ 那一行的 string.format("%.0f") 拿到表 ⇒ 游戏自己抛错 ⇒ 整帧 update 失败 ⇒
-- **启动失败**（第 75 轮实测：boot=failed detail=bad argument #6 to 'format'）。
-- 返回 nil 才能让调用方的 「or 0」 生效。
ui.scroll_max = function(id)
  local s = scroll_state[tostring(id)]
  if not s then return nil end
  return scroll_max(s)
end
-- 引擎 ui.scroll_visible(id)（「api/ui_nav.rs:234」）：「{start, stop}」，没有滚动区时 nil。
-- 字段名是 start/stop 而不是 start/end —— 引擎注释专门写了原因（Lua 里 「end」 是关键字，
-- 取值必须写 「range['end']」），照抄引擎才不会让脚本写出取不到的字段。
-- stop 要钳到 content：引擎 visible_range = (offset, min(offset + viewport, content))。
ui.scroll_visible = function(id)
  local s = scroll_state[tostring(id)]
  if not s then return nil end
  local start = s.offset or 0.0
  local stop = start + (s.view or 0.0)
  local total = s.total or 0.0
  if stop > total then stop = total end
  return { start = start, stop = stop }
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
      // input.nav_settings()：菜单导航手感来自「pack/project.toml」的 [input] 段。
      // 引擎是**同步读盘**（rustlua-gameplay/src/settings.rs 的 parse_nav_section），浏览器没有同步
      // 文件系统 ⇒ 启动时把这份**已经取到**的文本解析好、写进 Lua 全局，脚本调用只是读它。
      // 与引擎逐条同语义：只认 [input] 段（段外同名键不生效）、同名键取首次出现、毫秒键优先于旧帧键
      // （帧键按 60Hz 换算）、数值统一按 NavSettings::new 钳制（见 parseNavSection）。
      // **位置有要求**：必须在 `bridge.lua_boot()` 之后（Lua 状态此时才存在）、且在
      // `lua_execute_main()` 之前——main.lua 里 hints.apply_nav_settings() 会立刻读它。
      {
        const nav = parseNavSection(project);
        setLuaNumber('__rustlua_nav_threshold', nav.threshold);
        setLuaNumber('__rustlua_nav_delay_ms', nav.delayMs);
        setLuaNumber('__rustlua_nav_repeat_ms', nav.repeatMs);
      }
      // 键位权威：把工程 [input].map 的**键盘**键位编成 Lua 赋值再合并进前奏那张硬编码表，
      // 由 __rustlua_apply_project_bindings 逐动作覆盖（见前奏里的说明）。
      // 同样是"启动期一次"，位置与上面的 nav 回填相同：必须在前奏之后、lua_execute_main 之前。
      const inputMap = parseInputMap(project);
      // 同一段 chunk 里把两个"环境事实"也交给 Lua：map 的版本头（决定 `input.migration_notes()`
      // 要不要报 v1→v2 迁移）与设置文件在包内的路径（`input.settings_path()`）。
      // 放在这里而不是另起一次 doString：启动期少一次跨边界调用，且三者必须同时到位。
      doString(L, luaBindingsSource(inputMap.bindings) + '\n__rustlua_apply_project_bindings()'
        + '\n__rustlua_map_header = ' + luaQuote(inputMap.header || '')
        + '\n__rustlua_settings_path = ' + luaQuote('pack/project.toml'),
        'engine-host-bindings');
      // Lua 侧回读：合并**真的**改了几条。只报"我们解析了文件"是不够的——解析对了但没覆盖进表里，
      // 现象与没实现一模一样（键位照旧来自硬编码表）。这条读数把两件事分开。
      let appliedCount = '?';
      try {
        appliedCount = String(callGlobal(L, '__rustlua_binding_applied_probe') || '0');
      } catch (error) {
        appliedCount = 'probe_error';
      }
      // 读数（一次性写，之后不再变）：键位到底以谁为准、工程给了多少、多少动作只能靠回退。
      // 进 DOM 是为了**可断言**——这条差异过去只能靠读源码猜（第 69 轮就是在源码里数出来的）。
      {
        const body = document.body || document.documentElement;
        if (body) {
          // 来源三态必须分清：工程一个键盘键位都没给（整表都是回退）、部分给了（逐动作回退）、
          // 全给了（回退用不上）。写成一件事会让"这张表到底谁说了算"看不出来。
          const source = inputMap.keyed.length === 0
            ? 'fallback-only'
            : (inputMap.padOnly.length > 0 ? 'project+fallback' : 'project');
          // 基础串：启动期只算一次。当帧的编辑族计数（rebound/unbound/resets）由诊断块拼上去
          // —— rebind 是**运行期**发生的，只写一次的话"改了没有"永远看不出来。
          bindingReadoutBase = 'source=' + source
            + ' lua_applied=' + appliedCount
            + ' header=' + (inputMap.header || 'none')
            + ' project_keyed=' + inputMap.keyed.length
            + ' project_pad_only=' + inputMap.padOnly.length
            + ' project_actions=' + (inputMap.keyed.length + inputMap.padOnly.length)
            + ' unmapped=' + (inputMap.unmapped.length > 0 ? inputMap.unmapped.slice(0, 8).join('|') : '0')
            // 有损合并（左右修饰键 / 小键盘同形键）：工程绑 `key:shiftleft` 时 web 上右 Shift
            // 也会响应。不是解析错误，但必须看得见 —— 读数里给个数 + 名字（截前 6 个）。
            + ' aliased=' + (inputMap.aliased.length > 0
              ? inputMap.aliased.length + ':' + inputMap.aliased.slice(0, 6).join('|')
              : '0');
          body.setAttribute('data-rustlua-bindings', bindingReadoutBase);
        }
      }
      // WP10 迁移：main.lua 不再由 JS fetch 后注入，改经 wasm 桥读 vfs 预加载表
      // + pack 解码执行（与桌面/微信压缩包语义一致；执行前全部宿主补丁已在位）。
      bridge.lua_execute_main();
      // **诊断专用**的 Lua 执行通道（URL `rustlua-lua-do=<lua 源码>`）：这是引擎侧
      // `-WorldDo`（URL `rustlua-world-do=`）在 **JS 宿主**上的对等物。在此之前 JS 宿主没有任何
      // "把一段 Lua 送进这个世界跑"的入口，于是引擎宿主能验的东西（重绑定、设置往返、任意脚本
      // 断言）在 JS 宿主上只能靠改工程文件或加临时脚本 —— 两者都不该做。
      // 只读 `location.search`、且**只有诊断开关打开时才认**（发布包既没有这条 URL 也只有
      // 显式开诊断才会走到这里），执行范围就是这个世界本身：能力等价于游戏脚本自己。
      // 注意 `search` 是**别的闭包**里的局部名（本文件另有三处同名声明），这里必须自带取值，
      // 否则每次启动都会抛 ReferenceError —— 这正是 `node --check` 查不出的那类跨闭包自由变量。
      const luaDoMatch = DIAGNOSTICS_ON
        ? /[?&]rustlua-lua-do=([^&]*)/.exec(
          (typeof window !== 'undefined' && window.location && window.location.search) || '')
        : null;
      if (luaDoMatch) {
        const luaDoSource = decodeURIComponent(luaDoMatch[1].replace(/\+/g, ' '));
        try {
          doString(L, luaDoSource, 'rustlua-lua-do');
          luaDoStatus = 'ok';
        } catch (error) {
          // 不吞：状态进读数（`data-rustlua-bindings` 的 lua_do= 段），原错也留在控制台。
          luaDoStatus = 'error: ' + String((error && error.message) || error).slice(0, 200);
          console.error('[rustlua-lua-do] 执行失败：', String((error && error.message) || error));
        }
      }
    } catch (error) {
      // 这里的 hostGlobals 是上面 try 块里的 const：正常路径够得着，异常路径反而会二次抛
      // 「hostGlobals is not defined」，把真正的错误盖掉（第 66 轮实测：页面只报这一句，
      // 原始错误其实是前奏里一个 Lua 字符串没闭合）。异常路径的第一原则是让原错露出来，
      // 探针失败必须退化成哨兵值 + 原错照抛。
      let missing = ['__probe_error__'];
      try {
        if (typeof hostGlobals !== 'undefined') missing = missingHostGlobals(hostGlobals);
      } catch (ignored) {
        missing = ['__probe_error__'];
      }
      const detail = 'missing engine host API: ' + missing.join(', ');
      markBoot('failed', detail);
      throw new Error(String((error && error.message) || error) + ' [' + detail + ']');
    }
    callGlobal(L, 'on_create');
    const pendingTextures = new Set();
    // on_create 里就会挂形象/生成 NPC（`sprite.set` + `animation.attach` + `entity.spawn`），
    // 这些是**一次性 op**：不在这里立刻取走，首帧画的就是"还没挂外观"的场景。
    // 传 false = 只取快照、不推进速度与动画帧（首帧不该白走一次积分）。
    exportWorldState(L, sceneState, false);
    await loadGameFont();
    refreshSceneTextHidden(L);
    let uiElements = drawScene(ctx, sceneState, images, projectName, exportUiPayload(L), pendingTextures);
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
        // 经 Lua 换进来的场景不再画场景自带的 Text（原生侧布局快照已失效），详见
        // drawSceneUiEntity 的 suppressText 分支。
        next.sceneTextSuppressed = true;
        const nextImages = await loadSceneImages(next);
        doString(L, sceneBindings(next), 'engine-host-scene');
        sceneState = next;
        // 引擎换场景整张 world 重建，prefab 实例随之销毁：渲染侧同步清空，
        // 否则旧场景的预设实体（句柄已死）会残留在 sceneState 里继续画。
        prefabInstances.clear();
        images = nextImages;
        // 旧图集对象已随这次替换出局，染色缓存里的离屏画布跟着释放（见 releaseTintedTileCache）。
        releaseTintedTileCache();
        pendingTextures.clear();
        callGlobal(L, 'on_create');
        // 换场景后的 on_create 会重新落位、挂形象、生成 NPC —— 与启动路径同理，
        // 立刻取一次快照，别让新场景的第一帧画成"只有场景本体"。
        exportWorldState(L, sceneState, false);
        refreshSceneTextHidden(L);
        uiElements = drawScene(ctx, sceneState, images, projectName, exportUiPayload(L), pendingTextures);
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
        // 三个**可调旋钮**（第 75 轮加）：剧本的失败原因要能被**造出来**，否则验收门的分支
        // （跳过 / 红灯）永远只能靠"碰巧遇到"来验证。默认值与原来完全一致。
        //   rustlua-drive-wait=<帧>        —— 「等场景切换」的上限
        //   rustlua-drive-move=<键名>      —— 移动阶段按住的键（本侧键名，如 'w'/'a'）
        //   rustlua-drive-move-frames=<帧> —— 移动阶段的帧预算
        const knob = (name, fallback) => {
          const matched = new RegExp('[?&]' + name + '=([^&]*)').exec(search);
          const value = matched ? Number(decodeURIComponent(matched[1])) : NaN;
          return Number.isFinite(value) && value > 0 ? Math.floor(value) : fallback;
        };
        const moveKnob = /[?&]rustlua-drive-move=([^&]*)/.exec(search);
        driveAuto = {
          phase: 'confirm',
          frames: 0,
          confirmFrames: 2,
          sceneBefore: sceneState.name,
          waitLimit: knob('rustlua-drive-wait', 150),
          moveKey: moveKnob ? decodeURIComponent(moveKnob[1]).toLowerCase() : 'w',
          moveFrames: knob('rustlua-drive-move-frames', 12),
          positions: [],
          first: null
        };
      }
    } catch (e) { driveAuto = null; }

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
        if (d.navLastPos && Math.abs(pos[0] - d.navLastPos[0]) < 1 &&
            Math.abs(pos[1] - d.navLastPos[1]) < 1) {
          d.navStuckFrames = (d.navStuckFrames || 0) + 1;
        } else {
          d.navStuckFrames = 0;
        }
        d.navLastPos = pos;
        if (d.navDetourFrames > 0) {
          d.navDetourFrames -= 1;
          return 'detouring';
        }
        if (d.navStuckFrames >= 60 && (d.navDetourAttempts || 0) < 2) {
          if (d.exitKey) dispatchDriveKey('keyup', d.exitKey);
          const detourKey = (d.navDetourAttempts || 0) === 0 ? 'd' : 'a';
          dispatchDriveKey('keydown', detourKey);
          d.exitKey = detourKey;
          d.navDetourFrames = 10;
          d.navDetourAttempts = (d.navDetourAttempts || 0) + 1;
          d.navStuckFrames = 0;
          return 'detouring';
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
              ':scene(' + expect + ',frames=' + d.frames + ',alive=' + alive + ',scene=' + scene + ')');
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

    // 固定时间步（与引擎 `rustlua-infra::FixedTimeStep` 同语义：60Hz + 有界追帧 + 丢弃超额）。
    //
    // 为什么必须这样：此前**每个 rAF 回调固定推进一次 update**，而 `time.delta()` 恒为 1/60，
    // 于是逻辑速度直接绑死在显示器刷新率上——30Hz 半速、120Hz 倍速。现在由真实流逝时间
    // 决定这一帧要跑几个固定步，逻辑速度与刷新率解耦。
    //
    // 追帧上限之后的时间**丢弃**而不是无限补步：切后台/长卡顿回来时游戏时间"跳过"，
    // 而不是补跑上千步把主线程拖死（桌面 `game_app.rs` 取 `FixedTimeStep::new(60, 8)`）。
    // ---- WP7-13 / WP7-56：固定步的**策略与执行都在宿主 wasm** ----
    // `lua_bridge.rs::lua_fixed_frame` 内部就是 `rustlua_infra::frame_clock::FrameClock`（与引擎宿主
    // 同一个策略模块），`lua_fixed_run_steps` 则是**跑一步**的唯一实现（虚拟键 → `__rustlua_dt` →
    // `update()`）。此前这两件事各有一份 JS 实现（累积器 + 1e-9 落帧容差 + 追帧上限 + 丢帧计数，
    // 外加每步的那三句调用），同一份产物里两套定步长实现，任一处的调整都会让两条链悄悄分叉。
    // WP7-56 把 JS 那份**整份删掉**：这两处现在都只有一份实现，没有第二份状态也没有第二份顺序。
    const FIXED_STEPS_PER_SECOND = 60;
    const FIXED_DELTA = 1 / FIXED_STEPS_PER_SECOND;
    // 追帧上限之后的时间**丢弃**而不是无限补步：切后台/长卡顿回来时游戏时间"跳过"，
    // 而不是补跑上千步把主线程拖死（上限本身由宿主 wasm 持有）。
    let lastFrameTime = null;
    let resetClockNextFrame = false;
    let fixedClockViaWasm = null;
    let droppedSteps = 0;

    // 宿主桥。取不到 `lua_fixed_frame` 说明这份 wasm 是旧产物——**说清楚是哪一边旧**，
    // 而不是静默退回（与 `lua_set_number` / `lua_call_global` 的导出缺席判定同一套口径）。
    function fixedClockBridge() {
      const bridge = (typeof window !== 'undefined' && window.__RUSTLUA_LUA_BRIDGE) || null;
      return bridge && typeof bridge.lua_fixed_frame === 'function' ? bridge : null;
    }

    // 返回本帧要执行的固定步数。**不再保留 JS 累积器兜底**：兜底会把"产物换了一半"降级成
    // "逻辑偶尔慢一拍"，而这正是本项目反复踩的一类症状（与 `setLuaNumber` / `applyInputFrame` /
    // `callLuaHook` 同一口径：导出缺席就说清楚是哪一边旧）。
    // WP7-101（注释澄清，无逻辑改动）：这里**不自己攒累加器** —— 累加器与"追赶上限"都只有宿主一份
    // （`max_catch_up_steps: 8` / `max_frame_seconds: 0.25`，由适配层在建宿主时交给它）。
    // 本函数只做两件事：向宿主问"这一帧本该走几步"，并在必要时把固定步**执行**交给 wasm。
    // 拿不到宿主时钟时**抛错**而不是退回 JS 实现 —— 这就是"归一"在结构上的保证。
    function advanceFixedSteps(elapsedSeconds) {
      const bridge = fixedClockBridge();
      if (!bridge) {
        throw new Error('wasm 缺 lua_fixed_frame 导出：web-runtime.js 比同目录的 wasm 新，'
          + '两者必须来自同一次打包（检查宿主是否缓存了旧运行时，或产物只换了一半）');
      }
      // "跑步"的入口必须同时在：只有步数没有执行等于每帧什么都不发生，而画面上只是"逻辑不动"，
      // 比报错难查得多。
      if (typeof bridge.lua_fixed_run_steps !== 'function') {
        throw new Error('wasm 缺 lua_fixed_run_steps 导出：web-runtime.js 比同目录的 wasm 新，'
          + '两者必须来自同一次打包（固定步的执行顺序自 WP7-56 起只有 wasm 一份实现）');
      }
      if (fixedClockViaWasm === null) {
        fixedClockViaWasm = true;
        if (typeof console !== 'undefined' && console.log) {
          // 前半句是**门断言的原文**（`scripts/wechat-minigame-harness.js` 与计划文档都照它匹配）：
          // 改掉它等于把"固定步的权威在哪一边"这条自证弄丢，所以只往后**追加**新事实。
          console.log('web-runtime: 固定步时钟由宿主 wasm 持有'
            + '（rustlua_infra::frame_clock::FrameClock；固定步的**执行**也在 wasm：'
            + 'lua_bridge::lua_fixed_run_steps——顺序只有这一份实现）');
        }
      }
      // 丢帧计数也读宿主的：读数只认一个来源。
      if (typeof bridge.lua_fixed_steps_dropped === 'function') {
        droppedSteps = bridge.lua_fixed_steps_dropped();
      }
      xcFixed += 1;
      return bridge.lua_fixed_frame(elapsedSeconds);
    }

    // 跑 `steps` 个固定步。顺序（**虚拟键必须在输入批量落地之后**并回来、`__rustlua_dt` 必须写在
    // `update()` 之前、`step == 1` 后吃掉一次性边沿）只有一份实现：`lua_bridge.rs::lua_fixed_run_steps`。
    // 步长传 `FIXED_DELTA`（double 的 1/60）而不是 `FrameClock` 的 f32 步长：脚本看见的 dt 必须
    // 与旧实现逐位一致。
    function runFixedSteps(steps) {
      const bridge = fixedClockBridge();
      if (!bridge) return 0;
      xcFixed += 1;
      return bridge.lua_fixed_run_steps(steps, FIXED_DELTA);
    }

    // 累积器读数同样来自宿主（JS 那份已删）。读数只认一个来源，否则"acc="会变成两个数。
    function fixedAccumulatorReadout() {
      const bridge = fixedClockBridge();
      if (!bridge || typeof bridge.lua_fixed_accumulator_seconds !== 'function') return 'n/a';
      try {
        return bridge.lua_fixed_accumulator_seconds().toFixed(4);
      } catch (error) {
        return 'err';
      }
    }

    // 「谁在推进世界」：引擎宿主（微信侧 `wechat-adapter.js` 的 `engineHostOwnsTick`）接管固定步时，
    // 运行时**不能再跑 update** —— 两边同时跑就是把世界推进两次，而这种故障两边各自都正常、
    // 只有画面/手感不对劲。标记由宿主侧负责设置**与清除**：它起不来或降级时会清掉，
    // 所以"宿主死了 ⇒ 世界冻住（脚本照画、逻辑不动）"这条不会发生。
    function hostOwnsFixedTick() {
      try { return typeof window !== 'undefined' && window.__RUSTLUA_ENGINE_HOST_OWNS_TICK === true; }
      catch (error) { return false; }
    }

    // 宿主这一帧推进了几步（它跑完自己的固定步后写在全局上）。运行时的**帧级**阶段必须按它算 dt：
    // 相机抖动、动画积分、`__rustlua_dt` 都是"每帧一次"的量，用运行时自己的 steps（=0）会停摆。
    function hostFixedSteps() {
      try {
        const value = typeof window !== 'undefined' ? window.__RUSTLUA_ENGINE_HOST_STEPS : 0;
        return Number.isFinite(value) && value > 0 ? Math.min(value, FIXED_STEPS_PER_SECOND) : 0;
      } catch (error) { return 0; }
    }

    // 切后台时不保留时间债：回到前台的第一帧按"一个固定步"起算，而不是把
    // 挂起的几十秒当成一次巨大的 elapsed 灌进累积器。宿主侧的累积器**不动**——
    // 旧实现保留的余量这里也保留，两边的行为逐位等价（因此不调 `lua_fixed_reset_clock`；
    // 那个导出是给宿主/后续 WP8 用的）。
    if (typeof document !== 'undefined' && typeof document.addEventListener === 'function') {
      document.addEventListener('visibilitychange', function () {
        if (document.hidden) resetClockNextFrame = true;
      });
    }

    function frame(timestamp) {
      try {
        // 本帧真实流逝时间。没有时间戳（被宿主手工泵帧）或刚从后台回来时，
        // 按**一个固定步**起算：这条路径必须与"真实 rAF"区分开，否则测试宿主的
        // 零间隔泵帧会把逻辑饿死（累积器永远攒不满一个固定步）。
        let elapsedSeconds;
        const hasTimestamp = typeof timestamp === 'number' && isFinite(timestamp);
        if (!hasTimestamp || lastFrameTime === null || resetClockNextFrame) {
          elapsedSeconds = FIXED_DELTA;
          resetClockNextFrame = false;
        } else {
          elapsedSeconds = Math.max(0, (timestamp - lastFrameTime) / 1000);
        }
        if (hasTimestamp) lastFrameTime = timestamp;
        // 切换在途：只重绘当前画面（表现上"画面不跳"），**不推 update**——新场景数据
        // 尚未就绪时跑 Lua 只会拿到半初始化状态。rAF 链保持不断，切换完成后自动恢复。
        if (sceneSwitch === 'loading') {
          refreshSceneTextHidden(L);
          drawScene(ctx, sceneState, images, projectName, exportUiPayload(L), pendingTextures);
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
          drawScene(ctx, sceneState, images, projectName, exportUiPayload(L), pendingTextures);
          requestAnimationFrame(frame);
          return;
        }
        stepDrive();
        if (typeof window !== 'undefined') window.__frameCounter = (window.__frameCounter || 0) + 1;
        if (driveJhyt && typeof stepDriveJhyt === 'function') stepDriveJhyt();

        // ---- 固定步阶段（游戏逻辑）：与引擎 run_script_steps 的 step pipeline 同构 ----
        // 输入快照每帧只采一次（设备层重新采样），所有固定步共用同一份——引擎也是把
        // 同一个 `InputSnapshot`（clone）交给每一步。
        // WP7-101（注释澄清，无逻辑改动）：`steps` **不是运行时自己算的**。它来自
        // `advanceFixedSteps`（见其定义：缺 `lua_fixed_frame` 就抛错，没有第二份累加器），
        // 也就是**宿主时钟**给出的"这一帧本该走几步"（含丢帧计数与追赶上限 8 步 / 0.25 秒）。
        // 命名上叫 steps 只是历史；读懂它 = 读懂"固定步只有一个时钟、一份实现（WP7-56）"。
        const steps = advanceFixedSteps(elapsedSeconds);
        // 宿主接管固定步时，本帧的模拟步数来自**宿主**（它刚跑完自己的固定步并写在全局上）；
        // 宿主没接管时才等于运行时要跑的步数。相机抖动/帧级 dt 都用 `simSteps`。
        // 注意：时钟**照问**——`lua_fixed_frame` 是"这一帧本该走几步"的权威读数（含丢帧计数），
        // 即使世界由宿主推进，这个读数仍然是运行时要报的东西（`data-rustlua-timing`）。
        const hostTick = hostOwnsFixedTick();
        const simSteps = hostTick ? hostFixedSteps() : steps;
        // camera2d：先按**本帧模拟时间**推进抖动包络，再算相机并回填 Lua，最后才跑剧本 ——
        // 顺序必须是"回填在 update 之前"，否则脚本在 update 里问到的相机是上一帧的。
        if (camera2dState.shakeLeft > 0) {
          camera2dState.shakeLeft = Math.max(0, camera2dState.shakeLeft - FIXED_DELTA * simSteps);
        }
        refreshFrameCamera(sceneState);
        // 只保留**最后一个**固定步的 update 返回值：`data-rustlua-script-result` 报的是"这一刻"的
        // 脚本返回值，不是补步过程中的中间值（与旧 JS 循环同一语义——区别只是现在由
        // `lua_fixed_run_steps` 把最后一个带回来）。
        let result = null;
        // **输入注入的唯一所有者是运行时**（设备层就在这边：键盘/触摸/摇杆都在 JS 侧采样）。
        // 这里每帧采样一次、打包一次、跨界一次；宿主只管跑固定步、**不**注入输入。
        // 「谁注入输入」必须与「谁推进世界」同源，否则会掉进两个坑，而且两个坑都不报错：
        //   · 两边都注入 ⇒ 同一批输入被注入两次（各自看着都正常，只有手感不对）；
        //   · 宿主接管而运行时跳过 ⇒ 输入**根本没被注入**（第 88 轮真的留下了这个 bug：
        //     `hostTick` 时整段跳过 ⇒ 宿主拿到 undefined ⇒ 整局对按键没反应）。
        // `steps == 0` 且非宿主接管时不必注入：这一帧不推进世界，旧行为（高刷屏少一次跨界）不变。
        if (steps > 0 || hostTick) {
          // 快照在这里**打包成二进制并送进 Lua**（一次跨界）：送进去的是"原地改常驻表"的指令，
          // 不是一段要被编译的源码。
          applyInputFrame(webInput.snapshot());
        }
        if (!hostTick && steps > 0) {
          // 每步的三句调用（虚拟键 → `__rustlua_dt` → `update()`，首个固定步后吃一次性边沿）
          // 自 WP7-56 起只有 wasm 一份实现：`lua_bridge.rs::lua_fixed_run_steps`。
          // 这里以前是一段等价的 JS 循环，删掉它才是"归一"，而不是"搬了一份新的"。
          result = runFixedSteps(steps);
        }

        // ---- 帧级阶段：每个 rAF 回调只跑一次，与引擎的 frame pipeline 同构 ----
        // 速度积分与动画推进用的是**本帧实际推进的模拟时间**（simSteps × 固定步长），
        // 与引擎 `let elapsed = delta_seconds * steps as f32` 同一口径；被丢弃的追帧时间
        // 不该让实体多走一截。simSteps==0（帧率高于逻辑步）时按 0 推进，但状态导出照常做，
        // 否则高刷屏上会有帧拿到过期状态。
        setLuaNumber('__rustlua_dt', FIXED_DELTA * simSteps);
        // WP7-59：隐藏文本集现在**跟着状态一起到**（`H` 行），所以帧循环不再无条件拉它——
        // 每帧少一次跨界。只有兼容层自己覆盖了 `__rustlua_export_state`（于是没有 `H` 行）
        // 时才回退到老路；那种情况下 `sceneTextHiddenPulls` 会持续上涨，一眼能看出是谁在走老路。
        sceneTextHiddenFromState = false;
        exportWorldState(L, sceneState, true);
        if (!sceneTextHiddenFromState) refreshSceneTextHidden(L);
        // WP7-66：**先取载荷、再计时**——否则产出端会被算进消费端，两段就分不开了。
        const uiPayloadNow = exportUiPayload(L);
        const drawT0 = DIAGNOSTICS_ON ? exportNowMs() : 0;
        uiElements = drawScene(ctx, sceneState, images, projectName, uiPayloadNow, pendingTextures);
        if (DIAGNOSTICS_ON) recordExportCost('draw', exportNowMs() - drawT0);
        // 本帧新鲜闸门用完即关：下一次 drawScene 必须重新拿到 refreshFrameCamera 算的新值，
        // 否则"复用"就会悄悄变成"画上一帧的相机"。
        frameCameraFresh = false;
        frameCount += 1;
        const body = document.body || document.documentElement;
        if (body) {
          // 固定步读数：`steps` 是本帧实际跑的 update 次数、`dropped` 是累计被丢弃的
          // 追帧步数。刷新率无关性只能靠这两个数在真机上自证——"30Hz 半速 / 120Hz 倍速"
          // 这类回归不会让任何断言变红，却会让手感与动画速度全错（见 docs/32 的时间基一节）。
          // **这一条常驻**：三个数字、无跨边界调用，而"真机自证"要求不开任何开关就能读。
          // WP7-57：再补两个读数——`sim=` 是本帧**真正推进世界**的步数（宿主接管时来自宿主），
          // `tick=` 是谁在推（`host`/`runtime`）。"一个世界只能有一个 tick"因此不开任何开关就能读。
          body.setAttribute('data-rustlua-timing', 'steps=' + steps
            + ' dropped=' + droppedSteps
            + ' acc=' + fixedAccumulatorReadout()
            + ' sim=' + simSteps
            + ' tick=' + (hostTick ? 'host' : 'runtime'));
          // 输入批量读数的**常驻**项，与 timing 同一理由：M73-00 把每帧输入协议整个换成了
          // 二进制批量（旧的"每帧编译一段源码"已删除），"协议真的在落地"必须能在真机上不开
          // 任何开关读到。三个数都是 JS 侧计数器（不额外跨界）：`bits` 恒为 0 就是
          // "宿主与 wasm 对哪些位该变的理解不一致"的第一手线索；`unmapped` 列出落不进键表的
          // 键名——"某个键按了没反应"要能一眼分辨是"宿主没送到"还是"线格式不认这个名"。
          body.setAttribute('data-rustlua-input', inputStatsReadout());
          // WP7-59/60/61：状态回写的读数。和 timing 一样是**纯 JS 计数器**（无跨界、每帧只是拼几个
          // 数字），所以常驻不心疼。各字段各有用处：
          //   `pulls`     隐藏文本集是否还在走老路（WP7-59；正常 0/1）
          //   `state`     状态载荷累计字节数（"要不要换二进制批量"的基线，WP7-60）
          //   `tlines`    真正发出的 T 行数（全量重发 = 帧数 × 存活实体数，WP7-60）
          //   `exports`   状态回写总次数 —— `frame` 是帧循环那次，**每帧至多一次**（WP7-61）
          //   `snapshots` 启动/换场景的只读快照次数（传 false、不推进积分）
          //   `ui`        UI 载荷累计字节数（同一帧里唯一没量过的一块，WP7-61）
          body.setAttribute('data-rustlua-export', 'pulls=' + sceneTextHiddenPulls
            + ' state=' + stateBytes + ' tlines=' + stateTransformLines
            + ' exports=' + stateExports + ' frame=' + stateFrameExports
            + ' snapshots=' + stateSnapshotExports + ' ui=' + uiBytes
            + ' uiP=' + uiPanelRows + ':' + uiPanelBytes
            + ' uiI=' + uiImageRows + ':' + uiImageBytes
            + ' uiB=' + uiBarRows + ':' + uiBarBytes
            + ' uiX=' + uiTextRows + ':' + uiTextBytes
            + ' uiO=' + uiOtherRows + ':' + uiOtherBytes
            + ' uiSame=' + uiSameFrames
            + ' stateUs=' + Math.round(stateExportMs * 1000) + ' stateMaxUs=' + Math.round(stateExportMsMax * 1000)
            + ' uiUs=' + Math.round(uiExportMs * 1000) + ' uiMaxUs=' + Math.round(uiExportMsMax * 1000)
            + ' drawUs=' + Math.round(drawMs * 1000) + ' drawMaxUs=' + Math.round(drawMsMax * 1000)
            + ' slow2ms=s' + stateSlowFrames + '/u' + uiSlowFrames + '/d' + drawSlowFrames
            + ' tileUs=' + Math.round(tileMs * 1000) + ' collectUs=' + Math.round(collectMs * 1000)
            + ' tiles=' + tileCellsDrawn + '/' + tileCellsSeen + ' culled=' + tileCellsCulled
            + ' xlang=' + (xcCallGlobal + xcFixed + xcInput + xcSetNum)
            + ' xc=' + xcCallGlobal + '/' + xcFixed + '/' + xcInput + '/' + xcSetNum
            + ' cgName=' + xcNameText(xcNames)
            + ' snName=' + xcNameText(xsnNames)
            + ' br=' + bridgeProbe());
          // 以下读数只在显式开启诊断时写（见 DIAGNOSTICS_ON）。它们不只是字符串拼接：
          // world 那一行里有**两次跨 wasm 边界的 Lua 调用**（`__rustlua_world_summary`、
          // `__rustlua_tilemap_layers`，各自要回一个字符串），加上 rendererPlayerState 的遍历
          // 与多次 setAttribute。发布包里这些属性没有任何消费方（玩家不看 DOM 属性；微信的
          // 诊断覆盖层是调试工具，它开启时适配层会替我们把诊断打开）——于是这份开销是纯支出。
          if (DIAGNOSTICS_ON) {
            // 世界状态（场景实体数/地图/落点/存活实体数/主角坐标与当前动画）：
            // 宿主诊断覆盖层与无头验收都读它，"游戏在跑但世界是空的"这类问题不必再猜。
            // scene=/drawn= 由 JS 侧补（场景里声明的实体数、渲染器侧的主角位置），其余由 Lua 侧给。
            // tilemaps=画/声明 是**两个独立来源**（渲染循环遍历数 / Lua 注册表数），专门盯
            // "overlay 瓦片层整层没进画面"这一类静默差异（山庄 FarmGrid 种田层，实机 2026-10-04）。
            const drawnPlayer = rendererPlayerState(sceneState);
            const world = 'scene=' + (sceneState.declared || sceneState.entities.size) + ' '
              + String(callGlobal(L, '__rustlua_world_summary') || '')
              + (drawnPlayer ? ' drawn=' + drawnPlayer : '')
              + ' tilemaps=' + tileLayersDrawn + '/' + (String(callGlobal(L, '__rustlua_tilemap_layers') || '0').trim() || '0');
            body.setAttribute('data-rustlua-world', world.slice(0, 300));
            // 键位读数：基础串（来源/工程动作数）启动时算好、之后不变；**当帧**的编辑族计数
            // （bound/rebound/unbound/resets/overrides）与诊断 Lua 通道的结果每帧拼上去。
            // rebind 是运行期发生的、`rustlua-lua-do` 也是运行期执行的 —— 只写一次的话
            // "到底改没改、断言跑没跑"在页面上永远看不出来（第 71 轮实测踩到）。
            // 400 太紧：第 72 轮断言串在 `diag=` 处被截断，证据是**仪器**丢的不是被测对象丢的
            // （同一根串里已经塞下 8 个 API 的返回值）。放宽到 700，仍远小于属性长度的常识上限。
            body.setAttribute('data-rustlua-bindings', ((luaDoStatus === 'none'
              ? bindingReadoutBase
              : bindingReadoutBase + ' lua_do=' + luaDoStatus)
              + ' ' + String(callGlobal(L, '__rustlua_binding_probe') || '')).slice(0, 700));
            // ui-now 每帧写**真实** UI 元素数（诊断/剧本判定用；ui-elements 仍 30 帧一更，
            // 只服务宿主诊断覆盖层的低频采样，剧本靠 ui-now 拿到当帧真值）。
            body.setAttribute('data-rustlua-ui-now', String(uiElements));
            // 预设文字当帧真值（见 LAST_PREFAB_TEXT）：不进 world 串，那条有 300 字符截断。
            body.setAttribute('data-rustlua-prefab-text', LAST_PREFAB_TEXT.drawn + '/' + LAST_PREFAB_TEXT.total);
            body.setAttribute('data-rustlua-prefab-slice', LAST_PREFAB_SLICE.cells + '/' + LAST_PREFAB_SLICE.entities);
    // 具名 UI 文本（脚本用 `ui.text_at`/`ui.text` 画的字）：**这是无头验收唯一能读到**
    // "剧本这一帧说了什么"的通道。它画在 canvas 上，DOM 里没有、宿主 API 也查不到，
    // 于是"剧本跑到第几节、哪一节报错"过去只能靠"崩在哪一行"反推（第 67 轮加）。
    // 文本里的换行已被 Lua 侧转义成 `\n` 两字面字符，所以这里可以安全塞进单行属性值。
    body.setAttribute('data-rustlua-ui-texts', String(uiTexts(L) || '').slice(0, 600));
    // 剧本 `update()` 的**返回值**：`demo_main.update` 返回的正是"本帧所有 API 小节各干了什么"
    // 的聚合串（`demo_main <事实> | <最近一次按键干了什么>`）。它本来就是给开发者看的读数，
    // 但过去只写进页面上的一个 DOM 文本节点、且要显式开调试才写 ⇒ 无头验收看不到。
    // 这里让它进可断言的通道：一次运行就能读出"剧本跑到哪一节、那一节返回了什么"。
    //
    // 只保留**最后一次非空**值：有些帧 `update` 合法地返回 nil（分支没走到收尾），而 CDP 是在
    // 落定后抓**一帧**的 DOM ⇒ 直接写当帧值会让读数时有时无（实测同一产物两次运行一次有、一次空）。
    // 证据通道必须稳定，所以这里记"高水位"，用 `script-result` 属性报出来。
    if (result) lastScriptResult = String(result).slice(0, 1200);
    body.setAttribute('data-rustlua-script-result', lastScriptResult);
            if (frameCount <= 5 || frameCount % 30 === 0) {
              body.setAttribute('data-rustlua-drawn', String(sampleDrawnPixels(ctx)));
              body.setAttribute('data-rustlua-ui-elements', String(uiElements));
              body.setAttribute('data-rustlua-ui-summary', String(LAST_UI_SUMMARY).slice(0, 400));
            }
          }
        }
        // update() 的返回值是开发者读数（无头验收读 data-* 属性，不读这里）。常驻写进
        // DOM 等于在游戏画面上盖一条不随 contain 缩放的调试文字——浏览器发布包里它和
        // HUD 叠在一起，微信侧却没有，就成了"UI 显示不一样"。只在显式开 __RUSTLUA_DEBUG
        // 时才写。
        if (status && result && typeof window !== 'undefined' && window.__RUSTLUA_DEBUG) status.textContent = result;
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
