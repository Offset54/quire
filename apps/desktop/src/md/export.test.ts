import { describe, expect, it } from "vitest";
import { PDF_MAX_BYTES, pdfTooLarge } from "./export";

describe("PDF size limit", () => {
  it("passes pages up to the limit and reports the size of larger ones", () => {
    expect(pdfTooLarge("a".repeat(PDF_MAX_BYTES))).toBeNull();
    expect(pdfTooLarge("a".repeat(PDF_MAX_BYTES + 1))).toBe(PDF_MAX_BYTES + 1);
  });

  it("counts UTF-8 bytes, not characters", () => {
    // 3 bytes each: a third of the limit in characters is the limit in bytes.
    const n = Math.floor(PDF_MAX_BYTES / 3) + 1;
    expect(pdfTooLarge("한".repeat(n))).toBe(n * 3);
  });
});
