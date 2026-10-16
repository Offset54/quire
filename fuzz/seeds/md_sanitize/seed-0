# Malformed and nested markup

<scr<script>ipt>alert(1)</script>

<<script>script>alert(2)<</script>/script>

<img """><script>alert(3)</script>">

<a href="x" title="a"onclick="b">no space</a>

<div title='x" onclick="y'>quote mix</div>

<p/onclick=alert(4)>slash</p>

<img/src=x/onerror=alert(5)>

<a href=x onclick = alert(6) >spaces around equals</a>

<div
onclick
=
alert(7)>newlines</div>

<style>
</style
<script>alert(8)</script>

<textarea><script>alert(9)</script></textarea>

<title><script>alert(10)</script></title>

<xmp><script>alert(11)</script></xmp>

<plaintext><script>alert(12)</script>

<div>
<script>
alert(13)

</script>
</div>

- <script>alert(14)</script> in a list item
- > <img src=x onerror=alert(15)> in a quote

| a | b |
|---|---|
| <script>alert(16)</script> | <img src=x onerror=alert(17)> |

`<script>alert(18)</script>` in code is text.

```html
<script>alert(19)</script>
```

<a href="#" title="&quot; onclick=&quot;alert(20)">entity in attr</a>

<a href="&#0000106avascript:alert(21)">no semicolon</a>

<div attr="unterminated
