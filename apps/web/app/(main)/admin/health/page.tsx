import { api, requireSession } from "@/lib/session";
import { redirect } from "next/navigation";

export default async function HealthPage() {
  const session = await requireSession();
  if (!session.site_admin) redirect("/");
  const result = await api("/v1/stats");
  return (
    <>
      <h1>Health</h1>
      <pre className="patch">{JSON.stringify(result.json, null, 2)}</pre>
    </>
  );
}
