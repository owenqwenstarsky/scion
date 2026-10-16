import { cookies } from "next/headers";
import { redirect } from "next/navigation";
import { scion } from "./scion";

export type SessionView = {
  user_id: number;
  username: string;
  site_admin: boolean;
  must_change_password: boolean;
  via: string;
};

const cookieBase = {
  httpOnly: true,
  secure: true,
  sameSite: "lax" as const,
  path: "/",
};

export async function readCookies() {
  const jar = await cookies();
  return {
    token: jar.get("__Host-scion_session")?.value,
    csrf: jar.get("__Host-scion_csrf")?.value,
  };
}

export async function setSession(token: string, csrf: string) {
  const jar = await cookies();
  jar.set("__Host-scion_session", token, cookieBase);
  jar.set("__Host-scion_csrf", csrf, cookieBase);
}

export async function clearSession() {
  const jar = await cookies();
  jar.set("__Host-scion_session", "", { ...cookieBase, maxAge: 0 });
  jar.set("__Host-scion_csrf", "", { ...cookieBase, maxAge: 0 });
}

export async function currentSession(): Promise<SessionView | null> {
  const { token, csrf } = await readCookies();
  if (!token) return null;
  const result = await scion("/v1/auth/session", { session: token, csrf });
  if (result.status !== 200 || !result.json || typeof result.json !== "object") return null;
  return result.json as SessionView;
}

export async function requireSession(): Promise<SessionView> {
  const session = await currentSession();
  if (!session) redirect("/login");
  return session;
}

export async function api(path: string, method = "GET", body?: unknown) {
  const { token, csrf } = await readCookies();
  if (!token) redirect("/login");
  return scion(path, { method, body, session: token, csrf });
}
