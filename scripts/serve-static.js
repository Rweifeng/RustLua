// serve-static.js（M20-04）：零依赖静态文件服务器，供浏览器集成回归使用。
//
// 用法：node scripts/serve-static.js <root-dir> <port>
// 启动后打印 `serving http://127.0.0.1:<port>`，收到 SIGTERM/ctrl-c 退出。
//
// 只服务 root 内的文件（拒绝路径穿越），并显式给出 .wasm/.js 的 MIME，
// 保证浏览器 `WebAssembly.instantiateStreaming` 等路径可用。

const http = require("http");
const fs = require("fs");
const path = require("path");

const root = path.resolve(process.argv[2] || ".");
const port = Number(process.argv[3] || 8123);

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

server.listen(port, "127.0.0.1", () => {
  console.log(`serving http://127.0.0.1:${port} root=${root}`);
});

const shutdown = () => {
  server.close(() => process.exit(0));
  setTimeout(() => process.exit(0), 500).unref();
};
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
