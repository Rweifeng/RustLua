// serve-static.js（M20-04）：零依赖静态文件服务器，供浏览器集成回归使用。
//
// 用法：node scripts/dev/serve-static.js <root-dir> <port> [host]
// 启动后打印 `serving http://<host>:<port>`，收到 SIGTERM/ctrl-c 退出。
// host 省略时绑 127.0.0.1（既有行为逐字不变）；**真机验收**才需要显式给 `0.0.0.0`，
// 让同一局域网里的手机/平板访问——绑定回环时手机连不上，这不是产物的问题。
//
// 2026-10-17（真机验收的一轮实测）：发布包是 1,300+ 个小文件、**串行** fetch 的，
// 手机经 WiFi 每请求 30–50 ms 的往返叠起来就是几十秒的闪屏。这里补两件**只属于测试服务**的
// 事，不改产物、不改运行期契约：
//   * `gzip`：文本类资产（.lua/.scene/.toml/.json/.txt/.js/.html/.wasm）压后再发，
//     命中时多一条 `content-encoding: gzip`（浏览器自己解，`fetch` 侧无感）。
//   * 可缓存：资产给短 max-age + ETag，重复进入同一页面不必重下几 MB；入口文件
//     （HTML/manifest/project.toml）保持 `no-store`，否则改了产物刷新看不到新东西——
//     这条比"二次打开更快"重要。
//
// 只服务 root 内的文件（拒绝路径穿越），并显式给出 .wasm/.js 的 MIME，
// 保证浏览器 `WebAssembly.instantiateStreaming` 等路径可用。

const http = require("http");
const fs = require("fs");
const path = require("path");
const zlib = require("zlib");

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

// 值得再压一遍的后缀（白名单而不是黑名单：新增后缀时默认走原文，宁可少压不可乱压）。
// 图片/音频/字体（png/jpg/mp3/ogg/wav/ttf）本身已是压缩格式，再套 gzip 只烧 CPU 不省字节。
const COMPRESSIBLE = new Set([
  ".html", ".js", ".mjs", ".json", ".wasm", ".txt", ".toml", ".scene", ".lua",
]);

// 小于这个体积的响应不值得压：gzip 头 + 分块开销会吃掉收益，而 1,300 个小文件里
// 多数是几十字节的 Lua 片段。
const COMPRESS_MIN_BYTES = 1024;

// 缓存策略：**变了就一定要重新取**的入口文件保持 no-store；其余给短缓存 + ETag
// （命中 If-None-Match 给 304，比整份重传便宜得多）。
// 这里刻意**不用** `immutable` + 一年：真机验收的循环是"改产物 → 手机刷新"，
// 把同一路径钉成一年会让那一步静默读到旧字节。
const ALWAYS_FRESH = new Set(["index.html", "manifest.txt", "project.toml", "dependencies.json"]);

// 命中 gzip 的次数：启动行里报一次读数，让人（和排查脚本）不必靠猜"压缩到底生效没有"。
// 为什么还要周期性打印：真机验收时服务是被强杀的（Windows 上 CTRL-C/SIGTERM 不保证跑
// 退出钩子），只写在 shutdown 里等于永远读不到——这条读数必须**在跑的时候**就能看见。
let gzippedCount = 0;
let brotliCount = 0;
const GZIP_REPORT_EVERY = 250;

/** gzip 一次并给体（小文件与不可压类型返回 null，调用方走原文）。 */
function maybeGzip(buffer, extension, acceptsGzip) {
  if (!acceptsGzip) return null;
  if (!COMPRESSIBLE.has(extension)) return null;
  if (buffer.length < COMPRESS_MIN_BYTES) return null;
  return zlib.gzipSync(buffer);
}

/// brotli q9：比 gzip 再小 ~15%（5.16 MB 的 wasm 实测 gzip 2.07 MB / br 1.76 MB），
/// 耗时 0.66 s 对 0.16 s——所以**必须配缓存**，否则每次请求都重压 5 MB。
///
/// ⚠️ 它只对**安全上下文**生效：浏览器在 `http://` 下只宣告 `gzip, deflate`（实测 Edge 对
/// 局域网 HTTP 地址就是这样），brotli/zstd 要 HTTPS 或 localhost。所以"手机过 WiFi 联调"
/// 这条路拿不到它，别把它的收益算进真机首屏账里。
function maybeBrotli(buffer, extension, acceptsBrotli) {
  if (!acceptsBrotli) return null;
  if (!COMPRESSIBLE.has(extension)) return null;
  if (buffer.length < COMPRESS_MIN_BYTES) return null;
  return zlib.brotliCompressSync(buffer, {
    params: {
      [zlib.constants.BROTLI_PARAM_QUALITY]: 9,
      [zlib.constants.BROTLI_PARAM_SIZE_HINT]: buffer.length,
    },
  });
}

// 压缩结果缓存：key = "编码|路径|mtimeMs|size"。产物一次运行内不变，重压纯属浪费
// （brotli q9 对 5 MB 要 0.6 s，每次刷新都付一遍会让人以为"服务卡了"）。
const compressedCache = new Map();
const COMPRESSED_CACHE_MAX = 32;
function cacheGet(key) {
  const hit = compressedCache.get(key);
  if (hit) {
    // 命中即提到末尾（简易 LRU）。
    compressedCache.delete(key);
    compressedCache.set(key, hit);
  }
  return hit;
}
function cacheSet(key, value) {
  if (compressedCache.size >= COMPRESSED_CACHE_MAX) {
    compressedCache.delete(compressedCache.keys().next().value);
  }
  compressedCache.set(key, value);
}

