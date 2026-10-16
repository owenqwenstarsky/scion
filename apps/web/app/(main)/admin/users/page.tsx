"use client";

import { call, messageOf } from "@/lib/call";
import { useEffect, useState } from "react";

type User = { id: number; username: string; status: string; site_admin: boolean; locked_until?: number | null };

export default function UsersPage() {
  const [users, setUsers] = useState<User[]>([]);
  const [error, setError] = useState("");
  const [password, setPassword] = useState("");

  async function load() {
    const result = await call("/v1/users", "GET");
    if (result.status === 403) {
      setError("site admin required");
      return;
    }
    setUsers(((result.json as { users?: User[] }).users) ?? []);
  }

  useEffect(() => {
    let cancelled = false;
    call("/v1/users", "GET").then((result) => {
      if (cancelled) return;
      if (result.status === 403) setError("site admin required");
      else setUsers(((result.json as { users?: User[] }).users) ?? []);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  async function create(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    const result = await call("/v1/users", "POST", {
      username: form.get("username"),
      password: form.get("password"),
    });
    if (result.status !== 201) {
      setError(messageOf(result.json, "could not create user"));
      return;
    }
    setPassword(String(result.json.password ?? form.get("password") ?? ""));
    await load();
  }

  return (
    <>
      <h1>Users</h1>
      {error ? <p className="error">{error}</p> : null}
      <table>
        <tbody>
          {users.map((user) => (
            <tr key={user.id}>
              <td>{user.username}</td>
              <td>{user.status}</td>
              <td>{user.site_admin ? "admin" : ""}</td>
              <td>
                <button type="button" className="quiet" onClick={async () => {
                  await call(`/v1/users/${user.id}/status`, "POST", { status: user.status === "active" ? "disabled" : "active" });
                  await load();
                }}>{user.status === "active" ? "Disable" : "Enable"}</button>
                <button type="button" className="quiet" onClick={async () => { await call(`/v1/users/${user.id}/unlock`, "POST", {}); await load(); }}>Unlock</button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <form className="card" onSubmit={create}>
        <h2 style={{ marginTop: 0 }}>Create user</h2>
        <div className="row">
          <input name="username" placeholder="username" required />
          <input name="password" placeholder="password" required minLength={12} />
        </div>
        {password ? <p>created. password: <span className="mono">{password}</span></p> : null}
        <button type="submit">Create</button>
      </form>
    </>
  );
}
