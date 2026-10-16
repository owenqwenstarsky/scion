import { api } from "@/lib/session";
import Link from "next/link";

type Entry = { name: string; type: string; size?: number };

export default async function TreePage({
  params,
}: {
  params: Promise<{ owner: string; name: string; path?: string[] }>;
}) {
  const { owner, name, path = [] } = await params;
  const dir = path.join("/");
  const query = dir ? `?path=${encodeURIComponent(dir)}` : "";
  const result = await api(`/v1/repos/${owner}/${name}/tree${query}`);
  if (result.status === 404) {
    return <p className="card">This repository has no commits yet.</p>;
  }
  const entries = ((result.json as { entries?: Entry[] }).entries ?? []);
  return (
    <>
      <h1 className="mono">{dir || "/"}</h1>
      <table>
        <tbody>
          {entries.map((entry) => {
            const full = dir ? `${dir}/${entry.name}` : entry.name;
            const href = entry.type === "dir"
              ? `/r/${owner}/${name}/tree/${full}`
              : `/r/${owner}/${name}/blob/${full}`;
            return (
              <tr key={entry.name}>
                <td><Link href={href}>{entry.name}{entry.type === "dir" ? "/" : ""}</Link></td>
                <td className="muted">{entry.type === "file" ? entry.size : "dir"}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </>
  );
}
