#!/usr/bin/env node
// 本地静态资源服务：作为远程 CDN 的**测试替身**，用来在开发者工具里验证
// "包内读不到 → 远端下载 → 落地缓存 → 后续命中缓存" 这条路。
//
// 能证明什么：HTTP 请求/下载、缓存写入与读取、字节与清单校验、失败重试与回落顺序。
// 不能证明什么：CDN 行为（缓存/回源/限速）、真机 HTTPS 与 mp 后台 downloadFile 合法域名白名单。
// 开发者工具需要勾选「不校验合法域名、TLS 版本以及 HTTPS 证书」；手机访问不到本机的 127.0.0.1。
//
// 用法（也可被 harness 当库复用：createPackServer）：
//   node scripts/pack-server.js --root <发布目录> [--port 8787] [--host 127.0.0.1] [--log <文件>]

const fs = require('fs');
const http = require('http');
const path = require('path');

const MIME = {
  '.js': 'text/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.txt': 'text/plain; charset=utf-8',
  '.toml': 'text/plain; charset=utf-8',
  '.lua': 'text/plain; charset=utf-8',
  '.scene': 'text/plain; charset=utf-8',
  '.md': 'text/markdown; charset=utf-8',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
  '.wav': 'audio/wav',
  '.ogg': 'audio/ogg',
  '.mp3': 'audio/mpeg',
  '.ttf': 'font/ttf',
  '.otf': 'font/otf',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.wasm': 'application/wasm',
  '.bin': 'application/octet-stream'
};

// 把 URL 路径解析成 root 内的绝对路径；越界（.. / 绝对路径逃逸）一律拒绝。
function resolveWithin(root, urlPath) {
  let decoded = urlPath;
  try { decoded = decodeURIComponent(urlPath); } catch { return null; }
  const rel = decoded.replace(/^\/+/, '');
  const abs = path.resolve(root, rel);
  const rootWithSep = path.resolve(root) + path.sep;
  if (abs !== path.resolve(root) && !abs.startsWith(rootWithSep)) return null;
  return abs;
}

// options: { root, log(可选，写入请求日志的函数), transform(可选，(relPath, buffer) => buffer，用于反向测试),
//            failFor(可选，(relPath) => string|null：返回非空则按 500 应答，用于模拟"局部 CDN 故障"——
//            例如只有图片 500：文本资源照常、贴图必然加载失败，用来验证屏幕会不会自证） }
function createPackServer(options) {
  const root = path.resolve(options.root);
  const transform = options.transform || null;
  const failFor = options.failFor || null;
  const requests = [];

  const server = http.createServer((req, res) => {
    const urlPath = String(req.url || '/').split('?')[0].split('#')[0];
    const abs = resolveWithin(root, urlPath);
    const rel = abs ? path.relative(root, abs).replace(/\\/g, '/') : urlPath.replace(/^\/+/, '');
    let status = 200;
    let body = null;
    const simulated = (abs && fs.existsSync(abs) && fs.statSync(abs).isFile() && failFor) ? failFor(rel) : null;
    if (!abs || !fs.existsSync(abs) || !fs.statSync(abs).isFile()) {
      status = 404;
      body = Buffer.from(`not found: ${rel}\n`, 'utf8');
    } else if (simulated) {
      status = 500;
      body = Buffer.from(`simulated cdn error: ${simulated}\n`, 'utf8');
    } else {
      body = fs.readFileSync(abs);
      if (transform) body = transform(rel, body);
    }
    requests.push({ rel, status, bytes: body.length });
    if (options.log) options.log(`${req.method} /${rel} ${status} ${body.length}`);
    res.writeHead(status, {
      'Content-Type': MIME[path.extname(abs || rel).toLowerCase()] || 'application/octet-stream',
      'Content-Length': body.length,
      'Cache-Control': 'no-store'
    });
    res.end(body);
  });

  return {
    server,
    root,
    requests,
    listen(port, host) {
      return new Promise((resolve, reject) => {
        server.once('error', reject);
        server.listen(port, host, () => resolve(server.address()));
      });
    },
    close() {
      return new Promise((resolve) => server.close(() => resolve()));
    }
  };
}

function parseArgs(argv) {
  const args = { root: '.', port: 8787, host: '127.0.0.1', log: null };
  for (let i = 0; i < argv.length; i++) {
    const key = argv[i];
    if (key === '--root' && argv[i + 1]) args.root = argv[++i];
    else if (key === '--port' && argv[i + 1]) args.port = Number(argv[++i]);
    else if (key === '--host' && argv[i + 1]) args.host = argv[++i];
    else if (key === '--log' && argv[i + 1]) args.log = argv[++i];
    else return { error: `未知参数: ${key}` };
  }
  if (!Number.isInteger(args.port) || args.port <= 0 || args.port > 65535) return { error: `端口不合法: ${args.port}` };
  if (!fs.existsSync(args.root) || !fs.statSync(args.root).isDirectory()) return { error: `发布目录不存在: ${args.root}` };
  return { args };
}

async function main(argv) {
  const parsed = parseArgs(argv);
  if (parsed.error) {
    console.error(`pack_server_failed ${parsed.error}`);
    process.exit(2);
  }
  const { args } = parsed;
  const root = path.resolve(args.root);
  const logLine = (line) => {
    const stamped = `${new Date().toISOString()} ${line}`;
    console.log(stamped);
    if (args.log) {
      try { fs.appendFileSync(args.log, stamped + '\n'); } catch { /* 日志失败不影响服务 */ }
    }
  };

  const pack = createPackServer({ root, log: logLine });
  try {
    const address = await pack.listen(args.port, args.host);
    console.log(`pack_server_ok url=http://${args.host}:${address.port}/ root=${root}`);
    if (fs.existsSync(path.join(root, 'pack'))) {
      console.log(`==> 小游戏远端资源配置: ${JSON.stringify({ packBaseUrl: `http://${args.host}:${address.port}/pack/` })}`);
    }
  } catch (error) {
    const message = String((error && error.code) || (error && error.message) || error);
    console.error(`pack_server_failed ${message}${error && error.code === 'EADDRINUSE' ? ` port=${args.port}（换一个端口或先停掉占用进程）` : ''}`);
    process.exit(1);
  }
  process.on('SIGINT', async () => { await pack.close(); process.exit(0); });
  process.on('SIGTERM', async () => { await pack.close(); process.exit(0); });
}

module.exports = { createPackServer, resolveWithin, MIME };

if (require.main === module) {
  main(process.argv.slice(2));
}
