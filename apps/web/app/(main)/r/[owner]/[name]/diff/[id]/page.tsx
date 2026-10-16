import { api } from "@/lib/session";

export default async function DiffPage({
  params,
}: {
  params: Promise<{ owner: string; name: string; id: string }>;
}) {
  const { owner, name, id } = await params;
  const cs = await api(`/v1/repos/${owner}/${name}/changesets/${id}`);
  const diff = await api(`/v1/repos/${owner}/${name}/diff?to=${id}`);
  const commit = cs.json as { message?: string; author?: { name?: string; when?: number } };
  const files = ((diff.json as { files?: { path: string; status: string; patch: string }[] }).files ?? []);
  return (
    <>
      <h1 className="mono">{id.slice(0, 12)}</h1>
      <p>{commit.message}</p>
      <p className="muted">{commit.author?.name}</p>
      {files.map((file) => (
        <section key={file.path}>
          <h2 className="mono">{file.status} {file.path}</h2>
          <pre className="patch">{file.patch || "(binary or empty)"}</pre>
        </section>
      ))}
    </>
  );
}
