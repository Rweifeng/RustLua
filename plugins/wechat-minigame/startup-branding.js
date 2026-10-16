'use strict';

// Main-package asset, independent of pack routing, remote assets and the WASM bridge.
var SPLASH_PATH = 'branding/splash.png';
var NAVY = [15 / 255, 25 / 255, 43 / 255, 1];

function draw2d(canvas, getContext, image) {
  var ctx = getContext.call(canvas, '2d');
  if (!ctx) throw new Error('2D context unavailable');
  var scale = Math.min(canvas.width / image.width, canvas.height / image.height);
  var width = image.width * scale, height = image.height * scale;
  ctx.save();
  try {
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = 'source-over';
    ctx.fillStyle = '#0f192b';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(image, (canvas.width - width) / 2, (canvas.height - height) / 2, width, height);
  } finally {
    ctx.restore();
  }
}

function drawGl(canvas, getContext, image) {
  // Keep the last splash pixels while pack/WASM loads; never clear or resize at handoff.
  // Never request 2D on this canvas: context type is permanent in the host.
  // Match the engine's single-sample surface (context attributes cannot change later).
  var gl = getContext.call(canvas, 'webgl2', {
    alpha: true, antialias: false, depth: false, stencil: false,
    premultipliedAlpha: true, preserveDrawingBuffer: true
  });
  if (!gl) throw new Error('WebGL2 context unavailable');
  var state = {
    program: gl.getParameter(gl.CURRENT_PROGRAM),
    vao: gl.getParameter(gl.VERTEX_ARRAY_BINDING),
    framebuffer: gl.getParameter(gl.DRAW_FRAMEBUFFER_BINDING),
    viewport: gl.getParameter(gl.VIEWPORT),
    clear: gl.getParameter(gl.COLOR_CLEAR_VALUE),
    mask: gl.getParameter(gl.COLOR_WRITEMASK),
    active: gl.getParameter(gl.ACTIVE_TEXTURE),
    unpackBuffer: gl.getParameter(gl.PIXEL_UNPACK_BUFFER_BINDING)
  };
  var caps = [gl.BLEND, gl.DEPTH_TEST, gl.STENCIL_TEST, gl.SCISSOR_TEST, gl.CULL_FACE, gl.RASTERIZER_DISCARD];
  var enabled = caps.map(function (cap) { return gl.isEnabled(cap); });
  var unpack = [gl.UNPACK_ALIGNMENT, gl.UNPACK_FLIP_Y_WEBGL, gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL,
    gl.UNPACK_COLORSPACE_CONVERSION_WEBGL, gl.UNPACK_ROW_LENGTH, gl.UNPACK_IMAGE_HEIGHT,
    gl.UNPACK_SKIP_PIXELS, gl.UNPACK_SKIP_ROWS, gl.UNPACK_SKIP_IMAGES];
  var unpackValues = unpack.map(function (key) { return gl.getParameter(key); });
  var shaders = [], program = null, texture = null, vao = null;
  var oldTexture, oldSampler, textureStateSaved = false;
  function shader(type, source) {
    var result = gl.createShader(type);
    if (!result) throw new Error('Cannot create splash shader');
    shaders.push(result);
    gl.shaderSource(result, source);
    gl.compileShader(result);
    if (!gl.getShaderParameter(result, gl.COMPILE_STATUS)) {
      throw new Error('Splash shader: ' + gl.getShaderInfoLog(result));
    }
    return result;
  }
  try {
    gl.activeTexture(gl.TEXTURE0);
    oldTexture = gl.getParameter(gl.TEXTURE_BINDING_2D);
    oldSampler = gl.getParameter(gl.SAMPLER_BINDING);
    textureStateSaved = true;
    var vertex = shader(gl.VERTEX_SHADER, '#version 300 es\n' +
      'uniform vec2 fit; out highp vec2 uv;\n' +
      'void main() { vec2 p = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1));' +
      'uv = p; gl_Position = vec4((p * 2.0 - 1.0) * fit, 0.0, 1.0); }');
    var fragment = shader(gl.FRAGMENT_SHADER, '#version 300 es\nprecision mediump float;\n' +
      'uniform sampler2D splash; in highp vec2 uv; out vec4 color;\n' +
      'void main() { vec4 c = texture(splash, uv);' +
      'color = vec4(mix(vec3(15.0, 25.0, 43.0) / 255.0, c.rgb, c.a), 1.0); }');
    program = gl.createProgram();
    if (!program) throw new Error('Cannot create splash program');
    gl.attachShader(program, vertex);
    gl.attachShader(program, fragment);
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      throw new Error('Splash program: ' + gl.getProgramInfoLog(program));
    }
    texture = gl.createTexture();
    vao = gl.createVertexArray();
    if (!texture || !vao) throw new Error('Cannot allocate splash resources');
    // gl_VertexID + a private empty VAO means no vertex attribute/default VAO leakage.
    gl.bindVertexArray(vao);
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.bindSampler(0, null);
    gl.bindBuffer(gl.PIXEL_UNPACK_BUFFER, null);
    var values = [1, true, false, gl.NONE, 0, 0, 0, 0, 0];
    unpack.forEach(function (key, i) { gl.pixelStorei(key, values[i]); });
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, image);
    gl.useProgram(program);
    var scale = Math.min(canvas.width / image.width, canvas.height / image.height);
    gl.uniform2f(gl.getUniformLocation(program, 'fit'), image.width * scale / canvas.width, image.height * scale / canvas.height);
    gl.uniform1i(gl.getUniformLocation(program, 'splash'), 0);
    gl.bindFramebuffer(gl.DRAW_FRAMEBUFFER, null);
    gl.viewport(0, 0, canvas.width, canvas.height);
    caps.forEach(function (cap) { gl.disable(cap); });
    gl.colorMask(true, true, true, true);
    gl.clearColor(NAVY[0], NAVY[1], NAVY[2], NAVY[3]);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    gl.flush();
  } finally {
    gl.useProgram(state.program);
    gl.bindVertexArray(state.vao);
    gl.bindFramebuffer(gl.DRAW_FRAMEBUFFER, state.framebuffer);
    gl.viewport.apply(gl, state.viewport);
    gl.clearColor.apply(gl, state.clear);
    gl.colorMask.apply(gl, state.mask);
    caps.forEach(function (cap, i) { if (enabled[i]) gl.enable(cap); else gl.disable(cap); });
    unpack.forEach(function (key, i) { gl.pixelStorei(key, unpackValues[i]); });
    gl.bindBuffer(gl.PIXEL_UNPACK_BUFFER, state.unpackBuffer);
    if (textureStateSaved) {
      gl.bindTexture(gl.TEXTURE_2D, oldTexture);
      gl.bindSampler(0, oldSampler);
    }
    gl.activeTexture(state.active);
    if (vao) gl.deleteVertexArray(vao);
    if (texture) gl.deleteTexture(texture);
    if (program) gl.deleteProgram(program);
    shaders.forEach(function (item) { gl.deleteShader(item); });
  }
}

