import stripJsonComments from "strip-json-comments";

const completionSpec: Fig.Spec = {
  name: "rush",
  generateSpec: async () => JSON.parse(stripJsonComments("{}")),
};

export default completionSpec;
