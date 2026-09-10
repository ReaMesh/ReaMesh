import { describe, expect, it } from "vitest";
import { slowestLink } from "@/components/LinkNotice";
import type { Peer } from "@/lib/types";

const peer = (over: Partial<Peer>): Peer => ({
  id: "p",
  name: "",
  color: "#000",
  is_self: false,
  is_owner: false,
  ...over,
});

describe("slowestLink", () => {
  const me = peer({ id: "me", is_self: true, rtt: 0 });
  const host = peer({ id: "host", is_owner: true, rtt: 620 });
  const guest = peer({ id: "g", rtt: 900 });

  it("measures a client against its host only", () => {
    const l = slowestLink([me, host, guest], false);
    expect(l?.peer.id).toBe("host");
    expect(l?.rtt).toBe(620);
  });

  it("measures the host against its slowest guest", () => {
    const l = slowestLink([me, host, guest], true);
    expect(l?.peer.id).toBe("g");
  });

  it("treats 0 / missing as not measured", () => {
    expect(slowestLink([me, peer({ id: "host", is_owner: true })], false)).toBeNull();
    expect(slowestLink([me, peer({ id: "host", is_owner: true, rtt: 0 })], false)).toBeNull();
  });

  it("never picks itself", () => {
    expect(slowestLink([peer({ id: "me", is_self: true, rtt: 5000 })], true)).toBeNull();
  });
});
