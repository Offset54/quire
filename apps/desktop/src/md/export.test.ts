import { describe, expect, it } from "vitest";
import { PDF_MAX_BYTES, pdfTooLarge } from "./export";

describe("PDF size limit", () => {
  it("passes pages up to the limit", () => {
    expect(pdfTooLarge(PDF_MAX_BYTES)).toBe(false);
    expect(pdfTooLarge(PDF_MAX_BYTES + 1)).toBe(true);
  });
});
