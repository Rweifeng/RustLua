// web-check.js（M20-04）：浏览器侧集成契约检查。
//
// 在真实浏览器里验证 WebGL 集成所依赖的平台契约（与 game_web 的 WASM 入口
// 使用同一套资源路径与诊断约定）：
//   1. 发布包资源可被浏览器 fetch：pack/manifest.txt、pack/project.toml、
//      project.toml 声明的 startup_scene、manifest 首个资源；
//   2. WebGL2 上下文可用（浏览器渲染能力探测）；
//   3. 存储可用性：localStorage 不可用（隐私模式/被禁用）时降级到内存并标记，
//      不抛错；
//   4. 资源缺失诊断：请求一个不存在的资源必须得到可诊断的失败（404），
//      而不是静默成功。
//
// 结果写入 DOM：#result[data-status] = ok|fail，#details 为 JSON，
// 供 `msedge --headless --dump-dom` 断言。

(function () {
  const details = {};
  const result = document.getElementById("result");
  const detailsNode = document.getElementById("details");

  function finish(status, payload) {
    details.status = status;
    if (payload) {
      Object.assign(details, payload);
    }
    result.dataset.status = status;
    result.textContent = status;
    detailsNode.textContent = JSON.stringify(details, null, 2);
  }

  async function fetchText(relative) {
    const response = await fetch(relative, { cache: "no-store" });
    if (!response.ok) {
      throw new Error(`fetch ${relative} failed: ${response.status}`);
    }
    const text = await response.text();
    if (!text.trim()) {
      throw new Error(`fetch ${relative} returned empty body`);
    }
    return text;
  }

  function projectKey(text, key) {
    const prefix = `${key}`;
    const line = text
      .split(/\r?\n/)
      .map((value) => value.replace(/^\uFEFF/, "").trim())
      .find((value) => value.startsWith(`${prefix} =`) || value.startsWith(`${prefix}=`));
    if (!line) {
      return null;
    }
    // 去掉 `key`、可选的空格与 `=`，再剥离首尾引号。
    const raw = line.slice(prefix.length).replace(/^\s*=\s*/, "").trim();
    return raw.replace(/^"(.*)"$/, "$1");
  }

  function checkStorage() {
    try {
      const probe = "rustlua-webgl-probe";
      window.localStorage.setItem(probe, "1");
      window.localStorage.removeItem(probe);
      return { available: true, fallback: false };
    } catch (error) {
      // 存储不可用：降级到内存（游戏存档应走内存/提示玩家），不得抛错。
      return { available: false, fallback: true, reason: String(error) };
    }
  }

  function checkWebgl2() {
    try {
      const canvas = document.createElement("canvas");
      const context = canvas.getContext("webgl2");
      if (!context) {
        return { webgl2: false };
      }
      const debug = context.getExtension("WEBGL_debug_renderer_info");
      const renderer = debug
        ? context.getParameter(debug.UNMASKED_RENDERER_WEBGL)
        : context.getParameter(context.RENDERER);
      return { webgl2: true, renderer: String(renderer) };
    } catch (error) {
      return { webgl2: false, reason: String(error) };
    }
  }

  async function run() {
    details.userAgent = navigator.userAgent;
    details.origin = window.location.origin;
    details.webgl = checkWebgl2();
    details.storage = checkStorage();

    const manifest = await fetchText("pack/manifest.txt");
    const manifestLines = manifest.split(/\r?\n/);
    details.manifestHeader = manifestLines[0] || "";
    // assetc 清单格式：首行 `assetc_manifest=1<TAB>assetc-package-2`，其后每行
    // `相对路径<TAB>字节数<TAB>内容指纹`。
    const manifestFiles = manifestLines
      .map((line) => line.trim())
      .filter((line) => line && !line.startsWith("#") && !line.startsWith("assetc_manifest="))
      .map((line) => line.split("\t")[0].trim())
      .filter((entry) => entry.length > 0);
    details.manifestEntries = manifestFiles.length;
    if (manifestFiles.length === 0) {
      throw new Error("pack/manifest.txt has no resource entries");
    }

    const project = await fetchText("pack/project.toml");
    const startupScene = projectKey(project, "startup_scene") || "scenes/main.scene";
    details.startupScene = startupScene;
    const scene = await fetchText(`pack/${startupScene}`);
    details.sceneBytes = scene.length;

    // manifest 首个资源也要可取（验证资源枚举与 fetch 路径一致）。
    const firstAsset = manifestFiles[0];
    const assetBytes = (await fetchText(`pack/${firstAsset}`)).length;
    details.firstAsset = firstAsset;
    details.firstAssetBytes = assetBytes;

    // 资源缺失必须是可诊断的失败（404），而不是静默成功。
    const missing = await fetch("pack/__rustlua_missing_probe__.txt", { cache: "no-store" });
    details.missingStatus = missing.status;
    if (missing.ok) {
      throw new Error("missing resource unexpectedly resolved");
    }

    finish("ok");
  }

  run().catch((error) => {
    details.error = String(error);
    finish("fail");
  });
})();
