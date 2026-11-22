export const denoTasks: Fig.Generator = {
  script: ["deno", "task"],
  postProcess: (out) => out.split("\n").map((name) => ({ name })),
};
