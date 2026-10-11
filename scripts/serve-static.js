// serve-static.js（M20-04）：零依赖静态文件服务器，供浏览器集成回归使用。
//
// 用法：node scripts/serve-static.js <root-dir> <port> [host]
// 启动后打印 `serving http://<host>:<port>`，收到 SIGTERM/ctrl-c 退出。
// host 省略时绑 127.0.0.1（既有行为逐字不变）；**真机验收**才需要显式给 `0.0.0.0`，
// 让同一局域网里的手机/平板访问——绑定回环时手机连不上，这不是产物的问题。
//
// 只服务 root 内的文件（拒绝路径穿越），并显式给出 .wasm/.js 的 MIME，
// 保证浏览器 `WebAssembly.instantiateStreaming` 等路径可用。

const http = require("http");
const fs = require("fs");
const path = require("path");

const root = path.resolve(process.argv[2] || ".");
const port = Number(process.argv[3] || 8123);
// M73-00 实机验收：默认仍然只绑回环（门禁脚本行为不变）；`0.0.0.0` 是给真机开的口子。
const host = process.argv[4] || "127.0.0.1";

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".wasm": "application/wasm",
  ".txt": "text/plain; charset=utf-8",
  ".toml": "text/plain; charset=utf-8",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".wav": "audio/wav",
  ".ogg": "audio/ogg",
  ".mp3": "audio/mpeg",
  ".ttf": "font/ttf",
  ".scene": "text/plain; charset=utf-8",
  ".lua": "text/plain; charset=utf-8",
};

const server = http.createServer((request, response) => {
  const url = new URL(request.url, `http://127.0.0.1:${port}`);
  const requested = decodeURIComponent(url.pathname);
  const target = path.resolve(root, "." + requested);
  if (!target.startsWith(root)) {
    response.writeHead(403).end("forbidden");
    return;
  }
  fs.stat(target, (error, stats) => {
    if (!error && stats.isDirectory()) {
      const indexTarget = path.join(target, "index.html");
      fs.stat(indexTarget, (indexError, indexStats) => {
        if (indexError || !indexStats.isFile()) {
          response.writeHead(404).end("not found");
          return;
        }
        response.writeHead(200, {
          "content-type": MIME[".html"],
          "cache-control": "no-store",
        });
        fs.createReadStream(indexTarget).pipe(response);
      });
      return;
    }
    if (error || !stats.isFile()) {
      response.writeHead(404).end("not found");
      return;
    }
    response.writeHead(200, {
      "content-type": MIME[path.extname(target).toLowerCase()] || "application/octet-stream",
      "cache-control": "no-store",
    });
    fs.createReadStream(target).pipe(response);
  });
});

server.listen(port, host, () => {
  console.log(`serving http://${host}:${port} root=${root}`);
  if (host !== "127.0.0.1" && host !== "localhost") {
    // 真机侧要的就是这一行：手机该打开哪个地址由它给，别让人从 ipconfig 里猜。
    for (const list of Object.values(require("os").networkInterfaces())) {
      for (const item of list || []) {
        if (item.family === "IPv4" && !item.internal) {
          console.log(`  LAN: http://${item.address}:${port}/`);
        }
      }
    }
  }
});

const shutdown = () => {
  server.close(() => process.exit(0));
  setTimeout(() => process.exit(0), 500).unref();
};
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
