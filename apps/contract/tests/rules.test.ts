import assert from "node:assert/strict";
import { test } from "node:test";

import { contractRules, type ContractState } from "../src/rules.ts";

const genesis = {
  rules: "contract/1",
  parties: [
    { pubky: "alice", kid: "ak" },
    { pubky: "bob", kid: "bk" },
  ],
  witnesses: [],
  confirm_quorum: 2,
  options: {},
};

function fresh(): ContractState {
  return contractRules.init(genesis, [], []);
}

function link(author: string, kind: string, body: Record<string, unknown>) {
  return { seq: 1, round: 0, author, kind, body, ts: 0 };
}

test("an opening offer is the text, with no base and no previous", () => {
  const state = contractRules.apply(fresh(), link("alice", "offer", { id: "1", text: "Pay 10", base: "" }));
  assert.equal(state.offer?.author, 0);
  assert.equal(state.offer?.text, "Pay 10");
  assert.equal(state.offer?.base, "");
  assert.equal(state.offer?.previous, null);
  assert.equal(state.outcome, null);
  assert.equal(contractRules.status(state), null);
  assert.deepEqual(contractRules.obliged(state), [1]);
});

test("a revision must name the open offer and change the text", () => {
  let state = contractRules.apply(fresh(), link("alice", "offer", { id: "1", text: "Pay 10", base: "" }));
  assert.throws(
    () => contractRules.apply(state, link("bob", "offer", { id: "2", text: "Pay 12", base: "nope" })),
    /name the open offer/,
  );
  assert.throws(
    () => contractRules.apply(state, link("bob", "offer", { id: "2", text: "Pay 10", base: "1" })),
    /must change the text/,
  );
  state = contractRules.apply(state, link("bob", "offer", { id: "2", text: "Pay 12", base: "1" }));
  assert.equal(state.offer?.author, 1);
  assert.equal(state.offer?.text, "Pay 12");
  assert.equal(state.offer?.previous, "Pay 10");
  assert.deepEqual(contractRules.obliged(state), [0]);
});

test("the author cannot accept or reject their own offer", () => {
  const state = contractRules.apply(fresh(), link("alice", "offer", { id: "1", text: "Pay 10", base: "" }));
  assert.equal(contractRules.mayAppend(state, 0, "accept"), false);
  assert.equal(contractRules.mayAppend(state, 0, "reject"), false);
  assert.throws(() => contractRules.apply(state, link("alice", "accept", { id: "1" })), /cannot append/);
  assert.throws(() => contractRules.apply(state, link("alice", "reject", { id: "1" })), /cannot append/);
});

test("the other party accepting ends the negotiation on that text", () => {
  let state = contractRules.apply(fresh(), link("alice", "offer", { id: "1", text: "Pay 10", base: "" }));
  state = contractRules.apply(state, link("bob", "offer", { id: "2", text: "Pay 12", base: "1" }));
  state = contractRules.apply(state, link("alice", "accept", { id: "2" }));
  assert.equal(state.offer, null);
  assert.deepEqual(state.outcome, { kind: "agreed", text: "Pay 12" });
  assert.deepEqual(contractRules.status(state), { summary: "agreed" });
  assert.equal(contractRules.mayAppend(state, 0, "offer"), false);
  assert.throws(() => contractRules.apply(state, link("bob", "offer", { id: "3", text: "Pay 1", base: "" })), /ended/);
  assert.equal(contractRules.close(state, { reason: "finished", subject: [], pending: [] }).summary, "agreed");
});

test("an outright reject ends with no agreement, and a second offer is refused", () => {
  let state = contractRules.apply(fresh(), link("alice", "offer", { id: "1", text: "Pay 10", base: "" }));
  state = contractRules.apply(state, link("bob", "reject", { id: "1" }));
  assert.deepEqual(state.outcome, { kind: "rejected", text: "Pay 10" });
  assert.deepEqual(contractRules.status(state), { summary: "rejected" });
  assert.throws(() => contractRules.apply(state, link("alice", "offer", { id: "2", text: "Pay 9", base: "" })), /ended/);
  assert.equal(contractRules.close(state, { reason: "finished", subject: [], pending: [] }).summary, "rejected");
});

test("a finished close is refused while an offer is open, and walking away records no contract", () => {
  const state = contractRules.apply(fresh(), link("alice", "offer", { id: "1", text: "Pay 10", base: "" }));
  assert.throws(
    () => contractRules.close(state, { reason: "finished", subject: [], pending: [] }),
    /not ended/,
  );
  assert.equal(contractRules.close(state, { reason: "agreed", subject: [], pending: [] }).summary, "no contract");
  assert.equal(contractRules.close(state, { reason: "abandoned", subject: [], pending: [] }).summary, "no contract");
});

test("letting the negotiation go ends it for both parties, with no contract", () => {
  const state = contractRules.apply(
    contractRules.apply(fresh(), link("alice", "offer", { id: "1", text: "Pay 10", base: "" })),
    link("bob", "lapse", {}),
  );
  assert.equal(state.offer, null);
  assert.deepEqual(state.outcome, { kind: "lapsed", text: "Pay 10" });
  assert.deepEqual(contractRules.status(state), { summary: "no contract" });
  assert.equal(contractRules.mayAppend(state, 0, "lapse"), false);
  assert.equal(contractRules.mayAppend(state, 1, "offer"), false);
  assert.throws(() => contractRules.apply(state, link("alice", "lapse", {})), /ended/);
  assert.equal(contractRules.close(state, { reason: "finished", subject: [], pending: [] }).summary, "no contract");
});

test("genesis must name exactly two parties who both confirm", () => {
  assert.throws(
    () =>
      contractRules.init(
        { ...genesis, parties: [...genesis.parties, { pubky: "cara", kid: "" }] },
        [],
        [],
      ),
    /two parties/,
  );
  assert.throws(() => contractRules.init({ ...genesis, confirm_quorum: 1 }, [], []), /both parties/);
});
