import { scion } from "@/lib/scion";
import { setSession } from "@/lib/session";
import { NextResponse } from "next/server";

export async function POST(request: Request) {
  const body = (await request.json()) as { username?: string; password?: string };
  const result = await scion("/v1/auth/login", {
    method: "POST",
    body: { username: body.username ?? "", password: body.password ?? "" },
  });
  if (result.status !== 200 || !result.json || typeof result.json !== "object") {
    return NextResponse.json({ error: "invalid username or password" }, { status: 401 });
  }
  const payload = result.json as {
    session_token: string;
    csrf_token: string;
    must_change_password: boolean;
  };
  await setSession(payload.session_token, payload.csrf_token);
  return NextResponse.json({ must_change_password: payload.must_change_password });
}
