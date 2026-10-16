import { api } from "@/lib/session";

export default async function RepoAuditPage({ params }: { params: Promise<{ owner: string; name: string }> }) {
  const { owner, name } = await params;
  const result = await api(`/v1/repos/${owner}/${name}/audit`);
  const events = ((result.json as { events?: { ts: number; action: string; outcome: string; detail: string }[] }).events ?? []);
  return (
    <>
      <h1>Audit</h1>
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
