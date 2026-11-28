/* eslint-disable no-var */

declare global {
  var fig:
    | {
        constants:
          | {
              home?: string;
              themesFolder?: string;
              os?: string;
              newUriFormat?: boolean;
            }
          | undefined;
        quiet: boolean | undefined;
      }
    | undefined;

  var ipc:
    | {
        postMessage?: (message: string) => void;
      }
    | undefined;
}

export {};
