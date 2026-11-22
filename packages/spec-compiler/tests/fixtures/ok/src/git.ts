import { ai, filepaths } from "@fig/autocomplete-generators";

const completionSpec: Fig.Spec = {
  name: "git",
  icon: "fig://icon?type=git",
  subcommands: [
    {
      name: "commit",
      icon: "https://example.invalid/x.png",
      args: { name: "message", generators: ai({ name: "commit message" }) },
    },
    {
      name: "add",
      args: {
        name: "pathspec",
        generators: filepaths({ extensions: ["txt"] }),
      },
    },
  ],
};

export default completionSpec;
