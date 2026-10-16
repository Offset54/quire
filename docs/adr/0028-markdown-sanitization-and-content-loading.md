# 0028. Markdown sanitization and content loading: an allowlist in the core, images through our own scheme

- Status: Accepted
- Date: 2026-10-14

## Context

§6.2: sanitize embedded HTML with an allowlist (strip scripts and event handlers); block remote images by default, with a per-document "load remote content" toggle. §8: assume every file is hostile; no network access by default; remote content loads only after an explicit user action; a strict CSP, no remote scripts. §5.5: decode images at display resolution. User decision B (2026-10-13): any readable local path a document references loads in the preview without asking.

The preview renders blocks in the core (ADR-0026, ADR-0027) and inserts their HTML into the webview's DOM (ADR-0030), so whatever reaches the DOM must already be safe.

## Decision

**Sanitize in the core, before the HTML leaves it** (`crates/markdown/src/sanitize.rs`). comrak escapes text and builds the markup for Markdown constructs itself, so only raw HTML (HTML blocks and inline HTML) is cleaned:

- *Allowed tags*: GitHub's list without forms and media (`a`, `abbr`, `b`, `blockquote`, `br`, `code`, `del`, `details`, `div`, `h1`–`h6`, `img`, `kbd`, `li`, `ol`, `p`, `pre`, `span`, `sub`, `sup`, `summary`, `table` and its parts, `ul`, …). Other tags are dropped and their text kept.
- *Dropped with their content*: `script`, `style`, `iframe`, `object`, `embed`, `noscript`, `template`, `textarea`, `title`, `xmp`, `noembed`, `noframes`, `plaintext`, `frameset`, `frame`, `applet`, `svg`, `math`, `select`, `audio`, `video`, `canvas`. The open element is tracked across the fragments of one block (one state per top-level block), and text inside it is left out too.
- *Attributes*: a short global list (`align`, `alt`, `colspan`, `rowspan`, `title`, `width`, `height`, `open`, …); no `on*`, no `style`, no `class` or `id` from the document. Values are entity-decoded before they are judged (`&#106;avascript:` is `javascript:`), and re-escaped on output.
- *Links*: `href` keeps only `http`, `https`, `mailto`, relative and fragment URLs; every `a` gets `rel="noopener noreferrer"`. Links never navigate the webview (the navigation guard from M0 refuses it); in-document anchors scroll the preview.
- *Inputs*: only checkboxes (task lists written as HTML), always `disabled`.
- Comments, doctypes, CDATA and processing instructions go.

**Every image source goes through one policy** (`crates/markdown/src/images.rs`), for Markdown images and `<img>` alike:

