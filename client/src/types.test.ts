import { describe, expect, it } from "vitest";
import { attack, placeBuilding, repair, sell, trainUnit, type Command } from "./types";

// The server deserializes commands with serde: `Player` is a fieldless enum,
// so `player` must be the variant name string ("P0"), NOT an index (0).
// These JSON shapes are pinned by `crucible-sim/examples/wire_probe.rs`; if
// either side drifts, live-match commands fail to parse and are dropped
// silently.
describe("command wire format", () => {
  it("serializes PlaceBuilding exactly as the server expects", () => {
    const cmd: Command = placeBuilding("Refinery", [15, 11]);
    expect(JSON.stringify(cmd)).toBe(
      '{"PlaceBuilding":{"player":"P0","btype":"Refinery","tile":[15,11]}}',
    );
  });

  it("wraps commands in the client message shape", () => {
    const msg = { type: "commands", cmds: [placeBuilding("Barracks", [8, 8])] };
    const parsed = JSON.parse(JSON.stringify(msg));
    expect(parsed.type).toBe("commands");
    expect(parsed.cmds[0].PlaceBuilding.player).toBe("P0");
  });

  it("serializes TrainUnit player as a string too", () => {
    const cmd: Command = trainUnit(4, "Infantry");
    expect(JSON.stringify(cmd)).toBe(
      '{"TrainUnit":{"player":"P0","building":4,"utype":"Infantry"}}',
    );
  });

  it("serializes PlaceBuilding with PowerPlant", () => {
    const cmd: Command = placeBuilding("PowerPlant", [10, 12]);
    expect(JSON.stringify(cmd)).toBe(
      '{"PlaceBuilding":{"player":"P0","btype":"PowerPlant","tile":[10,12]}}',
    );
  });

  it("serializes Sell and Repair commands", () => {
    expect(JSON.stringify(sell(3))).toBe('{"Sell":{"player":"P0","building":3}}');
    expect(JSON.stringify(repair(3))).toBe('{"Repair":{"player":"P0","building":3}}');
  });

  it("serializes Attack with the target entity id", () => {
    const cmd: Command = attack([7, 8], 42);
    expect(JSON.stringify(cmd)).toBe(
      '{"Attack":{"player":"P0","units":[7,8],"target":42}}',
    );
  });
});
