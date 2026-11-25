import { createErrorInstance } from "@shellcue/shared/errors";

// LoadSpecErrors
export const MissingSpecError = createErrorInstance("MissingSpecError");
export const WrongDiffVersionedSpecError = createErrorInstance(
  "WrongDiffVersionedSpecError",
);
export const DisabledSpecError = createErrorInstance("DisabledSpecError");

// ParsingErrors
export const ParsingHistoryError = createErrorInstance("ParsingHistoryError");

export const ParseArgumentsError = createErrorInstance("ParseArgumentsError");
export const UpdateStateError = createErrorInstance("UpdateStateError");
