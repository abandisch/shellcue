import * as semver from "semver";
import { withTimeout } from "@shellcue/shared/utils";
import { executeCommand } from "@shellcue/api-bindings-wrappers";
import z from "zod";
import { MOST_USED_SPECS } from "./constants.js";

export type SpecFileImport =
  | {
      default: Fig.Spec;
      getVersionCommand?: Fig.GetVersionCommand;
    }
  | {
      default: Fig.Subcommand;
      versions: Fig.VersionDiffMap;
    };

// TODO: this is a problem for diff-versioned specs
export async function importFromBundledSpecs<T = SpecFileImport>(
  name: string,
): Promise<T> {
  return withTimeout(
    20000,
    import(
      /* @vite-ignore */
      `spec://localhost/${name}.js`
    ),
  );
}

async function jsonFromBundledSpecs(path: string): Promise<unknown> {
  return fetch(`spec://localhost/${path}.json`).then((res) => res.json());
}

const cachedCLIVersions: Record<string, string | undefined> = {};

export const getCachedCLIVersion = (key: string) =>
  cachedCLIVersions[key] ?? null;

export async function getVersionFromFullFile(
  specData: SpecFileImport,
  name: string,
) {
  // if the default export is a function it is a versioned spec
  if (typeof specData.default === "function") {
    try {
      const storageKey = `cliVersion-${name}`;
      const version = getCachedCLIVersion(storageKey);
      if (version !== null) {
        return version;
      }

      if ("getVersionCommand" in specData && specData.getVersionCommand) {
        const newVersion = await specData.getVersionCommand(executeCommand);
        cachedCLIVersions[storageKey] = newVersion;
        return newVersion;
      }

      const newVersion = semver.clean(
        (
          await executeCommand({
            command: name,
            args: ["--version"],
          })
        ).stdout,
      );
      if (newVersion) {
        cachedCLIVersions[storageKey] = newVersion;
        return newVersion;
      }
    } catch {
      /**/
    }
  }
  return undefined;
}

// TODO: cache this request using SWR strategy
let publicSpecsRequest:
  | Promise<{
      completions: Set<string>;
      diffVersionedSpecs: Set<string>;
    }>
  | undefined;

export function clearSpecIndex() {
  publicSpecsRequest = undefined;
}

const INDEX_ZOD = z.object({
  completions: z.array(z.string()),
  diffVersionedCompletions: z.array(z.string()),
});

const createPublicSpecsRequest = async () => {
  if (publicSpecsRequest === undefined) {
    publicSpecsRequest = jsonFromBundledSpecs("index")
      .then(INDEX_ZOD.parse)
      .then((index) => ({
        completions: new Set(index.completions),
        diffVersionedSpecs: new Set(index.diffVersionedCompletions),
      }))
      .catch(() => {
        publicSpecsRequest = undefined;
        return { completions: new Set(), diffVersionedSpecs: new Set() };
      });
  }
  return publicSpecsRequest;
};

export async function publicSpecExists(name: string): Promise<boolean> {
  const { completions } = await createPublicSpecsRequest();
  return completions.has(name);
}

export async function isDiffVersionedSpec(name: string): Promise<boolean> {
  const { diffVersionedSpecs } = await createPublicSpecsRequest();
  return diffVersionedSpecs.has(name);
}

export async function preloadSpecs(): Promise<SpecFileImport[]> {
  return Promise.all(
    MOST_USED_SPECS.map(async (name) => {
      // TODO: refactor everything to allow the correct diff-versioned specs to be loaded
      // too, now we are only loading the index
      if (await isDiffVersionedSpec(name)) {
        return importFromBundledSpecs(`${name}/index`);
      }
      return importFromBundledSpecs(name);
    }).map((promise) => promise.catch((e) => e)),
  );
}
