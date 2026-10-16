import { api, clearSession } from "@/lib/session";
import { NextResponse } from "next/server";

export async function POST(request: Request) {
  await api("/v1/auth/logout", "POST", {});
  await clearSession();
  return NextResponse.redirect(new URL("/login", request.url), 303);
}
