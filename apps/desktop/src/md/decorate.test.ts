import { describe, expect, it } from "vitest";
import { blockClass, inlineSpans } from "./decorate";
import { KIND } from "./protocol";

const classes = (s: string) => inlineSpans(s).map(([a, b, c]) => [s.slice(a, b), c]);

describe("source decoration", () => {
  it("marks headings, lists and quotes", () => {
    expect(classes("## Title")).toEqual([["##", "md-tok-mark"]]);
    expect(classes("- [x] done")).toEqual([["- [x]", "md-tok-mark"]]);
    expect(classes("> quoted")).toEqual([[">", "md-tok-mark"]]);
    expect(classes("#hashtag")).toEqual([]);
  });

  it("finds inline code, links and emphasis without overlaps", () => {
    expect(classes("a `**not bold**` b **bold** _em_ *em*")).toEqual([
      ["`**not bold**`", "md-tok-code"],
      ["**bold**", "md-tok-strong"],
      ["_em_", "md-tok-em"],
      ["*em*", "md-tok-em"],
    ]);
    expect(classes("see [text](http://x.y) and ![i](a.png) <https://e.com>")).toEqual([
      ["[text](http://x.y)", "md-tok-link"],
      ["![i](a.png)", "md-tok-link"],
      ["<https://e.com>", "md-tok-link"],
    ]);
    expect(classes("snake_case_name and 2*3*4")).toEqual([]);
    expect(classes("~~gone~~")).toEqual([["~~gone~~", "md-tok-del"]]);
  });

  it("classes rows by block kind", () => {
    expect(blockClass({ start: 0, end: 1, kind: KIND.heading, detail: 2 })).toBe(
      "md-src-h md-src-h2",
    );
    expect(blockClass({ start: 0, end: 1, kind: KIND.code, detail: 1 })).toBe("md-src-code");
    expect(blockClass({ start: 0, end: 1, kind: KIND.paragraph, detail: 0 })).toBe("");
    expect(blockClass(undefined)).toBe("");
  });
});