/** 依据体积与 mtime 造 ETag（本地产物够用：同字节必然同 mtime 组合的等价类很小）。 */
function etagFor(stats) {
  return `W/"${stats.size.toString(16)}-${Math.round(stats.mtimeMs).toString(16)}"`;
}

/** 发一个文件（含 304 / gzip / 流式三条路径）。`fileStats` 必为普通文件。 */
function sendFile(request, response, filePath, fileStats) {
  const base = path.basename(filePath);
  const extension = path.extname(filePath).toLowerCase();
  const etag = etagFor(fileStats);
  const headers = {
    "content-type": MIME[extension] || "application/octet-stream",
    etag,
    "cache-control": ALWAYS_FRESH.has(base) ? "no-store" : "public, max-age=3600",
    vary: "accept-encoding",
  };
  if (!ALWAYS_FRESH.has(base) && request.headers["if-none-match"] === etag) {
    response.writeHead(304, headers).end();
    return;
  }
  const acceptEncoding = String(request.headers["accept-encoding"] || "");
  const acceptsGzip = /\bgzip\b/.test(acceptEncoding);
  const acceptsBrotli = /\bbr\b/.test(acceptEncoding);
  if (COMPRESSIBLE.has(extension) && (acceptsBrotli || acceptsGzip) && request.method !== "HEAD") {
    // 要压就必须先有整个体，所以这一支读进内存；其余类型沿用既有的流式发送。
    fs.readFile(filePath, (readError, buffer) => {
      if (readError) {
        response.writeHead(404).end("not found");
        return;
      }
      // **优先 brotli**（同样内容再小 15%），拿不到再退 gzip；两者都没有就发原文。
      const encoding = acceptsBrotli ? "br" : "gzip";
      const cacheKey = `${encoding}|${filePath}|${Math.round(fileStats.mtimeMs)}|${buffer.length}`;
      let compressed = cacheGet(cacheKey);
      if (!compressed) {
        compressed =
          encoding === "br"
            ? maybeBrotli(buffer, extension, true)
            : maybeGzip(buffer, extension, true);
        if (compressed) cacheSet(cacheKey, compressed);
      }
      if (!compressed) {
        response.writeHead(200, { ...headers, "content-length": buffer.length }).end(buffer);
        return;
      }
      if (encoding === "br") {
        brotliCount += 1;
      } else {
        gzippedCount += 1;
      }
      if ((gzippedCount + brotliCount) % GZIP_REPORT_EVERY === 0) {
        console.log(`compressed responses: gzip=${gzippedCount} br=${brotliCount}`);
      }
      response
        .writeHead(200, {
          ...headers,
          "content-encoding": encoding,
          "content-length": compressed.length,
        })
        .end(compressed);
    });
    return;
  }
  response.writeHead(200, headers);
  if (request.method === "HEAD") {
    response.end();
    return;
  }
  fs.createReadStream(filePath).pipe(response);
}

function handler(request, response) {
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
        sendFile(request, response, indexTarget, indexStats);
      });
      return;
    }
    if (error || !stats.isFile()) {
      response.writeHead(404).end("not found");
      return;
    }
    sendFile(request, response, target, stats);
  });
}

// HTTPS 模式（`RUSTLUA_TLS_PFX` 指向 scripts/dev/https-cert.ps1 出的自签名 PFX）：
// Chromium 只在**安全上下文**才在 `Accept-Encoding` 里宣告 `br`——局域网 HTTP 实测只报
// `gzip, deflate`，于是 5.16 MB 的 wasm 只能压到 2.07 MB；改走 HTTPS 后同一份产物是 1.76 MB。
// PFX 直接交给 Node（`pfx` + `passphrase`），不用先转 PEM。
const tlsPfx = process.env.RUSTLUA_TLS_PFX;
const tlsPass = process.env.RUSTLUA_TLS_PASS || "";
const scheme = tlsPfx ? "https" : "http";
const server = tlsPfx
  ? require("https").createServer({ pfx: fs.readFileSync(tlsPfx), passphrase: tlsPass }, handler)
  : http.createServer(handler);

server.listen(port, host, () => {
  console.log(`serving ${scheme}://${host}:${port} root=${root}`);
  if (tlsPfx) {
    console.log("  TLS: 自签名证书 ⇒ 浏览器/手机会拦一次，点「高级 → 继续前往」即可；");
    console.log("       续走 HTTPS 后浏览器才会宣告 brotli（wasm 2.07 MB → 1.76 MB）。");
  }
  if (host !== "127.0.0.1" && host !== "localhost") {
    // 真机侧要的就是这一行：手机该打开哪个地址由它给，别让人从 ipconfig 里猜。
    for (const list of Object.values(require("os").networkInterfaces())) {
      for (const item of list || []) {
        if (item.family === "IPv4" && !item.internal) {
          console.log(`  LAN: ${scheme}://${item.address}:${port}/`);
        }
      }
    }
  }
});

const shutdown = () => {
  // 收尾时把 gzip 命中数打出来：一次真机验收后能直接回答"省的是网络还是本地磁盘缓存"。
  console.log(`compressed responses: gzip=${gzippedCount} br=${brotliCount}`);
  server.close(() => process.exit(0));
  setTimeout(() => process.exit(0), 500).unref();
};
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
