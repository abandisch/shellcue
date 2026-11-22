export * from "@fig/autocomplete-generators";

// AI is removed from ShellCue.
export const ai = () => ({ custom: async () => [] });
