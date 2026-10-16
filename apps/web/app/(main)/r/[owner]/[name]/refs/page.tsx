import { api } from "@/lib/session";
import Link from "next/link";

export default async function RefsPage({ params }: { params: Promise<{ owner: string; name: string }> }) {
  const { owner, name } = await params;
  const result = await api(`/v1/repos/${owner}/${name}/refs`);
  const refs = ((result.json as { refs?: { name: string; target: string; version: number }[] }).refs ?? []);
  const landings = await api(`/v1/repos/${owner}/${name}/landings`);
  const rows = ((landings.json as { landings?: { ref: string; landed_as: string; actor: string; ts: number }[] }).landings ?? []);
  return (
    <>
      <h1>Refs</h1>
      <table>
        <tbody>
          {refs.map((ref) => (
            <tr key={ref.name}>
              <td>{ref.name}</td>
              <td><Link className="mono" href={`/r/${owner}/${name}/diff/${ref.target}`}>{ref.target.slice(0, 12)}</Link></td>
              <td className="muted">v{ref.version}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <h2>Landings</h2>
      <table>
        <tbody>
          {rows.map((row) => (
            <tr key={`${row.ref}-${row.ts}`}>
              <td>{row.ref}</td>
              <td className="mono">{row.landed_as.slice(0, 12)}</td>
              <td>{row.actor}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}
