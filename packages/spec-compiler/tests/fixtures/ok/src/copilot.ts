import YAML from "yaml";

const completionSpec: Fig.Spec = {
  name: "copilot",
  args: {
    name: "service",
    generators: {
      script: ["cat", "copilot/.workspace"],
      postProcess: (out) =>
        Object.keys(YAML.parse(out)).map((name) => ({ name })),
    },
  },
};

export default completionSpec;
