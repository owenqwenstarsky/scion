"use client";

import { call, messageOf } from "@/lib/call";
import { useParams } from "next/navigation";
import { useEffect, useState } from "react";

type Grant = { username: string; role: string };

export default function MembersPage() {
  const params = useParams<{ owner: string; name: string }>();
  const [grants, setGrants] = useState<Grant[]>([]);
  const [error, setError] = useState("");

  async function load() {
    const result = await call(`/v1/repos/${params.owner}/${params.name}/grants`, "GET");
    setGrants(((result.json as { grants?: Grant[] }).grants) ?? []);
  }

  useEffect(() => {
    let cancelled = false;
    call(`/v1/repos/${params.owner}/${params.name}/grants`, "GET").then((result) => {
      if (!cancelled) setGrants(((result.json as { grants?: Grant[] }).grants) ?? []);
    });
    return () => {
      cancelled = true;
    };
  }, [params.owner, params.name]);

  async function onSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    const result = await call(`/v1/repos/${params.owner}/${params.name}/grants`, "PUT", {
      username: form.get("username"),
      role: form.get("role"),
    });
    if (result.status !== 204) {
      setError(messageOf(result.json, "grant failed"));
      return;
    }
    setError("");
    await load();
  }

  return (
    <>
      <h1>Members</h1>
      <table>
        <tbody>
          {grants.map((grant) => (
            <tr key={grant.username}><td>{grant.username}</td><td>{grant.role}</td></tr>
          ))}
        </tbody>
      </table>
      <form className="card" onSubmit={onSubmit}>
        <div className="row">
          <input name="username" placeholder="username" required />
          <select name="role" defaultValue="read">
            <option>read</option>
            <option>write</option>
            <option>maintain</option>
            <option>admin</option>
          </select>
        </div>
        {error ? <p className="error">{error}</p> : null}
        <button type="submit">Grant</button>
      </form>
    </>
  );
}
