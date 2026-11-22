import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

// A trailing "/" drops the whole directory.
const DROPPED_SPECS = [
  "q",
  "cw",
  "figterm",
  "fig/",
  "@fig/publish-spec",
  "@withfig/autocomplete-tools",
  "example/",
];
const HELPER_MODULES = ["heroku/shared", "deno/generators", "aws/regions"];
const ALLOWED_PACKAGES = new Set([
  "@fig/autocomplete-generators",
  "@fig/autocomplete-helpers",
  "yaml",
  "strip-json-comments",
  "semver",
]);
const GENERATORS_WRAPPER = fileURLToPath(
  new URL("./generators.js", import.meta.url),
);
const PACKAGE_DIR = fileURLToPath(new URL("..", import.meta.url));
const STORE_DIR = fileURLToPath(
  new URL("../../../node_modules", import.meta.url),
);
const REMOTE_ICON = /\bicon:\s*"https?:\/\/[^"\\\n]*"/g;
const MAX_LISTED = 20;

const listed = (list, name) =>
  list.some((entry) =>
    entry.endsWith("/") ? name.startsWith(entry) : name === entry,
  );

const isInside = (dir, file) => {
  const rel = path.relative(dir, file);
  return rel !== "" && !path.isAbsolute(rel) && rel.split(path.sep)[0] !== "..";
};

const toSpecName = (root, file) =>
  path.relative(root, file).split(path.sep).join("/").replace(/\.ts$/, "");

const parentOf = (name) =>
  name.includes("/") ? name.slice(0, name.lastIndexOf("/")) : "";

const formatList = (items) => {
  const shown = items.slice(0, MAX_LISTED);
  if (items.length > MAX_LISTED) {
    shown.push(`... and ${items.length - MAX_LISTED} more`);
  }
  return shown.join("\n  ");
};

