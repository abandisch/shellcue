import { createVersionedSpec } from "@fig/autocomplete-helpers";

export const getVersionCommand: Fig.GetVersionCommand = async () => "1.0.0";

export default createVersionedSpec("heroku", ["1.0.0"]);
