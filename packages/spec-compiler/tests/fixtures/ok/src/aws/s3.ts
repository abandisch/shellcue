import { regions } from "./regions";

const completionSpec: Fig.Spec = {
  name: "s3",
  args: { name: "region", suggestions: regions },
};

export default completionSpec;
