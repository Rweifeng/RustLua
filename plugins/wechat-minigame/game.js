require('./wechat-adapter.js');
var canvas=wx.createCanvas();
if(wx.requestAnimationFrame){function frame(t){if(globalThis.RustLuaWechatStart)globalThis.RustLuaWechatStart(canvas,t);wx.requestAnimationFrame(frame)}wx.requestAnimationFrame(frame)}
