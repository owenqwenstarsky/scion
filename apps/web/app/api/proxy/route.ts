import { readCookies } from "@/lib/session";
import { scion } from "@/lib/scion";

export async function POST(request: Request) {
  const { token, csrf } = await readCookies();
  if (!token || !csrf) return Response.json({ error: { message: "authentication required" } }, { status: 401 });
  const body = (await request.json()) as { method?: string; path?: string; body?: unknown };
  const path = body.path ?? "";
  const method = (body.method ?? "GET").toUpperCase();
  if (!path.startsWith("/v1/") || path.startsWith("/v1/setup") || path.startsWith("/v1/auth/login")) {
    return Response.json({ error: { message: "path is not proxied" } }, { status: 400 });
  }
  const result = await scion(path, { method, body: body.body, session: token, csrf });
  if (result.status === 204 || result.json == null) {
    return new Response(null, { status: result.status });
  }
  return Response.json(result.json, { status: result.status });
}
