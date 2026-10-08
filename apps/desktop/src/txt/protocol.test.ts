import { describe, expect, it } from "vitest";
import { decodeView, indexOf, offsetAt, textEnd, type Row } from "./protocol";

function encode(header: object, rows: Omit<Row, "textStart">[]): ArrayBuffer {
  const enc = new TextEncoder();
  const json = enc.encode(JSON.stringify({ ...header, rows: rows.length }));
  const parts: number[] = [];
  const u32 = (v: number) => {
    for (let i = 0; i < 4; i++) parts.push((v >>> (8 * i)) & 0xff);
  };
  const u64 = (v: bigint) => {
    for (let i = 0n; i < 8n; i++) parts.push(Number((v >> (8n * i)) & 0xffn));
  };
  u32(json.length);
  parts.push(...json);
  for (const r of rows) {
    const text = enc.encode(r.text);
    u64(BigInt(r.start));
    u64(BigInt(r.end));
    u64(BigInt(r.start));
    u64(r.line < 0 ? 0xffffffffffffffffn : BigInt(r.line));
    u32(r.flags);
    u32(r.col0);
    u32(r.subst.length);
    r.subst.forEach(u32);
    u32(text.length);
    parts.push(...text);
  }
  return new Uint8Array(parts).buffer;
}

describe("view messages", () => {
  it("decode header and rows", () => {
    const buf = encode({ revision: 3, anchorIndex: 1 }, [
      { start: 0, end: 3, line: 0, flags: 7, col0: 0, subst: [], text: "one" },
      { start: 4, end: 9, line: -1, flags: 3, col0: 0, subst: [2], text: "tw␁o😀" },
    ]);
    const m = decodeView(buf);
    expect(m.header.revision).toBe(3);
    expect(m.rows).toHaveLength(2);
    expect(m.rows[0]!.text).toBe("one");
    expect(m.rows[1]!.line).toBe(-1);
    expect(m.rows[1]!.subst).toEqual([2]);
  });

  it("maps UTF-16 indices to byte offsets and back", () => {
    // "a", substituted control (1 byte), "한" (3 bytes), "😀" (4 bytes, 2 units), "b"
    const row: Row = {
      start: 100,
      end: 110,
      textStart: 100,
      line: 0,
      flags: 3,
      col0: 0,
      subst: [1],
      text: "a␁한😀b",
    };
    const offsets = [100, 101, 102, 105, 109, 110];
    const indices = [0, 1, 2, 3, 5, 6];
    indices.forEach((u, k) => {
      expect(offsetAt(row, u)).toBe(offsets[k]);
      expect(indexOf(row, offsets[k]!)).toBe(u);
    });
    // Inside the surrogate pair rounds down.
    expect(offsetAt(row, 4)).toBe(105);
    expect(textEnd(row)).toBe(110);
  });
});
