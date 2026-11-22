import { denoTasks } from "./deno/generators";

const completionSpec: Fig.Spec = {
  name: "deno",
  subcommands: [
    { name: "task", args: { name: "task", generators: denoTasks } },
  ],
};

export default completionSpec;
