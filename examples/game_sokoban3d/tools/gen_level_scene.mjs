#!/usr/bin/env node
// 把 ASCII 关卡表编译成引擎的 .scene（scene_version=2）。
// 引擎侧没有"3D 关卡编辑器/AI 建 3D 场景"的通道，所以关卡数据由这张表生成，
// 场景里每个格子是一个带 Transform3D + MeshRenderer（+ Collider3d）的实体。
// 用法：node tools/gen_level_scene.mjs
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// '#'=墙  '.'=目标点  '$'=箱子  '@'=玩家  其余=地面
const LEVEL = [
  '#######',
  '# ... #',
  '# $$$ #',
  '#  @  #',
  '#######',
];

const COLS = LEVEL[0].length;
const ROWS = LEVEL.length;
const CUBE = 'assets/models/sokoban-cube.glb';
const M = {
  wall: 'assets/materials/wall.material',
  goal: 'assets/materials/goal.material',
  crate: 'assets/materials/crate.material',
  player: 'assets/materials/player.material',
};

// 网格坐标 → 世界坐标：关卡以原点为中心，+Y 朝上（引擎的世界 +y 是"上"）。
const worldX = (col) => col - (COLS - 1) / 2;
const worldZ = (row) => row - (ROWS - 1) / 2;
const f = (n) => n.toFixed(9);
const quat = (x, y, z, w) => `${f(x)},${f(y)},${f(z)},${f(w)}`;

const lines = ['scene_version=2', 'name=main'];
const meta = [];
let nextId = 1;
const ids = { walls: [], goals: [], boxes: [], player: null };

function transform(id, x, y, z, sx, sy, sz) {
  lines.push(
    `component\t${id}\tTransform\tposition=${f(x)},${f(y)},${f(z)};rotation=${quat(0, 0, 0, 1)};scale=${f(sx)},${f(sy)},${f(sz)}`,
  );
}

function entity(id, name, parent) {
  lines.push(parent === undefined ? `entity\t${id}\t${name}` : `entity\t${id}\t${name}\t${parent}`);
}

function mesh(id, material) {
  lines.push(`component\t${id}\tMeshRenderer\tmesh=${CUBE};material=${material}`);
}

// ── 相机 / 方向光 ────────────────────────────────────────────────────────────────
const cameraId = nextId++;
entity(cameraId, 'Camera3d');
lines.push(
  `component\t${cameraId}\tCamera\tprojection=perspective;position=0.000000000,8.500000000,7.500000000;target=0.000000000,0.000000000,0.000000000;fov=45.0000;near=0.100000001;far=100.000000000`,
);

const sunId = nextId++;
entity(sunId, 'Sun');
lines.push(
  `component\t${sunId}\tLight\tdirection=0.350000000,-0.900000000,0.264575131;color=1.000000000,1.000000000,1.000000000;intensity=1.000000000`,
);

// ── 地面（引擎唯一的内置图元哨兵） ────────────────────────────────────────────────
const floorId = nextId++;
entity(floorId, 'Floor');
transform(floorId, 0, 0, 0, 1, 1, 1);
lines.push(`component\t${floorId}\tMeshRenderer\tmesh=__ground_plane__`);
lines.push(
  `component\t${floorId}\tCollider3d\tshape=cuboid;half_width=4.000000000;half_height=0.100000000;half_z=4.000000000;dynamic=false`,
);

