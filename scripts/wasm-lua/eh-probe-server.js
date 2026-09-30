// eh-probe-server.js —— wasm 新 EH（try_table/exnref）跨浏览器探针的宿主服务器。
//
// 用法：node scripts/wasm-lua/eh-probe-server.js <bundle-root> <port> <probe-html> <out-json>
//   <bundle-root>  已发布的 webgl 产物目录（含 pkg/game_web_bg.wasm）；作为静态根。
//   <port>         监听端口（127.0.0.1）。
//   <probe-html>   eh-probe.html 的路径（从 /__eh_probe__ 提供）。
//   <out-json>     每收到一份结果就把「按 tag 汇总的结果数组」写到这里，供 PS runner 轮询。
//
// 为什么用 POST 回传而不是 --dump-dom：--dump-dom 是 Chromium 专有，Firefox/Safari 没有。
// 让探针页把结果 POST 回本服务器，则**任何**能跑 fetch 的浏览器都能被统一采集（Safari 手动
// `open -a Safari http://127.0.0.1:<port>/__eh_probe__?tag=safari` 也走同一条路）。
//
// 退出：收到 SIGTERM/SIGINT 退出；每收到一份结果打印 `EH_RESULT <tag> <verdict>` 到 stdout，
// 便于 runner 从子进程日志里实时判定。

const http = require("http");
const fs = require("fs");
const path = require("path");

const root = path.resolve(process.argv[2] || ".");
const port = Number(process.argv[3] || 8150);
const probeHtml = path.resolve(process.argv[4] || path.join(__dirname, "eh-probe.html"));
const outJson = process.argv[5] ? path.resolve(process.argv[5]) : "";

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

// tag -> result；按 tag 覆盖（同一浏览器重试以最后一次为准）。
const results = new Map();

function flush() {
  if (!outJson) { return; }
  const arr = Array.from(results.values());
  try {
    fs.mkdirSync(path.dirname(outJson), { recursive: true });
    fs.writeFileSync(outJson, JSON.stringify(arr, null, 2), "utf8");
  } catch (e) {
    console.error("EH_PROBE_SERVER write out-json failed:", e.message);
  }
}

function sendFile(response, file, type) {
  fs.stat(file, (error, stats) => {
    if (error || !stats.isFile()) {
      response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
      response.end("not found: " + file);
      return;
    }
    response.writeHead(200, { "content-type": type, "cache-control": "no-store" });
    fs.createReadStream(file).pipe(response);
  });
}

const server = http.createServer((request, response) => {
  const url = new URL(request.url, `http://127.0.0.1:${port}`);
  const pathname = decodeURIComponent(url.pathname);

  // 结果回传端点。
  if (pathname === "/__eh_result__" && request.method === "POST") {
    let body = "";
    request.on("data", (chunk) => { body += chunk; if (body.length > 1e6) { request.destroy(); } });
    request.on("end", () => {
      let parsed = null;
      try { parsed = JSON.parse(body); } catch (e) { /* 保留原始文本 */ }
      const tag = (parsed && parsed.tag) || url.searchParams.get("tag") || "unknown";
      const verdict = (parsed && parsed.verdict) || "MALFORMED";
      const record = parsed || { tag: tag, verdict: verdict, raw: body.slice(0, 2000) };
      record.receivedAt = new Date().toISOString();
      results.set(tag, record);
      flush();
      console.log(`EH_RESULT ${tag} ${verdict}`);
      response.writeHead(200, { "content-type": "text/plain; charset=utf-8", "access-control-allow-origin": "*" });
      response.end("ok");
    });
    return;
  }
  // 允许预检（个别引擎对跨源 POST 会先发 OPTIONS；同源一般不需要，但留着无害）。
  if (pathname === "/__eh_result__" && request.method === "OPTIONS") {
    response.writeHead(204, {
      "access-control-allow-origin": "*",
      "access-control-allow-methods": "POST, OPTIONS",
      "access-control-allow-headers": "content-type",
    });
    response.end();
    return;
  }

  // 探针页。
  if (pathname === "/__eh_probe__" || pathname === "/__eh_probe__/") {
    sendFile(response, probeHtml, MIME[".html"]);
    return;
  }

  // 静态文件（服务 bundle-root，防路径穿越）。
  const target = path.resolve(root, "." + pathname);
  if (!target.startsWith(root)) {
    response.writeHead(403, { "content-type": "text/plain; charset=utf-8" });
    response.end("forbidden");
    return;
  }
  fs.stat(target, (error, stats) => {
    if (!error && stats.isDirectory()) {
      sendFile(response, path.join(target, "index.html"), MIME[".html"]);
      return;
    }
    if (error || !stats.isFile()) {
      response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
      response.end("not found");
      return;
    }
    sendFile(response, target, MIME[path.extname(target).toLowerCase()] || "application/octet-stream");
  });
});

server.listen(port, "127.0.0.1", () => {
  console.log(`serving http://127.0.0.1:${port} root=${root}`);
  console.log(`probe   http://127.0.0.1:${port}/__eh_probe__?tag=<tag>&wasm=/pkg/game_web_bg.wasm`);
});

const shutdown = () => {
  server.close(() => process.exit(0));
  setTimeout(() => process.exit(0), 500).unref();
};
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