// Exactly one image and at most one platform frame callback. Late image/frame callbacks
// are inert after settlement, including timeout: nothing can paint over the engine.
module.exports = function showStartupBranding(options) {
  return new Promise(function (resolve) {
    var image = null, settled = false, loaded = false, drawn = false;
    var timer = setTimeout(function () { finish(new Error('local image/frame timed out')); }, 3000);
    function finish(error) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (image) {
        try { image.onload = null; image.onerror = null; } catch (ignored) { /* host object */ }
      }
      if (error && options.warn) {
        try { options.warn('wechat-adapter: startup splash skipped: ' + String(error.message || error)); } catch (ignored) { /* logging cannot block boot */ }
      }
      resolve(!error);
    }
    function draw() {
      if (settled || drawn) return;
      drawn = true;
      try {
        if (!(image.width > 0 && image.height > 0)) throw new Error('empty splash image');
        if (options.gpu) drawGl(options.canvas, options.getContext, image);
        else draw2d(options.canvas, options.getContext, image);
        // Yield out of the rendering callback before heavy synchronous WASM work.
        // No minimum display duration and no recurring rendering loop.
        setTimeout(function () { finish(); }, 0);
      } catch (error) { finish(error); }
    }
    try {
      image = options.createImage();
      image.onload = function () {
        if (settled || loaded) return;
        loaded = true;
        try {
          if (options.requestFrame) options.requestFrame(draw);
          else draw();
        } catch (error) { finish(error); }
      };
      image.onerror = function () { finish(new Error('cannot load ' + SPLASH_PATH)); };
      image.src = SPLASH_PATH;
    } catch (error) { finish(error); }
  });
};
