import fs from "node:fs/promises";
import { compileSpecs } from "./compile.js";

const USAGE = "usage: node src/cli.js --src <dir> --out <dir>";

function parseArgs(argv) {
  const values = new Map();
  for (let i = 0; i < argv.length; i += 2) {
    const flag = argv[i];
    const value = argv[i + 1];
    if (!["--src", "--out"].includes(flag) || values.has(flag) || !value) {
      return null;
    }
    values.set(flag, value);
  }
  if (values.size !== 2) return null;
  return { srcDir: values.get("--src"), outDir: values.get("--out") };
}

async function outDirUsable(outDir) {
  try {
    const stat = await fs.lstat(outDir);
    return stat.isDirectory() && (await fs.readdir(outDir)).length === 0;
  } catch (error) {
    if (error.code === "ENOENT") return true;
    throw error;
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args === null) {
    console.error(USAGE);
    process.exit(2);
  }
  if (!(await outDirUsable(args.outDir))) {
    console.error(
      `refusing to write to ${args.outDir}: it exists and is not an empty directory`,
    );
    process.exit(1);
  }
  const summary = await compileSpecs(args);
  console.log(`specs: ${summary.specs}`);
  console.log(`completions: ${summary.completions}`);
  console.log(`diff-versioned: ${summary.diffVersioned.join(" ")}`);
  console.log(`icons replaced: ${summary.iconsReplaced}`);
  console.log(`bytes: ${summary.bytes}`);
}

main().catch((error) => {
  console.error(`spec compile failed: ${error.message}`);
  process.exit(1);
});
