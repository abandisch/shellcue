import * as wrappers from "@aws/amazon-q-developer-cli-api-bindings-wrappers";
import * as loadHelpers from "../src/loadHelpers";
import { MOST_USED_SPECS } from "../src/constants";
import { vi, describe, beforeEach, afterEach, it, expect } from "vitest";

const {
  getVersionFromFullFile,
  clearSpecIndex,
  publicSpecExists,
  isDiffVersionedSpec,
} = loadHelpers;

const specData = {
  getVersionCommand: vi.fn().mockReturnValue(Promise.resolve("1.0.0")),
  default: () => {},
};

describe("test `getVersionFromFullFile`", () => {
  beforeEach(() => {
    vi.spyOn(loadHelpers, "getCachedCLIVersion").mockReturnValue(null);
  });
  afterEach(() => {
    vi.clearAllMocks();
  });
  it("missing `getVersionCommand` and working `command --version`", async () => {
    vi.spyOn(wrappers, "executeCommand").mockReturnValue(
      Promise.resolve({
        status: 0,
        stdout: "2.0.0",
        stderr: "",
      }),
    );
    const newSpecData = { ...specData, getVersionCommand: undefined };
    const version = await getVersionFromFullFile(newSpecData, "fig");
    expect(version).toEqual("2.0.0");
  });

  it("missing `getVersionCommand` and not working `command --version`", async () => {
    vi.spyOn(wrappers, "executeCommand").mockReturnValue(
      Promise.resolve({
        status: 1,
        stdout: "",
        stderr: "No command available.",
      }),
    );
    const newSpecData = { ...specData, getVersionCommand: undefined };
    const version = await getVersionFromFullFile(newSpecData, "npm");
    expect(version).toBeUndefined();
  });

  it("missing `getVersionCommand` and throwing `command --version`", async () => {
    vi.spyOn(wrappers, "executeCommand").mockReturnValue(Promise.reject());
    const newSpecData = { ...specData, getVersionCommand: undefined };
    const version = await getVersionFromFullFile(newSpecData, "npm");
    expect(version).toBeUndefined();
  });

  it("working `getVersionCommand`", async () => {
    vi.spyOn(wrappers, "executeCommand").mockReturnValue(
      Promise.resolve({
        status: 0,
        stdout: "",
        stderr: "No command available.",
      }),
    );
    const version = await getVersionFromFullFile(specData, "node");
    expect(version).toEqual("1.0.0");
  });
});

describe("bundled spec index", () => {
  const index = {
    completions: ["git", "aws"],
    diffVersionedCompletions: ["aws"],
  };

  beforeEach(() => {
    clearSpecIndex();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    clearSpecIndex();
  });

  it("requests only spec://localhost/index.json", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue({ json: () => Promise.resolve(index) });
    vi.stubGlobal("fetch", fetchMock);
    await publicSpecExists("git");
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock).toHaveBeenCalledWith("spec://localhost/index.json");
  });

  it("reflects a successfully loaded index", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue({ json: () => Promise.resolve(index) });
    vi.stubGlobal("fetch", fetchMock);
    expect(await publicSpecExists("git")).toBe(true);
    expect(await publicSpecExists("missing")).toBe(false);
    expect(await isDiffVersionedSpec("aws")).toBe(true);
    expect(await isDiffVersionedSpec("git")).toBe(false);
    for (const [url] of fetchMock.mock.calls) {
      expect(String(url).startsWith("spec://localhost/")).toBe(true);
    }
  });

  it("treats a failing fetch as an empty index", async () => {
    const fetchMock = vi.fn().mockRejectedValue(new Error("offline"));
    vi.stubGlobal("fetch", fetchMock);
    await expect(publicSpecExists("git")).resolves.toBe(false);
    await expect(isDiffVersionedSpec("aws")).resolves.toBe(false);
    expect(fetchMock.mock.calls.length).toBeGreaterThanOrEqual(1);
    for (const [url] of fetchMock.mock.calls) {
      expect(String(url).startsWith("spec://localhost/")).toBe(true);
    }
  });
});

describe("MOST_USED_SPECS", () => {
  it("does not preload the removed q spec", () => {
    expect(MOST_USED_SPECS).not.toContain("q");
  });
});
