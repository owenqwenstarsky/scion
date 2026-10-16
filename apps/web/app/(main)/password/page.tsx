"use client";

import { call, messageOf } from "@/lib/call";
import { useRouter } from "next/navigation";
import { useState } from "react";

export default function PasswordPage() {
  const router = useRouter();
  const [error, setError] = useState("");

  async function onSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    const result = await call("/v1/auth/change-password", "POST", {
      current: form.get("current"),
      new_password: form.get("new_password"),
    });
    if (result.status !== 204 && result.status !== 200) {
      setError(messageOf(result.json, "could not change password"));
      return;
    }
    router.push("/login");
    router.refresh();
  }

  return (
    <>
      <h1>Change password</h1>
      <p className="muted">The temporary password only works until you replace it. This signs every session out.</p>
      <form className="card" onSubmit={onSubmit}>
        <label htmlFor="current">Current password</label>
        <input id="current" name="current" type="password" required />
        <label htmlFor="new_password">New password</label>
        <input id="new_password" name="new_password" type="password" minLength={12} required />
        {error ? <p className="error">{error}</p> : null}
        <button type="submit">Update password</button>
      </form>
    </>
  );
}
