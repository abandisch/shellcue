import {
  executeCommand,
  fread,
  SettingsMap,
  updateSettings,
} from "@shellcue/api-bindings-wrappers";
import { SpecLocationSource } from "@shellcue/shared/utils";
import {
  getSpecPath,
  loadFigSubcommand,
  loadSubcommandCached,
} from "../src/loadSpec";
import * as loadHelpers from "../src/loadHelpers";
import {
  expect,
  it,
  beforeAll,
  describe,
  beforeEach,
  vi,
  Mock,
  afterEach,
} from "vitest";

vi.mock("../src/loadHelpers", () => ({
  importFromBundledSpecs: vi
    .fn()
    .mockResolvedValue({ default: { name: "loadFromBundle" } }),
  publicSpecExists: vi.fn(async (name: string) => name === "git"),
  getVersionFromFullFile: vi.fn(),
  isDiffVersionedSpec: vi.fn(),
}));

vi.mock("@shellcue/api-bindings-wrappers", async () => ({
  ...(await vi.importActual("@shellcue/api-bindings-wrappers")),
  executeCommand: vi.fn(),
  fread: vi.fn(),
}));

const LEGACY_DEV_SETTINGS: Record<string, unknown> = {
  "autocomplete.developerMode": true,
  "autocomplete.developerModeNPM": true,
  "autocomplete.developerModeNPMInvalidateCache": true,
  "autocomplete.devCompletionsFolder": "~/some-folder/",
  "autocomplete.devCompletionsServerPort": 3005,
  "autocomplete.personalShortcutsToken": "+",
};

beforeAll(() => {
  updateSettings({});
});

describe("getSpecPath", () => {
  const cwd = "test_cwd";

  it("works", async () => {
    expect(await getSpecPath("git", cwd)).toEqual({
      type: SpecLocationSource.GLOBAL,
      name: "git",
    });
  });

  it("works for specs containing a slash in the name", async () => {
    expect(
      await getSpecPath("@withfig/autocomplete-tools", cwd, false),
    ).toEqual({
      type: SpecLocationSource.GLOBAL,
      name: "@withfig/autocomplete-tools",
    });
  });

  it("works for scripts containing a slash in the name", async () => {
    expect(await getSpecPath("@withfig/autocomplete-tools", cwd)).toEqual({
      type: SpecLocationSource.LOCAL,
      name: "autocomplete-tools",
      path: `${cwd}/@withfig/`,
    });
  });

  it("works properly with local commands", async () => {
    expect(await getSpecPath("./test", cwd)).toEqual({
      type: SpecLocationSource.LOCAL,
      name: "test",
      path: `${cwd}/`,
    });
    expect(await getSpecPath("~/test", cwd)).toEqual({
      type: SpecLocationSource.LOCAL,
      path: `~/`,
      name: "test",
    });
    expect(await getSpecPath("/test", cwd)).toEqual({
      type: SpecLocationSource.LOCAL,
      path: `/`,
      name: "test",
    });
    expect(await getSpecPath("/dir/test", cwd)).toEqual({
      type: SpecLocationSource.LOCAL,
      path: `/dir/`,
      name: "test",
    });
    expect(await getSpecPath("~/dir/test", cwd)).toEqual({
      type: SpecLocationSource.LOCAL,
      path: `~/dir/`,
      name: "test",
    });
    expect(await getSpecPath("./dir/test", cwd)).toEqual({
      type: SpecLocationSource.LOCAL,
      path: `${cwd}/dir/`,
      name: "test",
    });
  });

  it("resolves ? as an ordinary name without running a command", async () => {
    expect(await getSpecPath("?", cwd)).toEqual({
      type: SpecLocationSource.GLOBAL,
      name: "?",
    });
    expect(executeCommand).not.toHaveBeenCalled();
  });

  it("resolves + as an ordinary name", async () => {
    updateSettings(LEGACY_DEV_SETTINGS as SettingsMap);
    expect(await getSpecPath("+", cwd)).toEqual({
      type: SpecLocationSource.GLOBAL,
      name: "+",
    });
    updateSettings({});
  });
});

