import { encode } from "../cve1";
import { runWpilogWorker } from "./workerCheck";

export interface PageRequest {
  workerUrl: string;
  cases: { id: string; base64: string }[];
}

function fromBase64(text: string): Uint8Array {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

(window as any).runWorkerCheck = async (request: PageRequest): Promise<string[]> => {
  const lines: string[] = [];
  for (const item of request.cases) {
    const responses = await runWpilogWorker(request.workerUrl, fromBase64(item.base64));
    lines.push(encode({ case: item.id, responses }));
  }
  return lines;
};
