"use client";

import { call, messageOf } from "@/lib/call";
import { useState } from "react";

export function CommitBox({ owner, name }: { owner: string; name: string }) {
  const [error, setError] = useState("");
  const [done, setDone] = useState("");

  async function onSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError("");
    const form = new FormData(event.currentTarget);
    const result = await call(`/v1/repos/${owner}/${name}/edits`, "POST", {
      ref: "main",
      message: form.get("message"),
      changes: [{ path: form.get("path"), op: "put", text: form.get("text") }],
    });
    if (result.status !== 200) {
      setError(messageOf(result.json, "edit failed"));
      return;
    }
    setDone(`landed ${String(result.json.landed ?? "")}`);
  }

  return (
    <form className="card" onSubmit={onSubmit}>
      <h2 style={{ marginTop: 0 }}>Write a file</h2>
      <label htmlFor="path">Path</label>
      <input id="path" name="path" required placeholder="README" />
      <label htmlFor="message">Message</label>
      <input id="message" name="message" required />
      <label htmlFor="text">Text</label>
      <textarea id="text" name="text" required />
      {error ? <p className="error">{error}</p> : null}
      {done ? <p>{done}</p> : null}
      <button type="submit">Land edit</button>
    </form>
  );
}
