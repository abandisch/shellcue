import logger, { Logger } from "loglevel";
import { convertSubcommand, initializeDefault } from "@fig/autocomplete-shared";
import {
  withTimeout,
  SpecLocationSource,
  splitPath,
} from "@shellcue/shared/utils";
import { Subcommand, SpecLocation } from "@shellcue/shared/internal";
import { SETTINGS, getSetting } from "@shellcue/api-bindings-wrappers";
import {
  importFromBundledSpecs,
  publicSpecExists,
  SpecFileImport,
  isDiffVersionedSpec,
} from "./loadHelpers.js";
import { DisabledSpecError, MissingSpecError } from "./errors.js";
import { specCache } from "./caches.js";
import { tryResolveSpecToSubcommand } from "./tryResolveSpecToSubcommand.js";

export const serializeSpecLocation = (location: SpecLocation): string => {
  if (location.type === SpecLocationSource.GLOBAL) {
    return `global://name=${location.name}`;
  }
  return `local://path=${location.path ?? ""}&name=${location.name}`;
};

export const getSpecPath = async (
  name: string,
  cwd: string,
  isScript?: boolean,
): Promise<SpecLocation> => {
  const [path, basename] = splitPath(name);

  if (!isScript) {
    const type = SpecLocationSource.GLOBAL;

    // If `isScript` is undefined, we are parsing the first token, and
    // any path with a / is a script.
    if (isScript === undefined) {
      // special-case: Symfony has "bin/console" which can be invoked directly
      // and should not require a user to create script completions for it
      if (name === "bin/console" || name.endsWith("/bin/console")) {
        return { name: "php/bin-console", type };
      }
      if (!path.includes("/")) {
        return { name, type };
      }
    } else if (["/", "./", "~/"].every((prefix) => !path.startsWith(prefix))) {
      return { name, type };
    }
  }

  const type = SpecLocationSource.LOCAL;
  if (path.startsWith("/") || path.startsWith("~/")) {
    return { name: basename, type, path };
  }

  const relative = path.startsWith("./") ? path.slice(2) : path;
  return { name: basename, type, path: `${cwd}/${relative}` };
};

type ResolvedSpecLocation =
  | { type: "public"; name: string }
  | { type: "private"; namespace: string; name: string };

export const importSpecFromLocation = async (
  specLocation: SpecLocation,
  localLogger: Logger = logger,
): Promise<{
  specFile: SpecFileImport;
  resolvedLocation?: ResolvedSpecLocation;
}> => {
  let specFile: SpecFileImport | undefined;
  let resolvedLocation: ResolvedSpecLocation | undefined;

  // Bundled specs only: no other location may supply spec code.
  if (specLocation.type === SpecLocationSource.GLOBAL) {
    const { name, diffVersionedFile: versionFileName } = specLocation;

    if (await publicSpecExists(name)) {
      try {
        const result = await importFromBundledSpecs(
          versionFileName ? `${name}/${versionFileName}` : name,
        );

        specFile = result;
        resolvedLocation = { type: "public", name };
      } catch (err) {
        localLogger.error("Unable to load bundled spec", err);
        throw err;
      }
    }
  }

  if (!specFile) {
    throw new MissingSpecError("No spec found");
  }

  return { specFile, resolvedLocation };
};

export const loadFigSubcommand = async (
  specLocation: SpecLocation,
  _context?: Fig.ShellContext,
  localLogger: Logger = logger,
): Promise<Fig.Subcommand> => {
  const { name } = specLocation;
  const location = (await isDiffVersionedSpec(name))
    ? { ...specLocation, diffVersionedFile: "index" }
    : specLocation;
  const { specFile } = await importSpecFromLocation(location, localLogger);
  const subcommand = await tryResolveSpecToSubcommand(specFile, specLocation);
  return subcommand;
};

export const loadSubcommandCached = async (
  specLocation: SpecLocation,
  context?: Fig.ShellContext,
  localLogger: Logger = logger,
): Promise<Subcommand> => {
  const { name, type: source } = specLocation;
  const path =
    specLocation.type === SpecLocationSource.LOCAL ? specLocation.path : "";

  // Do not load completion spec for commands that are 'disabled' by user
  const disabledSpecs =
    <string[]>getSetting(SETTINGS.DISABLE_FOR_COMMANDS) || [];
  if (disabledSpecs.includes(name)) {
    localLogger.info(`Not getting path for disabled spec ${name}`);
    throw new DisabledSpecError("Command requested disabled completion spec");
  }

  const key = [source, path || "", name].join(",");
  if (specCache.has(key)) {
    return specCache.get(key) as Subcommand;
  }

  const subcommand = await withTimeout(
    5000,
    loadFigSubcommand(specLocation, context, localLogger),
  );
  const converted = convertSubcommand(subcommand, initializeDefault);
  specCache.set(key, converted);
  return converted;
};
