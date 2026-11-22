import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { checkOutputs, compileSpecs } from "../src/compile.js";

const FIXTURES = fileURLToPath(new URL("./fixtures/", import.meta.url));
const tempDirs = [];

async function makeTempDir() {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "spec-compiler-"));
  tempDirs.push(dir);
  return dir;
}

// The package.json sits beside the out dir so Node loads the emitted .js files as ESM.
async function compileInto(srcDir) {
  const tmp = await makeTempDir();
  await fs.writeFile(path.join(tmp, "package.json"), '{ "type": "module" }\n');
  const outDir = path.join(tmp, "out");
  const summary = await compileSpecs({ srcDir, outDir });
  return { outDir, summary };
}

async function listFiles(dir, prefix = "") {
  const found = [];
  for (const entry of await fs.readdir(dir, { withFileTypes: true })) {
    const rel = `${prefix}${entry.name}`;
    if (entry.isDirectory()) {
      found.push(...(await listFiles(path.join(dir, entry.name), `${rel}/`)));
    } else {
      found.push(rel);
    }
  }
  return found.sort();
}

const readJson = async (file) => JSON.parse(await fs.readFile(file, "utf8"));
const importOutput = (outDir, rel) =>
  import(pathToFileURL(path.join(outDir, rel)).href);
const fixtureSrc = (name) => path.join(FIXTURES, name, "src");

afterAll(async () => {
  await Promise.all(
    tempDirs.map((dir) => fs.rm(dir, { recursive: true, force: true })),
  );
});

describe("compileSpecs on the ok fixture", () => {
  let outDir;
  let summary;

  beforeAll(async () => {
    ({ outDir, summary } = await compileInto(fixtureSrc("ok")));
  });

  it("writes the index and summary", async () => {
    expect(await readJson(path.join(outDir, "index.json"))).toEqual({
      completions: [
        "@scope/cli",
        "argo",
        "aws",
        "aws/s3",
        "c++",
        "copilot",
        "deno",
        "git",
        "heroku",
        "hub",
        "infracost",
        "nc",
        "rush",
      ],
      diffVersionedCompletions: ["heroku"],
    });
    expect(summary).toEqual({
      specs: 14,
      completions: 13,
      diffVersioned: ["heroku"],
      iconsReplaced: 1,
      bytes: expect.any(Number),
    });
  });

  it("emits exactly the expected files", async () => {
    expect(await listFiles(outDir)).toEqual([
      "@scope/cli.js",
      "argo.js",
      "aws.js",
      "aws/s3.js",
      "c++.js",
      "copilot.js",
      "deno.js",
      "git.js",
      "heroku/1.0.0.js",
      "heroku/index.js",
      "hub.js",
      "index.json",
      "infracost.js",
      "nc.js",
      "rush.js",
    ]);
  });

  it("emits specs whose unused imports name a dropped spec or an unlisted package", async () => {
    expect(await listFiles(outDir)).toEqual(
      expect.arrayContaining(["argo.js", "nc.js"]),
    );
  });

  it("bundles the pinned yaml and semver", async () => {
    const copilot = await importOutput(outDir, "copilot.js");
    expect(copilot.default.args.generators.postProcess("a: 1\nb: 2")).toEqual([
      { name: "a" },
      { name: "b" },
    ]);
    const infracost = await importOutput(outDir, "infracost.js");
    expect(infracost.default.args.generators.postProcess(" v1.2.3 ")).toEqual([
      { name: "1.2.3" },
    ]);
  });

  it("removes remote icon literals and keeps fig icons", async () => {
    for (const file of ["git.js", "hub.js"]) {
      const code = await fs.readFile(path.join(outDir, file), "utf8");
      expect(code).not.toContain("https://example.invalid");
      expect(code).toContain("fig://icon?type=git");
    }
  });

  it("bundles aliases and replaces the ai generator", async () => {
    const hub = await importOutput(outDir, "hub.js");
    expect(Object.keys(hub)).toEqual(["default"]);
    expect(hub.default.name).toBe("git");
    const commit = hub.default.subcommands.find((s) => s.name === "commit");
    await expect(commit.args.generators.custom()).resolves.toEqual([]);
  });

  it("keeps the exports of diff-versioned specs", async () => {
    const index = await importOutput(outDir, "heroku/index.js");
    expect(Object.keys(index).sort()).toEqual(["default", "getVersionCommand"]);
    const version = await importOutput(outDir, "heroku/1.0.0.js");
    expect(Object.keys(version).sort()).toEqual(["default", "versions"]);
    expect(version.default.name).toBe("heroku");
  });
});

