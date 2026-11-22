import { herokuDescription } from "./shared";

const completion: Fig.Subcommand = {
  name: "heroku",
  description: herokuDescription,
};

const versions: Fig.VersionDiffMap = {};

export { versions };
export default completion;