// ── 关卡格 ───────────────────────────────────────────────────────────────────────
for (let row = 0; row < ROWS; row += 1) {
  for (let col = 0; col < COLS; col += 1) {
    const ch = LEVEL[row][col];
    const x = worldX(col);
    const z = worldZ(row);
    if (ch === '#') {
      const id = nextId++;
      ids.walls.push(id);
      entity(id, `Wall_${col}_${row}`);
      transform(id, x, 0.8, z, 1, 1.6, 1);
      mesh(id, M.wall);
      lines.push(
        `component\t${id}\tCollider3d\tshape=cuboid;half_width=0.500000000;half_height=0.800000000;half_z=0.500000000;dynamic=false`,
      );
    } else if (ch === '.') {
      const id = nextId++;
      ids.goals.push(id);
      entity(id, `Goal_${col}_${row}`);
      transform(id, x, 0.03, z, 0.9, 0.06, 0.9);
      mesh(id, M.goal);
    } else if (ch === '$') {
      const id = nextId++;
      ids.boxes.push(id);
      entity(id, `Crate_${col}_${row}`);
      transform(id, x, 0.5, z, 0.92, 0.92, 0.92);
      mesh(id, M.crate);
      lines.push(
        `component\t${id}\tCollider3d\tshape=cuboid;half_width=0.460000000;half_height=0.460000000;half_z=0.460000000;dynamic=false`,
      );
    } else if (ch === '@') {
      const id = nextId++;
      ids.player = id;
      entity(id, 'Player');
      transform(id, x, 0.6, z, 0.7, 1.2, 0.7);
      mesh(id, M.player);
      lines.push(
        `component\t${id}\tCollider3d\tshape=cuboid;half_width=0.350000000;half_height=0.600000000;half_z=0.350000000;dynamic=false`,
      );
    }
  }
}

// ── IBL 环境 + 骨骼动画样本（3D 能力面：环境贴图与蒙皮剪辑都必须在真工程里可读） ──────
const envId = nextId++;
entity(envId, 'Studio');
lines.push(
  `component\t${envId}\tEnvironment\tmap=assets/env/studio.hdr;intensity=0.600000000;rotation=0.000000000`,
);

const warriorId = nextId++;
entity(warriorId, 'Warrior');
transform(warriorId, 3.4, 0, 2.6, 1, 1, 1);
lines.push(`component\t${warriorId}\tMeshRenderer\tmesh=assets/models/kaykit-warrior.glb`);
lines.push(
  `component\t${warriorId}\tAnimation3D\tmesh=assets/models/kaykit-warrior.glb;clip=Idle;looped=true`,
);

// ── HUD：3D 场景里叠一层 UI Canvas（画布实体固定为 0 号） ─────────────────────────
const canvasId = 0;
entity(canvasId, 'Canvas');
lines.push(
  `component\t${canvasId}\tUiLayout\tanchors=0.500000,0.500000,0.500000,0.500000;offsets=0.00,0.00,0.00,0.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.500000,0.500000`,
);
lines.push(`entity_meta\t${canvasId}\tcanvas.reference\t1280,720`);
lines.push(`entity_meta\t${canvasId}\tcanvas.safe_area\t0,0,0,0`);
lines.push(`entity_meta\t${canvasId}\tcanvas.scale_mode\tfit_both`);
lines.push(`entity_meta\t${canvasId}\tui.root\ttrue`);
const hud = [
  ['HudTitle', '3D Sokoban', 30, '0.950,0.860,0.550', -620, 318],
  ['HudStatus', 'push all crates onto the marks', 18, '0.700,0.820,0.950', -620, 286],
  ['HudPick', 'click a cell to ray-pick it', 16, '0.560,0.660,0.800', -620, 258],
];
for (const [name, text, size, color, ox, oy] of hud) {
  const id = nextId++;
  entity(id, name, canvasId);
  lines.push(
    `component\t${id}\tText\ttext=${text};font_size=${size}.00;color=${color}`,
  );
  lines.push(
    `component\t${id}\tTransform\tposition=0.000000000,0.000000000,0.000000000;rotation=${quat(0, 0, 0, 1)};scale=1.000000000,1.000000000,1.000000000`,
  );
  lines.push(
    `component\t${id}\tUiLayout\tanchors=0.5000,0.5000,0.5000,0.5000;offsets=${ox}.00,${oy}.00,${ox}.00,${oy}.00;min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.0000,0.0000`,
  );
}

// 层级面板的展开顺序：0 号画布排在最后（HUD 盖在 3D 之上）。
const order = [cameraId, sunId, floorId, ...ids.walls, ...ids.goals, ...ids.boxes, ids.player, envId, warriorId, canvasId];
lines.splice(2, 0, `meta\teditor.root_order\t${order.join(',')}`);

const out = path.join(root, 'scenes', 'main.scene');
fs.mkdirSync(path.dirname(out), { recursive: true });
fs.writeFileSync(out, `${lines.join('\n')}\n`);
console.log(
  `OK ${out} entities=${order.length} walls=${ids.walls.length} goals=${ids.goals.length} boxes=${ids.boxes.length}`,
);