describe("compileSpecs refuses", () => {
  it("imports of packages that are not allowed", async () => {
    await expect(compileInto(fixtureSrc("bad-package"))).rejects.toThrow(
      /"left-pad" from .*pad\.ts/,
    );
  });

  it("specs without a default export", async () => {
    await expect(compileInto(fixtureSrc("no-default"))).rejects.toThrow(
      "nodefault: no default export",
    );
  });

  it("imports that reach outside the specs", async () => {
    await expect(compileInto(fixtureSrc("outside"))).rejects.toThrow(
      /import of "\.\.\/outside" from .*escape\.ts leaves the specs/,
    );
  });

  it("absolute imports that reach outside the specs", async () => {
    const checkout = await makeTempDir();
    const srcDir = path.join(checkout, "src");
    await fs.mkdir(srcDir);
    const lib = path.join(checkout, "lib.js");
    await fs.writeFile(lib, 'export const NAME = "abs";\n');
    await fs.writeFile(
      path.join(srcDir, "abs.ts"),
      `import { NAME } from ${JSON.stringify(lib)};\n\nexport default { name: NAME };\n`,
    );

    const error = await compileInto(srcDir).catch((e) => e);
    expect(error).toBeInstanceOf(Error);
    expect(error.message).toContain(`import of "${lib}" from `);
    expect(error.message).toMatch(/abs\.ts leaves the specs/);
  });

  it("a specs source that is a symlink", async () => {
    const link = path.join(await makeTempDir(), "src");
    await fs.symlink(fixtureSrc("ok"), link);
    await expect(compileInto(link)).rejects.toThrow(
      `specs source ${link} is a symlink`,
    );
  });

  it("bundling a dropped spec", async () => {
    await expect(compileInto(fixtureSrc("dropped-import"))).rejects.toThrow(
      "dropped spec bundled: q.ts",
    );
  });

  it("version files without a versions export", async () => {
    await expect(compileInto(fixtureSrc("no-versions"))).rejects.toThrow(
      "tool/1.0.0: no versions export",
    );
  });

  it("a top-level spec named index", async () => {
    await expect(compileInto(fixtureSrc("top-level-index"))).rejects.toThrow(
      'a top-level spec named "index" is not allowed',
    );
  });
});

describe("compileSpecs symlinks", () => {
  it("refuses symlinked files and directories", async () => {
    const srcDir = path.join(await makeTempDir(), "src");
    await fs.mkdir(srcDir);
    await fs.writeFile(
      path.join(srcDir, "real.ts"),
      'export default { name: "real" };\n',
    );
    const target = path.join(FIXTURES, "symlink-target");
    await fs.symlink(
      path.join(target, "target.ts"),
      path.join(srcDir, "linked.ts"),
    );
    await fs.symlink(target, path.join(srcDir, "linked-dir"));

    const error = await compileInto(srcDir).catch((e) => e);
    expect(error).toBeInstanceOf(Error);
    expect(error.message).toContain(
      "symlinks are not allowed in the specs (2):",
    );
    expect(error.message).toContain("\n  linked.ts");
    expect(error.message).toContain("\n  linked-dir");
  });
});

