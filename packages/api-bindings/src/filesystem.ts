import { create } from "@bufbuild/protobuf";
import { sendReadFileRequest } from "./requests.js";
import { type FilePath, FilePathSchema } from "@shellcue/proto/fig";

function filePath(options: Omit<FilePath, "$typeName">) {
  return create(FilePathSchema, options);
}

export async function read(path: string) {
  const response = await sendReadFileRequest({
    path: filePath({ path, expandTildeInPath: true }),
  });
  if (response.type?.case === "text") {
    return response.type.value;
  }
  return null;
}
