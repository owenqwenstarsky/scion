import { scion } from "@/lib/scion";

export async function GET() {
  try {
    const result = await scion("/v1/health");
    return Response.json({ web: "ok", server: result.status });
  } catch (error) {
    return Response.json({ web: "ok", server: String(error) }, { status: 503 });
  }
}
