import { NextResponse } from "next/server";
import type { NextRequest } from "next/server";

const OPEN = ["/login", "/api/login", "/api/health"];

export function proxy(request: NextRequest) {
  const path = request.nextUrl.pathname;
  if (OPEN.some((item) => path === item || path.startsWith(`${item}/`))) {
    return NextResponse.next();
  }
  if (!request.cookies.get("__Host-scion_session")) {
    const url = request.nextUrl.clone();
    url.pathname = "/login";
    url.search = "";
    return NextResponse.redirect(url);
  }
  const headers = new Headers(request.headers);
  headers.set("x-pathname", request.nextUrl.pathname);
  return NextResponse.next({ request: { headers } });
}

export const config = {
  matcher: ["/((?!_next/static|_next/image|favicon.ico).*)"],
};
