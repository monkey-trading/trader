/** Tests of what the live store does when its reads fail or its feed goes quiet. */
import { ADDRESS, fillBody, viewBody } from "./fixtures";
import { accountMessage, harness, ok } from "./live-store-harness";
import { readAccount } from "./parsers";
import type { AccountView } from "./types";

describe("LiveStore reads", () => {
  it("keeps the reason when fills cannot be read", async () => {
    const { store, settle, open } = harness({ fills: () => Promise.resolve({ status: "unavailable", message: "node down" }) });
    await open();
    store.track(ADDRESS);
    await settle();

    expect(store.getSnapshot().fillFailures.get(ADDRESS)).toBe("node down");
  });

  it("says the whole curve is missing when the account cannot be read, tries again a few times, and clears it on success", async () => {
    let answers = 0;
    const { store, timers, settle, open } = harness({
      account: () => {
        answers += 1;
        return Promise.resolve(answers <= 3 ? { status: "unavailable", message: "node down" } : ok<AccountView>(readAccount(viewBody())));
      },
    });
    await open();
    store.track(ADDRESS);
    await settle();
    expect(store.getSnapshot().curveFailures.get(ADDRESS)).toBe("node down");
    for (let retry = 0; retry < 2; retry += 1) {
      timers.splice(0).at(-1)?.callback();
      await settle();
    }
    // The third failure is the last try: no further retry is scheduled.
    expect(answers).toBe(3);
    expect(timers.filter((t) => t.ms === 5000)).toHaveLength(0);
    expect(store.getSnapshot().curveFailures.get(ADDRESS)).toBe("node down");
    await store.refresh(ADDRESS);
    await settle();
    expect(store.getSnapshot().curveFailures.has(ADDRESS)).toBe(false);
  });

  it("takes a feed that says nothing for a minute while accounts report as lost, and connects again", async () => {
    const { store, feeds, silences, settle, open } = harness();
    await open();
    expect(silences).toHaveLength(1);
    // A message restarts the wait: the first timer's epoch is still current, but the bot said something since.
    feeds[0]?.handlers.onMessage(accountMessage("10300"));
    await settle();
    expect(silences).toHaveLength(2);

    silences.at(-1)?.callback();
    await settle();
    expect(store.getSnapshot().connection).toBe("reconnecting");
    expect(feeds[0]?.closed).toBe(true);
  });

  it("drops the reason fills could not be read once the feed brings them", async () => {
    const { store, feeds, settle, open } = harness({ fills: () => Promise.resolve({ status: "unavailable", message: "node down" }) });
    await open();
    store.track(ADDRESS);
    await settle();
    expect(store.getSnapshot().fillFailures.has(ADDRESS)).toBe(true);

    feeds[0]?.handlers.onMessage(JSON.stringify({ type: "fills", address: ADDRESS, fills: [fillBody()] }));
    await settle();
    expect(store.getSnapshot().fillFailures.has(ADDRESS)).toBe(false);
  });
});
