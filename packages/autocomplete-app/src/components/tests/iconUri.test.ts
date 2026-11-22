import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  classifyIcon,
  cssUrl,
  isRenderableIconUri,
  templateIconUri,
  transformIconUri,
} from "../iconUri";

describe("isRenderableIconUri", () => {
  it("rejects icons from any protocol other than fig:", () => {
    for (const uri of [
      "https://raw.githubusercontent.com/x/y.png",
      "http://example.com/icon.png",
      "data:image/png;base64,AAAA",
      "file:///etc/passwd",
    ]) {
      expect(isRenderableIconUri(new URL(uri))).toBe(false);
    }
  });

  it("accepts fig: icons", () => {
    expect(isRenderableIconUri(new URL("fig://icon?type=docker"))).toBe(true);
  });
});

describe("transformIconUri", () => {
  beforeEach(() => {
    vi.stubGlobal("fig", { constants: { os: "macos" } });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("keeps icon type requests on the local fig: protocol", () => {
    const result = transformIconUri(new URL("fig://icon?type=docker"));
    expect(result.protocol).toBe("fig:");
    expect(result.hostname).toBe("icon");
    expect(result.searchParams.get("type")).toBe("docker");
    expect(result.href).not.toMatch(/^https?:/);
  });
});

describe("classifyIcon", () => {
  beforeEach(() => {
    vi.stubGlobal("fig", { constants: { os: "macos" } });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("falls back to the default icon for non-fig: protocols", () => {
    for (const icon of [
      "https://raw.githubusercontent.com/x/y.png",
      "http://e.com/a.png",
      "data:image/png;base64,AAAA",
      "file:///etc/passwd",
    ]) {
      expect(classifyIcon(icon)).toEqual({ kind: "default" });
    }
  });

  it("falls back to the default icon for unknown fig: hosts", () => {
    for (const icon of [
      "fig://evil.com/x.png",
      "fig://a@icon/x",
      "fig://icon:8080/x",
      "fig:evil.com",
    ]) {
      expect(classifyIcon(icon)).toEqual({ kind: "default" });
    }
  });

  it("renders unparseable icons as text", () => {
    expect(classifyIcon("not a url")).toEqual({ kind: "text" });
  });

  it("renders fig: icon type requests", () => {
    const result = classifyIcon("fig://icon?type=docker");
    expect(result.kind).toBe("url");
    if (result.kind !== "url") return;
    expect(result.url.protocol).toBe("fig:");
    expect(result.url.hostname).toBe("icon");
    expect(result.url.searchParams.get("type")).toBe("docker");
  });

  it("renders fig: template icons", () => {
    expect(classifyIcon("fig://template?color=3498db&badge=x").kind).toBe(
      "url",
    );
  });
});

describe("classifyIcon on windows", () => {
  beforeEach(() => {
    vi.stubGlobal("fig", { constants: { os: "windows" } });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("maps icon requests to the fig.icon host", () => {
    const result = classifyIcon("fig://icon?type=docker");
    expect(result.kind).toBe("url");
    if (result.kind !== "url") return;
    expect(result.url.hostname).toBe("fig.icon");
  });

  it("does not let a spec choose the host", () => {
    expect(classifyIcon("fig://evil.com/x.png")).toEqual({ kind: "default" });
    expect(classifyIcon("fig:evil.com")).toEqual({ kind: "default" });
  });
});

describe("cssUrl", () => {
  it("quotes the url so it cannot break out of url()", () => {
    const css = cssUrl(
      new URL("fig://icon?type=x),url(https://evil.example/t.png"),
    );
    expect(css.startsWith('url("')).toBe(true);
    expect(css.endsWith('")')).toBe(true);
    expect(css.slice(5, -2)).not.toMatch(/["\\]/);
  });

  it("percent-encodes backslashes and double quotes", () => {
    const backslash = cssUrl(new URL("fig://icon/a\\b"));
    expect(backslash).toContain("%5C");
    expect(backslash.slice(5, -2)).not.toMatch(/["\\]/);

    const quote = cssUrl(new URL('fig://a"b/x'));
    expect(quote).toContain("%22");
    expect(quote.slice(5, -2)).not.toMatch(/["\\]/);
  });
});

describe("templateIconUri", () => {
  beforeEach(() => {
    vi.stubGlobal("fig", { constants: { os: "macos" } });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("keeps a six digit hex colour", () => {
    expect(templateIconUri("3498db").searchParams.get("color")).toBe("3498db");
  });

  it("drops any other colour", () => {
    for (const color of ["x),url(https://evil/a.png", null, "fff"]) {
      expect(templateIconUri(color).searchParams.has("color")).toBe(false);
    }
  });
});
