import { api } from "@/lib/session";
import Link from "next/link";

type Repo = {
  owner: string;
  name: string;
  description: string;
  role: string;
  default_ref: string;
};

export default async function HomePage() {
  const result = await api("/v1/repos");
  const repos = ((result.json as { repos?: Repo[] })?.repos ?? []);
  return (
    <>
      <h1>Repositories</h1>
      <p className="muted">You only see repositories you own or that someone has granted you.</p>
      {repos.length === 0 ? <p className="card">No repositories yet.</p> : null}
      {repos.map((repo) => (
        <article className="card" key={`${repo.owner}/${repo.name}`}>
          <h2 style={{ marginTop: 0 }}>
            <Link href={`/r/${repo.owner}/${repo.name}`}>{repo.owner}/{repo.name}</Link>
          </h2>
          <p>{repo.description || "No description."}</p>
          <p className="muted">role {repo.role} · default {repo.default_ref}</p>
        </article>
      ))}
    </>
  );
}
