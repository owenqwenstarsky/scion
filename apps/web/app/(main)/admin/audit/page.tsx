import { api, requireSession } from "@/lib/session";
import { redirect } from "next/navigation";

export default async function AuditPage() {
  const session = await requireSession();
  if (!session.site_admin) redirect("/");
  const result = await api("/v1/audit");
  const events = ((result.json as { events?: { ts: number; action: string; outcome: string; detail: string }[] }).events ?? []);
  return (
    <>
      <h1>Audit log</h1>
      <table>
        <tbody>
          {events.map((event, index) => (
            <tr key={index}>
              <td className="mono">{event.ts}</td>
              <td>{event.action}</td>
              <td>{event.outcome}</td>
              <td className="muted">{event.detail}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}
