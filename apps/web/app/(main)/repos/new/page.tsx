"use client";

import { call, messageOf } from "@/lib/call";
import { useRouter } from "next/navigation";
import { useState } from "react";

export default function NewRepoPage() {
  const router = useRouter();
  const [error, setError] = useState("");

  async function onSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    const name = String(form.get("name") ?? "");
    const result = await call("/v1/repos", "POST", {
      name,
      description: form.get("description") ?? "",
    });
    if (result.status !== 201) {
      setError(messageOf(result.json, "could not create repository"));
      return;
    }
    const owner = String(result.json.owner ?? "");
    router.push(`/r/${owner}/${name}`);
    router.refresh();
  }

  return (
    <>
      <h1>New repository</h1>
      <form className="card" onSubmit={onSubmit}>
        <label htmlFor="name">Name</label>
        <input id="name" name="name" required pattern="[A-Za-z][A-Za-z0-9_-]{0,38}" />
        <label htmlFor="description">Description</label>
        <input id="description" name="description" />
        {error ? <p className="error">{error}</p> : null}
        <button type="submit">Create</button>
      </form>
    </>
  );
}
