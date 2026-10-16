"use client";

export async function call(path: string, method: string, body?: unknown) {
  const response = await fetch("/api/proxy", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path, method, body }),
  });
  const text = await response.text();
  let json: { error?: { message?: string } } & Record<string, unknown> = {};
  if (text) {
    try {
      json = JSON.parse(text) as typeof json;
    } catch {
      json = { error: { message: text } };
    }
  }
  return { status: response.status, json };
}

export function messageOf(json: { error?: { message?: string } }, fallback: string) {
  return json.error?.message ?? fallback;
}
