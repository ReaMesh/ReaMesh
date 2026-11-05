import { describe, expect, it } from "vitest";
import { copyDaysLeft, findLink, joinInto, linkLabel, linkStateOf, type LinkedCopy } from "./copyLink";

const copy = (over: Partial<LinkedCopy> = {}): LinkedCopy => ({
  path: "D:/Songs/Nightfall",
  name: "Nightfall",
  project: "p1",
  owner: "alex",
  project_file: "Nightfall.rpp",
  hash: "base",
  ...over,
});

describe("copyDaysLeft", () => {
  const day = 864e5;
  const now = 100 * day;
  it("counts down from seven, a started day as a whole one", () => {
    expect(copyDaysLeft(now, now)).toBe(7);
    expect(copyDaysLeft(now - 1, now)).toBe(7);
    expect(copyDaysLeft(now - day, now)).toBe(6);
    expect(copyDaysLeft(now - 6.5 * day, now)).toBe(1);
  });
  it("stops at zero, and doesn't guess", () => {
    expect(copyDaysLeft(now - 7 * day, now)).toBe(0);
    expect(copyDaysLeft(now - 30 * day, now)).toBe(0);
    expect(copyDaysLeft(now + 5 * day, now)).toBe(7);
    expect(copyDaysLeft(0, now)).toBeNull();
  });
});

describe("linkLabel", () => {
  it("names a saved copy by its folder and a kept one by its session", () => {
    expect(linkLabel(copy({ path: "D:\\Songs\\Mein Mix" }))).toBe("Mein Mix");
    expect(linkLabel(copy({ path: "/R/ReaMesh/Nightfall-a1b2", kept: true }))).toBe("Nightfall");
  });
});

describe("findLink", () => {
  it("needs the project and the host", () => {
    const mine = copy();
    expect(findLink([mine], "p1", "alex")).toBe(mine);
  });

  it("ignores a host with a copied project id", () => {
    expect(findLink([copy()], "p1", "someone-else")).toBeNull();
  });

  it("ignores another project of the same host", () => {
    expect(findLink([copy()], "p2", "alex")).toBeNull();
  });

  it("matches nothing without an id or a host", () => {
    const blank = copy({ project: "", owner: "" });
    expect(findLink([blank], undefined, "alex")).toBeNull();
    expect(findLink([blank], "p1", undefined)).toBeNull();
    expect(findLink([blank], "", "")).toBeNull();
  });

  it("takes the newest link, which the list has first", () => {
    const newer = copy({ path: "D:/Newer" });
    expect(findLink([newer, copy()], "p1", "alex")).toBe(newer);
  });
});

describe("linkStateOf", () => {
  it("is the same when the copy already is what the host has", () => {
    expect(linkStateOf("hosted", "base", "hosted")).toBe("same");
    expect(linkStateOf("x", "x", "x")).toBe("same");
  });

  it("is an update when the copy is what it last agreed on", () => {
    expect(linkStateOf("base", "base", "hosted")).toBe("update");
    // The host is still hashing: the base alone can tell.
    expect(linkStateOf("base", "base", "")).toBe("update");
  });

  it("is a conflict when the copy changed", () => {
    expect(linkStateOf("mine", "base", "hosted")).toBe("conflict");
    expect(linkStateOf("mine", "base", "")).toBe("conflict");
  });

  it("is a conflict when the project file is gone, or nothing is known", () => {
    expect(linkStateOf(null, "base", "hosted")).toBe("conflict");
    expect(linkStateOf(null, "", "")).toBe("conflict");
    expect(linkStateOf("", "", "")).toBe("conflict");
  });
});

describe("joinInto", () => {
  const plain = { nofx: false, overwrite: false };

  it("goes into the folder when nothing can be lost", () => {
    expect(joinInto(copy(), "same", plain)).toEqual({ dest: copy().path });
    expect(joinInto(copy(), "update", plain)).toEqual({ dest: copy().path });
  });

  it("makes a temporary copy of a conflict by default", () => {
    expect(joinInto(copy(), "conflict", plain)).toEqual({});
  });

  it("overwrites only a conflict, and only when asked", () => {
    expect(joinInto(copy(), "conflict", { nofx: false, overwrite: true })).toEqual({
      dest: copy().path,
      overwrite: true,
    });
    expect(joinInto(copy(), "update", { nofx: false, overwrite: true })).toEqual({ dest: copy().path });
  });

  it("never uses the folder without effects", () => {
    for (const state of ["same", "update", "conflict"] as const) {
      expect(joinInto(copy(), state, { nofx: true, overwrite: true })).toEqual({});
    }
  });

  it("is a plain join without a link", () => {
    expect(joinInto(null, null, plain)).toEqual({});
  });
});
