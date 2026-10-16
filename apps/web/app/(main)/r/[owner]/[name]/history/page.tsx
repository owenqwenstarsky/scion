import { api } from "@/lib/session";
import Link from "next/link";

export default async function HistoryPage({
  params,
  searchParams,
}: {
  params: Promise<{ owner: string; name: string }>;
  searchParams: Promise<{ path?: string }>;
}) {
  const { owner, name } = await params;
  const { path } = await searchParams;
  const result = path
    ? await api(`/v1/repos/${owner}/${name}/history?path=${encodeURIComponent(path)}`)
    : await api(`/v1/repos/${owner}/${name}/log?limit=50`);
  const commits = ((result.json as { commits?: { id: string; message: string; author?: { name?: string } }[] }).commits ?? []);
  return (
    <>
      <h1>{path ? `History of ${path}` : "History"}</h1>
      <table>
        <tbody>
          {commits.map((commit) => (
            <tr key={commit.id}>
              <td><Link className="mono" href={`/r/${owner}/${name}/diff/${commit.id}`}>{commit.id.slice(0, 12)}</Link></td>
              <td>{commit.message.split("\n")[0]}</td>
              <td className="muted">{commit.author?.name}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}
