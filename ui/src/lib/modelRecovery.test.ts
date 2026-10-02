import { describe, expect, it } from "vitest";
import { hasActionableRuntimeWarning, hasLimitedMemoryProtection, recoveryRole, recoveryState } from "./modelRecovery";

describe("model recovery presentation", () => {
  it("names the task instead of exposing a long model filename", () => {
    expect(recoveryRole("chat: model.gguf")).toBe("Chat");
    expect(recoveryRole("embeddings: model.gguf")).toBe("Note search");
    expect(recoveryRole("whisper: model.bin")).toBe("Transcription");
    expect(recoveryRole("unrecognized model")).toBe("Local model");
  });
  it("treats legacy recovery records conservatively without inventing an automatic retry", () => {
    expect(recoveryState({ key: "old", label: "chat", reason: "old reason" })).toBe("cpu_only");
  });
  it("does not nag about handled fallback or memory-limit setup but preserves unfamiliar warnings", () => {
    const limited = "Native worker hard RAM containment unavailable: cgroup is not delegated";
    expect(hasLimitedMemoryProtection([limited])).toBe(true);
    expect(hasActionableRuntimeWarning([limited, "GPU headroom cannot be measured; using CPU."], [])).toBe(false);
    const record = { key: "a", label: "chat", reason: "Native exit was unexpected" };
    expect(hasActionableRuntimeWarning([record.reason], [record])).toBe(false);
    expect(hasActionableRuntimeWarning(["Unrecognized native warning"], [record])).toBe(true);
    expect(hasActionableRuntimeWarning(["GPU and CPU allocations both failed"], [record])).toBe(true);
  });
});
