import { describe, expect, it } from "vitest";
import { cleanRelativeFolder } from "./mediaFolder";

describe("cleanRelativeFolder", () => {
  it("keeps folders inside the project", () => {
    expect(cleanRelativeFolder("Media")).toBe("Media");
    expect(cleanRelativeFolder("Audio\\Takes/")).toBe("Audio/Takes");
    expect(cleanRelativeFolder("a//b")).toBe("a/b");
  });

  it("refuses anything that could leave it", () => {
    for (const bad of ["", "/Users/x/Media", "\\\\server\\share", "C:\\Media", "../Media", "Media/..", "Media.", "Me|dia"]) {
      expect(cleanRelativeFolder(bad)).toBe("");
    }
  });
});
