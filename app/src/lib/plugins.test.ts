import { describe, expect, it } from "vitest";
import { buildInstalledIndex, parseFxName, pluginStatus } from "./plugins";

describe("parseFxName", () => {
  it("splits format, name and vendor", () => {
    expect(parseFxName("VST3: Serum (Xfer Records)")).toMatchObject({
      fmt: "VST3",
      name: "Serum",
      vendor: "Xfer Records",
    });
    expect(parseFxName("VSTi: Kontakt 7 (Native Instruments)")).toMatchObject({
      fmt: "VSTi",
      name: "Kontakt 7",
      vendor: "Native Instruments",
    });
    expect(parseFxName("CLAP: Surge XT (Surge Synth Team)").fmt).toBe("CLAP");
    expect(parseFxName("JS: loser/waveShapingDstr")).toMatchObject({
      fmt: "JS",
      name: "loser/waveShapingDstr",
      vendor: "",
    });
  });

  it("survives names without format or vendor", () => {
    expect(parseFxName("Serum")).toMatchObject({ fmt: "", name: "Serum", vendor: "" });
    expect(parseFxName("VST: ReaEQ")).toMatchObject({ fmt: "VST", name: "ReaEQ" });
  });
});

describe("pluginStatus", () => {
  const index = buildInstalledIndex([
    "Serum (Xfer Records)",
    "!!!VSTiKontakt 7 (Native Instruments)", // instrument marker from the VST cache
    "CLAP: Surge XT (Surge Synth Team)",
    "JS: loser/waveShapingDstr",
  ]);

  it("matches by name+vendor regardless of format prefix", () => {
    expect(pluginStatus("VST3: Serum (Xfer Records)", index)).toBe("have");
    expect(pluginStatus("VSTi: Kontakt 7 (Native Instruments)", index)).toBe("have");
    expect(pluginStatus("CLAP: Surge XT (Surge Synth Team)", index)).toBe("have");
  });

  it("matches JS effects by exact path", () => {
    expect(pluginStatus("JS: loser/waveShapingDstr", index)).toBe("have");
    expect(pluginStatus("JS: someone/else", index)).toBe("missing");
  });

  it("reports genuinely absent plugins as missing", () => {
    expect(pluginStatus("VST3: Pro-Q 3 (FabFilter)", index)).toBe("missing");
  });

  it("treats REAPER's own plugins as always present", () => {
    expect(pluginStatus("VST: ReaEQ (Cockos)", index)).toBe("have");
  });

  it("never claims 'missing' when the scan yielded nothing", () => {
    expect(pluginStatus("VST3: Serum (Xfer Records)", null)).toBe("unknown");
    expect(pluginStatus("VST3: Serum (Xfer Records)", new Set())).toBe("unknown");
  });
});
