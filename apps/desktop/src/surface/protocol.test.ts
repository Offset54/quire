import { describe, expect, it } from "vitest";
import { decodePixels, HEADER_LEN, parseHeader, tileKey } from "./protocol";
import { decodeQoi } from "./qoi";

function message(encoding: number, payload: number[], w = 2, h = 2): ArrayBuffer {
  const buf = new ArrayBuffer(HEADER_LEN + payload.length);
  const v = new DataView(buf);
  v.setUint32(0, 0x314c5451, true);
  v.setUint32(4, encoding, true);
  v.setBigUint64(8, 7n, true);
  v.setBigUint64(16, 9n, true);
  v.setBigUint64(24, 123_456_789_012n, true);
  v.setUint32(32, 2, true);
  v.setUint32(36, 512, true);
  v.setUint32(40, w, true);
  v.setUint32(44, h, true);
  new Uint8Array(buf, HEADER_LEN).set(payload);
  return buf;
}

// 2×2 image: RGB(10,20,30), run ×1, diff(+1,0,-1), index of the first colour.
const QOI = [
  ...[0x71, 0x6f, 0x69, 0x66, 0, 0, 0, 2, 0, 0, 0, 2, 4, 0],
  ...[0xfe, 10, 20, 30, 0xc0, 0x79, 0x09],
  ...[0, 0, 0, 0, 0, 0, 0, 1],
];

describe("tile protocol", () => {
  it("parses the header written by transport.rs", () => {
    const h = parseHeader(
      message(
        0,
        Array.from({ length: 16 }, () => 0),
      ),
    );
    expect(h).toEqual({
      encoding: "raw",
      view: 7,
      layout: 9,
      row: 123_456_789_012,
      col: 2,
      size: 512,
      width: 2,
      height: 2,
    });
    expect(tileKey(h)).toBe("7:9:512:2:123456789012");
  });

  it("rejects malformed messages", () => {
    expect(() => parseHeader(new ArrayBuffer(10))).toThrow();
    const bad = message(0, []);
    new DataView(bad).setUint32(0, 0, true);
    expect(() => parseHeader(bad)).toThrow(/magic/);
    expect(() => parseHeader(message(7, []))).toThrow(/encoding/);
  });

  it("decodes QOI ops: RGB, run, diff and index", () => {
    expect(Array.from(decodeQoi(new Uint8Array(QOI)).data)).toEqual([
      ...[10, 20, 30, 255],
      ...[10, 20, 30, 255],
      ...[11, 20, 29, 255],
      ...[10, 20, 30, 255],
    ]);
    const buf = message(1, QOI);
    expect(decodePixels(buf, parseHeader(buf))[8]).toBe(11);
  });

  it("refuses payloads shorter than the declared size", () => {
    const buf = message(0, [1, 2, 3]);
    expect(() => decodePixels(buf, parseHeader(buf))).toThrow(/short/);
  });
});
