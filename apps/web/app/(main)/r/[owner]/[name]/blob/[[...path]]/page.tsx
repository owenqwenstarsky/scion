import { api } from "@/lib/session";
import Link from "next/link";

export default async function BlobPage({
  params,
}: {
  params: Promise<{ owner: string; name: string; path?: string[] }>;
}) {
  const { owner, name, path = [] } = await params;
  const filePath = path.join("/");
  const result = await api(`/v1/repos/${owner}/${name}/file?path=${encodeURIComponent(filePath)}`);
  if (result.status !== 200) return <p className="error">File not available.</p>;
  const body = result.json as { text?: string; binary?: boolean; size?: number };
  return (
    <>
      <h1 className="mono">{filePath}</h1>
      <p>
        <Link href={`/r/${owner}/${name}/blame/${filePath}`}>Blame</Link>
        {" · "}
        <Link href={`/r/${owner}/${name}/history?path=${encodeURIComponent(filePath)}`}>History</Link>
        {" · "}
        <span className="muted">{body.size} bytes</span>
      </p>
      {body.binary ? <p>Binary file.</p> : <pre className="patch">{body.text}</pre>}
    </>
  );
}
