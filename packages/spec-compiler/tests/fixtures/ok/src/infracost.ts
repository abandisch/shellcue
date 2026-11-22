import { clean } from "semver";

const completionSpec: Fig.Spec = {
  name: "infracost",
  args: {
    name: "version",
    generators: {
      script: ["infracost", "--version"],
      postProcess: (out) => [{ name: clean(out) ?? out }],
    },
  },
};

export default completionSpec;
