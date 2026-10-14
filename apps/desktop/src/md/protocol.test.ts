import { describe, expect, it } from "vitest";
import { MAGIC, decodeWindow } from "./protocol";

function encode(blocks: { id: number; start: number; end: number; html?: string }[]): ArrayBuffer {
  const enc = new TextEncoder();
  const parts: number[] = [];
  const u32 = (v: number) => {
    for (let i = 0; i < 4; i++) parts.push((v >>> (8 * i)) & 0xff);
  };
  const u64 = (v: number) => {
    const b = BigInt(v);
    for (let i = 0n; i < 8n; i++) parts.push(Number((b >> (8n * i)) & 0xffn));
  };
  u32(MAGIC);
  u32(1 | 4);
  u64(7);
  u64(3);
  u64(1000);
  u64(1000);
  u32(10);
  u32(1);
  u32(42);
  u32(blocks.length);
  for (const b of blocks) {
    u64(b.id);
    u64(b.start);
    u64(b.end);
    u64(b.id * 10);
    u32(2);
    parts.push(2, b.html === undefined ? 0 : 1, 0, 0);
    const h = enc.encode(b.html ?? "");
    u32(h.length);
    parts.push(...h);
  }
  return new Uint8Array(parts).buffer;
}

describe("preview window messages", () => {
  it("decode the header and blocks", () => {
    const m = decodeWindow(
      encode([
        { id: 5, start: 0, end: 10, html: "<p>한글</p>" },
        { id: 6, start: 10, end: 20 },
      ]),
    );
    expect(m.complete).toBe(true);
    expect(m.atStart).toBe(false);
    expect(m.atEnd).toBe(true);
    expect(m.seq).toBe(7);
    expect(m.revision).toBe(3);
    expect(m.first).toBe(10);
    expect(m.anchor).toBe(1);
    expect(m.indexedBlocks).toBe(42);
    expect(m.blocks.map((b) => [b.id, b.start, b.end, b.rev, b.lines])).toEqual([
      [5, 0, 10, 50, 2],
      [6, 10, 20, 60, 2],
    ]);
    expect(m.blocks[0]?.html).toBe("<p>한글</p>");
    expect(m.blocks[1]?.html).toBeUndefined();
  });

  it("rejects other messages", () => {
    expect(() => decodeWindow(new Uint8Array(64).buffer)).toThrow();
  });
});
