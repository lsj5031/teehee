import { expect, open, test } from "nsh/test";

test("sets up a stream and switches roles", async () => {
  const app = await open("./App.tsx", {
    platform: "windows",
    width: 1060,
    height: 720,
  });

  expect(app.texts()).toContain("Start a stream");
  expect(app.texts()).toContain("Start streaming");

  await app.press("Test tone");
  expect(app.texts()).toContain(
    "The test tone plays locally; it is not captured from Windows.",
  );

  await app.press("Receive");
  expect(app.texts()).toContain("Start listening");

  await app.press("Send");
  expect(app.texts()).toContain("Start a stream");
});

test("restores a saved receiver from app storage", async () => {
  const app = await open("./App.tsx", {
    platform: "windows",
    width: 1060,
    height: 720,
    storage: {
      "teehee.receivers.v1": JSON.stringify([
        {
          id: "manual-192.168.1.111:5000",
          name: "Mac",
          host: "192.168.1.111",
          port: 5000,
        },
      ]),
      "teehee.selected-receiver.v1": "manual-192.168.1.111:5000",
    },
  });

  await app.waitUntil(() => app.texts().includes("Mac"));
  expect(app.texts()).toContain("Mac");
});

test("keeps the active receiver when selecting the next receiver", async () => {
  const first = {
    id: "manual-127.0.0.1:55001",
    name: "Office speakers",
    host: "127.0.0.1",
    port: 55001,
  };
  const next = {
    id: "manual-127.0.0.1:55002",
    name: "Kitchen speakers",
    host: "127.0.0.1",
    port: 55002,
  };
  const app = await open("./App.tsx", {
    platform: "windows",
    width: 1060,
    height: 720,
    storage: {
      "teehee.receivers.v1": JSON.stringify([first, next]),
      "teehee.selected-receiver.v1": first.id,
      "teehee.preferences.v1": JSON.stringify({ source: "sine" }),
    },
  });

  await app.waitUntil(() => app.texts().includes(first.name));
  await app.press("Start streaming");
  await app.waitUntil(() => app.texts().includes("Live"));
  await app.press("Pause");
  await app.press("Receivers");
  expect(app.texts()).toContain("Choose a receiver for the next stream.");
  await app.press(next.name);
  expect(app.texts()).toContain("Paused");
  expect(app.texts()).toContain(first.name);
  expect(app.texts()).toContain("127.0.0.1:55001");
  expect(app.texts()).toContain("Office speakers · 127.0.0.1:55001");
  expect(app.texts()).not.toContain(next.name);
  await app.waitUntil(
    () => app.storage()["teehee.selected-receiver.v1"] === next.id,
  );

  await app.press("Audio");
  expect(app.texts()).toContain(first.name);
  expect(app.texts()).toContain("127.0.0.1:55001");
  expect(app.texts()).not.toContain(next.name);
  await app.press("Stream");
  await app.press("Stop streaming");
  expect(app.texts()).toContain("Start a stream");
  await app.press("Start streaming");
  expect(app.texts()).toContain("Kitchen speakers · 127.0.0.1:55002");
  expect(app.texts()).not.toContain(first.name);
  await app.press("Stop streaming");
});

test("writes audio preferences to app storage", async () => {
  const app = await open("./App.tsx", {
    platform: "windows",
    width: 1060,
    height: 720,
  });

  await app.press("Audio");
  await app.press("Stable");
  await app.press("9000");
  await app.waitUntil(() => {
    const stored = app.storage()["teehee.preferences.v1"];
    return (
      typeof stored === "string" &&
      stored.includes('"chunkMs":20') &&
      stored.includes('"mtu":9000')
    );
  });
});
