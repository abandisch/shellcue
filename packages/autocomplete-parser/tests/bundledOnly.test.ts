import fs from "node:fs";
import path from "node:path";
import { describe, it, expect } from "vitest";

const readSource = (file: string) =>
  fs.readFileSync(path.join(import.meta.dirname, "../src", file), "utf8");

const loadHelpers = readSource("loadHelpers.ts");
const loadSpec = readSource("loadSpec.ts");

const FORBIDDEN = [
  "createObjectURL",
  "new Blob",
  "fread",
  "http://",
  "localhost:",
  "eval(",
  "new Function",
];

const dynamicImports = (source: string) =>
  [...source.matchAll(/\bimport\(/g)].length;

describe("spec code comes only from bundled specs", () => {
  it("has no other way to load or evaluate code", () => {
    for (const token of FORBIDDEN) {
      expect(loadHelpers).not.toContain(token);
      expect(loadSpec).not.toContain(token);
    }
  });

  it("has a single dynamic import, in loadHelpers.ts", () => {
    expect(dynamicImports(loadHelpers)).toBe(1);
    expect(dynamicImports(loadSpec)).toBe(0);
  });

  it("imports from the bundled spec:// origin", () => {
    expect(loadHelpers).toContain("spec://localhost/");
  });
});
