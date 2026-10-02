import type { RuntimeRecovery } from "./knowledge";

export function recoveryState(record: RuntimeRecovery): NonNullable<RuntimeRecovery["state"]> {
  return record.state ?? "cpu_only";
}

export function recoveryRole(label: string): string {
  if (/^chat[:\s]/i.test(label)) return "Chat";
  if (/^embeddings?[:\s]/i.test(label)) return "Note search";
  if (/^(transcription|whisper)[:\s]/i.test(label)) return "Transcription";
  return "Local model";
}

export function recoverySummary(record: RuntimeRecovery): string {
  switch (recoveryState(record)) {
    case "retry_pending": return "Faster mode will be tried once on your next request.";
    case "retrying": return "Trying faster mode. No action needed.";
    case "cpu_only": return "Slower mode selected. Faster mode will not retry on its own.";
  }
}

export function hasLimitedMemoryProtection(warnings: string[]): boolean {
  return warnings.some(warning => warning.includes("hard RAM containment unavailable"));
}

export function hasActionableRuntimeWarning(warnings: string[], records: RuntimeRecovery[]): boolean {
  return warnings.some(warning =>
    !hasLimitedMemoryProtection([warning])
    && !records.some(record => record.reason && warning.includes(record.reason))
    && !(/GPU/i.test(warning) && /\busing CPU\b/i.test(warning)));
}
