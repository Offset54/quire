# Remote resources and embedding

![remote](https://tracker.example/pixel.png)

![protocol relative](//tracker.example/p.png)

<img src="https://tracker.example/raw.png" alt="raw">

<img src='http://tracker.example/single.png'>

<img srcset="https://tracker.example/1x.png 1x, https://tracker.example/2x.png 2x">

<picture><source srcset="https://tracker.example/s.webp"><img src="local.png"></picture>

<iframe src="https://e.com"></iframe>

<frameset><frame src="https://e.com"></frameset>

<object data="https://e.com/x.swf"></object>

<embed src="https://e.com/x.swf">

<video src="https://e.com/v.mp4" poster="https://e.com/p.png"></video>

<audio src="https://e.com/a.mp3"></audio>

<link rel="stylesheet" href="https://e.com/s.css">

<style>@import url(https://e.com/s.css); body { background: url(https://e.com/b.png) }</style>

<p style="background:url(https://e.com/bg.png)">styled</p>

<table background="https://e.com/t.png"><tr><td background="https://e.com/c.png">t</td></tr></table>

<meta http-equiv="refresh" content="0;url=https://e.com">

<base href="https://e.com/">

<form action="https://e.com"><input name="q"><button>go</button></form>

<input type="image" src="https://e.com/i.png">

<input type="checkbox" checked onchange="x()"> checkbox

<div data-src="http://quire-img.localhost/local/C%3A%5Csecret.png">forged data-src</div>

<img data-src="https://e.com/forged.png">

<a href="https://e.com/ok">a normal link stays</a>

<a ping="https://e.com/ping" href="#x">ping</a>

<img src="data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=">

<img src="data:image/png;base64,iVBORw0KGgo=">

<img src="file:///C:/Windows/win.ini">

<x-custom onclick=1>custom element</x-custom>

<isindex action="https://e.com">
