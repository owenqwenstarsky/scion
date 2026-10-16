"use client";

import { call, messageOf } from "@/lib/call";
import { useParams } from "next/navigation";
import { useEffect, useState } from "react";

type Rule = { id: number; glob: string; can_read: boolean; can_write: boolean };

export default function PathsPage() {
  const params = useParams<{ owner: string; name: string }>();
  const [rules, setRules] = useState<Rule[]>([]);
  const [error, setError] = useState("");

  useEffect(() => {
    call(`/v1/repos/${params.owner}/${params.name}/path-rules`, "GET").then((result) => {
      setRules(((result.json as { rules?: Rule[] }).rules) ?? []);
    });
  }, [params.owner, params.name]);

  async function onSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    const result = await call(`/v1/repos/${params.owner}/${params.name}/path-rules`, "POST", {
      glob: form.get("glob"),
      username: form.get("username") || undefined,
      can_read: form.get("can_read") === "on",
      can_write: form.get("can_write") === "on",
    });
    if (result.status !== 201) {
      setError(messageOf(result.json, "could not add rule"));
      return;
    }
    window.location.reload();
  }

  return (
    <>
      <h1>Path rules</h1>
      <p className="muted">Rules only deny. A matching rule with read or write unchecked hides that access.</p>
      <ul>{rules.map((rule) => <li key={rule.id} className="mono">{rule.glob} read={String(rule.can_read)} write={String(rule.can_write)}</li>)}</ul>
      <form className="card" onSubmit={onSubmit}>
        <label htmlFor="glob">Glob</label>
        <input id="glob" name="glob" placeholder="secret/**" required />
        <label htmlFor="username">Username (optional)</label>
        <input id="username" name="username" />
        <label><input type="checkbox" name="can_read" /> can read</label>
        <label><input type="checkbox" name="can_write" /> can write</label>
        {error ? <p className="error">{error}</p> : null}
        <button type="submit">Add rule</button>
      </form>
    </>
  );
}
