import fs from "node:fs";
import { Agent, fetch as undiciFetch } from "undici";

export type ScionResult = { status: number; json: unknown };

let dispatcher: Agent | undefined;
let dispatcherReady = false;

function agent(): Agent | undefined {
  if (dispatcherReady) return dispatcher;
  dispatcherReady = true;
  const caFile = process.env.SCION_CA_FILE;
  if (caFile) {
    dispatcher = new Agent({ connect: { ca: fs.readFileSync(caFile) } });
  } else if (process.env.SCION_TLS_INSECURE === "1") {
    dispatcher = new Agent({ connect: { rejectUnauthorized: false } });
  }
  return dispatcher;
}

function webToken(): string | undefined {
  if (process.env.SCION_WEB_TOKEN) return process.env.SCION_WEB_TOKEN;
  const file = process.env.SCION_WEB_TOKEN_FILE;
  if (!file) return undefined;
  return fs.readFileSync(file, "utf8").trim();
}

export async function scion(
  path: string,
  init: { method?: string; body?: unknown; session?: string; csrf?: string } = {},
): Promise<ScionResult> {
  const base = process.env.SCION_SERVER_URL;
  if (!base) throw new Error("SCION_SERVER_URL is not set");
  const headers: Record<string, string> = { accept: "application/json" };
  const token = webToken();
  if (token) headers.authorization = `Bearer ${token}`;
  if (init.session) headers["x-scion-session"] = init.session;
  if (init.csrf) headers["x-scion-csrf"] = init.csrf;
  let body: string | undefined;
  if (init.body !== undefined) {
    headers["content-type"] = "application/json";
    body = JSON.stringify(init.body);
  }
  const response = await undiciFetch(`${base.replace(/\/$/, "")}${path}`, {
    method: init.method ?? "GET",
    headers,
    body,
    dispatcher: agent(),
  });
  const text = await response.text();
  let json: unknown = null;
  if (text) {
    try {
      json = JSON.parse(text);
    } catch {
      json = { error: { message: text } };
    }
  }
  return { status: response.status, json };
}

export function errorMessage(json: unknown, fallback: string): string {
  if (json && typeof json === "object" && "error" in json) {
    const error = (json as { error?: { message?: string } }).error;
    if (error?.message) return error.message;
  }
  return fallback;
}
