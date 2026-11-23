import { fs as FileSystem } from "@shellcue/api-bindings";

export const fread = (path: string): Promise<string> =>
  FileSystem.read(path).then((out) => out ?? "");