// node_modules directories are walked only for symlinks, never for specs.
async function collectSources(dir, found = [], links = [], specs = true) {
  for (const entry of await fs.readdir(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    // A symlink can reach outside srcDir or into our install, so any one refuses the build.
    if (entry.isSymbolicLink()) {
      links.push(full);
    } else if (entry.isDirectory()) {
      const inSpecs = specs && entry.name !== "node_modules";
      await collectSources(full, found, links, inSpecs);
    } else if (
      specs &&
      entry.isFile() &&
      entry.name.endsWith(".ts") &&
      !entry.name.endsWith(".d.ts")
    ) {
      found.push(full);
    }
  }
  return { found, links };
}

function guardPlugin(realSrc, realWrapper, realStore, counts) {
  return {
    name: "shellcue-spec-guard",
    setup(build) {
      build.onResolve({ filter: /^[^./]/ }, async (args) => {
        if (args.pluginData?.shellcueResolving) return undefined;
        if (isInside(realStore, args.importer)) return undefined;
        const fromWrapper =
          args.importer === GENERATORS_WRAPPER || args.importer === realWrapper;
        if (args.path === "@fig/autocomplete-generators" && !fromWrapper) {
          return { path: GENERATORS_WRAPPER };
        }
        if (!ALLOWED_PACKAGES.has(args.path)) {
          return {
            errors: [
              {
                text: `import of "${args.path}" from ${args.importer} is not allowed`,
              },
            ],
          };
        }
        // Never walk up from the importer: node_modules in the specs checkout must not shadow ours.
        const resolved = await build.resolve(args.path, {
          resolveDir: PACKAGE_DIR,
          kind: args.kind,
          pluginData: { shellcueResolving: true },
        });
        if (resolved.errors.length > 0) return { errors: resolved.errors };
        return { path: resolved.path };
      });
      // Lexical containment is real containment: src and everything under it are symlink-free.
      build.onResolve({ filter: /^[./]/ }, (args) => {
        if (args.kind === "entry-point") return undefined;
        if (!isInside(realSrc, args.importer)) return undefined;
        if (isInside(realSrc, path.resolve(args.resolveDir, args.path))) {
          return undefined;
        }
        return {
          errors: [
            {
              text: `import of "${args.path}" from ${args.importer} leaves the specs`,
            },
          ],
        };
      });
      build.onLoad({ filter: /\.ts$/ }, async (args) => {
        if (!isInside(realSrc, args.path)) return undefined;
        const source = await fs.readFile(args.path, "utf8");
        // Remote icons are refused; only literal URLs are found, not ones in variables.
        const contents = source.replace(REMOTE_ICON, () => {
          counts.icons += 1;
          return "icon: undefined";
        });
        return { contents, loader: "ts" };
      });
    },
  };
}

export function checkOutputs(outputs, expectedNames, realSrc) {
  const expected = new Set(expectedNames);
  const emitted = [];
  const offenders = [];
  let bytes = 0;
  for (const [file, output] of Object.entries(outputs)) {
    bytes += output.bytes;
    if (output.imports.length > 0) {
      const paths = output.imports.map(({ path: p }) => p).join(", ");
      offenders.push(`${file}: imports left in output: ${paths}`);
    }
    const name = output.entryPoint
      ? toSpecName(realSrc, path.resolve(realSrc, output.entryPoint))
      : null;
    const posixFile = `/${file.split(path.sep).join("/")}`;
    if (
      name === null ||
      !expected.has(name) ||
      !posixFile.endsWith(`/${name}.js`)
    ) {
      offenders.push(`unexpected output: ${file}`);
      continue;
    }
    emitted.push({ name, exports: output.exports });
  }
  if (emitted.length !== expected.size) {
    offenders.push(
      `${expected.size} entry points but ${emitted.length} outputs`,
    );
  }
  return { emitted, bytes, offenders };
}

export async function compileSpecs({ srcDir, outDir }) {
  if ((await fs.lstat(srcDir)).isSymbolicLink()) {
    throw new Error(`specs source ${srcDir} is a symlink`);
  }
  const realSrc = await fs.realpath(srcDir);
  const absOut = path.resolve(outDir);
  const { found, links } = await collectSources(realSrc);
  if (links.length > 0) {
    const rels = links.map((link) =>
      path.relative(realSrc, link).split(path.sep).join("/"),
    );
    throw new Error(
      `symlinks are not allowed in the specs (${links.length}):\n  ${formatList(rels)}`,
    );
  }
  const entries = found
    .map((file) => ({ file, name: toSpecName(realSrc, file) }))
    .filter(
      ({ name }) =>
        !listed(DROPPED_SPECS, name) && !HELPER_MODULES.includes(name),
    )
    .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  if (entries.length === 0) throw new Error(`no specs found in ${realSrc}`);
  // "X/index" marks X as diff-versioned, so a top-level "index" has no name.
  if (entries.some(({ name }) => name === "index")) {
    throw new Error(`a top-level spec named "index" is not allowed`);
  }

  const realWrapper = await fs.realpath(GENERATORS_WRAPPER);
  const realStore = await fs.realpath(STORE_DIR);
  const counts = { icons: 0 };
  const result = await esbuild.build({
    entryPoints: entries.map(({ file, name }) => ({ in: file, out: name })),
    outdir: absOut,
    absWorkingDir: realSrc,
    bundle: true,
    format: "esm",
    platform: "browser",
    target: "es2020",
    minify: true,
    splitting: false,
    legalComments: "none",
    write: true,
    metafile: true,
    logLevel: "silent",
    // The specs repo's tsconfig.json must not steer resolution.
    tsconfigRaw: {},
    plugins: [guardPlugin(realSrc, realWrapper, realStore, counts)],
  });

  const offenders = [];
  for (const input of Object.keys(result.metafile.inputs)) {
    let real;
    try {
      real = await fs.realpath(path.resolve(realSrc, input));
    } catch {
      offenders.push(`unresolvable input: ${input}`);
      continue;
    }
    if (isInside(realSrc, real)) {
      const name = toSpecName(realSrc, real);
      if (name.split("/").includes("node_modules")) {
        offenders.push(`node_modules inside the specs: ${input}`);
      }
      if (listed(DROPPED_SPECS, name)) {
        offenders.push(`dropped spec bundled: ${input}`);
      }
    } else if (!isInside(realStore, real) && real !== realWrapper) {
      offenders.push(`input outside the specs: ${input}`);
    }
  }

  const { emitted, bytes, offenders: outputOffenders } = checkOutputs(
    result.metafile.outputs,
    entries.map(({ name }) => name),
    realSrc,
  );
  offenders.push(...outputOffenders);

  const diffVersioned = emitted
    .map(({ name }) => name)
    .filter((name) => name.endsWith("/index"))
    .map((name) => name.slice(0, -"/index".length))
    .sort();
  const diffSet = new Set(diffVersioned);
  for (const { name, exports } of emitted) {
    if (!exports.includes("default")) {
      offenders.push(`${name}: no default export`);
    }
    if (
      diffSet.has(parentOf(name)) &&
      !name.endsWith("/index") &&
      !exports.includes("versions")
    ) {
      offenders.push(`${name}: no versions export`);
    }
  }
  if (offenders.length > 0) {
    throw new Error(
      `spec validation failed (${offenders.length}):\n  ${formatList(offenders)}`,
    );
  }

  const completions = [
    ...new Set([
      ...emitted
        .map(({ name }) => name)
        .filter((name) => !diffSet.has(parentOf(name))),
      ...diffVersioned,
    ]),
  ].sort();
  const index = { completions, diffVersionedCompletions: diffVersioned };
  await fs.writeFile(
    path.join(absOut, "index.json"),
    `${JSON.stringify(index)}\n`,
  );

  return {
    specs: emitted.length,
    completions: completions.length,
    diffVersioned,
    iconsReplaced: counts.icons,
    bytes,
  };
}
