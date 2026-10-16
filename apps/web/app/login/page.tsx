"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";

export default function LoginPage() {
  const router = useRouter();
  const [error, setError] = useState("");

  async function onSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError("");
    const form = new FormData(event.currentTarget);
    const response = await fetch("/api/login", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        username: form.get("username"),
        password: form.get("password"),
      }),
    });
    const body = await response.json();
    if (!response.ok) {
      setError(body.error ?? "invalid username or password");
      return;
    }
    router.push(body.must_change_password ? "/password" : "/");
    router.refresh();
  }

  return (
    <main className="login-wrap">
      <h1>Scion</h1>
      <p className="muted">Sign in with the username and password issued for this server.</p>
      <form className="card" onSubmit={onSubmit}>
        <label htmlFor="username">Username</label>
        <input id="username" name="username" autoComplete="username" required />
        <label htmlFor="password">Password</label>
        <input id="password" name="password" type="password" autoComplete="current-password" required />
        {error ? <p className="error">{error}</p> : null}
        <button type="submit">Sign in</button>
      </form>
    </main>
  );
}
