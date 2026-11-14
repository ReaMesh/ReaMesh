import { describe, expect, it } from "vitest";
import { buildInstalledIndex, formatMismatch, parseFxName, pluginStatus } from "./plugins";
import { pluginSite } from "./pluginSites";

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

  it("handles names without prefix or vendor", () => {
    expect(parseFxName("Serum")).toMatchObject({ fmt: "", name: "Serum", vendor: "" });
    expect(parseFxName("VST: ReaEQ")).toMatchObject({ fmt: "VST", name: "ReaEQ" });
  });

  it("drops the channel layout REAPER appends", () => {
    expect(parseFxName("VSTi: ReaSynDr (Cockos) (4 out)")).toMatchObject({
      name: "ReaSynDr",
      vendor: "Cockos",
    });
    expect(parseFxName("VST3i: Kontakt 7 (Native Instruments) (64 out)")).toMatchObject({
      name: "Kontakt 7",
      vendor: "Native Instruments",
    });
    expect(parseFxName("VST: ReaComp (Cockos) (mono)").name).toBe("ReaComp");
  });
});

describe("pluginStatus", () => {
  const index = buildInstalledIndex([
    "Serum (Xfer Records)", // no format known: matches any
    "VST3i: Kontakt 7 (Native Instruments) (64 out)",
    "CLAP: Surge XT (Surge Synth Team)",
    "VST3: Pro-C 2 (FabFilter)",
    "VST: Massive (Native Instruments)",
    "VST3: Massive (Native Instruments)",
    "JS: loser/waveShapingDstr",
  ]);

  it("matches by name whatever the vendor spelling", () => {
    expect(pluginStatus("VST3: Serum (Xfer Records)", index)).toBe("have");
    expect(pluginStatus("VST3i: Kontakt 7 (Native Instruments GmbH) (16 out)", index)).toBe("have");
    expect(pluginStatus("CLAPi: Surge XT (Surge Synth Team)", index)).toBe("have");
  });

  it("tells a plugin installed in another format from a missing one", () => {
    expect(pluginStatus("VST: Pro-C 2 (FabFilter)", index)).toBe("format");
    expect(pluginStatus("AU: Kontakt 7 (Native Instruments)", index)).toBe("format");
    expect(pluginStatus("VSTi: Massive (Native Instruments)", index)).toBe("have");
    expect(formatMismatch("VST: Pro-C 2 (FabFilter)", index)).toEqual({ need: "VST2", have: "VST3" });
    expect(formatMismatch("AU: Massive (Native Instruments)", index)).toEqual({ need: "AU", have: "VST2, VST3" });
    expect(formatMismatch("VST3: Pro-C 2 (FabFilter)", index)).toBeNull();
    expect(formatMismatch("VST3: Pro-Q 3 (FabFilter)", index)).toBeNull();
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
    expect(pluginStatus("VST3: Serum (Xfer Records)", buildInstalledIndex([]))).toBe("unknown");
  });
});

describe("pluginSite", () => {
  it("knows the site of a well-known vendor, company suffix or not", () => {
    expect(pluginSite(parseFxName("VST3: Pro-Q 3 (FabFilter)"))).toEqual({
      host: "fabfilter.com",
      url: "https://fabfilter.com",
    });
    expect(pluginSite(parseFxName("VST3i: Kontakt 7 (Native Instruments GmbH) (64 out)")).host).toBe(
      "native-instruments.com",
    );
    expect(pluginSite(parseFxName("VST3: ValhallaRoom (Valhalla DSP, LLC)")).host).toBe("valhalladsp.com");
  });

  it("falls back to a search for anything else", () => {
    expect(pluginSite(parseFxName("VST3: Odd Thing (Nobody & Sons)"))).toEqual({
      host: "",
      url: "https://duckduckgo.com/?q=Odd%20Thing%20Nobody%20%26%20Sons%20plugin",
    });
    expect(pluginSite(parseFxName("JS: saike/Squashman.jsfx")).url).toBe(
      "https://duckduckgo.com/?q=Squashman.jsfx%20JSFX%20REAPER",
    );
  });
});
