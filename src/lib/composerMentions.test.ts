import { describe, expect, it } from "vitest";
import { queueMention, takeMentions } from "./composerMentions";

describe("menções pendentes do compositor", () => {
  it("entrega na ordem e esquece depois de entregar", () => {
    queueMention("p1", "@arquivo:a.ts");
    queueMention("p1", "@arquivo:b.ts");
    queueMention("p2", "@arquivo:c.ts");
    expect(takeMentions("p1")).toEqual(["@arquivo:a.ts", "@arquivo:b.ts"]);
    expect(takeMentions("p1")).toEqual([]);
    expect(takeMentions("p2")).toEqual(["@arquivo:c.ts"]);
  });
});
