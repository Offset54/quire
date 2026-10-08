// Minimal decoders for the encodings the core can produce. Correctness is
// checked against the raw tiles before any timing starts.

/** LZ4 block with a 4-byte little-endian uncompressed size prefix (lz4_flex `compress_prepend_size`). */
export function decodeLz4(src: Uint8Array): Uint8Array {
  const outLen = (src[0]! | (src[1]! << 8) | (src[2]! << 16) | (src[3]! << 24)) >>> 0;
  const out = new Uint8Array(outLen);
  let s = 4;
  let d = 0;
  while (s < src.length) {
    const token = src[s++]!;
    let lit = token >>> 4;
    if (lit === 15) {
      let b: number;
      do {
        b = src[s++]!;
        lit += b;
      } while (b === 255);
    }
    out.set(src.subarray(s, s + lit), d);
    s += lit;
    d += lit;
    if (s >= src.length) break;
    const offset = src[s]! | (src[s + 1]! << 8);
    s += 2;
    let len = (token & 15) + 4;
    if ((token & 15) === 15) {
      let b: number;
      do {
        b = src[s++]!;
        len += b;
      } while (b === 255);
    }
    let m = d - offset;
    if (offset >= len) {
      out.copyWithin(d, m, m + len);
      d += len;
    } else {
      // Overlapping match: repeat byte by byte.
      for (let k = 0; k < len; k++) out[d++] = out[m++]!;
    }
  }
  return out;
}

/** QOI image to RGBA. */
export function decodeQoi(src: Uint8Array): { data: Uint8Array; width: number; height: number } {
  const view = new DataView(src.buffer, src.byteOffset, src.byteLength);
  const width = view.getUint32(4);
  const height = view.getUint32(8);
  const out = new Uint8Array(width * height * 4);
  const index = new Uint8Array(64 * 4);
  let r = 0;
  let g = 0;
  let b = 0;
  let a = 255;
  let p = 14;
  const end = src.length - 8;
  let run = 0;
  for (let o = 0; o < out.length; o += 4) {
    if (run > 0) {
      run--;
    } else if (p < end) {
      const b1 = src[p++]!;
      if (b1 === 0xfe) {
        r = src[p++]!;
        g = src[p++]!;
        b = src[p++]!;
      } else if (b1 === 0xff) {
        r = src[p++]!;
        g = src[p++]!;
        b = src[p++]!;
        a = src[p++]!;
      } else {
        const tag = b1 & 0xc0;
        if (tag === 0x00) {
          const i = b1 * 4;
          r = index[i]!;
          g = index[i + 1]!;
          b = index[i + 2]!;
          a = index[i + 3]!;
        } else if (tag === 0x40) {
          r = (r + ((b1 >> 4) & 3) - 2) & 255;
          g = (g + ((b1 >> 2) & 3) - 2) & 255;
          b = (b + (b1 & 3) - 2) & 255;
        } else if (tag === 0x80) {
          const b2 = src[p++]!;
          const vg = (b1 & 0x3f) - 32;
          r = (r + vg - 8 + ((b2 >> 4) & 15)) & 255;
          g = (g + vg) & 255;
          b = (b + vg - 8 + (b2 & 15)) & 255;
        } else {
          run = b1 & 0x3f;
        }
      }
      const h = ((r * 3 + g * 5 + b * 7 + a * 11) % 64) * 4;
      index[h] = r;
      index[h + 1] = g;
      index[h + 2] = b;
      index[h + 3] = a;
    }
    out[o] = r;
    out[o + 1] = g;
    out[o + 2] = b;
    out[o + 3] = a;
  }
  return { data: out, width, height };
}
