import { capitalize } from "shared-utils";

export function formatLabel(text: string): string {
  return `[${capitalize(text)}]`;
}