| Source | Becomes |
|---|---|
| `data:image/png`, `gif`, `jpeg`, `webp` | `src`, inline (CSP allows `data:` images) |
| Local: relative path (against the document's folder), absolute path, `file:` URL | `data-src` with a URL of our image scheme; the preview sets `src` when the image comes near the viewport, with the display width |
| Remote: `http:`, `https:`, `//host` | `data-blocked-src`: a visible placeholder naming the source, until the document's toggle is on; then `data-src` with a URL of our scheme's remote route |
| Anything else (`javascript:`, `ftp:`, `data:` of other types) | a placeholder |

An unsaved document has no folder, so its relative images are placeholders.

**The `quire-img` URI scheme** (`apps/desktop/src-tauri/src/images.rs`, served as `http://quire-img.localhost/` on Windows) is the only image origin the CSP names besides `'self'`, `blob:` and `data:`:

- `local/<path>?w=<px>`: served only if the file's bytes are an image (PNG, JPEG, GIF, WebP, BMP, ICO or SVG, sniffed by signature; the extension is never trusted), at most 256 MiB. Raster images wider than `w` device pixels are decoded and scaled to `w` (Triangle filter; PNG if there is alpha, JPEG otherwise) before they reach the webview, so a 100-megapixel photo costs the webview only its display size. Animated GIF and WebP are served as they are. Decoding has a 1 GiB allocation limit (decompression bombs). Results are cached in an `LruCache` (`image-scheme`, `DecodedImage` class) under the memory governor, keyed by path, modification time, size and width.
- `remote/<doc>/<url>?w=<px>`: fetched by the host only while document `doc` has the toggle on (checked on every request, so a stale rendering can't fetch after the toggle goes off); only `http` and `https`; 20 s timeout, at most 5 redirects, 32 MiB, no cookies; the response must sniff as an image too. The webview's CSP never names a remote origin, so even a sanitizer bug can't make the webview fetch.
- Responses carry `X-Content-Type-Options: nosniff`, `Content-Security-Policy: default-src 'none'` and `Cache-Control: no-store`.

**The toggle** is per document and per session (`md_set_remote`): a banner says remote images were blocked and offers "Load remote content"; the status bar shows "Remote content on" while it is, and turns it off again. Switching re-renders the document (render options change, ADR-0030's cache drops every rendering).

**The trade-off of decision B**, recorded as the user asked: a hostile document can *display*, never *send*, any image file the user can read (`![](C:/Users/me/Pictures/id-card.png)`). Nothing in the preview can read pixels back or send them anywhere: no scripts run, the CSP allows no remote connection, and links and forms don't navigate. The scheme serves only files whose bytes are images, so a document can't display `win.ini` or a private key through it.

## Alternatives considered

| Option | License | Why not |
|---|---|---|
| `ammonia` (html5ever-based sanitizer) | MIT/Apache-2.0 | A full HTML5 parser and tree builder for fragments that are mostly a tag or two; its cleaning is per fragment, while ours must carry an open `<script>` across a block's fragments. Our tokenizer is 450 lines with its own tests, the hostile set and a planned fuzz target (task 12). |
| Sanitize in the frontend (DOMPurify) | Apache-2.0/MPL-2.0 | Unsanitized HTML would cross the IPC boundary and sit in the JS heap; the export path (task 11) needs the same cleaning without a webview. |
| `file:` or Tauri's asset protocol for local images | – | `asset:` needs a scope of allowed folders (decision B says any readable path), serves any file type, and has no display-size decoding. |
| Let the webview load remote images itself when the toggle is on | – | The CSP would have to allow remote origins for every document all the time. |
| `reqwest` | MIT/Apache-2.0 | Pulls in tokio and hyper for a few image downloads; not in our Windows graph today (Tauri uses it only on mobile). |
| `ureq` with rustls | MIT/Apache-2.0 | Its root store (`webpki-roots`/`webpki-root-certs`) is CDLA-Permissive-2.0, not on the §0 list. With native TLS, the OS certificate store is used instead. |

## Dependencies

- `image` 0.25.10 (MIT OR Apache-2.0), features `png`, `jpeg`, `gif`, `webp`, `bmp`, `ico`: decoding and scaling for the image scheme (already in the graph through the clipboard plugin; `gif` MIT/Apache, `image-webp` MIT/Apache, `zune-jpeg` MIT/Apache/Zlib, `color_quant` MIT added).
- `ureq` 3.4.2 (MIT OR Apache-2.0), `native-tls-no-default` only: remote image fetches. Brings `native-tls` (MIT/Apache), `schannel` (MIT) on Windows, `security-framework` (MIT/Apache) on macOS, `openssl` (Apache-2.0) / `openssl-sys` (MIT) against the system OpenSSL on Linux, `ureq-proto`, `der`, `rustls-pki-types`, `zeroize`, `base64ct`, `pem-rfc7468`, `utf8-zero` (all MIT/Apache).

`cargo deny check` passes with them.

## Consequences

- Raw HTML in the preview looks like GitHub's (same tag list), minus forms and media. `class` and `style` from documents are dropped, so HTML that relies on them renders plainer.
- Images in the preview cost the webview their display size only; the scaled copies count against the governor (`image-scheme`), and the webview's decoded images are reported as `preview-images` (§5.5).
- Exports (task 11) reuse the same sanitizer and policy, with images embedded as data URIs.
- The sanitizer gets a fuzz target (task 12).

## Evidence

- `crates/markdown/src/sanitize/tests.rs`: scripts, handlers, entity-encoded and whitespace-split `javascript:` URLs, frames, forms, styles, comments, drop-with-content across fragments, checkboxes, broken tags.
- `crates/markdown/tests/hostile.rs` over `tests/data/markdown/hostile/` (scripts, resources, malformed markup: 81 cases): the output is re-tokenized by an HTML5-style tokenizer written apart from the sanitizer (numeric references without semicolons decoded, as browsers do), and every tag, attribute, link scheme and image source is checked against the allowlist, with the remote toggle off and on.
- `apps/desktop/src-tauri/src/images.rs` tests: signature sniffing (`win.ini` under a `.png` name is refused), scaling 400 × 200 → 100 × 50, the toggle checked per request, non-HTTP remote URLs refused.
- `tests/ui/md-flows.test.mjs` (real app, WebDriver): a document's `<script>` and `onclick` don't survive; a local PNG loads through `quire-img`; a remote image is a placeholder with the banner; the toggle re-renders and the host attempts the fetch.

## Addendum (M2 task 12, 2026-10-15): Markdown links keep the same allowlist

The `md_sanitize` fuzz target found that links written in Markdown (`[x](url)`, `<url>` autolinks, reference links) went through comrak's own check, which only refuses `javascript:`, `vbscript:`, `file:` and non-image `data:`; any other scheme (`ms-msdt:`, `ms-settings:`, `search-ms:`, an unknown one) stayed a live `href`. That broke the decision above. Sanitized output now gives Markdown links the raw-HTML rule (`http`, `https`, `mailto`, relative and fragment URLs; anything else keeps its text with an empty `href`) and `rel="noopener noreferrer"`. The spec harness, which renders without sanitizing, keeps comrak's behavior. Evidence: `tests/data/markdown/hostile/links.md` in `tests/hostile.rs`; the crash input is in `fuzz/regressions/md_sanitize/`.