describe("loadFigSubcommand", () => {
  const cwd = "test_cwd";

  const expectNoSpec = async (specLocation: Fig.SpecLocation) => {
    await expect(loadFigSubcommand(specLocation)).rejects.toThrow(
      "No spec found",
    );
    expect(loadHelpers.importFromBundledSpecs).not.toHaveBeenCalled();
    expect(fread).not.toHaveBeenCalled();
    expect(executeCommand).not.toHaveBeenCalled();
  };

  beforeEach(() => {
    (loadHelpers.isDiffVersionedSpec as Mock).mockResolvedValue(false);
    updateSettings({});
  });

  afterEach(() => {
    vi.clearAllMocks();
    updateSettings({});
  });

  it("loads a spec from the bundled index", async () => {
    const result = await loadFigSubcommand({
      name: "git",
      type: SpecLocationSource.GLOBAL,
    });
    expect(loadHelpers.isDiffVersionedSpec).toHaveBeenCalledTimes(1);
    expect(loadHelpers.importFromBundledSpecs).toHaveBeenCalledTimes(1);
    expect(loadHelpers.importFromBundledSpecs).toHaveBeenCalledWith("git");
    expect(result.name).toBe("loadFromBundle");
    expect(fread).not.toHaveBeenCalled();
  });

  it("loads no spec for a local location", async () => {
    await expectNoSpec(await getSpecPath("./dir/test", cwd));
    await expectNoSpec(await getSpecPath("~/test", cwd));
    await expectNoSpec({ name: "git", type: SpecLocationSource.LOCAL });
  });

  it("loads no spec for the ? token", async () => {
    await expectNoSpec(await getSpecPath("?", cwd));
  });

  it("loads no spec for the + token", async () => {
    await expectNoSpec(await getSpecPath("+", cwd));
  });

  it("loads no spec for a name absent from the bundled index", async () => {
    await expectNoSpec({
      name: "not-bundled",
      type: SpecLocationSource.GLOBAL,
    });
    expect(loadHelpers.publicSpecExists).toHaveBeenCalledWith("not-bundled");
  });

  it("ignores former developer-mode settings", async () => {
    updateSettings(LEGACY_DEV_SETTINGS as SettingsMap);

    const result = await loadFigSubcommand({
      name: "git",
      type: SpecLocationSource.GLOBAL,
    });
    expect(result.name).toBe("loadFromBundle");
    expect(loadHelpers.importFromBundledSpecs).toHaveBeenCalledTimes(1);
    expect(loadHelpers.importFromBundledSpecs).toHaveBeenCalledWith("git");
    expect(fread).not.toHaveBeenCalled();
    expect(executeCommand).not.toHaveBeenCalled();

    vi.clearAllMocks();
    (loadHelpers.isDiffVersionedSpec as Mock).mockResolvedValue(false);
    await expectNoSpec({ name: "git", type: SpecLocationSource.LOCAL });
    await expectNoSpec(await getSpecPath("+", cwd));
  });

  it("ignores former developer-mode settings when loading through the cache", async () => {
    updateSettings(LEGACY_DEV_SETTINGS as SettingsMap);

    await expect(
      loadSubcommandCached({
        name: "not-bundled",
        type: SpecLocationSource.GLOBAL,
      }),
    ).rejects.toThrow("No spec found");
    expect(loadHelpers.importFromBundledSpecs).not.toHaveBeenCalled();
    expect(fread).not.toHaveBeenCalled();
    expect(executeCommand).not.toHaveBeenCalled();
  });
});

describe("loadSubcommandCached", () => {
  // This is broken right now...
  it.todo("works", async () => {
    const oldLoadSpec = loadFigSubcommand;
    (loadFigSubcommand as Mock) = vi.fn();
    (loadFigSubcommand as Mock).mockResolvedValue({ name: "exampleSpec" });
    const context: Fig.ShellContext = {
      currentWorkingDirectory: "",
      currentProcess: "",
      sshPrefix: "",
      environmentVariables: {},
    };

    await loadSubcommandCached(
      { name: "git", type: SpecLocationSource.LOCAL },
      context,
    );
    await loadSubcommandCached(
      { name: "git", type: SpecLocationSource.LOCAL },
      context,
    );
    expect(loadFigSubcommand).toHaveBeenCalledTimes(1);

    await loadSubcommandCached(
      { name: "hg", type: SpecLocationSource.LOCAL },
      context,
    );
    expect(loadFigSubcommand).toHaveBeenCalledTimes(2);
    (loadFigSubcommand as unknown) = oldLoadSpec;
  });
});
