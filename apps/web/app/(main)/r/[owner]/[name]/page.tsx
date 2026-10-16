import { CommitBox } from "@/components/commit-box";
import { api } from "@/lib/session";
import Link from "next/link";

export default async function RepoPage({ params }: { params: Promise<{ owner: string; name: string }> }) {
  const { owner, name } = await params;
  const repo = await api(`/v1/repos/${owner}/${name}`);
  const body = repo.json as { description?: string; capabilities?: { can_write?: boolean; can_land?: boolean } };
  const log = await api(`/v1/repos/${owner}/${name}/log?limit=5`);
  const commits = (log.json as { commits?: { id: string; message: string }[] })?.commits ?? [];
  return (
    <>
      <h1>{owner}/{name}</h1>
      <p>{body.description || "No description."}</p>
      <h2>Recent history</h2>
      {commits.length === 0 ? <p className="muted">No commits yet.</p> : null}
      <ul>
        {commits.map((commit) => (
          <li key={commit.id}>
            <Link className="mono" href={`/r/${owner}/${name}/diff/${commit.id}`}>{commit.id.slice(0, 12)}</Link>
            {" "}
            {commit.message.split("\n")[0]}
          </li>
        ))}
      </ul>
      {body.capabilities?.can_write ? <CommitBox owner={owner} name={name} /> : null}
    </>
  );
}