// The repo .gitignore drops node_modules/, so these trees are built at run time.
describe("compileSpecs node_modules in the specs checkout", () => {
  async function writeNested(file, contents) {
    await fs.mkdir(path.dirname(file), { recursive: true });
    await fs.writeFile(file, contents);
  }

  it("does not let it shadow a pinned package", async () => {
    const checkout = await makeTempDir();
    const srcDir = path.join(checkout, "src");
    await writeNested(
      path.join(srcDir, "node_modules", "yaml", "index.js"),
      'export default { parse: () => "shadow-in-src-5c1e" };\n',
    );
    await writeNested(
      path.join(checkout, "node_modules", "yaml", "index.js"),
      'export default { parse: () => "shadow-in-checkout-9b2d" };\n',
    );
    await writeNested(
      path.join(srcDir, "conf.ts"),
      [
        'import YAML from "yaml";',
        "",
        "const completionSpec: Fig.Spec = {",
        '  name: "conf",',
        "  generateSpec: async (tokens) => YAML.parse(tokens[0]),",
        "};",
        "",
        "export default completionSpec;",
        "",
      ].join("\n"),
    );

    const { outDir } = await compileInto(srcDir);
    const code = await fs.readFile(path.join(outDir, "conf.js"), "utf8");
    expect(code).not.toContain("shadow-in-src-5c1e");
    expect(code).not.toContain("shadow-in-checkout-9b2d");
    const conf = await importOutput(outDir, "conf.js");
    await expect(conf.default.generateSpec(["a: 1"])).resolves.toEqual({
      a: 1,
    });
    expect(await readJson(path.join(outDir, "index.json"))).toEqual({
      completions: ["conf"],
      diffVersionedCompletions: [],
    });
    expect(await listFiles(outDir)).toEqual(["conf.js", "index.json"]);
  });

  it("does not emit specs inside it", async () => {
    const srcDir = path.join(await makeTempDir(), "src");
    await writeNested(
      path.join(srcDir, "real.ts"),
      'export default { name: "real" };\n',
    );
    await writeNested(
      path.join(srcDir, "node_modules", "x", "x.ts"),
      'export default { name: "x" };\n',
    );

    const { outDir } = await compileInto(srcDir);
    expect(await readJson(path.join(outDir, "index.json"))).toEqual({
      completions: ["real"],
      diffVersionedCompletions: [],
    });
    expect(await listFiles(outDir)).toEqual(["index.json", "real.js"]);
  });
});

describe("checkOutputs", () => {
  const SRC = "/specs/src";
  const output = (entryPoint, fields = {}) => ({
    bytes: 10,
    imports: [],
    exports: ["default"],
    entryPoint,
    ...fields,
  });

  it("returns the emitted specs and summed bytes", () => {
    const outputs = {
      "../out/git.js": output("git.ts"),
      "../out/aws/s3.js": output("aws/s3.ts", { bytes: 20 }),
    };
    expect(checkOutputs(outputs, ["aws/s3", "git"], SRC)).toEqual({
      emitted: [
        { name: "git", exports: ["default"] },
        { name: "aws/s3", exports: ["default"] },
      ],
      bytes: 30,
      offenders: [],
    });
  });

  it("refuses imports left in an output", () => {
    const outputs = {
      "../out/git.js": output("git.ts", {
        imports: [{ path: "yaml", kind: "import-statement" }],
      }),
    };
    expect(checkOutputs(outputs, ["git"], SRC).offenders).toEqual([
      "../out/git.js: imports left in output: yaml",
    ]);
  });

  it("refuses an output for an unexpected entry point", () => {
    const outputs = { "../out/q.js": output("q.ts") };
    expect(checkOutputs(outputs, ["git"], SRC).offenders).toEqual([
      "unexpected output: ../out/q.js",
      "1 entry points but 0 outputs",
    ]);
  });

  it("refuses fewer outputs than entry points", () => {
    const outputs = { "../out/git.js": output("git.ts") };
    expect(checkOutputs(outputs, ["git", "hub"], SRC).offenders).toEqual([
      "2 entry points but 1 outputs",
    ]);
  });
});
