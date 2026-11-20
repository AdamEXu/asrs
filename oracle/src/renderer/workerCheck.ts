/**
 * Drives one wpilogWorker the same way in a browser page and under the oracle shims: `start`
 * with the file, then `parseField` for every key of the initial log in order. Every such key
 * names a start record, so each request gets exactly one `fields` reply. Returns the initial and
 * fields messages in arrival order.
 */
export function runWpilogWorker(workerUrl: string, bytes: Uint8Array): Promise<unknown[]> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(workerUrl, { type: "module" });
    const collected: unknown[] = [];
    let expected = 0;
    let received = 0;
    const finish = () => {
      worker.terminate();
      resolve(collected);
    };
    worker.onerror = (event: any) => {
      worker.terminate();
      reject(new Error(String(event?.message ?? event)));
    };
    worker.onmessage = (event: MessageEvent) => {
      const message = event.data;
      switch (message.type) {
        case "progress":
          return;
        case "failed":
          collected.push(message);
          finish();
          return;
        case "initial": {
          collected.push(message);
          const keys = Object.keys(message.log.fields);
          expected = keys.length;
          if (expected === 0) finish();
          keys.forEach((key) => worker.postMessage({ type: "parseField", key }));
          return;
        }
        case "fields":
          collected.push(message);
          if (++received === expected) finish();
          return;
      }
    };
    const copy = new Uint8Array(bytes);
    worker.postMessage({ type: "start", data: [copy] }, [copy.buffer]);
  });
}
