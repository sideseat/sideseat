/** The label for reasoning whose text the model withheld and only signed. */
export const WITHHELD_REASONING_LABEL = "Reasoning: text omitted, signed";
/** The label for reasoning with no text whose signature, if it had one, the telemetry did not carry. */
export const OMITTED_REASONING_LABEL = "Reasoning: text omitted";

/** What a reasoning block with no text is called, or `undefined` when it has text to show. */
export function omittedReasoningLabel(text: string, signed?: boolean): string | undefined {
  if (text.trim() !== "") return undefined;
  return signed ? WITHHELD_REASONING_LABEL : OMITTED_REASONING_LABEL;
}
