// "" is the legacy fig:///path form
const RENDERABLE_FIG_HOSTS = ["icon", "asset", "template", "path", ""];

export const isRenderableIconUri = (icon: URL) =>
  icon.protocol === "fig:" &&
  RENDERABLE_FIG_HOSTS.includes(icon.hostname) &&
  icon.username === "" &&
  icon.password === "" &&
  icon.port === "" &&
  // an opaque path like fig:evil.com would become the host in transformIconUri
  (icon.pathname === "" || icon.pathname.startsWith("/"));

export const transformIconUri = (icon: URL): URL => {
  let { host } = icon;

  if (host === "" && fig.constants?.newUriFormat) {
    host = "path";
  }

  return new URL(`fig://${host}${icon.pathname}${icon.search}${icon.hash}`);
};

export const classifyIcon = (
  icon: string,
): { kind: "text" } | { kind: "default" } | { kind: "url"; url: URL } => {
  let url: URL;
  try {
    url = new URL(icon);
  } catch {
    return { kind: "text" };
  }
  return isRenderableIconUri(url)
    ? { kind: "url", url: transformIconUri(url) }
    : { kind: "default" };
};

export const templateIconUri = (color: string | null | undefined): URL =>
  transformIconUri(
    new URL(
      color && /^[0-9a-fA-F]{6}$/.test(color)
        ? `fig://template?color=${color}`
        : "fig://template",
    ),
  );

export const cssUrl = (url: URL): string =>
  `url("${url.href
    .replace(/\\/g, "%5C")
    .replace(/"/g, "%22")
    .replace(/\n/g, "%0A")
    .replace(/\r/g, "%0D")
    .replace(/\f/g, "%0C")}")`;
