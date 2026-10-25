#!/usr/bin/env node
// 生成推箱子用的 3D 立方体 glTF 二进制（.glb）。
// 引擎没有内置基本图元（只有 `__ground_plane__` 哨兵），也没有 MCP 建模工具，
// 所以网格只能由外部产出后走 ModelImporter 导入。
// 用法：node tools/gen_cube_glb.mjs
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const out = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
  'assets',
  'models',
  'sokoban-cube.glb',
);

// 六个面，每面 4 个独立顶点（法线不共享）。
const faces = [
  { n: [0, 0, 1], v: [[-0.5, -0.5, 0.5], [0.5, -0.5, 0.5], [0.5, 0.5, 0.5], [-0.5, 0.5, 0.5]] },
  { n: [0, 0, -1], v: [[0.5, -0.5, -0.5], [-0.5, -0.5, -0.5], [-0.5, 0.5, -0.5], [0.5, 0.5, -0.5]] },
  { n: [1, 0, 0], v: [[0.5, -0.5, 0.5], [0.5, -0.5, -0.5], [0.5, 0.5, -0.5], [0.5, 0.5, 0.5]] },
  { n: [-1, 0, 0], v: [[-0.5, -0.5, -0.5], [-0.5, -0.5, 0.5], [-0.5, 0.5, 0.5], [-0.5, 0.5, -0.5]] },
  { n: [0, 1, 0], v: [[-0.5, 0.5, 0.5], [0.5, 0.5, 0.5], [0.5, 0.5, -0.5], [-0.5, 0.5, -0.5]] },
  { n: [0, -1, 0], v: [[-0.5, -0.5, -0.5], [0.5, -0.5, -0.5], [0.5, -0.5, 0.5], [-0.5, -0.5, 0.5]] },
];

const positions = [];
const normals = [];
const texcoords = [];
const indices = [];
faces.forEach((face, f) => {
  const base = f * 4;
  face.v.forEach((p, i) => {
    positions.push(p[0], p[1], p[2]);
    normals.push(face.n[0], face.n[1], face.n[2]);
    texcoords.push(i === 1 || i === 2 ? 1 : 0, i >= 2 ? 1 : 0);
  });
  indices.push(base, base + 1, base + 2, base, base + 2, base + 3);
});

const min = [Infinity, Infinity, Infinity];
const max = [-Infinity, -Infinity, -Infinity];
for (let i = 0; i < positions.length; i += 3) {
  for (let k = 0; k < 3; k += 1) {
    min[k] = Math.min(min[k], positions[i + k]);
    max[k] = Math.max(max[k], positions[i + k]);
  }
}

const positionBytes = new Float32Array(positions).buffer;
const normalBytes = new Float32Array(normals).buffer;
const uvBytes = new Float32Array(texcoords).buffer;
const indexBytes = new Uint16Array(indices).buffer;
const pad4 = (n) => (n + 3) & ~3;
const binLength = pad4(positionBytes.byteLength) + pad4(normalBytes.byteLength) + pad4(uvBytes.byteLength) + indexBytes.byteLength;
const bin = Buffer.alloc(binLength, 0);
let cursor = 0;
for (const source of [positionBytes, normalBytes, uvBytes, indexBytes]) {
  Buffer.from(source).copy(bin, cursor);
  cursor += pad4(source.byteLength);
}
const offsetPosition = 0;
const offsetNormal = pad4(positionBytes.byteLength);
const offsetTexcoord = offsetNormal + pad4(normalBytes.byteLength);
const offsetIndex = offsetTexcoord + pad4(uvBytes.byteLength);

const json = {
  asset: { version: '2.0', generator: 'rustlua-sokoban3d/tools/gen_cube_glb.mjs' },
  scene: 0,
  scenes: [{ nodes: [0] }],
  nodes: [{ mesh: 0, name: 'Cube' }],
  meshes: [{ name: 'Cube', primitives: [{ attributes: { POSITION: 0, NORMAL: 1, TEXCOORD_0: 2 }, indices: 3, material: 0 }] }],
  materials: [
    {
      name: 'CubeBase',
      pbrMetallicRoughness: { baseColorFactor: [0.82, 0.55, 0.28, 1], metallicFactor: 0, roughnessFactor: 0.75 },
    },
  ],
  accessors: [
    { bufferView: 0, componentType: 5126, count: positions.length / 3, type: 'VEC3', min, max },
    { bufferView: 1, componentType: 5126, count: normals.length / 3, type: 'VEC3' },
    { bufferView: 2, componentType: 5126, count: texcoords.length / 2, type: 'VEC2' },
    { bufferView: 3, componentType: 5123, count: indices.length, type: 'SCALAR' },
  ],
  bufferViews: [
    { buffer: 0, byteOffset: offsetPosition, byteLength: positionBytes.byteLength, target: 34962 },
    { buffer: 0, byteOffset: offsetNormal, byteLength: normalBytes.byteLength, target: 34962 },
    { buffer: 0, byteOffset: offsetTexcoord, byteLength: uvBytes.byteLength, target: 34962 },
    { buffer: 0, byteOffset: offsetIndex, byteLength: indexBytes.byteLength, target: 34963 },
  ],
  buffers: [{ byteLength: binLength }],
};

const jsonBytes = Buffer.from(JSON.stringify(json), 'utf8');
const jsonPadded = Buffer.alloc(pad4(jsonBytes.byteLength), 0x20);
jsonBytes.copy(jsonPadded);

const total = 12 + 8 + jsonPadded.length + 8 + bin.length;
const header = Buffer.alloc(12);
header.writeUInt32LE(0x46546c67, 0);
header.writeUInt32LE(2, 4);
header.writeUInt32LE(total, 8);
const jsonHead = Buffer.alloc(8);
jsonHead.writeUInt32LE(jsonPadded.length, 0);
jsonHead.writeUInt32LE(0x4e4f534a, 4);
const binHead = Buffer.alloc(8);
binHead.writeUInt32LE(bin.length, 0);
binHead.writeUInt32LE(0x004e4942, 4);

fs.mkdirSync(path.dirname(out), { recursive: true });
fs.writeFileSync(out, Buffer.concat([header, jsonHead, jsonPadded, binHead, bin]));
console.log(`OK ${out} bytes=${fs.statSync(out).size} verts=${positions.length / 3} tris=${indices.length / 3}`);
