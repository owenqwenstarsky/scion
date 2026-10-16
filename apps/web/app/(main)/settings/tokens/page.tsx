"use client";

import { call, messageOf } from "@/lib/call";
import { useEffect, useState } from "react";

type Token = { id: number; name: string; prefix: string; max_role: string };

export default function TokensPage() {
  const [tokens, setTokens] = useState<Token[]>([]);
  const [fresh, setFresh] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    call("/v1/tokens", "GET").then((result) => setTokens(((result.json as { tokens?: Token[] }).tokens) ?? []));
  }, []);

  async function onSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    const result = await call("/v1/tokens", "POST", {
      name: form.get("name"),
      max_role: form.get("max_role"),
    });
    if (result.status !== 201) {
      setError(messageOf(result.json, "could not create token"));
      return;
    }
    setFresh(String(result.json.token ?? ""));
    const listed = await call("/v1/tokens", "GET");
    setTokens(((listed.json as { tokens?: Token[] }).tokens) ?? []);
  }

  return (
    <>
      <h1>API tokens</h1>
      <p className="muted">A token can only narrow your role. The secret is shown once.</p>
      <table>
        <tbody>
          {tokens.map((token) => (
            <tr key={token.id}><td>{token.name}</td><td className="mono">{token.prefix}</td><td>{token.max_role}</td></tr>
          ))}
        </tbody>
      </table>
      <form className="card" onSubmit={onSubmit}>
        <div className="row">
          <input name="name" placeholder="name" required />
          <select name="max_role" defaultValue="read">
            <option>read</option>
            <option>write</option>
            <option>maintain</option>
            <option>admin</option>
          </select>
        </div>
        {error ? <p className="error">{error}</p> : null}
        {fresh ? <p className="mono">token {fresh}</p> : null}
        <button type="submit">Create token</button>
      </form>
    </>
  );
}
